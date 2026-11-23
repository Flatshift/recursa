import { describe, expect, it } from 'vitest';
import { binsDone, gapSeconds, markPath, minutesLeft, sentOf, chartOf, colourOf, ecdfPoints, groupsOf, markOf, mergedMedian, medianOf, minutesOf, rangeOf, shortlist, slotOf, toggled, verdictOf } from './extended';
import type { ExtBin, ExtRow } from './types';

const cell = (p50: number | null, sent = 50, lost = 0) => ({ sent, lost, p5: p50, p25: p50, p50, p75: p50, p95: p50 });
const bin = (at: number, removed = false, p = 10): ExtBin => ({ at, end: at + 15000, removed, r: [cell(p), cell(p + 1)] });
const row = (t: number, call: ExtRow['call']): ExtRow => ({ t, sent: 5000, lost: 0, loss_pct: 0, blocked: 0, p50: 10, p90: 12, p95: 14, vs: null, call });

describe('medianOf', () => {
  it('takes the middle, or the mean of the two middle', () => {
    expect(medianOf([])).toBeNull();
    expect(medianOf([3])).toBe(3);
    expect(medianOf([1, 2, 9])).toBe(2);
    expect(medianOf([1, 2, 4, 9])).toBe(3);
  });
});

describe('mergedMedian', () => {
  it('pools the lists', () => {
    expect(mergedMedian([[1, 5], [3], []])).toBe(3);
    expect(mergedMedian([[], []])).toBeNull();
  });
});

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
  it('sums what the first target was sent', () => expect(sentOf(ext)).toBe(100));
  it('counts minutes left at the run pace', () => expect(minutesLeft(ext)).toBe(Math.ceil((98 * 15) / 60)));
  it('has none left at the end', () => expect(minutesLeft({ ...ext, total: 100 })).toBe(0));
  it('adds up one kind of gap', () => expect(gapSeconds([[0, 30000, 'sleep'], [40000, 50000, 'stall'], [60000, 90000, 'sleep']], 'sleep')).toBe(60));
});

describe('markPath', () => {
  it('draws a closed shape for each mark', () => {
    for (const m of ['circle', 'square', 'triangle', 'diamond'] as const) expect(markPath(m, 5, 5, 3)).toMatch(/z$/);
  });
});
