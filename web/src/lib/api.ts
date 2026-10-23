// The UI's only transport layer. In the browser (the `recursa` CLI) calls go to the local
// HTTP server with a per-launch token; in the desktop app they go over Tauri IPC to the same
// engine routes. Nothing else in the UI knows which is in use.

import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Caps } from './types';

/** Running inside the desktop app rather than a browser tab. */
export const desktop = '__TAURI_INTERNALS__' in window;

/** The iOS app: a phone layout and touch. The shell sets the flag before the page runs. What a build can
 *  do is `caps` (crates/engine/src/caps.rs), not this. */
export const mobile = desktop && '__RECURSA_MOBILE__' in window;

/** The Mac or Windows app, where Settings has a window of its own. */
export const shell = desktop && !mobile;

/** The macOS app, whose window has traffic lights over its top-left corner. */
export const mac = shell && /Mac/.test(navigator.userAgent);

// A launch link's fragment, which never reaches the server, carries a one-time code (`#code=`) that the server trades
// for a session token. The token is kept for this port so that other tabs of the same launch work; the next launch
// replaces it.
const tokenKey = `recursa-token:${location.port}`;
let token = '';
/** Why the launch link's code was refused (already used, or not valid), shown instead of a bare 401. */
let sessionError = '';

const ready: Promise<void> = desktop
  ? Promise.resolve()
  : (async () => {
      const h = new URLSearchParams(location.hash.slice(1));
      const code = h.get('code');
      if (code) {
        history.replaceState(history.state, '', location.pathname + location.search);
        try {
          const res = await fetch('/api/session', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ code }),
          });
          const data = (await res.json().catch(() => null)) as { token?: string; error?: string } | null;
          if (res.ok && data?.token) token = data.token;
          else sessionError = data?.error ?? '';
        } catch {
          // The first API call reports a server that isn't there.
        }
      }
      try {
        if (token) localStorage.setItem(tokenKey, token);
        else token = localStorage.getItem(tokenKey) ?? '';
      } catch {
        // Storage blocked: this tab still works with the token it was given.
      }
    })();

class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message);
  }
}

/** An IPC rejection carries the engine's `{ status, error }`; anything else is a bridge failure. */
function ipcError(e: unknown): ApiError {
  const x = e as { status?: number; error?: string } | null;
  return x && typeof x.error === 'string' ? new ApiError(x.error, x.status ?? 0) : new ApiError(String(e), 0);
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  if (desktop) {
    try {
      return await invoke<T>('api', { method, path, body: body ?? null });
    } catch (e) {
      throw ipcError(e);
    }
  }
  await ready;
  let res: Response;
  try {
    res = await fetch(path, {
      method,
      headers: {
        'x-recursa-token': token,
        ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  } catch {
    throw new ApiError('Recursa isn’t responding. Is the app still running?', 0);
  }
  const text = await res.text();
  let data: unknown = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = null;
  }
  if (!res.ok) {
    const msg = (res.status === 401 && sessionError) || (data as { error?: string } | null)?.error || `Request failed (${res.status}).`;
    throw new ApiError(msg, res.status);
  }
  return data as T;
}

export const api = {
  get: <T>(p: string) => call<T>('GET', p),
  post: <T>(p: string, b?: unknown) => call<T>('POST', p, b ?? {}),
  put: <T>(p: string, b: unknown) => call<T>('PUT', p, b),
  del: <T>(p: string) => call<T>('DELETE', p),
};

export interface RunStream {
  close(): void;
}

/**
 * Follow a live run: a `snapshot` event first, then every change (events carry `seq`).
 * `onEnd` fires if the stream ends on its own because the run left memory, so the caller
 * can fall back to a plain fetch.
 */
export function streamRun(runId: string, onEvent: (ev: any) => void, onEnd: () => void): RunStream {
  if (desktop) {
    let closed = false;
    let sub: number | null = null;
    const ch = new Channel<any>((ev) => {
      if (closed) return;
      if (ev?.type === 'end') onEnd();
      else onEvent(ev);
    });
    invoke<number>('subscribe', { runId, onEvent: ch }).then(
      (id) => (closed ? invoke('unsubscribe', { id }) : (sub = id)),
      () => closed || onEnd(),
    );
    return {
      close() {
        closed = true;
        if (sub !== null) invoke('unsubscribe', { id: sub }).catch(() => {});
      },
    };
  }
  const url = `/api/runs/${encodeURIComponent(runId)}/events?token=${encodeURIComponent(token)}`;
  const es = new EventSource(url);
  es.onmessage = (m) => {
    let ev: any;
    try {
      ev = JSON.parse(m.data);
    } catch {
      return;
    }
    onEvent(ev);
  };
  // The server closes the stream when the run is evicted.
  es.onerror = () => es.readyState === EventSource.CLOSED && onEnd();
  return { close: () => es.close() };
}

/** Save a run as CSV or JSON: a native save panel in the app, a download in the browser.
 *  Resolves to the saved file's name, or null when cancelled or handed to the browser. */
export async function saveExport(runId: string, format: 'csv' | 'json'): Promise<string | null> {
  if (desktop) {
    try {
      return await invoke<string | null>('export_run', { id: runId, format });
    } catch (e) {
      throw ipcError(e);
    }
  }
  // Fetched with the token in a header, then saved from memory, so the token stays out of the download list.
  await ready;
  let res: Response;
  try {
    res = await fetch(`/api/runs/${encodeURIComponent(runId)}/export?format=${format}`, { headers: { 'x-recursa-token': token } });
  } catch {
    throw new ApiError('Recursa isn’t responding. Is the app still running?', 0);
  }
  if (!res.ok) {
    const data = (await res.json().catch(() => null)) as { error?: string } | null;
    const msg = (res.status === 401 && sessionError) || data?.error || `Export failed (${res.status}).`;
    throw new ApiError(msg, res.status);
  }
  const name = /filename="([^"]+)"/.exec(res.headers.get('content-disposition') ?? '')?.[1] ?? `recursa-${runId}.${format}`;
  const url = URL.createObjectURL(await res.blob());
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.append(a);
  a.click();
  a.remove();
  // Not at once: some browsers are still reading the blob when click() returns.
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
  return null;
}

/** Ask before a destructive action. `ok` labels the confirming button where the platform allows. */
export async function confirmAction(message: string, ok: string): Promise<boolean> {
  if (desktop) return invoke<boolean>('confirm', { message, ok }).catch(() => false);
  return window.confirm(message);
}

/** Desktop only: app menu commands (`go:/path`, `export:csv`, …) and monitoring toggled from the menu bar. */
export function onDesktop(event: 'recursa:menu' | 'recursa:monitor', f: (payload: string) => void) {
  if (desktop) listen<string>(event, (e) => f(e.payload));
}

/** Desktop only: a system notification (the browser tab has its own title instead). */
export function notifyDone(title: string, body: string) {
  if (desktop) invoke('notify', { title, body }).catch(() => {});
}

let caps: Caps | null = null;
/** The bootstrap's `caps`, for the calls below that only some builds have. */
export function setCaps(c: Caps) {
  caps = c;
}

/** Open the Settings window, on `section` (a Settings pane id) when given. On the phone Settings is
 *  a page, so the caller navigates instead. */
export function openSettings(section?: string) {
  if (caps?.settings_window) invoke('open_settings', { section: section ?? null }).catch(() => {});
}

export interface MenuItem {
  id: string;
  label: string;
  enabled?: boolean;
}
/** A separator between groups of items. */
export const SEPARATOR: MenuItem = { id: '-', label: '' };

let pendingMenu: ((id: string) => void) | null = null;
let menuSeq = 0;
if (desktop) {
  listen<string>('recursa:context', (e) => {
    const [seq, ...rest] = e.payload.split(':');
    if (Number(seq) === menuSeq && pendingMenu) pendingMenu(rest.join(':'));
    pendingMenu = null;
  });
}

/** Pop up a native menu at the pointer where the build has them. `pick` runs only if an item is chosen.
 *  Returns false elsewhere, where the caller falls back to its own menu. */
export function nativeMenu(items: MenuItem[], pick: (id: string) => void): boolean {
  if (!caps?.native_menus) return false;
  menuSeq++;
  pendingMenu = pick;
  invoke('context_menu', { seq: menuSeq, items: items.map((i) => ({ id: i.id, label: i.label, enabled: i.enabled ?? true })) }).catch(
    () => (pendingMenu = null),
  );
  return true;
}

/** Open at Login, where the build has it. */
export const loginItem = {
  get: () => (caps?.login_item ? invoke<boolean>('get_login') : Promise.resolve(false)),
  set: (on: boolean) => (caps?.login_item ? invoke<boolean>('set_login', { enabled: on }) : Promise.resolve(false)),
};

/** Put text on the clipboard: through the app in the desktop build, where a menu pick isn't
 *  a page gesture the WebView would accept. */
export function copyText(text: string) {
  if (desktop) invoke('copy_text', { text }).catch(() => {});
  else navigator.clipboard?.writeText(text).catch(() => {});
}
