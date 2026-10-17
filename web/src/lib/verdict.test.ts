// The UI's scoring functions mirror bench.rs; these port the engine's tests step for step, with the same numbers.

import { describe, it, expect } from 'vitest';
import { CONNECTION_SHARE, CDN_ROUND_TRIPS, HIT_SHARE, TESTS, TIMEOUT_COST_MS, WEIGHTS, buildRows, compositeOf, perLookupOf, tiedWith, timeoutCost, verdict, type Dist } from './verdict';
import { ms } from './format';
import type { RunState, TestKind } from './types';

type Entry = { id: string; provider: string; ms: Partial<Record<TestKind, number[]>>; timeouts?: number; hits?: number | null; cdn?: number | null };

/** A finished run with one UDP target per entry. `ms` is the answered round trips per test (missing test = not run);
 *  `timeouts` adds that many timed-out samples per test; `hits` sets Target.cache_hits; `cdn` sets the resolver's
 *  penalty_ms (undefined = unmapped, and run.cdn stays null when no entry has one). */
function fakeRun(entries: Entry[]): RunState {
  const samples = entries.flatMap((e, t) =>
    TESTS.flatMap((k) => {
      const v = e.ms[k];
      if (!v) return [];
      const ok = v.map((x, i) => ({ t, k, i, us: x * 1000, o: 'ok', at: 0, h: 0 }));
      const to = Array.from({ length: e.timeouts ?? 0 }, (_, j) => ({ t, k, i: v.length + j, us: 0, o: 'timeout', at: 0, h: 0 }));
      return [...ok, ...to];
    }),
  );
  const mapped = entries.filter((e) => e.cdn != null);
  return {
    id: 't',
    config: { tests: TESTS.filter((k) => entries.some((e) => e.ms[k])), transports: ['udp'] },
    resolvers: entries.map((e) => ({ id: e.id, provider: e.provider, variant: '', group: 'public' })),
    targets: entries.map((e, idx) => ({ idx, resolver_id: e.id, provider: e.provider, variant: '', transport: 'udp', intercepted: false, cache_hits: e.hits })),
    samples,
    cdn: mapped.length ? { resolvers: Object.fromEntries(mapped.map((e) => [e.id, { penalty_ms: e.cdn }])) } : null,
    probes: {},
    intel: {},
    notes: [],
    paths: {},
  } as unknown as RunState;
}

/** A Dist as the engine tests set it: only what the scores read. */
function dist(p50: number | null, p95: number | null, o: Partial<Dist> = {}): Dist {
  return { n: 0, sent: 0, timeouts: 0, errors: 0, values: [], p25: null, p50, p75: null, p95, sd: 0, min: null, max: null, ...o };
}
const all = (p50: number, p95: number, sd = 0): Record<TestKind, Dist> => ({ cached: dist(p50, p95, { sd }), uncached: dist(p50, p95, { sd }), tld: dist(p50, p95, { sd }) });
const flat = (c: number, u: number, t: number, sent = 0): Record<TestKind, Dist> => ({ cached: dist(c, c, { sent }), uncached: dist(u, u, { sent }), tld: dist(t, t, { sent }) });
const same = (n: number, v: number) => Array.from({ length: n }, () => v);

describe('compositeOf', () => {
  it('mirrors consistency_and_cached_answers_count_in_the_composite', () => {
    const score = (d: Record<TestKind, Dist>) => compositeOf(d, TESTS)!;
    expect(Object.values(WEIGHTS).reduce((a, w) => a + w, 0)).toBeCloseTo(1, 9);
    // Same median: the one with the long tail ranks behind the steady one.
    const steady = score(all(15, 16));
    const erratic = score(all(15, 40));
    expect(steady).toBeCloseTo(15.5, 9);
    expect(erratic).toBeCloseTo(27.5, 9);
    // Same median and 95th percentile: the wider spread ranks behind.
    expect(score(all(15, 21, 4))).toBeCloseTo(19, 9);
    expect(score(all(15, 21, 8))).toBeGreaterThan(score(all(15, 21, 4)));
    // A slightly slower median can win on steadiness.
    expect(score(all(16, 17))).toBeLessThan(erratic);
    // Cached answers weigh most, then uncached, then TLD.
    const cachedSlow = score({ ...all(10, 10), cached: dist(20, 20) });
    const uncachedSlow = score({ ...all(10, 10), uncached: dist(20, 20) });
    const tldSlow = score({ ...all(10, 10), tld: dist(20, 20) });
    expect(cachedSlow).toBeGreaterThan(uncachedSlow);
    expect(uncachedSlow).toBeGreaterThan(tldSlow);
    expect(cachedSlow).toBeCloseTo(14, 9);
    expect(tldSlow).toBeCloseTo(12.5, 9);
    // A test with no answers is left out rather than counted as zero.
    expect(score({ ...all(12, 12), tld: dist(null, null) })).toBeCloseTo(12, 9);
  });

  it('mirrors timeouts_cost_a_retry_in_the_composite', () => {
    const d = { cached: dist(10, 10, { sent: 100 }), uncached: dist(10, 10, { sent: 100 }), tld: dist(10, 10, { sent: 100 }) };
    expect(compositeOf(d, TESTS)).toBeCloseTo(10, 9);
    // 3 timeouts in 300 queries: 1% of lookups wait a second more, 10 ms on average.
    d.uncached.timeouts = 3;
    expect(compositeOf(d, TESTS)).toBeCloseTo(20, 9);
    // The same through buildRows: one timeout per test among 101 queries.
    const run = fakeRun([
      { id: 'a', provider: 'A', ms: { cached: same(100, 10), uncached: same(100, 10), tld: same(100, 10) }, timeouts: 1 },
      { id: 'b', provider: 'B', ms: { cached: same(100, 10), uncached: same(100, 10), tld: same(100, 10) } },
    ]);
    const [a, b] = buildRows(run);
    expect(timeoutCost(a)).toBeCloseTo((TIMEOUT_COST_MS * 3) / 303, 9);
    expect(a.composite).toBeCloseTo(10 + timeoutCost(a), 9);
    expect(timeoutCost(b)).toBe(0);
  });
});

describe('perLookupOf', () => {
  const lab = HIT_SHARE.lab;
  const per = (d: Record<TestKind, Dist>, hits: number | null, cdn: number | null, share: number, tests: TestKind[] = TESTS) => perLookupOf(d, tests, hits, cdn, share)!;

  it('follows the mix, the hit rate and the CDN', () => {
    const base = flat(15, 25, 35);
    const miss = (0.35 * 25 + 0.25 * 35) / 0.6;
    // 1. The Lab mix is the speed score.
    expect(per(base, null, null, lab)).toBeCloseTo(compositeOf(base, TESTS)!, 9);
    // 2. A home network answers more from cache, and cached is the fastest.
    const home = per(base, null, null, HIT_SHARE.home);
    expect(home).toBeCloseTo(0.8 * 15 + 0.2 * miss, 9);
    expect(home).toBeLessThan(per(base, null, null, lab));
    // 3. Cache-hit correction.
    expect(per(flat(20, 25, 25), 0.6, null, 1)).toBeCloseTo((20 - 0.4 * 25) / 0.6, 6);
    expect(per(flat(20, 10, 10), 0.6, null, 1)).toBeCloseTo(20, 9);
    for (const r of [0, 1]) expect(per(flat(20, 25, 25), r, null, 1)).toBeCloseTo(20, 9);
    // 4. The CDN term.
    expect(per(base, null, 8, lab) - per(base, null, null, lab)).toBeCloseTo(CDN_ROUND_TRIPS * 8 * CONNECTION_SHARE, 9);
    expect(CDN_ROUND_TRIPS * 8 * CONNECTION_SHARE).toBeCloseTo(12, 9);
    // 5. Timeouts, with `sent` summed over the tests.
    const slow = flat(15, 25, 35, 30);
    slow.tld.timeouts = 3;
    expect(per(slow, null, null, lab)).toBeCloseTo(per(flat(15, 25, 35, 30), null, null, lab) + (TIMEOUT_COST_MS * 3) / 90, 9);
    // 6. Missing tests.
    expect(per({ cached: dist(null, null), uncached: dist(null, null), tld: dist(35, 35) }, null, null, lab)).toBeCloseTo(35, 9);
    expect(perLookupOf(all(NaN, NaN), [], null, null, lab)).toBeNull();
    expect(perLookupOf({ cached: dist(null, null), uncached: dist(null, null), tld: dist(null, null) }, TESTS, null, null, lab)).toBeNull();
    // 7. Cached only, with a hit rate: nothing to correct it with.
    const cachedOnly = { cached: dist(20, 20), uncached: dist(null, null), tld: dist(null, null) };
    expect(per(cachedOnly, 0.6, null, lab)).toBeCloseTo(20, 9);
    // 8. A test in the dist but not in the config is ignored.
    const withTld = flat(15, 25, 99);
    expect(per(withTld, 0.6, 2, HIT_SHARE.home, ['cached', 'uncached'])).toBeCloseTo(per(flat(15, 25, 35), 0.6, 2, HIT_SHARE.home, ['cached', 'uncached']), 9);
    expect(per(withTld, 0.6, 2, HIT_SHARE.home, ['cached', 'uncached'])).not.toBeCloseTo(per(withTld, 0.6, 2, HIT_SHARE.home), 3);
  });
});

describe('buildRows', () => {
  it('fills perLookup and cdnTerm', () => {
    const v = { cached: same(30, 15), uncached: same(30, 25), tld: same(30, 35) };
    const run = fakeRun([
      { id: 'a', provider: 'A', ms: v, hits: 0.6, cdn: 8 },
      { id: 'b', provider: 'B', ms: v },
    ]);
    const rows = buildRows(run);
    expect(rows.map((r) => r.cdnTerm)).toEqual([12, null]);
    expect(rows[0].perLookup).toBe(perLookupOf(rows[0].dist, TESTS, 0.6, 8, HIT_SHARE.home));
    expect(rows[1].perLookup).toBe(perLookupOf(rows[1].dist, TESTS, null, null, HIT_SHARE.home));
    // At the Lab mix with no hit rate and no CDN, per lookup is the speed score.
    for (const r of buildRows(fakeRun([{ id: 'b', provider: 'B', ms: v, timeouts: 2 }]), [], HIT_SHARE.lab)) {
      expect(r.perLookup).toBeCloseTo(r.composite!, 9);
    }
  });
});

describe('tiedWith', () => {
  // Round trips that wander by a few ms around a centre, the same pattern for each target.
  const wobble = [0.0, 3.1, -2.4, 5.6, -1.2, 0.8, 7.9, -3.3, 2.2, -0.6];
  const around = (centre: number) => Array.from({ length: 60 }, (_, i) => centre + wobble[i % 10]);
  const pair = (a: number[], b: number[]) => {
    const run = fakeRun([
      { id: 'a', provider: 'A', ms: { cached: a } },
      { id: 'b', provider: 'B', ms: { cached: b } },
    ]);
    return { run, rows: buildRows(run) };
  };
  const perLookup = (d: Record<TestKind, Dist>, tests: TestKind[], row: { target: { cache_hits?: number | null }; cdn: number | null }) =>
    perLookupOf(d, tests, row.target.cache_hits ?? null, row.cdn, HIT_SHARE.home);

  it('mirrors close_scores_are_tied_and_clear_gaps_are_not, for both scorers', () => {
    for (const score of [undefined, perLookup]) {
      const close = pair(around(20), around(20.4));
      expect(tiedWith(close.run, close.rows[0], close.rows[1], score)).toBe(true);
      const far = pair(around(20), around(30));
      expect(tiedWith(far.run, far.rows[0], far.rows[1], score)).toBe(false);
      // Nothing to redraw is no tie either.
      const none = pair([], []);
      expect(tiedWith(none.run, none.rows[0], none.rows[1], score)).toBe(false);
    }
  });
});

describe('verdict: Best from here', () => {
  const all30 = (x: number) => ({ cached: same(30, x), uncached: same(30, x), tld: same(30, x) });
  const abc = (cdnA: number | undefined, cdnB?: number, cdnC?: number) =>
    fakeRun([
      { id: 'a', provider: 'A', ms: all30(15), cdn: cdnA },
      { id: 'b', provider: 'B', ms: all30(18), cdn: cdnB },
      { id: 'c', provider: 'C', ms: all30(40), cdn: cdnC },
    ]);
  const from = (run: RunState, share = HIT_SHARE.home) => {
    const v = verdict(run, buildRows(run, [], share), share);
    return { v, pick: v.picks[v.picks.length - 1] };
  };

  it('weighs the CDN term, so the fastest answers are not always the best from here', () => {
    const run = abc(10, 1, 0);
    const rows = buildRows(run);
    const { v, pick } = from(run);
    expect(v.best!.target.provider).toBe('A');
    expect(pick.goal).toBe('Best from here');
    expect(pick.name).toBe('B');
    expect(pick.verified).toBe(true);
    expect(rows[0].perLookup).toBeCloseTo(30, 9);
    expect(rows[1].perLookup).toBeCloseTo(19.5, 9);
    expect(pick.evidence.startsWith(ms(rows[1].perLookup))).toBe(true);
    expect(pick.evidence).toContain('per lookup at the Home mix');
    expect(pick.evidence).toContain('ahead of A');
    expect(pick.evidence).not.toContain('no CDN term');
  });

  it('says level when the figures differ by less than 0.05 ms', () => {
    expect(from(abc(3, 1, 0)).pick.evidence).toContain('level with');
  });

  it('says so when no resolver has a CDN term', () => {
    const run = abc(undefined);
    expect(run.cdn).toBeNull();
    expect(buildRows(run).every((r) => r.cdnTerm == null)).toBe(true);
    expect(from(run).pick.evidence.endsWith('(no CDN term)')).toBe(true);
  });

  it('names the next provider, not another resolver of the same one', () => {
    const run = fakeRun([
      { id: 'a', provider: 'A', ms: all30(15) },
      { id: 'a2', provider: 'A', ms: all30(16) },
      { id: 'b', provider: 'B', ms: all30(18) },
    ]);
    const { pick } = from(run);
    expect(pick.name).toBe('A');
    expect(pick.evidence).toContain('ahead of B');
  });

  it('follows the chosen mix without moving the fastest pick', () => {
    const run = abc(10, 1, 0);
    const home = from(run);
    const lab = from(run, HIT_SHARE.lab);
    expect(lab.pick.evidence).toContain('Lab mix');
    expect(lab.v.best!.target.idx).toBe(home.v.best!.target.idx);
    expect(lab.v.tie).toBe(home.v.tie);
    expect(lab.v.sentence).toBe(home.v.sentence);
  });
});
