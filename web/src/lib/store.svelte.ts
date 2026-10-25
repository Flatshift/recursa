import { api, confirmAction, desktop, mac, mobile, notifyDone, onDesktop, saveExport, setCaps, streamRun, type RunStream } from './api';
import { flushSync } from 'svelte';
import { resolverCount } from './format';
import { reduceMotion } from './motion';
import { phaseTab } from './phases';
import type { Mix } from './verdict';
import type { Bootstrap, Env, Outcome, Resolver, RunConfig, RunRow, RunState, Sample } from './types';

type Route =
  | { name: 'bench' }
  | { name: 'run'; id: string }
  | { name: 'resolvers' }
  | { name: 'resolver'; id: string }
  | { name: 'reliability' }
  | { name: 'time' }
  | { name: 'method' }
  | { name: 'settings' };

/** The views a run's results are split into, in order (⌥⌘1–6; Ctrl+Shift+1–6 elsewhere). */
export const RUN_VIEWS = ['overview', 'latency', 'path', 'cdn', 'caps', 'numbers'] as const;
export type RunView = (typeof RUN_VIEWS)[number];

/** The view where a live run's current phase shows its work: the latency rounds on Latency, the path check on Path, the
 *  probes on Capabilities, the CDN mapping on CDN. Network intel is too short to jump for, so it keeps the last one. */
function liveView(r: Pick<RunState, 'phase'>, from: RunView = 'latency'): RunView {
  if (r.phase === 'intel') return from === 'overview' ? 'latency' : from;
  return phaseTab(r.phase) ?? 'latency';
}

function parse(path: string): Route {
  const m = path.match(/^\/run\/([\w-]+)/);
  if (m) return { name: 'run', id: m[1] };
  const d = path.match(/^\/resolver\/([\w-]+)/);
  if (d) return { name: 'resolver', id: d[1] };
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
  /** Resolver shown in the inspector pane (null: the inspector is closed). */
  inspect = $state<string | null>(null);
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
      if (mac) document.documentElement.classList.add('mac');
      if (mobile) document.documentElement.classList.add('mobile');
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
    else if (cmd === 'run:save' || cmd === 'run:delete') {
      // The menu items act on the finished run on screen.
      const open = this.route.name === 'run' && this.run?.id === this.route.id && this.run.status !== 'running' ? this.run : null;
      const row = open && this.boot?.runs.find((r) => r.id === open.id);
      if (!row) return this.notify('Open a finished run first.');
      if (cmd === 'run:save') this.setSaved(row.id, !row.saved);
      else this.deleteRun(row.id);
    } else if (cmd === 'runs:clear') this.clearRuns();
  }

  setRunView(v: RunView) {
    if (!RUN_VIEWS.includes(v)) return;
    if (this.route.name !== 'run') return;
    this.runView = v;
    if (this.run?.status === 'running') this.#viewChosen = true;
  }

  /** A run update waiting for its view transition's first snapshot; later events build on it. */
  #staged: { run: RunState; view: RunView } | null = null;

  /** Show `next` on view `v`. With `animate`, as a view transition: the results panel slides toward the side of its tab
   *  (or fades through, staying on one view) while the toolbar and progress rail hold still. */
  #show(next: RunState, v: RunView, animate: boolean) {
    if (this.#staged) {
      this.#staged = { run: next, view: v };
      return;
    }
    if (!animate || reduceMotion || !document.startViewTransition || document.hidden) {
      this.run = next;
      this.runView = v;
      return;
    }
    const root = document.documentElement;
    const dir = (to: RunView) => {
      const a = RUN_VIEWS.indexOf(this.runView);
      const b = RUN_VIEWS.indexOf(to);
      return b > a ? 'fwd' : b < a ? 'back' : 'swap';
    };
    root.dataset.vt = dir(v);
    this.#staged = { run: next, view: v };
    let ran = false;
    const apply = () => {
      if (ran) return;
      ran = true;
      const s = this.#staged;
      this.#staged = null;
      // The user opened another run meanwhile.
      if (!s || this.run?.id !== s.run.id) return;
      this.run = s.run;
      if (!this.#viewChosen) {
        // Events staged while the snapshot was taken may have moved the view on; slide toward where it ends up.
        root.dataset.vt = dir(s.view);
        this.runView = s.view;
      }
      flushSync();
    };
    const t = document.startViewTransition(apply);
    t.ready.catch(() => {});
    t.finished.finally(() => delete root.dataset.vt);
    // A page that stops rendering (a hidden or occluded window) never runs the callback; don't hold events behind it.
    setTimeout(() => {
      if (ran) return;
      t.skipTransition();
      apply();
    }, 1000);
  }

  setSidebar(on: boolean, remember = true) {
    this.sidebar = on;
    if (remember) writeLocal('recursa.sidebar', on);
  }

  /** Back to the user's own choice (after a narrow window hid the sidebar). */
  restoreSidebar() {
    this.sidebar = readLocal<boolean>('recursa.sidebar', true);
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
      setCaps(this.boot.caps);
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

  /** Mark a run as saved, or not. Saved runs are skipped by Clear all runs and by automatic cleanup. */
  async setSaved(id: string, saved: boolean) {
    try {
      await api.put(`/api/runs/${id}/saved`, { saved });
      if (this.boot) this.boot = { ...this.boot, runs: this.boot.runs.map((r) => (r.id === id ? { ...r, saved } : r)) };
    } catch (e) {
      this.notify((e as Error).message, 'error');
    }
  }

  /** Delete every unsaved finished run, after asking with the count. */
  async clearRuns() {
    try {
      const runs = await api.get<RunRow[]>('/api/runs');
      // The list stops at 200 rows, so past that the count is unknown.
      const capped = runs.length >= 200;
      const n = runs.filter((r) => !r.saved && r.id !== this.boot?.active_run).length;
      if (n === 0 && !capped) {
        this.notify(runs.length ? 'Every run is saved.' : 'There are no runs to clear.');
        return;
      }
      const what = capped ? 'every unsaved run' : n === 1 ? 'the 1 unsaved run' : `all ${n} unsaved runs`;
      if (!(await confirmAction(`Delete ${what}? Saved runs stay. Results can’t be recovered.`, 'Clear Runs'))) return;
      const res = await api.del<{ deleted: number }>('/api/runs');
      const deleted = res?.deleted ?? n;
      if (this.boot) this.boot = { ...this.boot, runs: this.boot.runs.filter((r) => r.saved || r.id === this.boot?.active_run) };
      const open = this.route.name === 'run' ? this.route.id : null;
      if (open && this.run?.status !== 'running' && !this.boot?.runs.some((r) => r.id === open)) {
        this.run = null;
        this.navigate('/', true);
      }
      this.notify(deleted === 1 ? 'Run deleted.' : `${deleted} runs deleted.`);
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
      this.runView = r.status === 'running' ? liveView(r) : 'overview';
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
  }

  #connect(id: string) {
    this.#close();
    this.#stream = streamRun(
      id,
      (ev) => this.#apply(ev, id),
      () => {
        // The stream ended on its own (the run left memory); fall back to a fetch.
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
    const cur = this.#staged?.run ?? this.run;
    if (!cur || cur.id !== id) return;
    if (typeof ev.seq === 'number' && ev.seq <= cur.seq) return;
    const next: RunState = { ...cur, seq: ev.seq ?? cur.seq };
    switch (ev.type) {
      case 'phase':
        next.phase = ev.phase;
        next.total = ev.total;
        next.done = 0;
        // Follow the run to the view where this phase shows its work, unless the user picked one.
        if (!this.#viewChosen) {
          const from = this.#staged?.view ?? this.runView;
          const v = liveView(next, from);
          return this.#show(next, v, v !== from);
        }
        break;
      case 'progress':
        next.done = ev.done;
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
      case 'cdn_cell': {
        // One lookup landed: the resolver's cell for that CDN, as it stands now.
        const l = cur.cdn_lookups?.[ev.resolver_id] ?? { answered: 0, asked: 0, finished: false, cells: {} };
        next.cdn_lookups = { ...cur.cdn_lookups, [ev.resolver_id]: { ...l, cells: { ...l.cells, [ev.cdn]: ev.cell } } };
        break;
      }
      case 'cdn_lookup':
        next.cdn_lookups = { ...cur.cdn_lookups, [ev.resolver_id]: ev.lookup };
        next.done = ev.done ?? cur.done;
        break;
      case 'cdn_timing': {
        // Each update carries only the edges that changed since the last.
        const best = { ...cur.cdn_timing?.best };
        for (const [ip, v] of Object.entries(ev.best ?? {}) as [string, number][]) best[ip] = Math.min(v, best[ip] ?? Infinity);
        const silent = [...new Set([...(cur.cdn_timing?.silent ?? []), ...(ev.silent ?? [])])];
        next.cdn_timing = { ...ev.timing, best, silent };
        break;
      }
      case 'cdn_owner': {
        // The network behind one edge, named during the intel phase.
        const e = cur.cdn?.edges[ev.ip];
        if (!cur.cdn || !e) break;
        next.cdn = { ...cur.cdn, edges: { ...cur.cdn.edges, [ev.ip]: { ...e, asn: ev.asn, holder: ev.holder, on_net: ev.on_net } } };
        break;
      }
      case 'cdn':
        // The live grid stays up through intel; the finished map waits for the end of the run.
        next.cdn = ev.cdn;
        break;
      case 'link':
        next.link = ev.link;
        break;
      case 'paths':
        next.paths = ev.paths;
        next.targets = ev.targets;
        break;
      case 'route':
        next.routes = { ...cur.routes, [ev.resolver_id]: ev.trace };
        next.done = ev.done ?? cur.done;
        break;
      case 'routes':
        next.routes = ev.routes;
        break;
      case 'done': {
        this.#close();
        const r = ev.run as RunState;
        this.#show(r, this.#viewChosen ? this.runView : 'overview', true);
        this.#recordRun(r);
        this.#announceDone(r);
        return;
      }
    }
    this.#show(next, this.#staged?.view ?? this.runView, false);
  }

  /** Tell the user a run finished while they were in another app. */
  #announceDone(r: RunState) {
    if (document.hasFocus()) return;
    const n = resolverCount(r.targets);
    const title = r.status === 'cancelled' ? 'Benchmark stopped' : 'Benchmark finished';
    notifyDone(title, `Results for ${n} resolver${n === 1 ? '' : 's'} are ready.`);
  }

  #recordRun(r: RunState) {
    if (!this.boot) return;
    const row: RunRow = { id: r.id, started_at: r.started_at, finished_at: r.finished_at, status: r.status, summary: {}, saved: false };
    this.boot = { ...this.boot, active_run: null, runs: [row, ...this.boot.runs.filter((x) => x.id !== r.id)] };
    api.get<RunRow[]>('/api/runs').then((runs) => this.boot && (this.boot = { ...this.boot, runs }));
  }
}

export const app = new AppState();
