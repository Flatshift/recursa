// Derived views of a run: per-target statistics, ranking, and the verdict.
// The rules here are deliberately simple and are restated in the UI so the
// ranking can always be explained in one sentence.

import type { Check, Resolver, RunState, Target, TestKind } from './types';
import { TRANSPORT_LABEL, label, ms, pct } from './format';

export const TESTS: TestKind[] = ['cached', 'uncached', 'tld'];
/** What each test counts for in the speed score (mirrors `TEST_WEIGHTS` in bench.rs): cached answers are most of
 *  what people get, uncached lookups are the delays they notice, and TLD lookups show how fast a resolver gets through
 *  the root and TLD servers. */
export const WEIGHTS: Record<TestKind, number> = { cached: 0.4, uncached: 0.35, tld: 0.25 };
/** How far a test's typical latency is pulled from its median toward its 95th percentile (mirrors `TAIL_WEIGHT`):
 *  consistency counts, so a steady resolver beats an erratic one with the same median. */
export const TAIL_WEIGHT = 0.5;
/** How much of a test's standard deviation is added on top (mirrors `JITTER_WEIGHT`): two resolvers can share a 95th
 *  percentile yet differ in how far their stragglers go. */
export const JITTER_WEIGHT = 0.25;
export const RELIABLE = 0.98;

/** Whether a resolver is expected to block a category: one it claims, or one on this network (or added by hand) that
 *  the user is checking. */
export function expectsBlock(r: Resolver, cat: string): boolean {
  return r.group === 'system' || r.group === 'custom' || r.published.filtering.includes(cat);
}
/** A new connection waits at least this many round trips on the CDN edge the resolver chose (the TCP handshake, the
 *  TLS handshake, then the first request) before anything arrives, while the lookup itself is paid once. */
export const CDN_ROUND_TRIPS = 3;
/** What a timeout costs in the speed score, in ms (mirrors `TIMEOUT_COST_MS`): a stub resolver waits about a second
 *  before asking again, so each timeout adds that, spread over every query sent. */
export const TIMEOUT_COST_MS = 1000;
/** Share of lookups that open a new connection and so pay the CDN edge penalty `CDN_ROUND_TRIPS` times. */
export const CONNECTION_SHARE = 0.5;
/** The kind of network the per-lookup figure is for. */
export type Mix = 'home' | 'office' | 'lab';
/** Share of a network's lookups the resolver answers from its cache, by kind of network. Lab is the speed score's
 *  own weighting; Home and Office reflect that the big resolvers hold the long tail of popular names warm. */
export const HIT_SHARE: Record<Mix, number> = { home: 0.8, office: 0.6, lab: 0.4 };
export const MIX_LABEL: Record<Mix, string> = { home: 'Home', office: 'Office', lab: 'Lab' };
/** The mix name for a hit share, as a label ("Home"), or the share itself when it is none of the three. */
function mixLabel(hitShare: number): string {
  const m = (Object.keys(HIT_SHARE) as Mix[]).find((k) => HIT_SHARE[k] === hitShare);
  return m ? MIX_LABEL[m] : pct(hitShare, 0);
}

export interface Dist {
  n: number;
  sent: number;
  timeouts: number;
  errors: number;
  values: number[]; // sorted ms of successful answers
  p25: number | null;
  p50: number | null;
  p75: number | null;
  p95: number | null;
  /** Sample standard deviation (as the engine's `sd`). */
  sd: number;
  min: number | null;
  max: number | null;
}

/** A local forwarder among the resolvers this computer uses (dnsmasq, Pi-hole, the gateway's DNS proxy; mirrors
 *  `is_baseline` in bench.rs): ranked as a reference line, never named the best pick, since it isn't an alternative to
 *  choose and its cache answers from your own network. An ISP's resolver is no such thing and competes. Runs made
 *  before detection existed have no `forwarder`; the same address, as the app knows it now, stands in. */
function forwarderOf(resolver: Resolver | undefined, ip: string, current: Resolver[] = []): string | null {
  if (!resolver || resolver.group !== 'system') return null;
  return resolver.forwarder || current.find((c) => c.group === 'system' && c.forwarder && (c.ipv4[0] ?? c.ipv6[0] ?? c.name) === ip)?.forwarder || null;
}

export interface Row {
  target: Target;
  resolver: Resolver | undefined;
  /** Your own local forwarder (what it is), shown for reference and never the pick; null for everything else. */
  forwarder: string | null;
  baseline: boolean;
  dist: Record<TestKind, Dist>;
  /** The speed score, in ms: what the fastest pick and the ranking sort by. */
  composite: number | null;
  /** What one lookup costs on average at the chosen traffic mix, in ms. */
  perLookup: number | null;
  /** The CDN edge part of `perLookup` alone; null when edges weren't mapped for this resolver. */
  cdnTerm: number | null;
  /** The resolver's median CDN penalty (ms per round trip), when CDN mapping ran. It has its own pick. */
  cdn: number | null;
  success: number;
  sent: number;
  intercepted: boolean;
  measured: boolean;
  eligible: boolean;
}

/** Rows partway (t from 0 to 1) from one live copy to the next: percentiles, scores and counts move in between,
 *  everything else is the newer copy's. A row with no earlier measured copy shows as it is. */
export function blendRows(a: Row[], b: Row[], t: number): Row[] {
  const prev = new Map(a.map((r) => [r.target.idx, r]));
  const mix = (x: number | null, y: number | null) => (x == null || y == null ? y : x + (y - x) * t);
  const count = (x: number, y: number) => Math.round(x + (y - x) * t);
  return b.map((r) => {
    const p = prev.get(r.target.idx);
    if (!p?.measured || !r.measured) return r;
    const dist = { ...r.dist };
    for (const k of Object.keys(dist) as TestKind[]) {
      const d = r.dist[k];
      const e = p.dist[k];
      if (!e) continue;
      dist[k] = {
        ...d,
        n: count(e.n, d.n),
        sent: count(e.sent, d.sent),
        timeouts: count(e.timeouts, d.timeouts),
        errors: count(e.errors, d.errors),
        p25: mix(e.p25, d.p25),
        p50: mix(e.p50, d.p50),
        p75: mix(e.p75, d.p75),
        p95: mix(e.p95, d.p95),
        sd: e.sd + (d.sd - e.sd) * t,
        min: mix(e.min, d.min),
        max: mix(e.max, d.max),
      };
    }
    return {
      ...r,
      dist,
      composite: mix(p.composite, r.composite),
      perLookup: mix(p.perLookup, r.perLookup),
      cdnTerm: mix(p.cdnTerm, r.cdnTerm),
      cdn: mix(p.cdn, r.cdn),
      success: p.success + (r.success - p.success) * t,
      sent: count(p.sent, r.sent),
    };
  });
}

function q(sorted: number[], p: number): number | null {
  if (!sorted.length) return null;
  const h = (sorted.length - 1) * p;
  const lo = Math.floor(h);
  const hi = Math.ceil(h);
  return sorted[lo] + (h - lo) * (sorted[hi] - sorted[lo]);
}

function sdev(v: number[]): number {
  if (v.length < 2) return 0;
  const m = v.reduce((a, x) => a + x, 0) / v.length;
  return Math.sqrt(v.reduce((a, x) => a + (x - m) ** 2, 0) / (v.length - 1));
}

function emptyDist(): Dist {
  return { n: 0, sent: 0, timeouts: 0, errors: 0, values: [], p25: null, p50: null, p75: null, p95: null, sd: 0, min: null, max: null };
}

export function buildRows(run: RunState, current: Resolver[] = [], hitShare = HIT_SHARE.home): Row[] {
  const n = run.targets.length;
  const acc: Record<TestKind, { v: number[]; sent: number; to: number; err: number }>[] = Array.from({ length: n }, () => ({
    cached: { v: [], sent: 0, to: 0, err: 0 },
    uncached: { v: [], sent: 0, to: 0, err: 0 },
    tld: { v: [], sent: 0, to: 0, err: 0 },
  }));
  for (const s of run.samples) {
    const a = acc[s.t]?.[s.k];
    // An Uncached answer that came from a cache measured nothing the test is for (as `compute_stats`).
    if (!a || s.o === 'from_cache') continue;
    a.sent++;
    if (s.o === 'ok') a.v.push(s.us / 1000);
    else if (s.o === 'timeout') a.to++;
    else a.err++;
  }
  const byId = new Map(run.resolvers.map((r) => [r.id, r]));
  const tests = run.config.tests;
  return run.targets.map((t, i) => {
    const dist = {} as Record<TestKind, Dist>;
    let ok = 0;
    let sent = 0;
    for (const k of TESTS) {
      const a = acc[i][k];
      const v = a.v.sort((x, y) => x - y);
      const d: Dist = {
        ...emptyDist(),
        n: v.length,
        sent: a.sent,
        timeouts: a.to,
        errors: a.err,
        values: v,
        p25: q(v, 0.25),
        p50: q(v, 0.5),
        p75: q(v, 0.75),
        p95: q(v, 0.95),
        sd: sdev(v),
        min: v[0] ?? null,
        max: v[v.length - 1] ?? null,
      };
      dist[k] = d;
      ok += d.n;
      sent += d.sent;
    }
    const success = sent ? ok / sent : 0;
    const composite = compositeOf(dist, tests);
    const measured = sent > 0;
    const cdn = run.cdn?.resolvers[t.resolver_id]?.penalty_ms ?? null;
    const cdnTerm = cdn == null ? null : CDN_ROUND_TRIPS * cdn * CONNECTION_SHARE;
    return {
      target: t,
      resolver: byId.get(t.resolver_id),
      forwarder: forwarderOf(byId.get(t.resolver_id), t.ip, current),
      baseline: forwarderOf(byId.get(t.resolver_id), t.ip, current) != null,
      dist,
      composite,
      perLookup: perLookupOf(dist, tests, t.cache_hits ?? null, cdn, hitShare),
      cdnTerm,
      cdn,
      success,
      sent,
      intercepted: t.intercepted,
      measured,
      eligible: measured && composite != null && !t.intercepted && success >= RELIABLE,
    };
  });
}

/** One test's typical latency (mirrors `typical` in bench.rs): the median, pulled toward the 95th percentile, plus
 *  part of the deviation. */
function typical(d: Dist): number | null {
  return d.p50 == null ? null : d.p50 + TAIL_WEIGHT * Math.max(0, (d.p95 ?? d.p50) - d.p50) + JITTER_WEIGHT * d.sd;
}

/** The speed score (mirrors `composite` in bench.rs): each test's typical latency by its weight, plus
 *  the timeouts' average cost. */
export function compositeOf(dist: Record<TestKind, Dist>, tests: TestKind[]): number | null {
  let sum = 0;
  let w = 0;
  let to = 0;
  let sent = 0;
  for (const k of TESTS) {
    to += dist[k].timeouts;
    sent += dist[k].sent;
    const v = tests.includes(k) ? typical(dist[k]) : null;
    if (v == null) continue;
    sum += v * WEIGHTS[k];
    w += WEIGHTS[k];
  }
  return w ? sum / w + (sent ? (TIMEOUT_COST_MS * to) / sent : 0) : null;
}

/** What one lookup costs on average at a traffic mix: the cached figure, corrected
 *  for answers that were really fresh lookups when the cache-hit rate is known, and the uncached and TLD figures for the
 *  rest, plus the timeouts' cost and the CDN edge penalty on the lookups that open a new connection. */
export function perLookupOf(dist: Record<TestKind, Dist>, tests: TestKind[], cacheHits: number | null, cdn: number | null, hitShare: number): number | null {
  const typ = (k: TestKind) => (tests.includes(k) ? typical(dist[k]) : null);
  let msum = 0;
  let mw = 0;
  for (const k of ['uncached', 'tld'] as const) {
    const v = typ(k);
    if (v == null) continue;
    msum += v * WEIGHTS[k];
    mw += WEIGHTS[k];
  }
  const miss = mw ? msum / mw : null;
  const c = typ('cached');
  const hit = c != null && cacheHits != null && cacheHits > 0 && cacheHits < 1 && miss != null ? Math.min(c, Math.max(0, (c - (1 - cacheHits) * miss) / cacheHits)) : c;
  let sum = 0;
  let w = 0;
  for (const [v, wt] of [[hit, hitShare], [miss, 1 - hitShare]] as const) {
    if (v == null) continue;
    sum += v * wt;
    w += wt;
  }
  if (!w) return null;
  const to = TESTS.reduce((a, k) => a + dist[k].timeouts, 0);
  const sent = TESTS.reduce((a, k) => a + dist[k].sent, 0);
  return sum / w + (sent ? (TIMEOUT_COST_MS * to) / sent : 0) + (cdn == null ? 0 : CDN_ROUND_TRIPS * cdn * CONNECTION_SHARE);
}

/** The timeouts' share of a row's composite, in ms. */
export function timeoutCost(r: Row): number {
  const to = TESTS.reduce((a, k) => a + r.dist[k].timeouts, 0);
  return r.sent ? (TIMEOUT_COST_MS * to) / r.sent : 0;
}

/** A small seeded generator, so the same run always gives the same answer. */
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}

/** Scores one row's distributions: the speed score, or the per-lookup figure with the row's own `cache_hits` and `cdn`
 *  held fixed. */
type Scorer = (dist: Record<TestKind, Dist>, tests: TestKind[], row: Row) => number | null;

/** The row's score again from its own answers drawn with replacement (timeouts and errors drawn as such). */
function resample(r: Row, tests: TestKind[], rand: () => number, score: Scorer): number | null {
  const dist = {} as Record<TestKind, Dist>;
  for (const k of TESTS) {
    const d = r.dist[k];
    const v: number[] = [];
    let to = 0;
    for (let j = 0; j < d.sent; j++) {
      const x = Math.floor(rand() * d.sent);
      if (x < d.n) v.push(d.values[x]);
      else if (x < d.n + d.timeouts) to++;
    }
    v.sort((x, y) => x - y);
    dist[k] = { ...emptyDist(), n: v.length, sent: d.sent, timeouts: to, values: v, p50: q(v, 0.5), p95: q(v, 0.95), sd: sdev(v) };
  }
  return score(dist, tests, r);
}

/** Whether `behind` is behind `ahead` by less than this run can tell apart: 200 bootstrap redraws of both
 *  speed scores (or whatever `score` computes), and the 95% interval of the difference reaches zero. Mirrors `tied` in
 *  bench.rs, which marks a tie in the run list. */
export function tiedWith(run: RunState, ahead: Row, behind: Row, score: Scorer = compositeOf): boolean {
  const tests = run.config.tests;
  const rand = mulberry32(hash(`${run.id}:${ahead.target.idx}:${behind.target.idx}`));
  const diffs: number[] = [];
  for (let i = 0; i < 200; i++) {
    const a = resample(ahead, tests, rand, score);
    const b = resample(behind, tests, rand, score);
    if (a != null && b != null) diffs.push(b - a);
  }
  if (diffs.length < 100) return false;
  diffs.sort((x, y) => x - y);
  return (q(diffs, 0.025) ?? 0) <= 0;
}

/** 'rank': the speed score (or one test's median), reliable targets first. */
export type SortKey = 'rank' | TestKind | 'success' | 'p95' | 'name' | 'resolver';

export function rank(rows: Row[], key: SortKey = 'rank', test: TestKind | 'all' = 'all'): Row[] {
  if (key === 'resolver') {
    // Each resolver's rows together (its transports), resolvers in order of their best row.
    const ranked = rank(rows, 'rank', test);
    const first = new Map<string, number>();
    ranked.forEach((r, i) => first.has(r.target.resolver_id) || first.set(r.target.resolver_id, i));
    const pos = new Map(ranked.map((r, i) => [r, i]));
    return ranked.sort((a, b) => first.get(a.target.resolver_id)! - first.get(b.target.resolver_id)! || pos.get(a)! - pos.get(b)!);
  }
  const tier = (r: Row) => (!r.measured ? 3 : r.intercepted ? 2 : r.success < RELIABLE ? 1 : 0);
  const metric = (r: Row): number => {
    if (key === 'success') return -r.success;
    if (key === 'p95') return worstP95(r) ?? Infinity;
    const k = key === 'rank' ? test : key;
    if (k === 'all' || k === 'name') return r.composite ?? Infinity;
    return r.dist[k].p50 ?? Infinity;
  };
  return [...rows].sort((a, b) => {
    if (key === 'name') return label(a.target.provider, a.target.variant).localeCompare(label(b.target.provider, b.target.variant));
    if (key === 'rank') {
      const t = tier(a) - tier(b);
      if (t) return t;
    }
    return metric(a) - metric(b);
  });
}

export function worstP95(r: Row): number | null {
  const v = TESTS.map((k) => r.dist[k].p95).filter((x): x is number => x != null);
  return v.length ? Math.max(...v) : null;
}

/** How much a resolver's answer times vary: each test's standard deviation, weighted like the ranking. Lower is steadier. */
export function spread(r: Row): number | null {
  let sum = 0;
  let w = 0;
  for (const k of TESTS) {
    if (r.dist[k].p50 == null) continue;
    sum += r.dist[k].sd * WEIGHTS[k];
    w += WEIGHTS[k];
  }
  return w ? sum / w : null;
}

export function check(run: RunState, resolverId: string, key: string): Check | undefined {
  return run.probes[resolverId]?.checks.find((c) => c.key === key);
}

export interface Pick {
  goal: string;
  resolverId: string;
  name: string;
  evidence: string;
  verified: boolean;
}

export interface Verdict {
  /** The fastest pick: the lowest speed score among reliable resolvers that aren't your own forwarder. */
  best: Row | null;
  /** The next resolver from another provider. */
  runnerUp: Row | null;
  /** Whether this run can't tell `best` and `runnerUp` apart. */
  tie: boolean;
  sentence: string;
  detail: string;
  picks: Pick[];
  caveats: string[];
}

/** Best eligible row per resolver (a resolver may have several transports). */
function bestPerResolver(rows: Row[]): Map<string, Row> {
  const m = new Map<string, Row>();
  for (const r of rank(rows.filter((x) => x.eligible && !x.baseline))) {
    if (!m.has(r.target.resolver_id)) m.set(r.target.resolver_id, r);
  }
  return m;
}

export function verdict(run: RunState, rows: Row[], hitShare = HIT_SHARE.home): Verdict {
  // Your own local resolver is a reference line, not a candidate.
  const eligible = rank(rows.filter((r) => r.eligible && !r.baseline));
  const best = eligible[0] ?? null;
  const runnerUp = eligible.find((r) => best && r.target.provider !== best.target.provider) ?? null;
  const caveats: string[] = [];
  const intercepted = rows.filter((r) => r.intercepted);
  if (intercepted.length) {
    caveats.push(
      `${intercepted.length === 1 ? 'One plain-DNS target was' : `${intercepted.length} plain-DNS targets were`} answered by a device on your network rather than the resolver, so ${intercepted.length === 1 ? 'it is' : 'they are'} left out of the ranking.`,
    );
  }
  if (!best) {
    return {
      best: null,
      runnerUp: null,
      tie: false,
      sentence: rows.some((r) => r.measured) ? 'No resolver answered reliably enough to recommend.' : 'Nothing has been measured yet.',
      detail: rows.some((r) => r.measured)
        ? `Every measured resolver dropped more than ${pct(1 - RELIABLE, 0)} of queries or was intercepted. See the notes below.`
        : '',
      picks: [],
      caveats,
    };
  }
  const nameOfRow = (r: Row) => label(r.target.provider, r.target.variant);
  const name = nameOfRow(best);
  const ahead = rank(rows).find((r) => r.eligible && r.baseline && (r.composite ?? Infinity) < (best.composite ?? Infinity));
  if (ahead) {
    caveats.push(
      `${nameOfRow(ahead)} scores faster than ${name}, but it is a forwarder on your network (${ahead.forwarder}), so it is a reference line and not a pick: it answers repeats from a cache on your network and forwards the rest to whatever you choose.`,
    );
  }
  const multi = run.config.transports.length > 1;
  const via = (r: Row) => (multi || r.target.transport !== 'udp' ? ` over ${TRANSPORT_LABEL[r.target.transport]}` : '');

  // Ties happen (a forwarder and its upstream); "0.00 ms slower" would read as noise, and a gap this run's own
  // variation could produce is said to be one rather than crowning a winner.
  const gap = runnerUp?.composite != null && best.composite != null ? runnerUp.composite - best.composite : null;
  const level = gap != null && gap < 0.05;
  const tie = runnerUp != null && gap != null && (level || tiedWith(run, best, runnerUp));

  const per = bestPerResolver(rows);
  const picks: Pick[] = [];
  const speedOf = (id: string) => per.get(id)?.composite ?? Infinity;
  const candidates = [...per.keys()];
  const nameOf = (id: string) => nameOfRow(per.get(id)!);

  const next = runnerUp ? nameOfRow(runnerUp) : '';
  picks.push({
    goal: 'Fastest answers',
    resolverId: best.target.resolver_id,
    name,
    evidence: `${ms(best.composite)} speed score${
      gap == null ? '' : level ? `, level with ${next}` : tie ? `, tied with ${next}: ${ms(gap)} is within this run’s variation` : `, ${ms(gap)} ahead of ${next}`
    }`,
    verified: true,
  });

  if (run.cdn) {
    const cdnCands = candidates
      .map((id) => ({ id, c: run.cdn!.resolvers[id] }))
      .filter((x) => x.c && x.c.penalty_ms != null)
      .sort((a, b) => a.c.penalty_ms! - b.c.penalty_ms! || speedOf(a.id) - speedOf(b.id));
    const top = cdnCands[0];
    if (top) {
      const basis =
        top.c.basis === 'ecs'
          ? 'CDNs map it by your subnet (ECS)'
          : top.c.basis === 'on_net'
            ? 'resolves from inside your ISP'
            : top.c.basis === 'ecs_substitute'
              ? 'sends a substitute subnet'
              : 'CDNs map it by its own location';
      picks.push({
        goal: 'Nearest CDN edges',
        resolverId: top.id,
        name: nameOf(top.id),
        evidence: `${top.c.penalty_ms! < 0.05 ? 'on the best edges' : `${ms(top.c.penalty_ms)} from the best edges`}; ${basis}`,
        verified: true,
      });
    }
  }

  // Protection and privacy are things your own resolver can already give you (a Pi-hole with blocklists, forwarding to
  // a filtering upstream), so it competes for those picks and keeps them when it meets the bar: no change needed. Only a
  // local forwarder counts for privacy; an ISP's resolver sees who is asking.
  const own = new Map<string, Row>();
  for (const r of rank(rows.filter((x) => x.eligible && x.resolver?.group === 'system'))) {
    if (!own.has(r.target.resolver_id)) own.set(r.target.resolver_id, r);
  }
  const pool = [...new Set([...candidates, ...own.keys()])];
  const rowOf = (id: string) => per.get(id) ?? own.get(id)!;
  const speedIn = (id: string) => rowOf(id).composite ?? Infinity;
  const ownFirst = (a: string, b: string) => Number(own.has(b)) - Number(own.has(a)) || speedIn(a) - speedIn(b);
  const yours = (id: string) => (own.has(id) ? 'already your resolver, no change needed; ' : '');

  const protective = pool
    .filter((id) => check(run, id, 'filter_malware')?.status === 'pass' && check(run, id, 'dnssec')?.present === true)
    .sort(ownFirst);
  if (protective[0]) {
    const mal = run.probes[protective[0]]?.filtering?.malware;
    picks.push({
      goal: 'Best protection',
      resolverId: protective[0],
      name: nameOfRow(rowOf(protective[0])),
      evidence: `${yours(protective[0])}blocked ${mal ? `${mal.blocked} of ${mal.tested}` : 'most'} malware test domains; validates DNSSEC`,
      verified: true,
    });
  }

  // Blocking trackers keeps them from seeing you at all, so among private resolvers the ones that do come first.
  const tracks = (id: string) => (check(run, id, 'filter_ads')?.present === true ? 1 : 0);
  const priv = pool
    .filter((id) => {
      const r = rowOf(id);
      return (
        (r.resolver?.group !== 'system' || r.forwarder != null) &&
        check(run, id, 'ecs')?.present === false &&
        check(run, id, 'qmin')?.present === true &&
        check(run, id, 'dnssec')?.present === true
      );
    })
    .sort((a, b) => tracks(b) - tracks(a) || ownFirst(a, b));
  if (priv[0]) {
    picks.push({
      goal: 'Most private',
      resolverId: priv[0],
      name: nameOfRow(rowOf(priv[0])),
      evidence: `${yours(priv[0])}no client subnet sent, QNAME minimisation on, validates DNSSEC${tracks(priv[0]) ? ', blocks trackers' : ''}`,
      verified: true,
    });
  }

  const steady = [...per.values()].sort((a, b) => b.success - a.success || (spread(a) ?? Infinity) - (spread(b) ?? Infinity));
  if (steady[0]) {
    picks.push({
      goal: 'Steadiest',
      resolverId: steady[0].target.resolver_id,
      name: label(steady[0].target.provider, steady[0].target.variant),
      evidence: `${pct(steady[0].success)} answered; answer times vary by only ${ms(spread(steady[0]))} (standard deviation)`,
      verified: true,
    });
  }

  // Best from here adds the speed figures, the measured cache-hit rate and the CDN penalty at the chosen mix; unlike
  // Fastest it may be a tie, which it says.
  const lookups = [...per.values()].filter((r) => r.perLookup != null).sort((a, b) => a.perLookup! - b.perLookup! || (a.composite ?? Infinity) - (b.composite ?? Infinity));
  const top = lookups[0];
  if (top) {
    const nextRow = lookups.find((r) => r.target.provider !== top.target.provider);
    const pgap = nextRow ? nextRow.perLookup! - top.perLookup! : null;
    const plevel = pgap != null && pgap < 0.05;
    const perLookupScorer: Scorer = (dist, tests, row) => perLookupOf(dist, tests, row.target.cache_hits ?? null, row.cdn, hitShare);
    const ptie = nextRow != null && pgap != null && (plevel || tiedWith(run, top, nextRow, perLookupScorer));
    const pnext = nextRow ? nameOfRow(nextRow) : '';
    picks.push({
      goal: 'Best from here',
      resolverId: top.target.resolver_id,
      name: nameOfRow(top),
      evidence: `${ms(top.perLookup)} per lookup at the ${mixLabel(hitShare)} mix${
        pgap == null ? '' : plevel ? `, level with ${pnext}` : ptie ? `, tied with ${pnext}: ${ms(pgap)} is within this run’s variation` : `, ${ms(pgap)} ahead of ${pnext}`
      }${lookups.every((r) => r.cdnTerm == null) ? ' (no CDN term)' : ''}`,
      verified: true,
    });
  }

  const sentence = tie
    ? `${name}${via(best)} and ${next}${via(runnerUp!)} are tied for fastest from this network.`
    : `${name}${via(best)} answers fastest from this network.`;
  const parts: string[] = [];
  const u = best.dist.uncached.p50;
  const c = best.dist.cached.p50;
  if (u != null) parts.push(`${ms(u)} median on uncached lookups`);
  if (c != null) parts.push(`${ms(c)} cached`);
  if (best.cdn != null) parts.push(best.cdn < 0.05 ? 'CDN edges as near as any resolver found' : `CDN edges ${ms(best.cdn)} from the nearest`);
  parts.push(best.success === 1 ? 'every query answered' : `${pct(best.success)} answered`);
  let detail = parts.join(', ') + '.';
  if (runnerUp && gap != null && !tie) detail += ` ${next} is next, ${ms(gap)} slower.`;
  // The quickest answer can send you to slower edges, and every new connection pays for that.
  // Under a millisecond a round trip, the difference isn't worth a sentence.
  const near = per.get(picks.find((p) => p.goal === 'Nearest CDN edges')?.resolverId ?? '');
  if (near && near.target.resolver_id !== best.target.resolver_id && near.cdn != null && best.cdn != null && best.cdn - near.cdn >= 1 && near.composite != null && best.composite != null) {
    const slower = tie && near.target.resolver_id === runnerUp?.target.resolver_id ? '' : ` answers ${ms(Math.max(0, near.composite - best.composite))} slower but`;
    detail += ` ${nameOfRow(near)}${slower} sends you to CDN edges ${ms(best.cdn - near.cdn)} nearer, and every new connection pays that at least ${CDN_ROUND_TRIPS} times.`;
  }
  const also = picks.slice(1).filter((p) => p.resolverId === best.target.resolver_id).map((p) => p.goal[0].toLowerCase() + p.goal.slice(1));
  if (also.length) detail += ` It is also the pick for ${also.length > 1 ? `${also.slice(0, -1).join(', ')} and ${also[also.length - 1]}` : also[0]}.`;

  return { best, runnerUp, tie, sentence, detail, picks, caveats };
}
