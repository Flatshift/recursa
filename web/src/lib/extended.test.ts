import { describe, expect, it } from 'vitest';
import { asksOf, binsDone, gapSeconds, markPath, minutesLeft, sentOf, chartOf, chartsOf, slowsOf, colourOf, ecdfPoints, groupsOf, markOf, minutesOf, plannedSlots, rangeOf, shortlist, slotOf, toggled, verdictOf } from './extended';
import type { ExtBin, ExtRow } from './types';

const cell = (p50: number | null, sent = 50, lost = 0) => ({ sent, lost, p5: p50, p25: p50, p50, p75: p50, p95: p50 });
const bin = (at: number, removed = false, p = 10): ExtBin => ({ at, end: at + 15000, removed, r: [cell(p), cell(p + 1)] });
const row = (t: number, call: ExtRow['call']): ExtRow => ({ t, sent: 5000, lost: 0, loss_pct: 0, blocked: 0, p50: 10, p90: 12, p95: 14, vs: null, call });

describe('shortlist', () => {
  const ranked = ['a', 'b', 'c', 'd', 'e'].map((id) => ({ id }));
  it('is the top three plus the current resolver', () => {
    expect(shortlist(ranked, ['e']).map((r) => r.id)).toEqual(['a', 'b', 'c', 'e']);
  });
  it('does not repeat a current resolver already in the top three', () => {
    const s = shortlist(ranked, ['b']);
    expect(s.map((r) => r.id)).toEqual(['a', 'b', 'c']);
    expect(s[1].current).toBe(true);
  });
  it('is shorter when fewer ran', () => {
    expect(shortlist(ranked.slice(0, 2), [])).toHaveLength(2);
  });
});

describe('toggled', () => {
  it('adds up to four and always allows removing', () => {
    expect(toggled(['a'], 'b')).toEqual(['a', 'b']);
    expect(toggled(['a', 'b', 'c', 'd'], 'e')).toEqual(['a', 'b', 'c', 'd']);
    expect(toggled(['a', 'b', 'c', 'd'], 'b')).toEqual(['a', 'c', 'd']);
  });
});

describe('chartOf', () => {
  it('places bins by wall-clock slot, leaving a hole for a sleep', () => {
    const { series, gaps } = chartOf({ bin_s: 15, bins: [bin(0), bin(15000), bin(60000)], gaps: [[30000, 60000, 'sleep']] }, ['A', 'B']);
    expect(series[0].cells.map((c) => c?.p50 ?? null)).toEqual([10, 10, null, null, 10]);
    expect(series[1].cells[4]?.p50).toBe(11);
    expect(gaps).toEqual([{ t0: 30000, t1: 60000, kind: 'sleep', label: 'Computer asleep' }]);
  });
  it('turns removed bins into one merged left-out stretch with no cells', () => {
    const { series, gaps } = chartOf({ bin_s: 15, bins: [bin(0), bin(15000, true), bin(30000, true), bin(45000)], gaps: [] }, ['A', 'B']);
    expect(series[0].cells.map((c) => c != null)).toEqual([true, false, false, true]);
    expect(gaps).toEqual([{ t0: 15000, t1: 45000, kind: 'removed', label: 'Left out' }]);
  });
});

describe('chartsOf', () => {
  // Ten fine bins; the first two are warm-up, so they have no repeat figures.
  const fine = (i: number): ExtBin => ({ ...bin(i * 15000), rep: i < 2 ? [cell(null, 0), cell(null, 0)] : [cell(8), cell(9)] });
  const coldBin = (at: number): ExtBin => ({ at, end: at + 75000 - 15000, removed: false, r: [cell(40, 10), cell(41, 10)] });
  const split = {
    bin_s: 15,
    cold_bin_s: 75,
    bins: Array.from({ length: 10 }, (_, i) => fine(i)),
    cold_bins: [coldBin(0), coldBin(75000)],
    gaps: [] as [number, number, 'sleep' | 'stall'][],
  };
  it('is null for a run from before the split', () => {
    expect(chartsOf({ bin_s: 15, bins: [bin(0)], gaps: [], cold_bin_s: undefined, cold_bins: undefined }, ['A', 'B'])).toBeNull();
  });
  it('shows the first asks of the warm-up bins in the repeat chart and shades them as a soft gap', () => {
    const firsts = { ...split, bins: split.bins.map((b) => ({ ...b, first: [cell(30), cell(null, 0)] })) };
    const c = chartsOf(firsts, ['A', 'B'])!;
    expect(c.repeat.series[0].cells.map((x) => x?.p50)).toEqual([30, 30, 8, 8, 8, 8, 8, 8, 8, 8]);
    expect(c.repeat.series[1].cells.map((x) => x?.p50)).toEqual([undefined, undefined, 9, 9, 9, 9, 9, 9, 9, 9]);
    expect(c.repeat.gaps).toEqual([{ t0: 0, t1: 30000, kind: 'warmup', label: 'Cache filling', soft: true }]);
  });
  it('shows every lookup of the warm-up bins on a run without first-ask cells', () => {
    const c = chartsOf(split, ['A', 'B'])!;
    expect(c.repeat.series[0].cells.map((x) => x?.p50)).toEqual([10, 10, 8, 8, 8, 8, 8, 8, 8, 8]);
    expect(c.repeat.series[1].cells.map((x) => x?.p50)).toEqual([11, 11, 9, 9, 9, 9, 9, 9, 9, 9]);
    expect(c.repeat.gaps).toEqual([{ t0: 0, t1: 30000, kind: 'warmup', label: 'Cache filling', soft: true }]);
    expect(c.cold.gaps).toEqual([]);
  });
  it('has no warm-up gap when the first bin has repeat cells', () => {
    const c = chartsOf({ ...split, bins: split.bins.map((b) => ({ ...b, rep: [cell(8), cell(9)] })) }, ['A', 'B'])!;
    expect(c.repeat.gaps).toEqual([]);
  });
  it('is null for all lookups when no bin has them, and otherwise shares the repeat slots and gaps', () => {
    expect(chartsOf(split, ['A', 'B'])!.all).toBeNull();
    const c = chartsOf({ ...split, bins: split.bins.map((b, i) => ({ ...b, all: [cell(12 + i), cell(13)] })) }, ['A', 'B'])!;
    expect(c.all!.series[0].cells).toHaveLength(10);
    expect(c.all!.series[0].cells[0]!.p50).toBe(12);
    expect(c.all!.gaps).toEqual(c.repeat.gaps);
    expect(c.all!.gaps[0].kind).toBe('warmup');
  });
  it('draws cold cells one cold bin wide over the same span', () => {
    const c = chartsOf(split, ['A', 'B'])!;
    expect(c.cold.series[0].cells.map((x) => (x ? x.t1 - x.t0 : null))).toEqual([75000, 75000]);
    expect(c.repeat.series[0].cells).toHaveLength(10);
    expect(c.repeat.series[0].cells.length * 15000).toBe(c.cold.series[0].cells.length * 75000);
  });
  it('shares the sleep gaps and rounds a short run up to whole cold bins', () => {
    const c = chartsOf({ ...split, bins: split.bins.slice(0, 7), gaps: [[30000, 45000, 'sleep']] }, ['A', 'B'])!;
    expect(c.cold.series[0].cells).toHaveLength(2);
    expect(c.repeat.series[0].cells).toHaveLength(10);
    expect(c.cold.gaps).toEqual([{ t0: 30000, t1: 45000, kind: 'sleep', label: 'Computer asleep' }]);
    expect(c.repeat.gaps.map((g) => g.kind)).toEqual(['warmup', 'sleep']);
  });
  it('keeps a cold bin that starts late in its slot after a sleep inside the span', () => {
    const bins = [...split.bins.slice(0, 5), fine(8)];
    const c = chartsOf({ ...split, bins, cold_bins: [coldBin(0), coldBin(120000)] }, ['A', 'B'])!;
    expect(c.cold.series[0].cells.map((x) => x != null)).toEqual([true, true]);
    expect(c.repeat.series[0].cells).toHaveLength(10);
  });
});

describe('slotOf', () => {
  it('rounds to the nearest slot', () => {
    expect(slotOf({ at: 15200 }, 15)).toBe(1);
    expect(slotOf({ at: 29800 }, 15)).toBe(2);
  });
});

describe('verdictOf', () => {
  it('reads the next row call', () => {
    expect(verdictOf([row(2, 'lead'), row(0, 'slower')], false)).toMatchObject({ kind: 'Lowest median', head: [2] });
    expect(verdictOf([row(2, 'lead'), row(0, 'tie')], false)).toMatchObject({ kind: 'Practical tie for first', head: [2, 0] });
    expect(verdictOf([row(2, 'lead'), row(0, 'unclear')], false)).toMatchObject({ kind: 'Too close to call', head: [2, 0] });
  });
  it('has no verdict when the engine says so or no call was made', () => {
    expect(verdictOf([row(2, 'lead'), row(0, 'tie')], true).kind).toBe('No verdict');
    expect(verdictOf([row(2, null), row(0, null)], false).kind).toBe('No verdict');
    expect(verdictOf([], false).kind).toBe('No verdict');
  });
});

describe('small helpers', () => {
  it('formats a range', () => expect(rangeOf({ lo: 0.44, hi: 2.04 })).toBe('0.4 to 2.0'));
  it('counts bins with queries sent', () => expect(binsDone({ bins: [bin(0), { ...bin(15000), r: [cell(null, 0), cell(null, 0)] }] })).toBe(1));
  it('gives minutes of queries', () => expect(minutesOf(5000, 300)).toBe(25));
  it('keeps a resolver on one colour and mark', () => {
    expect(colourOf(0)).toBe('var(--r-1)');
    expect(colourOf(5)).toBe('var(--r-2)');
    expect(markOf(2)).toBe('triangle');
  });
  it('drops empty groups', () => expect(groupsOf([{ key: 'a', count: 0, r: [] }, { key: 'b', count: 3, r: [] }])).toHaveLength(1));
  it('pairs thresholds with a resolver share', () => expect(ecdfPoints({ x: [1, 2], r: [[0.1, 0.5]] }, 0)).toEqual([[1, 0.1], [2, 0.5]]));
});

describe('progress', () => {
  const ext = { total: 5000, per_bin: 50, bin_s: 15, bins: [bin(0), bin(15000)] };
  it('counts the lookups in the bins sent', () => expect(sentOf(ext)).toBe(100));
  it('counts a bin whose lookups are out of the figures', () => expect(sentOf({ ...ext, bins: [{ ...bin(0), r: [] }] })).toBe(50));
  it('counts minutes left at the run pace', () => expect(minutesLeft(ext)).toBe(Math.ceil((98 * 15) / 60)));
  it('counts minutes left at the Fast pace, whose bins are 3.75 s', () => expect(minutesLeft({ ...ext, bin_s: 3.75 })).toBe(Math.ceil((98 * 3.75) / 60)));
  it('counts the minutes of a length and pace', () => {
    expect(minutesOf(5000, 75)).toBe(6);
    expect(minutesOf(10000, 600)).toBe(100);
  });
  it('has none left at the end', () => expect(minutesLeft({ ...ext, total: 100 })).toBe(0));
  it('counts a live run by its answers, not its bins', () => {
    expect(sentOf(ext, { done: 3 * 120, total: 3 * 5000 })).toBe(120);
    expect(minutesLeft(ext, { done: 3 * 120, total: 3 * 5000 })).toBe(Math.ceil((4880 * 0.3) / 60));
  });
  it('adds up one kind of gap', () => expect(gapSeconds([[0, 30000, 'sleep'], [40000, 50000, 'stall'], [60000, 90000, 'sleep']], 'sleep')).toBe(60));
  it('plans the whole run on the chart', () => {
    expect(plannedSlots({ ...ext, bins: [] })).toBe(100);
    expect(plannedSlots(ext)).toBe(100);
    // A sleep after the second bin pushes the rest along by its slots.
    expect(plannedSlots({ ...ext, bins: [bin(0), bin(60000)] })).toBe(103);
    expect(plannedSlots({ ...ext, total: 100 })).toBe(2);
  });
});

describe('markPath', () => {
  it('draws a closed shape for each mark', () => {
    for (const m of ['circle', 'square', 'triangle', 'diamond'] as const) expect(markPath(m, 5, 5, 3)).toMatch(/z$/);
  });
});

describe('asksOf', () => {
  it('reads the ask count from the run, so older runs still say 3', () => {
    expect(asksOf({ common: 500, tail: 1000 }, 5000)).toBe(8);
    expect(asksOf({ common: 500, tail: 3500 }, 5000)).toBe(3);
    expect(asksOf({ common: 0, tail: 0 }, 0)).toBe(0);
  });
});

describe('slowsOf', () => {
  const rb = (i: number, p: number[]): ExtBin => ({ at: i * 15000, end: (i + 1) * 15000, removed: false, r: p.map((x) => cell(x)), rep: p.map((x) => cell(x)) });
  const ext = (bins: ExtBin[]) => ({ bins, bin_s: 15 });
  const steady = [20, 30, 40];
  const run = (slow: number[], hot = [60, 90, 120], n = 20) =>
    Array.from({ length: n }, (_, i) => rb(i, slow.includes(i) ? hot : steady));
  const remote = [false, false, false];
  const host = (over: Partial<Record<number, number>>): [number, number | null, number, number | null, number | null, number | null, number | null][] =>
    Array.from({ length: 60 }, (_, i) => {
      const r: [number, number | null, number, number | null, number | null, number | null, number | null] = [120000 + i * 1000, 0, 0, 0, 0, 0, 0];
      for (const [k, v] of Object.entries(over)) r[Number(k)] = v as number;
      return r;
    });

  it('is empty without repeat bins', () => {
    expect(slowsOf(ext([bin(0), bin(15000)]), remote)).toEqual([]);
  });
  it('marks a stretch where every resolver slowed', () => {
    expect(slowsOf(ext(run([8, 9, 10, 11])), remote)).toEqual([
      { t0: 120000, t1: 180000, kind: 'shared', label: 'All slowed together' },
    ]);
  });
  it('ignores one slow bin and one resolver slow alone', () => {
    expect(slowsOf(ext(run([8])), remote)).toEqual([]);
    const bins = Array.from({ length: 20 }, (_, i) => rb(i, i >= 8 && i < 12 ? [60, 30, 40] : steady));
    expect(slowsOf(ext(bins), remote)).toEqual([]);
  });
  it('bridges a one-bin hole', () => {
    const bins = run([8, 9, 11, 12]);
    expect(slowsOf(ext(bins), remote).map((s) => [s.t0, s.t1])).toEqual([[120000, 195000]]);
    expect(slowsOf(ext(bins.filter((b) => b.at !== 10 * 15000)), remote)).toHaveLength(1);
  });
  it('ignores a removed bin', () => {
    const bins = run([8, 9]);
    bins[9] = { ...bins[9], removed: true };
    expect(slowsOf(ext(bins), remote)).toEqual([]);
  });
  it('names the local network when a LAN resolver slowed too, else the internet link', () => {
    const lan = [true, false, false];
    expect(slowsOf(ext(run([8, 9, 10])), lan)[0].kind).toBe('local');
    const bins = Array.from({ length: 20 }, (_, i) => rb(i, i >= 8 && i < 11 ? [20, 90, 120] : steady));
    expect(slowsOf(ext(bins), lan)[0].kind).toBe('upstream');
  });
  it('names the host when its rows show it', () => {
    const bins = ext(run([8, 9, 10, 11]));
    const at = (samples: ReturnType<typeof host>) => slowsOf(bins, remote, { samples: [], p50: 5, host: { samples } })[0].kind;
    expect(at(host({ 6: 200_000 }))).toBe('wifi');
    expect(at(host({ 3: 4_000_000, 4: 2_000_000 }))).toBe('traffic');
    expect(at(host({ 1: 6_000 }))).toBe('busy');
    expect(at(host({ 1: 6_000, 3: 9_000_000 }))).toBe('busy');
  });
  it('names the local network from stalled gateway echoes', () => {
    const samples: [number, number][] = Array.from({ length: 60 }, (_, i) => [120000 + i * 1000, i < 5 ? 0 : 2000]);
    expect(slowsOf(ext(run([8, 9, 10, 11])), remote, { samples, p50: 2 })[0].kind).toBe('local');
  });
  it("weighs a long run's rows by their echo counts", () => {
    // Three seconds of sixty hold one stalled echo in ten: 3 of 600, not 3 of 60.
    const rows = (bad: number) => Array.from({ length: 60 }, (_, i) => [120000 + i * 1000, i < 3 ? 30000 : 2000, i < 3 ? bad : 0, 10]);
    expect(slowsOf(ext(run([8, 9, 10, 11])), remote, { samples: rows(1), p50: 2 })[0].kind).toBe('shared');
    expect(slowsOf(ext(run([8, 9, 10, 11])), remote, { samples: rows(5), p50: 2 })[0].kind).toBe('local');
  });
  it('leaves out a slowdown under 3 ms', () => {
    const bins = Array.from({ length: 20 }, (_, i) => rb(i, i >= 8 && i < 12 ? [6.5, 8] : [4, 5]));
    expect(slowsOf(ext(bins), [false, false])).toEqual([]);
  });
});
