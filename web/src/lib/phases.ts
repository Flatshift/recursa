import type { RunView } from './store.svelte';

interface Phase {
  id: string;
  name: string;
  /** Before the latency rounds. */
  lead?: boolean;
  /** The checks after the latency rounds report per handshake or per resolver, not per query. */
  check?: boolean;
  /** The run option that switches an optional check on. */
  opt?: 'probes' | 'cdn';
  /** The results view where this phase shows its work. */
  tab?: RunView;
}

/** Every phase of a run in the order it runs: setup, the latency rounds (a run picks which), then the checks. */
const PHASES: Phase[] = [
  { id: 'setup', name: 'Setup', lead: true },
  { id: 'warmup', name: 'Warm-up', lead: true },
  { id: 'cached', name: 'Cached' },
  { id: 'uncached', name: 'Uncached' },
  { id: 'tld', name: 'TLD' },
  { id: 'path', name: 'Path check', check: true, tab: 'path' },
  { id: 'probes', name: 'Probes', check: true, opt: 'probes', tab: 'caps' },
  { id: 'cdn', name: 'CDN mapping', check: true, opt: 'cdn', tab: 'cdn' },
  { id: 'intel', name: 'Network intel', check: true },
];

export const phaseName = (id: string): string => PHASES.find((p) => p.id === id)?.name ?? id;

/** Whether a phase is one of the checks after the latency rounds. */
export const isCheck = (id: string): boolean => !!PHASES.find((p) => p.id === id)?.check;

/** The checks this run does, in run order. */
export const checksOf = (c: { probes: boolean; cdn: boolean }): string[] => PHASES.filter((p) => p.check && (!p.opt || c[p.opt])).map((p) => p.id);

/** Every phase this run goes through, in order. */
export const phasesOf = (c: { tests: string[]; probes: boolean; cdn: boolean }): string[] => [...PHASES.filter((p) => p.lead).map((p) => p.id), ...c.tests, ...checksOf(c)];

/** The view a phase fills. Network intel has none of its own: where it shows depends on the caller. */
export const phaseTab = (id: string): RunView | undefined => PHASES.find((p) => p.id === id)?.tab;
