// Warm-up answers, drawn as hollow rings while a run warms up. They are never recorded.

import { sinFrac } from './motion';
import type { TestKind, WarmSample } from './types';

type WarmByTarget = Map<number, Partial<Record<TestKind, WarmSample[]>>>;

/** Warm-up answers per target and test, in arrival order, for the tests shown. */
export function warmByTarget(warm: WarmSample[], tests: TestKind[]): WarmByTarget {
  const m: WarmByTarget = new Map();
  for (const w of warm) {
    if (!tests.includes(w.k)) continue;
    let byLane = m.get(w.t);
    if (!byLane) m.set(w.t, (byLane = {}));
    (byLane[w.k] ??= []).push(w);
  }
  return m;
}

/** How many warm-up answers a target has, over the tests shown. */
export const warmCount = (by: WarmByTarget, idx: number, tests: TestKind[]) => tests.reduce((n, k) => n + (by.get(idx)?.[k]?.length ?? 0), 0);

/** A stable value in [-1, 1) for the i-th warm-up answer of a test on a target. */
export function warmJitter(t: number, k: TestKind, i: number): number {
  return sinFrac((t + 1) * 12.9898 + (i + 1) * 78.233 + (k === 'cached' ? 4 : k === 'uncached' ? 5 : 6) * 37.719) * 2 - 1;
}
