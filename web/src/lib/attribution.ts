// What a monitoring cell or outage says about whose failure it was: the resolver's own, or one shared across operators.
import type { MonitorCell, MonitorGap, MonitorOutage } from './types';

export type CellClass = 'none' | 'shared' | 'thin' | 'up' | 'some' | 'most' | 'down';

/**
 * A cell is judged on the samples outside shared cycles; thin coverage (under half the cell measured) wins over any answer.
 * `from` and `until` bound what could have been measured: the resolver's first check and the present.
 */
export function cellClass(c: MonitorCell | undefined, cellMs: number, from = -Infinity, until = Infinity): CellClass {
  if (!c || c[1] === 0) return 'none';
  const shared = c[6] ?? 0;
  const covered = c[7] ?? null;
  const span = Math.min(until, c[0] + cellMs) - Math.max(from, c[0]);
  if (covered != null && covered < span / 2) return 'thin';
  const own = c[1] - shared;
  if (own <= 0) return shared > 0 ? 'shared' : 'none';
  const r = c[2] / own;
  return r >= 0.99 ? 'up' : r >= 0.9 ? 'some' : r > 0 ? 'most' : 'down';
}

/** Why most of a cell went unmeasured: the cause with more of the gap time, when gaps fill at least half of `[start, end)`. */
export function gapCause(gaps: MonitorGap[], start: number, end: number): MonitorGap[2] | null {
  const lost = (cause: MonitorGap[2]) => gaps.reduce((ms, g) => (g[2] === cause ? ms + Math.max(0, Math.min(g[1], end) - Math.max(g[0], start)) : ms), 0);
  const [sleep, stall] = [lost('sleep'), lost('stall')];
  if (sleep + stall <= 0 || sleep + stall < (end - start) / 2) return null;
  return sleep >= stall ? 'sleep' : 'stall';
}

/** The headline answered share: outside shared cycles, falling back to the raw one. */
export const ownSuccess = (h: { query_success: number | null; query_success_own: number | null }): number | null => h.query_success_own ?? h.query_success;

/** The outages table's "Seen as" text. */
export function seenAs(o: Pick<MonitorOutage, 'cycles' | 'shared' | 'unclear'>): string {
  if (!o.shared && !o.unclear) return 'This resolver only';
  if (o.shared === o.cycles) return 'Several operators at once';
  if (o.unclear === o.cycles) return 'Unclear';
  if (o.shared + o.unclear >= o.cycles) return 'Several operators at once, or unclear';
  return `Mixed (${o.cycles - o.shared - o.unclear} of ${o.cycles} cycles this resolver only)`;
}
