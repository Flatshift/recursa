import { api, confirmAction, desktop, mac, notifyDone, onDesktop, saveExport, streamRun, type RunStream } from './api';
import type { Mix } from './verdict';
import type { Bootstrap, Env, Outcome, Resolver, RunConfig, RunRow, RunState, Sample } from './types';

export type Route =
  | { name: 'bench' }
  | { name: 'run'; id: string }
  | { name: 'resolvers' }
  | { name: 'deepdive'; id: string }
  | { name: 'reliability' }
  | { name: 'time' }
  | { name: 'method' }
  | { name: 'settings' };

/** The views a run's results are split into, in order (⌥⌘1–5; Ctrl+Shift+1–5 elsewhere). */
export const RUN_VIEWS = ['overview', 'latency', 'cdn', 'caps', 'numbers'] as const;
export type RunView = (typeof RUN_VIEWS)[number];

function parse(path: string): Route {
  const m = path.match(/^\/run\/([\w-]+)/);
  if (m) return { name: 'run', id: m[1] };
  const d = path.match(/^\/deep-dive\/([\w-]+)/);
  if (d) return { name: 'deepdive', id: d[1] };
  if (path.startsWith('/resolvers')) return { name: 'resolvers' };
  if (path.startsWith('/reliability')) return { name: 'reliability' };
  if (path.startsWith('/time')) return { name: 'time' };
  if (path.startsWith('/method')) return { name: 'method' };
  if (path.startsWith('/settings')) return { name: 'settings' };
  return { name: 'bench' };
}

type Theme = 'system' | 'light' | 'dark';

export function readLocal<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : fallback;
  } catch {
    return fallback;
  }
}

export function writeLocal(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* storage unavailable: preferences just won't persist */
  }
}

class AppState {
  boot = $state<Bootstrap | null>(null);
  bootError = $state<string | null>(null);
  route = $state<Route>(parse(location.pathname));
  theme = $state<Theme>(readLocal<Theme>('recursa.theme', 'system'));
  /** The kind of network the per-lookup figure is for. */
  mix = $state<Mix>(readLocal<Mix>('recursa.mix', 'home'));
  /** The run on screen. Raw: replaced wholesale on every event batch. */
  run = $state.raw<RunState | null>(null);
  runError = $state<string | null>(null);
  live = $state(false);
  /** Resolver shown in the inspector pane (null: the inspector is closed). */
  inspect = $state<string | null>(null);
  inspectorWidth = $state(readLocal<number>('recursa.inspector', 380));
  /** Which part of a run's results is on screen. */
  runView = $state<RunView>('overview');
  /** The sidebar is shown (narrow windows hide it until asked). */
  sidebar = $state(readLocal<boolean>('recursa.sidebar', true));
  /** Bumped to ask the current page to focus its filter field (⌘F; Ctrl+F elsewhere). */
  findRev = $state(0);
  /** The user picked a results view while the run was live; don't switch for them at the end. */
  #viewChosen = false;
  #openSeq = 0;
  draft = $state<RunConfig | null>(null);
  toast = $state<{ text: string; tone: 'info' | 'error' } | null>(null);
  /** Bumped when monitoring changes outside the UI (the desktop menu-bar item). */
  monitorRev = $state(0);

  #stream: RunStream | null = null;
  #toastTimer: ReturnType<typeof setTimeout> | undefined;

  constructor() {
    window.addEventListener('popstate', () => {
      this.route = parse(location.pathname);
      this.#onRoute();
    });
    this.applyTheme();
    if (desktop) {
      document.documentElement.classList.add('desktop');
      if (mac) document.documentElement.classList.add('mac');
      onDesktop('recursa:menu', (cmd) => this.command(cmd));
      onDesktop('recursa:monitor', () => {
        this.monitorRev++;
        this.refreshBoot();
      });
    }
  }

  /** App commands, from the native menus or keyboard shortcuts in the browser. */
  command(cmd: string) {
    if (cmd.startsWith('go:')) this.navigate(cmd.slice(3));
    else if (cmd === 'export:csv' || cmd === 'export:json') this.exportRun(cmd.slice(7) as 'csv' | 'json');
    else if (cmd.startsWith('view:')) this.setRunView(cmd.slice(5) as RunView);
    else if (cmd === 'toggle:sidebar') this.setSidebar(!this.sidebar);
    else if (cmd === 'toggle:inspector') this.toggleInspector();
    else if (cmd === 'find') this.findRev++;
    else if (cmd === 'run:start' && this.route.name === 'bench') this.startRun();
    else if (cmd === 'run:stop') this.cancelRun();
  }

  setRunView(v: RunView) {
    if (!RUN_VIEWS.includes(v)) return;
    if (this.route.name !== 'run') return;
    this.runView = v;
    if (this.run?.status === 'running') this.#viewChosen = true;
  }

  setSidebar(on: boolean, remember = true) {
    this.sidebar = on;
    if (remember) writeLocal('recursa.sidebar', on);
  }

  /** Back to the user's own choice (after a narrow window hid the sidebar). */
  restoreSidebar() {
    this.sidebar = readLocal<boolean>('recursa.sidebar', true);
  }

  setInspectorWidth(w: number) {
    this.inspectorWidth = Math.round(Math.min(560, Math.max(300, w)));
    writeLocal('recursa.inspector', this.inspectorWidth);
  }

  /** Show the inspector for the selected (or first) resolver, or hide it. */
  toggleInspector() {
    if (this.inspect) this.inspect = null;
    else if (this.route.name === 'run') this.inspect = this.run?.resolvers[0]?.id ?? null;
    else if (this.route.name === 'resolvers') this.inspect = this.resolvers[0]?.id ?? null;
  }

  get env(): Env | null {
    return this.boot?.env ?? null;
  }

  get resolvers(): Resolver[] {
    return this.boot?.resolvers ?? [];
  }

  async init() {
    try {
      this.boot = await api.get<Bootstrap>('/api/bootstrap');
      this.bootError = null;
      const saved = readLocal<RunConfig | null>('recursa.draft', null);
      const ids = new Set(this.boot.resolvers.map((r) => r.id));
      // A custom entry folded into another row (the system resolver's, or a catalog entry's) is selected as that row.
      const folded = new Map(this.boot.resolvers.flatMap((r) => [r.custom_id, ...(r.aliases ?? [])].filter((c) => c).map((c) => [c!, r.id] as const)));
      this.draft = saved
        ? {
            ...this.boot.defaults,
            ...saved,
            filters: { ...this.boot.defaults.filters, ...saved.filters },
            resolvers: [...new Set(saved.resolvers.map((id) => folded.get(id) ?? id))].filter((id) => ids.has(id)),
          }
        : $state.snapshot(this.boot.defaults);
      this.#onRoute();
      if (!this.#envSettled(this.boot.env)) this.#pollEnv();
      if (this.boot.active_run && this.route.name === 'bench') this.navigate(`/run/${this.boot.active_run}`);
    } catch (e) {
      this.bootError = (e as Error).message;
    }
  }

  /** Background detection and calibration have all reported. */
  #envSettled(env: Env) {
    return !!(env.vantage && env.interception && env.calibration);
  }

  async #pollEnv() {
    for (let i = 0; i < 20; i++) {
      await new Promise((r) => setTimeout(r, 1500));
      try {
        const env = await api.get<Env>('/api/env');
        if (this.boot) this.boot = { ...this.boot, env };
        if (this.#envSettled(env)) return;
      } catch {
        /* keep trying quietly */
      }
    }
  }

  async recheckInterception(): Promise<void> {
    try {
      const env = await api.post<Env>('/api/interception');
      if (this.boot) this.boot = { ...this.boot, env };
      const v = env.interception?.verdict;
      this.notify(
        v === 'direct'
          ? 'Port 53 reaches the resolvers directly.'
          : v === 'intercepted'
            ? 'Port 53 is still being intercepted on this network.'
            : v === 'partial'
              ? 'Some port-53 traffic looks intercepted.'
              : 'Couldn’t decide: some checks failed.',
        v === 'intercepted' ? 'error' : 'info',
      );
    } catch (e) {
      this.notify((e as Error).message, 'error');
    }
  }

  async refreshBoot() {
    try {
      const b = await api.get<Bootstrap>('/api/bootstrap');
      this.boot = b;
    } catch (e) {
      this.notify((e as Error).message, 'error');
    }
  }

  saveDraft() {
    if (this.draft) writeLocal('recursa.draft', this.draft);
  }

  setMix(m: Mix) {
    this.mix = m;
    writeLocal('recursa.mix', m);
  }

  setTheme(t: Theme) {
    this.theme = t;
    writeLocal('recursa.theme', t);
    this.applyTheme();
  }

  applyTheme() {
    const el = document.documentElement;
    if (this.theme === 'system') el.removeAttribute('data-theme');
    else el.setAttribute('data-theme', this.theme);
  }

  navigate(path: string, replace = false) {
    if (location.pathname !== path) {
      if (replace) history.replaceState(null, '', path);
      else history.pushState(null, '', path);
    }
    this.route = parse(path);
    this.#onRoute();
  }

  #onRoute() {
    const r = this.route;
    if (r.name === 'run') {
      if (this.run?.id !== r.id) this.openRun(r.id);
    } else if (r.name !== 'resolvers') {
      this.inspect = null;
    }
  }

  notify(text: string, tone: 'info' | 'error' = 'info') {
    this.toast = { text, tone };
    clearTimeout(this.#toastTimer);
    this.#toastTimer = setTimeout(() => (this.toast = null), 5200);
  }

  async startRun() {
    if (!this.draft) return;
    this.saveDraft();
    try {
      const { id } = await api.post<{ id: string }>('/api/runs', this.draft);
      this.run = null;
      this.navigate(`/run/${id}`);
    } catch (e) {
      this.notify((e as Error).message, 'error');
    }
  }

  async cancelRun() {
    if (!this.run) return;
    try {
      await api.post(`/api/runs/${this.run.id}/cancel`);
    } catch (e) {
      this.notify((e as Error).message, 'error');
    }
  }

  /** Delete a finished run from history, after asking. */
  async deleteRun(id: string) {
    if (!(await confirmAction('Delete this run? Its results can’t be recovered.', 'Delete Run'))) return;
    try {
      await api.del(`/api/runs/${id}`);
      if (this.boot) this.boot = { ...this.boot, runs: this.boot.runs.filter((r) => r.id !== id) };
      // Deleting another run from the sidebar leaves the one on screen alone.
      if (this.route.name === 'run' && this.route.id === id) {
        this.run = null;
        this.navigate('/', true);
      }
      this.notify('Run deleted.');
    } catch (e) {
      this.notify((e as Error).message, 'error');
    }
  }

  /** Save a run as CSV or JSON: the one on screen, unless `id` names another. */
  async exportRun(format: 'csv' | 'json', id?: string) {
    const onScreen = this.route.name === 'run' ? this.run : null;
    const target = id ?? onScreen?.id;
    if (!target) return this.notify('Open a run to export it.');
    if (target === onScreen?.id && onScreen.status === 'running') return this.notify('Export is available once the run finishes.');
    try {
      const saved = await saveExport(target, format);
      if (saved) this.notify(`Saved ${saved}.`);
    } catch (e) {
      this.notify((e as Error).message, 'error');
    }
  }

  async openRun(id: string) {
    this.#close();
    this.runError = null;
    this.run = null;
    this.inspect = null;
    const seq = ++this.#openSeq;
    try {
      const r = await api.get<RunState>(`/api/runs/${id}`);
      if (seq !== this.#openSeq) return; // a newer openRun replaced this one
      this.run = r;
      // A live run opens on its samples landing; a finished one on its verdict.
      this.runView = r.status === 'running' ? 'latency' : 'overview';
      this.#viewChosen = false;
      if (r.status === 'running') this.#connect(id);
    } catch (e) {
      if (seq !== this.#openSeq) return;
      this.runError = (e as Error).message;
    }
  }

  #close() {
    this.#stream?.close();
    this.#stream = null;
    this.live = false;
  }

  #connect(id: string) {
    this.#close();
    this.live = true;
    this.#stream = streamRun(
      id,
      (ev) => this.#apply(ev, id),
      () => {
        // The stream ended on its own (the run left memory); fall back to a fetch.
        this.live = false;
        if (this.run?.status === 'running') this.openRun(id);
      },
    );
  }

  #apply(ev: any, id: string) {
    if (ev.type === 'snapshot') {
      this.run = ev.run as RunState;
      return;
    }
    if (ev.type === 'resync') {
      this.openRun(id);
      return;
    }
    const cur = this.run;
    if (!cur || cur.id !== id) return;
    if (typeof ev.seq === 'number' && ev.seq <= cur.seq) return;
    const next: RunState = { ...cur, seq: ev.seq ?? cur.seq };
    switch (ev.type) {
      case 'phase':
        next.phase = ev.phase;
        next.total = ev.total;
        next.done = 0;
        break;
      case 'targets':
        next.targets = ev.targets;
        break;
      case 'samples':
        next.samples = cur.samples.concat(ev.samples);
        next.done = ev.done;
        next.total = ev.total;
        break;
      case 'warm':
        next.warm = (cur.warm ?? []).concat(ev.samples);
        next.done = ev.done;
        next.total = ev.total;
        break;
      case 'outcomes': {
        // Samples the engine judged afterwards (Uncached answers that came from a cache).
        const key = (x: { t: number; k: string; i: number }) => `${x.t}:${x.k}:${x.i}`;
        const changed = new Map<string, Outcome>(ev.samples.map((x: Sample) => [key(x), x.o]));
        next.samples = cur.samples.map((x) => (changed.has(key(x)) ? { ...x, o: changed.get(key(x))! } : x));
        break;
      }
      case 'note':
        next.notes = [...cur.notes, ev.note];
        break;
      case 'probe':
        next.probes = { ...cur.probes, [ev.resolver_id]: ev.report };
        next.done = ev.done ?? cur.done;
        break;
      case 'intel':
        next.intel = { ...cur.intel, [ev.resolver_id]: ev.intel };
        next.done = ev.done ?? cur.done;
        break;
      case 'cdn':
        next.cdn = ev.cdn;
        break;
      case 'link':
        next.link = ev.link;
        break;
      case 'paths':
        next.paths = ev.paths;
        next.targets = ev.targets;
        break;
      case 'done':
        this.run = ev.run as RunState;
        this.#close();
        this.#recordRun(ev.run as RunState);
        if (!this.#viewChosen) this.runView = 'overview';
        this.#announceDone(ev.run as RunState);
        return;
    }
    this.run = next;
  }

  /** Tell the user a run finished while they were in another app. */
  #announceDone(r: RunState) {
    if (document.hasFocus()) return;
    const n = new Set(r.targets.map((t) => t.resolver_id)).size;
    const title = r.status === 'cancelled' ? 'Benchmark stopped' : 'Benchmark finished';
    notifyDone(title, `Results for ${n} resolver${n === 1 ? '' : 's'} are ready.`);
  }

  #recordRun(r: RunState) {
    if (!this.boot) return;
    const row: RunRow = { id: r.id, started_at: r.started_at, finished_at: r.finished_at, status: r.status, summary: {} };
    this.boot = { ...this.boot, active_run: null, runs: [row, ...this.boot.runs.filter((x) => x.id !== r.id)] };
    api.get<RunRow[]>('/api/runs').then((runs) => this.boot && (this.boot = { ...this.boot, runs }));
  }
}

export const app = new AppState();
