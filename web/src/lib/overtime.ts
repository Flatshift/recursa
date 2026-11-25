// Pure helpers of the over-time chart: the cell grid, label collision, time ticks and the line and band paths.

/** How a cell's lost queries read on the loss strip: the share lost, or why a cell is not judged. */
export type LossClass = 'up' | 'some' | 'most' | 'down' | 'shared' | 'thin';

/** One bin of one series, times in ms. Latencies are in ms and null when nothing answered. `note` is appended to the tooltip. */
export interface OverTimeCell {
  t0: number;
  t1: number;
  p5: number | null;
  p25: number | null;
  p50: number | null;
  p75: number | null;
  p95: number | null;
  sent: number;
  lost: number;
  cls?: LossClass;
  note?: string;
}

/** `cells` is one slot per bin of the shared grid, null where the series has nothing. */
export interface OverTimeSeries {
  id: string;
  name: string;
  sub?: string;
  cells: (OverTimeCell | null)[];
}

/** A stretch drawn hatched and named across every panel: asleep, stalled, or left out of the figures. */
export interface OverTimeGap {
  t0: number;
  t1: number;
  kind: string;
  label: string;
  /** Part of the plan, not a fault: dotted instead of hatched, and named inside the plot. */
  soft?: boolean;
}

/** Why every resolver slowed at once, as far as the run can tell. */
export type SlowKind = 'busy' | 'traffic' | 'wifi' | 'local' | 'upstream' | 'shared';

/** A stretch where every resolver slowed together. Unlike a gap its cells still draw and still count. */
export interface OverTimeSlow {
  t0: number;
  t1: number;
  kind: SlowKind;
  label: string;
}

/** Where the cause sits; each family has its own mark on the chart and in its legend. */
export const SLOW_FAMILY: Record<SlowKind, 'host' | 'net' | 'up' | 'any'> = {
  busy: 'host',
  traffic: 'host',
  wifi: 'net',
  local: 'net',
  upstream: 'up',
  shared: 'any',
};

/** An operator-reported incident on one series' panel. */
export interface OverTimeMark {
  id: string;
  t: number;
  title: string;
}

/** A struck-through stretch on one series' panel; `shared`: several operators failed at once, so not this series' own. */
export interface OverTimeOutage {
  id: string;
  t0: number;
  t1: number;
  shared: boolean;
  label: string;
}

/** Slot `i` starts at `base + i * width`. Null when no series has a cell. */
export function slotGrid(series: OverTimeSeries[]): { base: number; width: number; n: number } | null {
  const n = Math.max(0, ...series.map((s) => s.cells.length));
  for (const s of series)
    for (let i = 0; i < s.cells.length; i++) {
      const c = s.cells[i];
      if (c && c.t1 > c.t0) return { base: c.t0 - i * (c.t1 - c.t0), width: c.t1 - c.t0, n };
    }
  return null;
}

/** Index of the last non-null entry, -1 when there is none. */
export function lastFilled(a: unknown[]): number {
  for (let i = a.length - 1; i >= 0; i--) if (a[i] != null) return i;
  return -1;
}

/** Series `b` with each cell eased from its value in `a` by `t` (0..1), in log space as the axes are. A cell new in `b` shows as it is. */
export function blendSeries(a: OverTimeSeries[], b: OverTimeSeries[], t: number): OverTimeSeries[] {
  const mix = (x: number | null, y: number | null) => (x != null && y != null && x > 0 && y > 0 ? x * (y / x) ** t : y);
  return b.map((s, i) => ({
    ...s,
    cells: s.cells.map((c, k) => {
      const p = a[i]?.cells[k];
      return c && p ? { ...c, p5: mix(p.p5, c.p5), p25: mix(p.p25, c.p25), p50: mix(p.p50, c.p50), p75: mix(p.p75, c.p75), p95: mix(p.p95, c.p95) } : c;
    }),
  }));
}

/** The slot under a pixel x of a plot starting at `x0` and `w` wide; null outside it. */
export function slotAt(px: number, x0: number, w: number, n: number): number | null {
  const i = Math.floor(((px - x0) / w) * n);
  return n > 0 && px >= x0 && i >= 0 && i < n ? i : null;
}

/** The loss strip's class for a cell: the caller's, else by share lost; null when nothing was lost. */
export function lossClass(c: Pick<OverTimeCell, 'sent' | 'lost' | 'cls'>): LossClass | null {
  if (c.cls) return c.cls;
  if (!c.lost || !c.sent) return null;
  const f = c.lost / c.sent;
  return f >= 0.25 ? 'down' : f >= 0.1 ? 'most' : 'some';
}

/** Bar height on the loss strip, 0..1. A cell too thinly measured to judge has none. */
export function lossHeight(c: Pick<OverTimeCell, 'sent' | 'lost'>, cls: LossClass): number {
  if (cls === 'thin') return 0;
  if (cls === 'up') return 0.14;
  if (cls === 'shared') return 0.32;
  return Math.min(1, 0.32 + (c.sent ? c.lost / c.sent : 0) * 6);
}

/** A line through the points, broken at every null. A lone point becomes a short tick `hw` either side. */
export function linePath(xs: number[], ys: (number | null)[], hw = 0): string {
  let d = '';
  let run = 0;
  const f = (v: number) => v.toFixed(1);
  for (let i = 0; i <= xs.length; i++) {
    const y = i < xs.length ? ys[i] : null;
    if (y == null) {
      if (run === 1 && hw > 0) d += `m${f(-hw)} 0h${f(2 * hw)}`;
      run = 0;
      continue;
    }
    if (run === 0) d += `M${f(xs[i])} ${f(y)}`;
    else d += `L${f(xs[i])} ${f(y)}`;
    run++;
  }
  return d;
}

/** The filled band between `hi` and `lo` (pixel y per slot), one shape per unbroken run. */
export function bandPath(xs: number[], hi: (number | null)[], lo: (number | null)[], hw: number): string {
  const f = (v: number) => v.toFixed(1);
  let d = '';
  let from = -1;
  for (let i = 0; i <= xs.length; i++) {
    const ok = i < xs.length && hi[i] != null && lo[i] != null;
    if (ok && from < 0) from = i;
    if (!ok && from >= 0) {
      const a = from;
      const b = i - 1;
      d += `M${f(xs[a] - hw)} ${f(hi[a]!)}`;
      for (let k = a; k <= b; k++) d += `L${f(xs[k])} ${f(hi[k]!)}`;
      d += `L${f(xs[b] + hw)} ${f(hi[b]!)}L${f(xs[b] + hw)} ${f(lo[b]!)}`;
      for (let k = b; k >= a; k--) d += `L${f(xs[k])} ${f(lo[k]!)}`;
      d += `L${f(xs[a] - hw)} ${f(lo[a]!)}Z`;
      from = -1;
    }
  }
  return d;
}

/** Label positions kept `gap` apart inside [lo, hi], as near their wanted y as that allows. Same order as `want`. */
export function spreadLabels(want: number[], gap: number, lo: number, hi: number): number[] {
  const order = want.map((_, i) => i).sort((a, b) => want[a] - want[b]);
  const pos = order.map((i) => want[i]);
  const last = pos.length - 1;
  for (let k = 0; k <= last; k++) pos[k] = Math.max(k ? pos[k - 1] + gap : lo, pos[k]);
  if (last >= 0 && pos[last] > hi) pos[last] = hi;
  for (let k = last - 1; k >= 0; k--) pos[k] = Math.min(pos[k], pos[k + 1] - gap);
  for (let k = 0; k <= last; k++) pos[k] = Math.max(k ? pos[k - 1] + gap : lo, pos[k]);
  const out = new Array<number>(want.length);
  order.forEach((i, k) => (out[i] = pos[k]));
  return out;
}

/** Which horizontal spans can be drawn without touching one already kept, trying them in the order given. */
export function fitSpans(spans: [number, number][], pad = 6): boolean[] {
  const kept: [number, number][] = [];
  return spans.map(([a, b]) => {
    if (kept.some(([c, d]) => a < d + pad && b + pad > c)) return false;
    kept.push([a, b]);
    return true;
  });
}

const STEPS = [15e3, 30e3, 60e3, 120e3, 300e3, 600e3, 900e3, 1.8e6, 3.6e6, 7.2e6, 1.08e7, 2.16e7, 4.32e7, 8.64e7, 1.728e8, 6.048e8];

/** Round times in [t0, t1) on the coarsest-fitting step of 15 s up to a week, at most `max` of them; hours and days fall on local boundaries. */
export function timeTicks(t0: number, t1: number, max: number): number[] {
  const step = STEPS.find((s) => (t1 - t0) / s <= max) ?? STEPS[STEPS.length - 1];
  const off = -new Date(t0).getTimezoneOffset() * 60_000;
  const out: number[] = [];
  for (let t = Math.ceil((t0 + off) / step) * step - off; t < t1; t += step) out.push(t);
  return out;
}
