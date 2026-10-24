// One controller for the background monitors: the resolver monitor (`/api/monitor`) and the time monitor
// (`/api/ntp/monitor`). It loads the record for a window, saves the config, polls `/last` so the record refreshes
// once a cycle has been recorded, and clears the history. The words of each notice stay with the page that shows it.

import { untrack } from 'svelte';
import { api, confirmAction } from './api';
import { app } from './store.svelte';
import type { MonitorConfig, MonitorResp } from './types';

export interface MonitorOptions<C> {
  /** `/api/monitor` or `/api/ntp/monitor`; `/last` and `/history` hang off it. */
  endpoint: string;
  /** The window to load, as a query string (`hours=72`). The record loads when it changes and is polled while it is
   *  set; without it the controller only saves. */
  query?: () => string;
  /** The notice after a save; none when omitted. */
  saved?: (cfg: C) => string;
  /** Where a failure's message goes; a toast when omitted. */
  onerror?: (message: string) => void;
  /** Runs after a save with the saved config. */
  onsaved?: (cfg: C) => void;
  /** The question before the history is deleted; `clearHistory` does nothing without one. */
  clearPrompt?: string;
}

export function monitor<C, R extends { config: C; last: number | null }>(o: MonitorOptions<C>) {
  let data = $state<R | null>(null);
  let cfg = $state<C | null>(null);
  let saving = $state(false);
  let seq = 0;
  const fail = (e: unknown) => (o.onerror ?? ((m: string) => app.notify(m, 'error')))((e as Error).message);

  async function load() {
    if (!o.query) return;
    const n = ++seq;
    try {
      const r = await api.get<R>(`${o.endpoint}?${o.query()}`);
      if (n !== seq) return; // a newer request superseded this one
      data = r;
      if (!cfg) cfg = $state.snapshot(r.config) as C;
    } catch (e) {
      fail(e);
    }
  }
  /** Drop the config edited here and take the server's as the new baseline. */
  function reset() {
    cfg = null;
    load();
  }

  if (o.query) {
    $effect(() => {
      o.query!();
      untrack(load);
    });
    // Refetch the record only when a cycle has been recorded since.
    $effect(() => {
      const t = setInterval(async () => {
        if (document.hidden) return;
        try {
          const { last } = await api.get<{ last: number | null }>(`${o.endpoint}/last`);
          if (last !== data?.last) load();
        } catch {
          /* the next tick tries again */
        }
      }, 60_000);
      return () => clearInterval(t);
    });
  }

  async function save(next: C) {
    saving = true;
    try {
      const saved = await api.put<C>(o.endpoint, next);
      cfg = saved;
      o.onsaved?.(saved);
      if (o.saved) app.notify(o.saved(saved));
      load();
    } catch (e) {
      fail(e);
    } finally {
      saving = false;
    }
  }

  async function clearHistory() {
    if (!o.clearPrompt || !(await confirmAction(o.clearPrompt, 'Delete History'))) return;
    try {
      await api.del(`${o.endpoint}/history`);
    } catch (e) {
      fail(e);
    }
    load();
  }

  return {
    /** The window's record, with the server's config and when the last cycle ran. */
    get data() {
      return data;
    },
    /** The config as edited here; null until it has loaded. */
    get cfg() {
      return cfg;
    },
    get saving() {
      return saving;
    },
    load,
    reset,
    save,
    clearHistory,
  };
}

/** The resolver monitor. Its config also rides in the bootstrap, which the sidebar's footer reads, so a save updates
 *  that copy too. */
export function resolverMonitor(o: Omit<MonitorOptions<MonitorConfig>, 'endpoint' | 'onsaved'>) {
  return monitor<MonitorConfig, MonitorResp>({
    ...o,
    endpoint: '/api/monitor',
    onsaved: (c) => {
      if (app.boot) app.boot = { ...app.boot, monitor: c };
    },
  });
}
