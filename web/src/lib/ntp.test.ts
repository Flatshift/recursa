import { describe, it, expect } from 'vitest';
import { answeredCells, answeredState, cut, symmetricTicks } from './ntp';

const DAY = 86_400_000;
const HOUR = 3_600_000;
const pt = (ts: number, sent: number, ok: number) => ({ ts, sent, ok });

describe('answeredState', () => {
  it('uses 99.9 and 99 as the edges', () => {
    expect(answeredState(1000, 1000)).toBe('good');
    expect(answeredState(999, 1000)).toBe('good');
    expect(answeredState(998, 1000)).toBe('fair');
    expect(answeredState(99, 100)).toBe('fair');
    expect(answeredState(98, 100)).toBe('poor');
    expect(answeredState(0, 5)).toBe('poor');
  });
  it('is none with no checks', () => {
    expect(answeredState(0, 0)).toBe('none');
  });
});

describe('cut', () => {
  it('never rounds a share up across an edge', () => {
    expect(cut(99.896, 2)).toBe('99.89');
    expect(cut(99.896, 1)).toBe('99.8');
    expect(cut(98.96, 1)).toBe('98.9');
    expect(cut((999 / 1000) * 100, 2)).toBe('99.90');
    expect(cut(99.3, 1)).toBe('99.3');
    expect(cut(100, 0)).toBe('100');
  });
});

describe('answeredCells', () => {
  const until = 10 * DAY + 5 * HOUR;
  it('covers the window from the period that holds its start to the one that holds until', () => {
    expect(answeredCells([], until, 1)).toHaveLength(25);
    expect(answeredCells([], until, 7)).toHaveLength(8);
    const c = answeredCells([], until, 30);
    expect(c).toHaveLength(31);
    expect(c[0]).toMatchObject({ start: -20 * DAY, end: -19 * DAY });
    expect(c[30]).toMatchObject({ start: 10 * DAY, end: 11 * DAY });
    expect(c.every((x) => x.state === 'none')).toBe(true);
    const h = answeredCells([], until + 20 * 60_000, 1);
    expect(h[0]).toMatchObject({ start: 9 * DAY + 5 * HOUR, end: 9 * DAY + 6 * HOUR });
    expect(h[24]).toMatchObject({ start: 10 * DAY + 5 * HOUR, end: 10 * DAY + 6 * HOUR });
  });
  it('sums sent and ok within a cell and puts a boundary point in the later cell', () => {
    const c = answeredCells([pt(9 * DAY, 2, 2), pt(9 * DAY + 100, 3, 2), pt(10 * DAY - 1, 1, 1), pt(10 * DAY, 4, 4)], until, 7);
    expect(c[6]).toMatchObject({ sent: 6, ok: 5, state: 'poor' });
    expect(c[7]).toMatchObject({ sent: 4, ok: 4, state: 'good' });
  });
  it('keeps what the window holds before its first whole period', () => {
    // A day ending at 05:20: the checks from 05:20 to 06:00 the day before are in the window.
    const c = answeredCells([pt(9 * DAY + 5 * HOUR + 30 * 60_000, 4, 1)], until + 20 * 60_000, 1);
    expect(c[0]).toMatchObject({ sent: 4, ok: 1, state: 'poor' });
  });
  it('ignores points outside the cells', () => {
    const c = answeredCells([pt(3 * DAY - 1, 1, 1), pt(11 * DAY, 1, 1)], until, 7);
    expect(c.every((x) => x.sent === 0)).toBe(true);
  });
});

describe('symmetricTicks', () => {
  it('is symmetric about zero and covers the range', () => {
    expect(symmetricTicks(3.4)).toEqual([-4, -2, 0, 2, 4]);
    expect(symmetricTicks(0.9)).toEqual([-1, -0.5, 0, 0.5, 1]);
    expect(symmetricTicks(0)).toEqual([-0.1, 0, 0.1]);
  });
  it('steps up for large offsets', () => {
    expect(symmetricTicks(250)).toEqual([-300, -200, -100, 0, 100, 200, 300]);
  });
  it('keeps to a few ticks however far off a clock is', () => {
    expect(symmetricTicks(3_600_000)).toEqual([-4_000_000, -2_000_000, 0, 2_000_000, 4_000_000]);
    expect(symmetricTicks(1e12).length).toBeLessThanOrEqual(7);
    expect(symmetricTicks(3000)).toEqual([-3000, -2000, -1000, 0, 1000, 2000, 3000]);
  });
});
