// The UI's scoring functions mirror bench.rs; these port the engine's tests step for step, with the same numbers.

import { describe, it, expect } from 'vitest';
import { CONNECTION_SHARE, CDN_ROUND_TRIPS, HIT_SHARE, PART_WEIGHTS, TESTS, TIMEOUT_COST_MS, WEIGHTS, buildRows, compositeOf, excused, failureCost, perLookupOf, tiedWith, verdict, winsorMean, type Dist } from './verdict';
import { ms } from './format';
import type { Outcome, RunState, TestKind } from './types';

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
function dist(wmean: number | null, o: Partial<Dist> = {}): Dist {
  return { n: 0, sent: 0, timeouts: 0, errors: 0, failures: 0, values: [], p25: null, p50: wmean, p75: null, p95: wmean, sd: 0, min: null, max: null, wmean, limited: false, ...o };
}
const all = (w: number): Record<TestKind, Dist> => ({ cached: dist(w), uncached: dist(w), tld: dist(w) });
const flat = (c: number, u: number, t: number, sent = 0): Record<TestKind, Dist> => ({ cached: dist(c, { sent }), uncached: dist(u, { sent }), tld: dist(t, { sent }) });
const same = (n: number, v: number) => Array.from({ length: n }, () => v);

describe('compositeOf', () => {
  it('mirrors cost_is_the_winsorised_mean_and_cached_weighs_most', () => {
    const score = (d: Record<TestKind, Dist>) => compositeOf(d, TESTS)!;
    expect(Object.values(WEIGHTS).reduce((a, w) => a + w, 0)).toBeCloseTo(1, 9);
    // 29 answers near 15 ms and one of 1830 ms: the straggler moves the score by well under 1 ms.
    const v = [...Array.from({ length: 29 }, (_, i) => 14 + i / 14), 1830];
    const w = winsorMean(v)!;
    expect(w).toBeGreaterThan(14);
    expect(Math.abs(w - 15)).toBeLessThan(1);
    expect(score({ cached: dist(w), uncached: dist(w), tld: dist(w) })).toBeCloseTo(w, 9);
    // Cached answers weigh most, then uncached, then TLD.
    const cachedSlow = score({ ...all(10), cached: dist(20) });
    const uncachedSlow = score({ ...all(10), uncached: dist(20) });
    const tldSlow = score({ ...all(10), tld: dist(20) });
    expect(cachedSlow).toBeGreaterThan(uncachedSlow);
    expect(uncachedSlow).toBeGreaterThan(tldSlow);
    expect(cachedSlow).toBeCloseTo(15, 9);
    expect(uncachedSlow).toBeCloseTo(14, 9);
    expect(tldSlow).toBeCloseTo(11, 9);
    // A test with no answers is left out rather than counted as zero.
    expect(score({ ...all(12), tld: dist(null) })).toBeCloseTo(12, 9);
  });

  it('winsorMean matches the engine', () => {
    const v = [...Array.from({ length: 29 }, (_, i) => i + 1), 1000];
    expect(winsorMean(v)).toBeCloseTo((378 + 3 * 27.1) / 30, 9);
    expect(winsorMean(v)).toBeCloseTo(15.31, 2);
    expect(winsorMean([])).toBeNull();
  });

  it('leaves a limited test out of the score, the timeouts and the answered share', () => {
    const d = { cached: dist(10, { sent: 30, n: 30 }), uncached: dist(10, { sent: 30, n: 30 }), tld: dist(500, { sent: 30, n: 10, timeouts: 20, limited: true }) };
    expect(compositeOf(d, TESTS)).toBeCloseTo(10, 9);
    const run = fakeRun([{ id: 'a', provider: 'A', ms: { cached: same(30, 10), uncached: same(30, 10), tld: same(10, 10) }, timeouts: 0 }]);
    const [open] = buildRows(run);
    expect(open.sent).toBe(70);
    (run.targets[0] as unknown as { stats: unknown }).stats = { tld: { limited: true } };
    const [lim] = buildRows(run);
    expect(lim.dist.tld.limited).toBe(true);
    expect(lim.success).toBe(1);
    expect(lim.sent).toBe(60);
    expect(lim.composite).toBeCloseTo(10, 9);
    const timed = { ...d, tld: { ...d.tld, timeouts: 20 } };
    expect(failureCost({ dist: timed, sent: 60 } as never)).toBe(0);
  });

  it('mirrors timeouts_cost_a_retry_in_the_composite', () => {
    const d = { cached: dist(10, { sent: 100 }), uncached: dist(10, { sent: 100 }), tld: dist(10, { sent: 100 }) };
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
    expect(failureCost(a)).toBeCloseTo((TIMEOUT_COST_MS * 3) / 303, 9);
    expect(a.composite).toBeCloseTo(10 + failureCost(a), 9);
    expect(failureCost(b)).toBe(0);
  });
});

describe('compositeOf parts', () => {
  const cache = (d: Record<TestKind, Dist>, tests: TestKind[] = TESTS) => compositeOf(d, tests, PART_WEIGHTS.cache)!;
  const fresh = (d: Record<TestKind, Dist>, tests: TestKind[] = TESTS) => compositeOf(d, tests, PART_WEIGHTS.fresh)!;

  it('splits the speed score into a cache half and a fresh half whose mean is the full score', () => {
    const d = flat(10, 20, 30);
    expect(cache(d)).toBeCloseTo(10, 9);
    expect(fresh(d)).toBeCloseTo(22, 9);
    expect(compositeOf(d, TESTS)).toBeCloseTo(16, 9);
    expect(compositeOf(d, TESTS)).toBeCloseTo((cache(d) + fresh(d)) / 2, 9);
  });

  it('charges each part only the failures of its own tests', () => {
    const d = flat(10, 20, 30, 10);
    d.cached.timeouts = 1;
    expect(cache(d)).toBeCloseTo(10 + 100, 9);
    expect(fresh(d)).toBeCloseTo(22, 9);
    expect(compositeOf(d, TESTS)).toBeCloseTo(16 + 1000 / 30, 9);
    // A limited TLD leaves the fresh part as the uncached figure alone.
    d.tld.limited = true;
    expect(fresh(d)).toBeCloseTo(20, 9);
  });

  it('leaves a part out when none of its tests ran', () => {
    expect(fresh(flat(10, 20, 30), ['cached'])).toBeNull();
    expect(cache(flat(10, 20, 30), ['uncached'])).toBeNull();
  });
});

/** A finished run from raw samples: one UDP target per entry of `ids` (repeat an id for more transports of one resolver). */
function sampleRun(ids: string[], samples: { t: number; i: number; o: Outcome; at: number; us?: number }[]): RunState {
  return {
    id: 't',
    config: { tests: ['cached'], transports: ['udp'] },
    resolvers: [...new Set(ids)].map((id) => ({ id, provider: id, variant: '', group: 'public' })),
    targets: ids.map((id, idx) => ({ idx, resolver_id: id, provider: id, variant: '', transport: 'udp', intercepted: false })),
    samples: samples.map((x) => ({ k: 'cached', us: x.o === 'ok' ? 10_000 : 0, h: 0, ...x })),
    cdn: null,
    probes: {},
    intel: {},
    notes: [],
    paths: {},
  } as unknown as RunState;
}
/** 29 answers then one timeout at `at`, for target `t`. */
const blipped = (t: number, at: number) => [
  ...Array.from({ length: 29 }, (_, i) => ({ t, i, o: 'ok' as Outcome, at: i * 10 })),
  { t, i: 29, o: 'timeout' as Outcome, at },
];

describe('failed answers and blips', () => {
  it('mirrors failed_answers_cost_like_timeouts', () => {
    const build = (bad: Outcome) =>
      buildRows(sampleRun(['a'], Array.from({ length: 30 }, (_, i) => ({ t: 0, i, o: i < 27 ? ('ok' as Outcome) : bad, at: i * 20 }))))[0];
    const refused = build('refused');
    expect(refused.dist.cached.failures).toBe(3);
    expect(refused.composite).toBeCloseTo(refused.dist.cached.wmean! + 100, 0);
    const blocked = build('blocked');
    expect(blocked.dist.cached.failures).toBe(0);
    expect(blocked.dist.cached.errors).toBe(3);
    expect(blocked.composite).toBeCloseTo(blocked.dist.cached.wmean!, 0);
    expect(blocked.success).toBeCloseTo(0.9, 9);
  });

  it('mirrors a_blip_across_resolvers_is_excused', () => {
    const three = sampleRun(['a', 'b', 'c'], [...blipped(0, 5000), ...blipped(1, 5300), ...blipped(2, 5900)]);
    const ex = excused(three);
    expect([...ex].sort((x, y) => x - y)).toEqual([29, 59, 89]);
    for (const row of buildRows(three)) {
      expect(row.dist.cached.sent).toBe(29);
      expect(row.dist.cached.timeouts).toBe(0);
      expect(failureCost(row)).toBe(0);
    }
    // Two resolvers in the bucket and the third elsewhere: nothing excused.
    expect(excused(sampleRun(['a', 'b', 'c'], [...blipped(0, 5000), ...blipped(1, 5300), ...blipped(2, 9000)])).size).toBe(0);
    // Four transports of one resolver failing together are one resolver's limit, not a blip.
    const four = sampleRun(['a', 'a', 'a', 'a'], [0, 1, 2, 3].flatMap((t) => blipped(t, 5000 + t * 100)));
    expect(excused(four).size).toBe(0);
  });

  it('mirrors a_neterr_streak_is_a_tld_stall', () => {
    // The engine marks it limited; here the flag arrives on the dist, and the score leaves the test out.
    const d = { cached: dist(15, { sent: 30 }), uncached: dist(15, { sent: 30 }), tld: dist(15, { sent: 30, n: 10, errors: 20, failures: 20, limited: true }) };
    expect(compositeOf(d, TESTS)).toBeCloseTo(15, 9);
    expect(failureCost({ dist: d, sent: 90 } as never)).toBe(0);
  });
});

describe('perLookupOf', () => {
  const lab = HIT_SHARE.lab;
  const per = (d: Record<TestKind, Dist>, hits: number | null, cdn: number | null, share: number, tests: TestKind[] = TESTS) => perLookupOf(d, tests, hits, cdn, share)!;

  it('follows the mix, the hit rate and the CDN', () => {
    const base = flat(15, 25, 35);
    const miss = (0.4 * 25 + 0.1 * 35) / 0.5;
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
    expect(per({ cached: dist(null), uncached: dist(null), tld: dist(35) }, null, null, lab)).toBeCloseTo(35, 9);
    expect(perLookupOf(all(NaN), [], null, null, lab)).toBeNull();
    expect(perLookupOf({ cached: dist(null), uncached: dist(null), tld: dist(null) }, TESTS, null, null, lab)).toBeNull();
    // 7. Cached only, with a hit rate: nothing to correct it with.
    const cachedOnly = { cached: dist(20), uncached: dist(null), tld: dist(null) };
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

describe('tiedWith: fresh part', () => {
  const wobble = [0.0, 3.1, -2.4, 5.6, -1.2, 0.8, 7.9, -3.3, 2.2, -0.6];
  const around = (centre: number) => Array.from({ length: 60 }, (_, i) => centre + wobble[i % 10]);
  const pair = (a: number[], b: number[]) => {
    const run = fakeRun([
      { id: 'a', provider: 'A', ms: { uncached: a, tld: a } },
      { id: 'b', provider: 'B', ms: { uncached: b, tld: b } },
    ]);
    return { run, rows: buildRows(run) };
  };
  const fresh = (d: Record<TestKind, Dist>, tests: TestKind[]) => compositeOf(d, tests, PART_WEIGHTS.fresh);

  it('ties close scores and not clear gaps', () => {
    const close = pair(around(20), around(20.4));
    expect(tiedWith(close.run, close.rows[0], close.rows[1], fresh)).toBe(true);
    const far = pair(around(20), around(30));
    expect(tiedWith(far.run, far.rows[0], far.rows[1], fresh)).toBe(false);
  });
});

describe('verdict: cache and fresh-lookup picks', () => {
  const run = () =>
    fakeRun([
      { id: 'a', provider: 'A', ms: { cached: same(30, 5), uncached: same(30, 40), tld: same(30, 40) } },
      { id: 'b', provider: 'B', ms: { cached: same(30, 20), uncached: same(30, 15), tld: same(30, 15) } },
    ]);

  it('names the cache leader and the fresh-lookup leader right after Fastest answers', () => {
    const r = run();
    const v = verdict(r, buildRows(r));
    expect(v.picks.slice(0, 3).map((p) => p.goal)).toEqual(['Fastest answers', 'Fastest cache', 'Fastest fresh lookups']);
    const [, cache, fresh] = v.picks;
    expect(cache.name).toBe('A');
    expect(cache.evidence).toBe(`${ms(5)} on cached answers, ${ms(15)} ahead of B`);
    expect(fresh.name).toBe('B');
    expect(fresh.evidence).toBe(`${ms(15)} on fresh lookups (uncached and TLD), ${ms(25)} ahead of A`);
  });

  it('never picks a forwarder, however fast its cache', () => {
    const r = fakeRun([
      { id: 'f', provider: 'F', ms: { cached: same(30, 1), uncached: same(30, 60), tld: same(30, 60) } },
      ...run().resolvers.map((x, i) => ({ id: x.id, provider: x.provider, ms: i ? { cached: same(30, 20), uncached: same(30, 15), tld: same(30, 15) } : { cached: same(30, 5), uncached: same(30, 40), tld: same(30, 40) } })),
    ]);
    Object.assign(r.resolvers[0], { group: 'system', forwarder: 'dnsmasq' });
    const v = verdict(r, buildRows(r));
    expect(v.picks.find((p) => p.goal === 'Fastest cache')!.name).toBe('A');
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
    expect(pick.parts).toEqual([
      ['Cache hit', '18.0 ms'],
      ['Fresh lookup', '18.0 ms'],
      ['Mix', '80% cached'],
      ['CDN edges', '+1.50 ms'],
    ]);
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
