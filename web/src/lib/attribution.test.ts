import { describe, it, expect } from 'vitest';
import { cellClass, gapCause, ownSuccess, seenAs } from './attribution';
import type { MonitorCell, MonitorGap } from './types';

const HOUR = 3_600_000;
const cell = (samples: number, answered: number, shared?: number, covered?: number | null): MonitorCell => [0, samples, answered, 10, 20, [0], shared, covered];

describe('cellClass', () => {
  it('is none for a missing or empty cell', () => {
    expect([cellClass(undefined, HOUR), cellClass(cell(0, 0), HOUR)]).toEqual(['none', 'none']);
  });
  it('grades an answered cell by its answered ratio', () => {
    expect(cellClass(cell(30, 30, 0, HOUR), HOUR)).toBe('up');
    expect(cellClass(cell(30, 0, 0, HOUR), HOUR)).toBe('down');
  });
  it('judges a partly shared cell on its own samples only', () => {
    expect(cellClass(cell(30, 20, 10, HOUR), HOUR)).toBe('up');
    expect(cellClass(cell(30, 10, 10, HOUR), HOUR)).toBe('most');
  });
  it('is shared when every sample fell in a shared cycle', () => {
    expect(cellClass(cell(30, 0, 30, HOUR), HOUR)).toBe('shared');
  });
  it('is thin when under half the cell was measured, whatever answered', () => {
    expect(cellClass(cell(30, 30, 0, HOUR / 4), HOUR)).toBe('thin');
    expect(cellClass(cell(30, 30, 0, HOUR / 2), HOUR)).toBe('up');
  });
  it('measures thinness against the part of the cell that could be measured', () => {
    // A cell still in progress, and the cell monitoring began in.
    expect(cellClass(cell(30, 30, 0, HOUR / 4), HOUR, -Infinity, HOUR / 3)).toBe('up');
    expect(cellClass(cell(30, 30, 0, HOUR / 4), HOUR, (2 * HOUR) / 3)).toBe('up');
    expect(cellClass(cell(30, 30, 0, HOUR / 12), HOUR, -Infinity, HOUR / 3)).toBe('thin');
  });
  it('treats unknown coverage as not thin', () => {
    expect(cellClass(cell(30, 30, 0, null), HOUR)).toBe('up');
  });
  it('reads a legacy 6-item cell', () => {
    const old: MonitorCell = [0, 30, 15, 10, 20, [0]];
    expect(cellClass(old, HOUR)).toBe('most');
  });
});

describe('gapCause', () => {
  it('names a cause only when gaps fill at least half the span', () => {
    expect(gapCause([[0, 30_000, 'stall']], 0, HOUR)).toBeNull();
    expect(gapCause([[-HOUR, HOUR / 2, 'sleep']], 0, HOUR)).toBe('sleep');
    expect(gapCause([], 0, HOUR)).toBeNull();
  });
  it('picks the cause with more of the lost time', () => {
    const gaps: MonitorGap[] = [[0, HOUR / 4, 'sleep'], [HOUR / 4, HOUR, 'stall']];
    expect(gapCause(gaps, 0, HOUR)).toBe('stall');
  });
});

describe('ownSuccess', () => {
  it('falls back to the raw figure', () => {
    expect(ownSuccess({ query_success: 0.9, query_success_own: 0.99 })).toBe(0.99);
    expect(ownSuccess({ query_success: 0.9, query_success_own: null })).toBe(0.9);
  });
});

describe('seenAs', () => {
  it('names each cause', () => {
    expect(seenAs({ cycles: 4, shared: 0, unclear: 0 })).toBe('This resolver only');
    expect(seenAs({ cycles: 4, shared: 4, unclear: 0 })).toBe('Several operators at once');
    expect(seenAs({ cycles: 4, shared: 0, unclear: 4 })).toBe('Unclear');
    expect(seenAs({ cycles: 4, shared: 1, unclear: 1 })).toBe('Mixed (2 of 4 cycles this resolver only)');
    expect(seenAs({ cycles: 4, shared: 2, unclear: 2 })).toBe('Several operators at once, or unclear');
  });
});
