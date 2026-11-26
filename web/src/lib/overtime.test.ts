import { describe, it, expect } from 'vitest';
import { bandPath, blendSeries, fitSpans, lastFilled, linePath, lossClass, lossHeight, overlayValues, slotAt, slotGrid, spreadLabels, timeTicks, type OverTimeCell, type OverTimeSeries } from './overtime';

const cell = (t0: number, w = 15_000, over: Partial<OverTimeCell> = {}): OverTimeCell => ({ t0, t1: t0 + w, p5: 4, p25: 5, p50: 6, p75: 8, p95: 12, sent: 50, lost: 0, ...over });
const series = (cells: (OverTimeCell | null)[]): OverTimeSeries => ({ id: 'a', name: 'Example', cells });

describe('slotGrid', () => {
  it('finds the grid from the first cell, wherever it sits', () => {
    expect(slotGrid([series([null, null, cell(1_030_000)])])).toEqual({ base: 1_000_000, width: 15_000, n: 3 });
  });
  it('spans the longest series and is null with no cell', () => {
    expect(slotGrid([series([cell(0)]), series([cell(0), null, null])])?.n).toBe(3);
    expect(slotGrid([series([null, null])])).toBeNull();
  });
});

describe('blendSeries', () => {
  const a = [series([cell(0, 15_000, { p50: 10 }), null])];
  const b = [series([cell(0, 15_000, { p50: 1000 }), cell(15_000)])];
  it('eases a cell in log space', () => {
    expect(blendSeries(a, b, 0)[0].cells[0]?.p50).toBeCloseTo(10);
    expect(blendSeries(a, b, 0.5)[0].cells[0]?.p50).toBeCloseTo(100);
    expect(blendSeries(a, b, 1)[0].cells[0]?.p50).toBeCloseTo(1000);
  });
  it('shows a new cell as it is and keeps the counts', () => {
    expect(blendSeries(a, b, 0)[0].cells[1]).toEqual(cell(15_000));
    expect(blendSeries(a, b, 0.5)[0].cells[0]?.sent).toBe(50);
  });
});

describe('slotAt', () => {
  it('maps a pixel to its slot and rejects outside the plot', () => {
    expect([slotAt(40, 40, 100, 10), slotAt(139, 40, 100, 10), slotAt(140, 40, 100, 10), slotAt(39, 40, 100, 10)]).toEqual([0, 9, null, null]);
  });
});

describe('loss strip', () => {
  it('takes the caller class, else grades the share lost, else none', () => {
    expect(lossClass({ sent: 50, lost: 0 })).toBeNull();
    expect(lossClass({ sent: 50, lost: 2 })).toBe('some');
    expect(lossClass({ sent: 50, lost: 6 })).toBe('most');
    expect(lossClass({ sent: 50, lost: 13 })).toBe('down');
    expect(lossClass({ sent: 50, lost: 0, cls: 'shared' })).toBe('shared');
  });
  it('sizes bars: fixed for up, shared and thin, growing with loss otherwise, capped at full', () => {
    const c = { sent: 100, lost: 5 };
    expect([lossHeight(c, 'up'), lossHeight(c, 'shared'), lossHeight(c, 'thin')]).toEqual([0.14, 0.32, 0]);
    expect(lossHeight(c, 'some')).toBeCloseTo(0.62);
    expect(lossHeight({ sent: 100, lost: 100 }, 'down')).toBe(1);
  });
});

describe('linePath', () => {
  it('breaks at a null and starts a new stroke after it', () => {
    expect(linePath([0, 10, 20, 30], [5, 6, null, 7])).toBe('M0.0 5.0L10.0 6.0M30.0 7.0');
  });
  it('draws a lone point as a short tick centred on it', () => {
    expect(linePath([0, 10, 20], [null, 5, null], 3)).toBe('M10.0 5.0m-3.0 0h6.0');
  });
  it('is empty with no points', () => {
    expect(linePath([0, 10], [null, null])).toBe('');
  });
});

describe('bandPath', () => {
  it('closes one shape per unbroken run', () => {
    const d = bandPath([0, 10, 20], [1, 1, null], [3, 3, null], 0);
    expect(d.match(/Z/g)).toHaveLength(1);
    expect(d.startsWith('M0.0 1.0L0.0 1.0L10.0 1.0')).toBe(true);
    const two = bandPath([0, 10, 20], [1, null, 1], [3, null, 3], 2);
    expect(two.match(/M/g)).toHaveLength(2);
    expect(two).toContain('M-2.0 1.0');
  });
  it('skips a slot missing either edge', () => {
    expect(bandPath([0, 10], [1, 1], [null, 3], 0).match(/Z/g)).toHaveLength(1);
    expect(bandPath([0], [null], [null], 1)).toBe('');
  });
});

describe('overlayValues', () => {
  it('covers the half band and the median, never the nine-in-ten ends', () => {
    expect(overlayValues(cell(0))).toEqual([5, 6, 8]);
  });
  it('falls back to the median where the half band is missing', () => {
    expect(overlayValues(cell(0, 15_000, { p25: null, p75: null }))).toEqual([6, 6, 6]);
    expect(overlayValues(cell(0, 15_000, { p50: null, p25: null, p75: null }))).toEqual([null, null, null]);
  });
});

describe('spreadLabels', () => {
  it('leaves labels already apart alone', () => {
    expect(spreadLabels([10, 50, 90], 16, 0, 100)).toEqual([10, 50, 90]);
  });
  it('pushes close labels apart and keeps the input order', () => {
    const out = spreadLabels([50, 20, 52, 51], 16, 0, 200);
    const sorted = [...out].sort((a, b) => a - b);
    for (let i = 1; i < sorted.length; i++) expect(sorted[i] - sorted[i - 1]).toBeGreaterThanOrEqual(16 - 1e-9);
    expect(out[1]).toBe(20);
    expect(out[0]).toBeLessThan(out[3]);
    expect(out[3]).toBeLessThan(out[2]);
  });
  it('pulls a stack at the bottom edge back inside', () => {
    const out = spreadLabels([98, 99, 100], 16, 0, 100);
    expect(Math.max(...out)).toBe(100);
    expect(Math.min(...out)).toBeGreaterThanOrEqual(0);
  });
});

describe('fitSpans', () => {
  it('keeps the first of two overlapping spans and any clear of it', () => {
    expect(fitSpans([[0, 50], [40, 90], [60, 100]], 6)).toEqual([true, false, true]);
  });
});

describe('timeTicks', () => {
  it('lands on round steps inside the window, within the cap', () => {
    const t0 = Date.UTC(2026, 0, 1, 10, 0, 7);
    const t = timeTicks(t0, t0 + 25 * 60_000, 8);
    expect(t.length).toBeGreaterThan(1);
    expect(t.length).toBeLessThanOrEqual(8);
    expect(t.every((x) => x >= t0 && x < t0 + 25 * 60_000)).toBe(true);
    const gaps = new Set(t.slice(1).map((x, i) => x - t[i]));
    expect(gaps.size).toBe(1);
    expect([...gaps][0] % 60_000).toBe(0);
  });
  it('uses 15 s steps for a short window', () => {
    expect(timeTicks(0, 60_000, 8).slice(1).map((x, i) => x - timeTicks(0, 60_000, 8)[i])).toEqual([15_000, 15_000, 15_000]);
  });
});

describe('lastFilled', () => {
  it('finds the last non-null entry', () => {
    expect([lastFilled([1, null, 2, null]), lastFilled([null]), lastFilled([])]).toEqual([2, -1, -1]);
  });
});
