// Pure helpers of the Extended test's screens. The engine decides the verdict, the gaps and the calls; these only
// lay its figures out for the shared over-time chart, the setup and the text.

import type { ExtBin, ExtBinCell, ExtCall, ExtendedState, ExtGroup, ExtNames, ExtRow, HostSample } from './types';
import type { OverTimeCell, OverTimeGap, OverTimeSeries, OverTimeSlow, SlowKind } from './overtime';

export const MIN_PICK = 2;
export const MAX_PICK = 4;

/** The mark shapes, by position in the run: the same on the chart, in the legend and in the table. */
export const MARKS = ['circle', 'square', 'triangle', 'diamond'] as const;

export const SLOW_LABEL: Record<SlowKind, string> = {
  busy: 'Slowed by this computer',
  traffic: 'Slowed by other traffic',
  wifi: 'Slowed by Wi-Fi',
  local: 'Slowed by local network',
  upstream: 'Slowed by internet link',
  shared: 'All slowed together',
};

/** A resolver is slow in a bin at this multiple of its own median bin p75... */
const SLOW_FACTOR = 1.5;
/** ...and at least this many ms above it. */
const SLOW_MIN_MS = 3;
/** A stretch needs this many shared-slow bins. */
const SLOW_MIN_BINS = 2;
/** Host thresholds, mirroring crates/engine/src/link.rs. */
const HOST_LOOPBACK_US = 5_000;
const HOST_LAG_US = 20_000;
const HOST_TRAFFIC_BPS = 5_000_000;
const HOST_AWDL_BPS = 100_000;
const ECHO_STALL_FLOOR_US = 20_000;
const ECHO_STALL_FACTOR = 5;
const ECHO_STALL_SHARE = 0.02;
/** The share of host rows that must trip a threshold to name the cause. */
const HOST_SHARE = 0.2;
export type Mark = (typeof MARKS)[number];

/** A resolver's colour token and mark by its position among the run's targets. */
export const colourOf = (t: number): string => `var(--r-${(t % 4) + 1})`;
export const markOf = (t: number): Mark => MARKS[t % 4];

export const CALL_LABEL: Record<ExtCall, string> = {
  lead: 'Lowest median',
  tie: 'Practical tie',
  slower: 'Slower',
  unclear: 'Unclear',
};

export const GROUP_LABEL: Record<string, string> = {
  common_first: 'Common names, first ask',
  common_later: 'Common names, later asks',
  tail: 'Less common names',
  tail_com: 'Less common, .com',
  tail_generic: 'Less common, other generic domains',
  tail_cc: 'Less common, country domains',
};

/** The part of the world each internet registry serves. */
export const REGION_LABEL: Record<string, string> = {
  afrinic: 'Africa',
  apnic: 'Asia Pacific',
  arin: 'North America',
  lacnic: 'Latin America and the Caribbean',
  ripe: 'Europe, the Middle East and Central Asia',
};

export const GAP_LABEL = { sleep: 'Computer asleep', stall: 'Recursa stalled', removed: 'Left out', warmup: 'Cache filling' } as const;

/** The resolvers Setup offers from a ranked list: the top `top`, then the current resolver if it is not among them. */
export function shortlist<T extends { id: string }>(ranked: T[], currentIds: string[], top = 3): (T & { current: boolean })[] {
  const out = ranked.slice(0, top);
  for (const r of ranked.slice(top)) if (currentIds.includes(r.id)) out.push(r);
  return out.map((r) => ({ ...r, current: currentIds.includes(r.id) }));
}

/** The picks after ticking or unticking one: never past the most the test takes. */
export function toggled(picked: string[], id: string): string[] {
  if (picked.includes(id)) return picked.filter((x) => x !== id);
  return picked.length >= MAX_PICK ? picked : [...picked, id];
}

/** The slot of a bin on the chart's grid: sleeps leave empty slots rather than shifting the later bins. */
export const slotOf = (bin: Pick<ExtBin, 'at'>, binS: number): number => Math.round(bin.at / (binS * 1000));

/** Series and gaps for the over-time chart, times in ms since the run began. `names` is by target index. */
export function chartOf(ext: Pick<ExtendedState, 'bins' | 'gaps' | 'bin_s'>, names: string[]): { series: OverTimeSeries[]; gaps: OverTimeGap[] } {
  const w = ext.bin_s * 1000;
  const slots = ext.bins.reduce((m, b) => Math.max(m, slotOf(b, ext.bin_s) + 1), 0);
  const series = names.map((name, t): OverTimeSeries => {
    const cells: (OverTimeCell | null)[] = Array.from({ length: slots }, () => null);
    for (const b of ext.bins) {
      const c = b.r[t];
      if (!c || b.removed || c.sent === 0) continue;
      const i = slotOf(b, ext.bin_s);
      cells[i] = { t0: i * w, t1: (i + 1) * w, p5: c.p5, p25: c.p25, p50: c.p50, p75: c.p75, p95: c.p95, sent: c.sent, lost: c.lost };
    }
    return { id: String(t), name, cells };
  });
  return { series, gaps: gapsOf(ext) };
}

/** The sleeps, stalls and left-out stretches, times in ms since the run began. */
function gapsOf(ext: Pick<ExtendedState, 'bins' | 'gaps' | 'bin_s'>): OverTimeGap[] {
  const w = ext.bin_s * 1000;
  const gaps: OverTimeGap[] = ext.gaps.map(([t0, t1, kind]) => ({ t0, t1, kind, label: GAP_LABEL[kind] }));
  // A left-out bin is a gap too; neighbours merge into one stretch.
  for (const b of ext.bins) {
    if (!b.removed) continue;
    const i = slotOf(b, ext.bin_s);
    const last = gaps.find((g) => g.kind === 'removed' && g.t1 === i * w);
    if (last) last.t1 = (i + 1) * w;
    else gaps.push({ t0: i * w, t1: (i + 1) * w, kind: 'removed', label: GAP_LABEL.removed });
  }
  return gaps.sort((a, b) => a.t0 - b.t0);
}

/**
 * The split charts of a run that has them: repeat lookups in `bin_s` bins and cold lookups in `cold_bin_s` bins, on one
 * time axis. Null for a run from before the split.
 */
export function chartsOf(
  ext: Pick<ExtendedState, 'bins' | 'gaps' | 'bin_s' | 'cold_bin_s' | 'cold_bins'>,
  names: string[],
): {
  repeat: { series: OverTimeSeries[]; gaps: OverTimeGap[] };
  cold: { series: OverTimeSeries[]; gaps: OverTimeGap[] };
  all: { series: OverTimeSeries[]; gaps: OverTimeGap[] } | null;
} | null {
  if (ext.cold_bin_s == null || !ext.cold_bins) return null;
  const coldBins = ext.cold_bins;
  const w = ext.bin_s * 1000;
  const cw = ext.cold_bin_s * 1000;
  const k = ext.cold_bin_s / ext.bin_s;
  const fine = ext.bins.reduce((m, b) => Math.max(m, slotOf(b, ext.bin_s) + 1), 0);
  const coldSlots = Math.ceil(fine / k);
  const repeatSlots = coldSlots * k;
  let firstRepeat = -1;
  const fineCell = (i: number, c: ExtBinCell): OverTimeCell => ({ t0: i * w, t1: (i + 1) * w, p5: c.p5, p25: c.p25, p50: c.p50, p75: c.p75, p95: c.p95, sent: c.sent, lost: c.lost });
  const fineSeries = (pick: (b: ExtBin) => ExtBinCell[] | undefined, mark: boolean) =>
    names.map((name, t): OverTimeSeries => {
      const cells: (OverTimeCell | null)[] = Array.from({ length: repeatSlots }, () => null);
      for (const b of ext.bins) {
        const c = pick(b)?.[t];
        if (!c || b.removed || c.sent === 0) continue;
        const i = slotOf(b, ext.bin_s);
        cells[i] = fineCell(i, c);
        if (mark && (firstRepeat < 0 || i < firstRepeat)) firstRepeat = i;
      }
      return { id: String(t), name, cells };
    });
  const repeat = fineSeries((b) => b.rep, true);
  // While the caches fill there are no repeat asks yet, so the warm-up shows the first asks that fill them (everything
  // asked then, on a run from before first asks were kept apart).
  for (const b of ext.bins) {
    const i = slotOf(b, ext.bin_s);
    if (b.removed || i >= firstRepeat) continue;
    repeat.forEach((s, t) => {
      const c = (b.first ?? b.r)[t];
      if (c?.sent) s.cells[i] = fineCell(i, c);
    });
  }
  const all = ext.bins.some((b) => b.all) ? fineSeries((b) => b.all, false) : null;
  const cold = names.map((name, t): OverTimeSeries => {
    const cells: (OverTimeCell | null)[] = Array.from({ length: coldSlots }, () => null);
    for (const b of coldBins) {
      const c = b.r[t];
      if (!c || b.removed || c.sent === 0) continue;
      // The cold slot its first bin falls in: after a sleep that bin can start anywhere in the slot.
      const i = Math.floor(slotOf(b, ext.bin_s) / k);
      cells[i] = { t0: i * cw, t1: (i + 1) * cw, p5: c.p5, p25: c.p25, p50: c.p50, p75: c.p75, p95: c.p95, sent: c.sent, lost: c.lost };
    }
    return { id: String(t), name, cells };
  });
  const gaps = gapsOf(ext);
  const warm: OverTimeGap[] =
    firstRepeat > 0 ? [{ t0: 0, t1: firstRepeat * w, kind: 'warmup', label: GAP_LABEL.warmup, soft: true }] : [];
  const repeatGaps = () => [...warm, ...gaps.map((g) => ({ ...g }))].sort((a, b) => a.t0 - b.t0);
  return {
    repeat: { series: repeat, gaps: repeatGaps() },
    cold: { series: cold, gaps },
    // While the caches fill, these bins hold only names asked once, so the warm-up is marked as on the repeat chart.
    all: all && { series: all, gaps: repeatGaps() },
  };
}

/** The stretches where every resolver slowed together, from the repeat lookups; the run's ranking still counts them. Runs from before the split charts have none. */
export function slowsOf(
  ext: Pick<ExtendedState, 'bins' | 'bin_s'>,
  lan: boolean[],
  link?: { samples: number[][]; p50: number; host?: { samples: HostSample[] } },
): OverTimeSlow[] {
  const bins = ext.bins.filter((b) => b.rep && !b.removed).sort((a, b) => slotOf(a, ext.bin_s) - slotOf(b, ext.bin_s));
  if (!bins.length) return [];
  const usable = (b: ExtBin, t: number) => {
    const c = b.rep![t];
    return c && c.sent > 0 && c.p75 != null ? c.p75 : null;
  };
  const base = lan.map((_, t) => {
    const v = bins.map((b) => usable(b, t)).filter((x): x is number => x != null).sort((a, b) => a - b);
    const h = v.length >> 1;
    return v.length ? (v.length % 2 ? v[h] : (v[h - 1] + v[h]) / 2) : null;
  });
  const slow = (b: ExtBin, t: number) => {
    const p = usable(b, t);
    return p != null && base[t] != null && p >= SLOW_FACTOR * base[t]! && p - base[t]! >= SLOW_MIN_MS;
  };
  const remote = lan.flatMap((on, t) => (on ? [] : [t]));
  const trigger = remote.length ? remote : lan.map((_, t) => t);
  const hot = bins.filter((b) => trigger.every((t) => slow(b, t)) && lan.filter((_, t) => slow(b, t)).length >= 2);
  const runs: ExtBin[][] = [];
  for (const b of hot) {
    const last = runs[runs.length - 1];
    if (last && slotOf(b, ext.bin_s) - slotOf(last[last.length - 1], ext.bin_s) <= 2) last.push(b);
    else runs.push([b]);
  }
  const w = ext.bin_s * 1000;
  return runs
    .filter((run) => run.length >= SLOW_MIN_BINS)
    .map((run): OverTimeSlow => {
      const t0 = slotOf(run[0], ext.bin_s) * w;
      const t1 = (slotOf(run[run.length - 1], ext.bin_s) + 1) * w;
      const rows = (link?.host?.samples ?? []).filter((r) => r[0] >= t0 && r[0] < t1);
      const share = (hit: (r: HostSample) => boolean) => rows.filter(hit).length / rows.length;
      const echoes = (link?.samples ?? []).filter((s) => s[0] >= t0 && s[0] < t1);
      const stallUs = Math.max(ECHO_STALL_FLOOR_US, ECHO_STALL_FACTOR * (link?.p50 ?? 0) * 1000);
      // A long run stores one row a second, with its own counts of stalled and sent echoes.
      const sent = echoes.reduce((n, s) => n + (s[3] ?? 1), 0);
      const stalled = echoes.reduce((n, s) => n + (s[2] ?? (s[1] === 0 || s[1] > stallUs ? 1 : 0)), 0);
      let kind: SlowKind;
      if (rows.length && share((r) => (r[1] ?? 0) > HOST_LOOPBACK_US || r[2] > HOST_LAG_US) >= HOST_SHARE) kind = 'busy';
      else if (rows.length && share((r) => (r[3] ?? 0) + (r[4] ?? 0) > HOST_TRAFFIC_BPS) >= HOST_SHARE) kind = 'traffic';
      else if (rows.length && share((r) => (r[6] ?? 0) > HOST_AWDL_BPS) >= HOST_SHARE) kind = 'wifi';
      else if (sent && stalled / sent > ECHO_STALL_SHARE) kind = 'local';
      else if (lan.some((l, t) => l && run.filter((b) => slow(b, t)).length >= run.length / 2)) kind = 'local';
      else kind = lan.some(Boolean) ? 'upstream' : 'shared';
      return { t0, t1, kind, label: SLOW_LABEL[kind] };
    });
}

export type VerdictKind = 'Practical tie for first' | 'Lowest median' | 'Too close to call' | 'No verdict';

export interface ExtVerdict {
  kind: VerdictKind;
  /** The resolvers the headline names, by target index. */
  head: number[];
  lead: ExtRow | null;
  next: ExtRow | null;
}

/** Read the engine's verdict: the lead and the next row, and the call the next row carries. No figure is worked out here. */
export function verdictOf(rows: ExtRow[], noVerdict: boolean): ExtVerdict {
  const lead = rows[0] ?? null;
  const next = rows[1] ?? null;
  if (noVerdict || !lead || !next || !next.call) return { kind: 'No verdict', head: [], lead, next };
  if (next.call === 'slower') return { kind: 'Lowest median', head: [lead.t], lead, next };
  return { kind: next.call === 'tie' ? 'Practical tie for first' : 'Too close to call', head: [lead.t, next.t], lead, next };
}

/** How many times each common name was asked: 3 in runs from before the eight-ask plan, 8 since, 16 in a 10,000 run. */
export function asksOf(names: Pick<ExtNames, 'common' | 'tail'>, total: number): number {
  return names.common ? Math.round((total - names.tail) / names.common) : 0;
}

/** A gap and its range as the table prints it: "+1.2" and "0.4 to 2.0". */
export function rangeOf(v: { lo: number; hi: number }): string {
  return `${v.lo.toFixed(1)} to ${v.hi.toFixed(1)}`;
}

/** How many minutes a run takes: a count of lookups at one per `gap_ms`. */
export function minutesOf(total: number, gapMs: number): number {
  return Math.round((total * gapMs) / 60000);
}

/** The group rows by key, the engine's order. Groups nothing was asked of are left out. */
export function groupsOf(groups: ExtGroup[]): ExtGroup[] {
  return groups.filter((g) => g.count > 0);
}

/** The ECDF as one polyline per resolver: points are [threshold ms, share 0 to 1]. */
export function ecdfPoints(e: { x: number[]; r: number[][] }, t: number): [number, number][] {
  const r = e.r[t] ?? [];
  return e.x.map((x, i): [number, number] => [x, r[i] ?? 0]);
}

/** An SVG path for a mark of half-width `r` centred on (cx, cy). */
export function markPath(shape: Mark, cx: number, cy: number, r: number): string {
  const f = (n: number) => +n.toFixed(2);
  switch (shape) {
    case 'circle':
      return `M${f(cx - r)},${f(cy)}a${r},${r} 0 1,0 ${2 * r},0a${r},${r} 0 1,0 ${-2 * r},0z`;
    case 'square':
      return `M${f(cx - r)},${f(cy - r)}h${2 * r}v${2 * r}h${-2 * r}z`;
    case 'triangle':
      return `M${f(cx)},${f(cy - r)}L${f(cx + r)},${f(cy + r * 0.8)}L${f(cx - r)},${f(cy + r * 0.8)}z`;
    case 'diamond':
      return `M${f(cx)},${f(cy - r * 1.2)}L${f(cx + r * 1.2)},${f(cy)}L${f(cx)},${f(cy + r * 1.2)}L${f(cx - r * 1.2)},${f(cy)}z`;
  }
}

/** Lookups sent to each resolver so far. While the run sends, its answer count says, several times a second; after, the
 * bins do, as a bin's own counts leave out what the figures leave out. */
export function sentOf(ext: Pick<ExtendedState, 'bins' | 'total' | 'per_bin'>, live?: { done: number; total: number }): number {
  if (live && live.total > 0) return Math.min(ext.total, Math.floor((live.done / live.total) * ext.total));
  return Math.min(ext.total, ext.bins.length * ext.per_bin);
}

/** Minutes left at the run's own pace, rounded up. */
export function minutesLeft(ext: Pick<ExtendedState, 'bins' | 'total' | 'per_bin' | 'bin_s'>, live?: { done: number; total: number }): number {
  return Math.max(0, Math.ceil(((ext.total - sentOf(ext, live)) * ext.bin_s) / ext.per_bin / 60));
}

/** The fine slots a live run fills at its own pace, so its chart keeps one width and fills from the left. A sleep adds slots. */
export function plannedSlots(ext: Pick<ExtendedState, 'bins' | 'total' | 'per_bin' | 'bin_s'>): number {
  const fine = ext.bins.reduce((m, b) => Math.max(m, slotOf(b, ext.bin_s) + 1), 0);
  return fine + Math.max(0, Math.ceil(ext.total / ext.per_bin) - ext.bins.length);
}

/** The seconds of a gap list that were of one kind. */
export function gapSeconds(gaps: ExtendedState['gaps'], kind: 'sleep' | 'stall'): number {
  return Math.round(gaps.filter((g) => g[2] === kind).reduce((n, g) => n + (g[1] - g[0]), 0) / 1000);
}
