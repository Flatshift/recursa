// Derived views of a run: per-target statistics, ranking, and the verdict.
// The rules here are deliberately simple and are restated in the UI so the
// ranking can always be explained in one sentence.

import type { Check, Outcome, Resolver, RunState, Target, TestKind } from './types';
import { TRANSPORT_LABEL, label, list, ms, pct } from './format';

export const TESTS: TestKind[] = ['cached', 'uncached', 'tld'];
/** What each test counts for in the speed score (mirrors `TEST_WEIGHTS` in bench.rs): cached answers are most of
 *  what people get, uncached lookups are the delays they notice, and TLD lookups (NXDOMAIN) are a small share of real
 *  traffic. */
export const WEIGHTS: Record<TestKind, number> = { cached: 0.5, uncached: 0.4, tld: 0.1 };
/** The two halves of the speed score, each a pick of its own: the cached test alone, and the uncached and TLD tests at
 *  the full score's 4:1 ratio, so before the failure cost the speed score is the mean of the two. */
export const PART_WEIGHTS: Record<'cache' | 'fresh', Record<TestKind, number>> = {
  cache: { cached: 1, uncached: 0, tld: 0 },
  fresh: { cached: 0, uncached: 0.8, tld: 0.2 },
};
export const RELIABLE = 0.98;

/** Whether a resolver is expected to block a category: one it claims, or one on this network (or added by hand) that
 *  the user is checking. */
export function expectsBlock(r: Resolver, cat: string): boolean {
  return r.group === 'system' || r.group === 'custom' || r.published.filtering.includes(cat);
}
/** A new connection waits at least this many round trips on the CDN edge the resolver chose (the TCP handshake, the
 *  TLS handshake, then the first request) before anything arrives, while the lookup itself is paid once. */
export const CDN_ROUND_TRIPS = 3;
/** What a timeout or a failed answer costs in the speed score, in ms (mirrors `TIMEOUT_COST_MS`): a stub resolver waits
 *  about a second before asking again, so each one adds that, spread over every query sent. */
export const TIMEOUT_COST_MS = 1000;
/** Failures from this many distinct resolvers inside one window of this length are a network blip from here, excused
 *  and counted as nothing (mirrors `BLIP_WINDOW_MS` and `BLIP_RESOLVERS`). */
const BLIP_WINDOW_MS = 2000;
const BLIP_RESOLVERS = 3;
/** The blip rule as the UI states it, so the copy follows the constants. */
export const BLIP_RULE = `Failures that hit ${BLIP_RESOLVERS} or more resolvers within the same ${BLIP_WINDOW_MS / 1000} seconds are a network blip from here and count as nothing.`;
/** Whether an outcome costs in the score: no usable answer, and the cause sits at the resolver or on the path to it
 *  (mirrors `Outcome::costs`). A filter's block or rewrite is the resolver doing as configured. */
function costs(o: Outcome): boolean {
  return o === 'servfail' || o === 'refused' || o === 'rcode' || o === 'neterr' || o === 'unreachable';
}
/** Indices into `run.samples` of failures that hit `BLIP_RESOLVERS` or more distinct resolvers within one
 *  `BLIP_WINDOW_MS` bucket (as `excuse_blips`). The engine sets the flag when the phase closes; the live view computes it. */
export function excused(run: RunState): Set<number> {
  const failed = (o: Outcome) => o === 'timeout' || costs(o);
  const buckets = new Map<number, Set<string>>();
  run.samples.forEach((s) => {
    const id = run.targets[s.t]?.resolver_id;
    if (id == null || !failed(s.o)) return;
    const b = Math.floor(s.at / BLIP_WINDOW_MS);
    if (!buckets.has(b)) buckets.set(b, new Set());
    buckets.get(b)!.add(id);
  });
  const out = new Set<number>();
  run.samples.forEach((s, i) => {
    if (failed(s.o) && (buckets.get(Math.floor(s.at / BLIP_WINDOW_MS))?.size ?? 0) >= BLIP_RESOLVERS) out.add(i);
  });
  return out;
}
/** Share of lookups that open a new connection and so pay the CDN edge penalty `CDN_ROUND_TRIPS` times. */
export const CONNECTION_SHARE = 0.5;
/** The kind of network the per-lookup figure is for. */
export type Mix = 'home' | 'office' | 'lab';
/** Share of a network's lookups the resolver answers from its cache, by kind of network. Lab is the speed score's
 *  own weighting; Home and Office reflect that the big resolvers hold the long tail of popular names warm. */
export const HIT_SHARE: Record<Mix, number> = { home: 0.8, office: 0.6, lab: 0.5 };
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
  /** Errors that cost in the score (SERVFAIL, REFUSED, other error codes, network errors); a subset of `errors`. */
  failures: number;
  values: number[]; // sorted ms of successful answers
  p25: number | null;
  p50: number | null;
  p75: number | null;
  p95: number | null;
  /** Sample standard deviation (as the engine's `sd`). */
  sd: number;
  min: number | null;
  max: number | null;
  /** The mean after the slowest tenth is pulled down to the 90th percentile (as the engine's `wmean`). */
  wmean: number | null;
  /** The resolver began refusing or went silent during this test: left out of the score and the answered share. */
  limited: boolean;
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
        failures: count(e.failures, d.failures),
        p25: mix(e.p25, d.p25),
        p50: mix(e.p50, d.p50),
        p75: mix(e.p75, d.p75),
        p95: mix(e.p95, d.p95),
        sd: e.sd + (d.sd - e.sd) * t,
        min: mix(e.min, d.min),
        max: mix(e.max, d.max),
        wmean: mix(e.wmean, d.wmean),
        limited: d.limited,
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

/** The mean after capping each value at the 90th percentile (mirrors `summarize`'s `wmean`); `values` sorted. */
export function winsorMean(v: number[]): number | null {
  const cap = q(v, 0.9);
  return cap == null ? null : v.reduce((a, x) => a + Math.min(x, cap), 0) / v.length;
}

function emptyDist(): Dist {
  return { n: 0, sent: 0, timeouts: 0, errors: 0, failures: 0, values: [], p25: null, p50: null, p75: null, p95: null, sd: 0, min: null, max: null, wmean: null, limited: false };
}

export function buildRows(run: RunState, current: Resolver[] = [], hitShare = HIT_SHARE.home): Row[] {
  const n = run.targets.length;
  const acc: Record<TestKind, { v: number[]; sent: number; to: number; err: number; fail: number }>[] = Array.from({ length: n }, () => ({
    cached: { v: [], sent: 0, to: 0, err: 0, fail: 0 },
    uncached: { v: [], sent: 0, to: 0, err: 0, fail: 0 },
    tld: { v: [], sent: 0, to: 0, err: 0, fail: 0 },
  }));
  const skip = excused(run);
  run.samples.forEach((s, j) => {
    const a = acc[s.t]?.[s.k];
    // An Uncached answer that came from a cache measured nothing the test is for, and an excused failure was the
    // network's: neither is counted (as `compute_stats`).
    if (!a || s.o === 'from_cache' || skip.has(j)) return;
    a.sent++;
    if (s.o === 'ok') a.v.push(s.us / 1000);
    else if (s.o === 'timeout') a.to++;
    else {
      a.err++;
      if (costs(s.o)) a.fail++;
    }
  });
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
        failures: a.fail,
        values: v,
        p25: q(v, 0.25),
        p50: q(v, 0.5),
        p75: q(v, 0.75),
        p95: q(v, 0.95),
        sd: sdev(v),
        min: v[0] ?? null,
        max: v[v.length - 1] ?? null,
        wmean: winsorMean(v),
        limited: t.stats?.[k]?.limited ?? false,
      };
      dist[k] = d;
      if (!d.limited) {
        ok += d.n;
        sent += d.sent;
      }
    }
    const success = sent ? ok / sent : 0;
    const composite = compositeOf(dist, tests);
    const measured = sent > 0;
    const cdn = run.cdn?.resolvers[t.resolver_id]?.penalty_ms ?? null;
    const cdnTerm = cdn == null ? null : CDN_ROUND_TRIPS * cdn * CONNECTION_SHARE;
    const fwd = forwarderOf(byId.get(t.resolver_id), t.ip, current);
    return {
      target: t,
      resolver: byId.get(t.resolver_id),
      forwarder: fwd,
      baseline: fwd != null,
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

/** One test's cost (mirrors `cost` in bench.rs): its winsorised mean, or nothing for a limited test or one with no
 *  answers. */
function cost(d: Dist): number | null {
  return d.limited ? null : d.wmean;
}

/** The speed score (mirrors `composite` in bench.rs): each test's cost by its weight, plus the average cost of
 *  timeouts and failed answers. A limited test is left out of both, and so is a test the weights give nothing. */
export function compositeOf(dist: Record<TestKind, Dist>, tests: TestKind[], weights: Record<TestKind, number> = WEIGHTS): number | null {
  let sum = 0;
  let w = 0;
  let to = 0;
  let sent = 0;
  for (const k of TESTS) {
    if (dist[k].limited || !(weights[k] > 0)) continue;
    to += dist[k].timeouts + dist[k].failures;
    sent += dist[k].sent;
    const v = tests.includes(k) ? cost(dist[k]) : null;
    if (v == null) continue;
    sum += v * weights[k];
    w += weights[k];
  }
  return w ? sum / w + (sent ? (TIMEOUT_COST_MS * to) / sent : 0) : null;
}

/** The terms behind a per-lookup figure: what a cached and a fresh lookup cost, what timeouts and failed answers add,
 *  and the CDN edge penalty, with their sum. */
interface PerLookupTerms {
  /** The cached figure corrected for answers that were really fresh lookups; null when the cached test did not run. */
  hit: number | null;
  /** The uncached and TLD figures blended by their weights; null when neither test ran. */
  miss: number | null;
  /** What timeouts and failed answers add, in ms. */
  failures: number;
  /** What CDN edge distance adds on lookups that open a new connection; null without CDN mapping. */
  cdn: number | null;
  total: number;
}

/** What one lookup costs on average at a traffic mix: the cached figure, corrected
 *  for answers that were really fresh lookups when the cache-hit rate is known, and the uncached and TLD figures for the
 *  rest, plus the cost of timeouts and failed answers and the CDN edge penalty on the lookups that open a new connection. */
function perLookupTerms(dist: Record<TestKind, Dist>, tests: TestKind[], cacheHits: number | null, cdn: number | null, hitShare: number): PerLookupTerms | null {
  const typ = (k: TestKind) => (tests.includes(k) ? cost(dist[k]) : null);
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
  const kept = TESTS.filter((k) => !dist[k].limited);
  const to = kept.reduce((a, k) => a + dist[k].timeouts + dist[k].failures, 0);
  const sent = kept.reduce((a, k) => a + dist[k].sent, 0);
  const failures = sent ? (TIMEOUT_COST_MS * to) / sent : 0;
  const edge = cdn == null ? null : CDN_ROUND_TRIPS * cdn * CONNECTION_SHARE;
  return { hit, miss, failures, cdn: edge, total: sum / w + failures + (edge ?? 0) };
}

export function perLookupOf(dist: Record<TestKind, Dist>, tests: TestKind[], cacheHits: number | null, cdn: number | null, hitShare: number): number | null {
  return perLookupTerms(dist, tests, cacheHits, cdn, hitShare)?.total ?? null;
}

/** The share of a row's composite that timeouts and failed answers cost, in ms. */
export function failureCost(r: Row): number {
  const to = TESTS.reduce((a, k) => a + (r.dist[k].limited ? 0 : r.dist[k].timeouts + r.dist[k].failures), 0);
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

/** The row's score again from its own answers drawn with replacement (timeouts, failed answers and other errors drawn as such). */
function resample(r: Row, tests: TestKind[], rand: () => number, score: Scorer): number | null {
  const dist = {} as Record<TestKind, Dist>;
  for (const k of TESTS) {
    const d = r.dist[k];
    // A limited test draws nothing and is left out, as `tied` does.
    if (d.limited) {
      dist[k] = { ...emptyDist(), limited: true };
      continue;
    }
    const v: number[] = [];
    let to = 0;
    let fail = 0;
    for (let j = 0; j < d.sent; j++) {
      const x = Math.floor(rand() * d.sent);
      if (x < d.n) v.push(d.values[x]);
      else if (x < d.n + d.timeouts) to++;
      else if (x < d.n + d.timeouts + d.failures) fail++;
    }
    v.sort((x, y) => x - y);
    dist[k] = { ...emptyDist(), n: v.length, sent: d.sent, timeouts: to, failures: fail, values: v, p50: q(v, 0.5), p95: q(v, 0.95), sd: sdev(v), wmean: winsorMean(v) };
  }
  return score(dist, tests, r);
}

/** Whether `behind` is behind `ahead` by less than this run can tell apart: 200 bootstrap redraws of both
 *  speed scores (or whatever `score` computes), and the 95% interval of the difference reaches zero. Mirrors `tied` in
 *  bench.rs, which marks a tie in the run list. */
export function tiedWith(run: RunState, ahead: Row, behind: Row, score: Scorer = (dist, tests) => compositeOf(dist, tests)): boolean {
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
function spread(r: Row): number | null {
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

/** The small graph a pick row draws: what that goal was decided on. */
export type PickGraph =
  /** The winner's answered queries on the shared log axis, for the tests the goal counts, with its score as a tick. */
  | { kind: 'dots'; target: number; tests: TestKind[]; at: number | null }
  /** Each candidate's distance from the best CDN edges on one axis from zero, winner first; nearer zero wins. */
  | { kind: 'edges'; cands: { name: string; v: number }[] }
  /** Each candidate's answer-time spread on one axis from zero, winner first; nearer zero wins. */
  | { kind: 'spread'; cands: { name: string; v: number; ok: number }[] }
  /** Each candidate's share of live threats blocked on one axis from zero to all, winner first; further right wins. */
  | { kind: 'share'; cands: { name: string; blocked: number; tested: number }[] }
  /** The terms a figure adds up, as one bar; labels match the pick's parts. */
  | { kind: 'stack'; terms: { label: string; v: number; c: 'cached' | 'uncached' | 'neutral' | 'tld' }[] }
  /** Each probed resolver a column, each privacy property a row, filled where it holds; the winner's column first. */
  | { kind: 'checks'; rows: string[]; cols: { name: string; on: boolean[]; win: boolean }[] };

/** The goals a verdict picks for, in the order the picks list them. */
export const GOAL = {
  fastest: 'Fastest answers',
  cache: 'Fastest cache',
  fresh: 'Fastest fresh lookups',
  cdn: 'Nearest CDN edges',
  protection: 'Best protection',
  private: 'Most private',
  steadiest: 'Steadiest',
} as const;

interface Pick {
  goal: string;
  resolverId: string;
  name: string;
  /** The deciding number or fact, a few words. */
  figure: string;
  /** What sits behind the figure, one short phrase; may be empty. */
  note: string;
  /** The full sentence the figure and note abbreviate. */
  evidence: string;
  verified: boolean;
  /** The terms the figure adds up, as label and value pairs; only Best from here has them. */
  parts?: [string, string][];
  graph?: PickGraph;
}

export interface Verdict {
  /** The fastest pick: the lowest speed score among reliable resolvers that aren't your own forwarder. */
  best: Row | null;
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
  const lead = (first: Row, a: number | null | undefined, other: Row | null | undefined, b: number | null | undefined, scorer?: Scorer) => {
    if (!other || a == null || b == null) return { gap: null, tie: false, text: '', short: '' };
    const gap = b - a;
    const next = nameOfRow(other);
    // The short form names the provider only: the runner-up is always from another provider.
    const prov = other.target.provider;
    if (gap < 0.05) return { gap, tie: true, text: `, level with ${next}`, short: `level with ${prov}` };
    if (tiedWith(run, first, other, scorer)) return { gap, tie: true, text: `, tied with ${next}: ${ms(gap)} is within this run’s variation`, short: `tied with ${prov}` };
    return { gap, tie: false, text: `, ${ms(gap)} ahead of ${next}`, short: `${ms(gap)} ahead of ${prov}` };
  };
  const { gap, tie, text: fastest, short: fastestShort } = lead(best, best.composite, runnerUp, runnerUp?.composite);

  const per = bestPerResolver(rows);
  const picks: Pick[] = [];
  const speedOf = (id: string) => per.get(id)?.composite ?? Infinity;
  const candidates = [...per.keys()];
  const nameOf = (id: string) => nameOfRow(per.get(id)!);

  const next = runnerUp ? nameOfRow(runnerUp) : '';
  picks.push({
    goal: GOAL.fastest,
    resolverId: best.target.resolver_id,
    name,
    figure: ms(best.composite),
    note: fastestShort,
    evidence: `${ms(best.composite)} speed score${fastest}`,
    verified: true,
    graph: { kind: 'dots', target: best.target.idx, tests: run.config.tests, at: best.composite },
  });

  // The two halves of the speed score as picks of their own. A resolver with no figure for a half is left out of it.
  const halves = [
    { goal: GOAL.cache, part: PART_WEIGHTS.cache, what: 'on cached answers', tests: ['cached'] as TestKind[] },
    { goal: GOAL.fresh, part: PART_WEIGHTS.fresh, what: 'on fresh lookups (uncached and TLD)', tests: ['uncached', 'tld'] as TestKind[] },
  ];
  for (const { goal, part, what, tests } of halves) {
    const partScorer: Scorer = (dist, tests) => compositeOf(dist, tests, part);
    const scored = [...per.values()]
      .map((r) => ({ r, s: compositeOf(r.dist, run.config.tests, part) }))
      .filter((x): x is { r: Row; s: number } => x.s != null)
      .sort((a, b) => a.s - b.s || (a.r.composite ?? Infinity) - (b.r.composite ?? Infinity));
    const first = scored[0];
    if (!first) continue;
    const other = scored.find((x) => x.r.target.provider !== first.r.target.provider);
    const l = lead(first.r, first.s, other?.r, other?.s, partScorer);
    picks.push({
      goal,
      resolverId: first.r.target.resolver_id,
      name: nameOfRow(first.r),
      figure: ms(first.s),
      note: l.short,
      evidence: `${ms(first.s)} ${what}${l.text}`,
      verified: true,
      graph: { kind: 'dots', target: first.r.target.idx, tests: tests.filter((k) => run.config.tests.includes(k)), at: first.s },
    });
  }

  if (run.cdn) {
    const cdnCands = candidates
      .map((id) => ({ id, c: run.cdn!.resolvers[id] }))
      .filter((x) => x.c && x.c.penalty_ms != null)
      .sort((a, b) => a.c.penalty_ms! - b.c.penalty_ms! || speedOf(a.id) - speedOf(b.id));
    const top = cdnCands[0];
    if (top) {
      const [basis, basisShort] =
        top.c.basis === 'ecs'
          ? ['CDNs map it by your subnet (ECS)', 'by your subnet (ECS)']
          : top.c.basis === 'on_net'
            ? ['resolves from inside your ISP', 'inside your ISP']
            : top.c.basis === 'ecs_substitute'
              ? ['sends a substitute subnet', 'substitute subnet']
              : ['CDNs map it by its own location', 'by its own location'];
      const onBest = top.c.penalty_ms! < 0.05;
      picks.push({
        goal: GOAL.cdn,
        resolverId: top.id,
        name: nameOf(top.id),
        figure: onBest ? 'best edges' : `+${ms(top.c.penalty_ms)}`,
        note: basisShort,
        evidence: `${onBest ? 'on the best edges' : `${ms(top.c.penalty_ms)} from the best edges`}; ${basis}`,
        verified: true,
        graph: { kind: 'edges', cands: cdnCands.map((x) => ({ name: nameOf(x.id), v: x.c.penalty_ms! })) },
      });
    }
  }

  // Protection and privacy are things your own resolver can already give you (a Pi-hole with blocklists, forwarding to
  // a filtering upstream), so it competes for those picks. It keeps privacy when it meets the bar, as no change is needed;
  // protection ties go to the faster cache. Only a local forwarder counts for privacy; an ISP's resolver sees who is asking.
  const own = new Map<string, Row>();
  for (const r of rank(rows.filter((x) => x.eligible && x.resolver?.group === 'system'))) {
    if (!own.has(r.target.resolver_id)) own.set(r.target.resolver_id, r);
  }
  const pool = [...new Set([...candidates, ...own.keys()])];
  const rowOf = (id: string) => per.get(id) ?? own.get(id)!;
  const speedIn = (id: string) => rowOf(id).composite ?? Infinity;
  const ownFirst = (a: string, b: string) => Number(own.has(b)) - Number(own.has(a)) || speedIn(a) - speedIn(b);
  const yours = (id: string) => (own.has(id) ? 'already your resolver, no change needed; ' : '');
  const yoursShort = (id: string) => (own.has(id) ? 'already yours, ' : '');
  // Equal protection goes to the faster cached answers, which is most of what you get.
  const cacheIn = (id: string) => compositeOf(rowOf(id).dist, run.config.tests, PART_WEIGHTS.cache) ?? Infinity;
  const cacheFirst = (a: string, b: string) => cacheIn(a) - cacheIn(b) || ownFirst(a, b);

  // With the live threat test, protection is the share of live threats a resolver blocked; without it, the test names
  // only show that blocking is on, so the pick is the resolver holding the most protections and the graph shows them.
  const dnssecOf = (id: string) => check(run, id, 'dnssec')?.present;
  const liveOf = (id: string) => {
    const cats = run.live?.[id]?.categories;
    const [m, p] = [cats?.malware, cats?.phishing];
    return { mb: m?.blocked ?? 0, mt: m?.tested ?? 0, pb: p?.blocked ?? 0, pt: p?.tested ?? 0 };
  };
  const liveTested = pool.filter((id) => liveOf(id).mt + liveOf(id).pt > 0);
  if (liveTested.length) {
    const share = (id: string) => {
      const { mb, mt, pb, pt } = liveOf(id);
      return { blocked: mb + pb, tested: mt + pt };
    };
    const ranked = liveTested
      .filter((id) => dnssecOf(id) !== false)
      .sort((a, b) => share(b).blocked / share(b).tested - share(a).blocked / share(a).tested || cacheFirst(a, b));
    const win = ranked[0];
    if (win && share(win).blocked >= 1) {
      const { mb, mt, pb, pt } = liveOf(win);
      const { blocked, tested } = share(win);
      const second = ranked[1];
      const dnssec = dnssecOf(win) === true;
      const by = [mt && `malware ${mb} of ${mt}`, pt && `phishing ${pb} of ${pt}`].filter(Boolean);
      const feeds = mt ? ['URLhaus', 'ThreatView'] : [];
      picks.push({
        goal: GOAL.protection,
        resolverId: win,
        name: nameOfRow(rowOf(win)),
        figure: `blocked ${blocked} of ${tested}`,
        note: `${yoursShort(win)}live ${by.join(', ')}${dnssec ? ', DNSSEC' : ''}`,
        evidence: `${yours(win)}blocked ${blocked} of ${tested} live hosts from today's ${list(feeds)} feeds${second ? `; next best ${nameOfRow(rowOf(second))} ${share(second).blocked} of ${share(second).tested}` : ''}${dnssec ? '; validates DNSSEC' : ''}`,
        verified: true,
        graph: { kind: 'share', cands: ranked.map((id) => ({ name: nameOfRow(rowOf(id)), ...share(id) })) },
      });
    }
  } else {
    // The bar is malware blocking and DNSSEC; the rest decide between resolvers that clear it.
    const held = (id: string) => [
      check(run, id, 'filter_malware')?.status === 'pass',
      check(run, id, 'filter_phishing')?.status === 'pass',
      dnssecOf(id) === true,
      check(run, id, 'rebind')?.present === true,
    ];
    const count = (id: string) => held(id).filter(Boolean).length;
    const protective = pool.filter((id) => held(id)[0] && held(id)[2]).sort((a, b) => count(b) - count(a) || cacheFirst(a, b));
    const win = protective[0];
    if (win) {
      const rebind = held(win)[3];
      const hit = run.probes[win]?.filtering?.malware?.samples.filter((s) => s.blocked).map((s) => s.domain) ?? [];
      picks.push({
        goal: GOAL.protection,
        resolverId: win,
        name: nameOfRow(rowOf(win)),
        figure: `${count(win)} of ${held(win).length} held`,
        note: `${yoursShort(win)}${rebind ? 'DNSSEC, rebinding protection' : 'DNSSEC'}; test names`,
        evidence: `${yours(win)}blocked ${hit.length ? list(hit) : 'a malware test name'}; validates DNSSEC${rebind ? '; strips private-address answers' : ''}. Test names show blocking is on, not how much it catches; the live threat test in setup measures that.`,
        verified: true,
        graph: {
          kind: 'checks',
          rows: ['Malware', 'Phishing', 'DNSSEC', 'Rebinding'],
          cols: pool
            .filter((id) => run.probes[id])
            .sort((a, b) => Number(b === win) - Number(a === win) || count(b) - count(a))
            .map((id) => ({ name: nameOfRow(rowOf(id)), on: held(id), win: id === win })),
        },
      });
    }
  }

  // Blocking trackers keeps them from seeing you at all, so among private resolvers the ones that do come first.
  const tracks = (id: string) => (check(run, id, 'filter_ads')?.present === true ? 1 : 0);
  const privacy: [string, (id: string) => boolean][] = [
    ['No client subnet', (id) => check(run, id, 'ecs')?.present === false],
    ['QNAME minimisation', (id) => check(run, id, 'qmin')?.present === true],
    ['DNSSEC', (id) => check(run, id, 'dnssec')?.present === true],
    ['Blocks trackers', (id) => tracks(id) === 1],
  ];
  const named = pool.filter((id) => {
    const r = rowOf(id);
    return r.resolver?.group !== 'system' || r.forwarder != null;
  });
  const priv = named.filter((id) => privacy.slice(0, 3).every(([, has]) => has(id))).sort((a, b) => tracks(b) - tracks(a) || ownFirst(a, b));
  if (priv[0]) {
    const held = (id: string) => privacy.map(([, has]) => has(id));
    const count = (id: string) => held(id).filter(Boolean).length;
    const probed = named.filter((id) => check(run, id, 'ecs') != null).sort((a, b) => Number(b === priv[0]) - Number(a === priv[0]) || count(b) - count(a));
    picks.push({
      goal: GOAL.private,
      resolverId: priv[0],
      name: nameOfRow(rowOf(priv[0])),
      figure: 'no client subnet',
      note: `${yoursShort(priv[0])}QNAME minimisation, DNSSEC${tracks(priv[0]) ? ', blocks trackers' : ''}`,
      evidence: `${yours(priv[0])}no client subnet sent, QNAME minimisation on, validates DNSSEC${tracks(priv[0]) ? ', blocks trackers' : ''}`,
      verified: true,
      graph: {
        kind: 'checks',
        rows: privacy.map(([l]) => l),
        cols: probed.map((id) => ({ name: nameOfRow(rowOf(id)), on: held(id), win: id === priv[0] })),
      },
    });
  }

  const steady = [...per.values()].sort((a, b) => b.success - a.success || (spread(a) ?? Infinity) - (spread(b) ?? Infinity));
  if (steady[0]) {
    picks.push({
      goal: GOAL.steadiest,
      resolverId: steady[0].target.resolver_id,
      name: label(steady[0].target.provider, steady[0].target.variant),
      figure: `±${ms(spread(steady[0]))}`,
      note: `${pct(steady[0].success)} answered`,
      evidence: `${pct(steady[0].success)} answered; answer times vary by only ${ms(spread(steady[0]))} (standard deviation)`,
      verified: true,
      graph:
        spread(steady[0]) != null
          ? {
              kind: 'spread',
              cands: steady
                .filter((r) => spread(r) != null)
                .map((r) => ({ name: nameOfRow(r), v: spread(r)!, ok: r.success })),
            }
          : undefined,
    });
  }

  // Best from here adds the speed figures, the measured cache-hit rate and the CDN penalty at the chosen mix; unlike
  // Fastest it may be a tie, which it says.
  const lookups = [...per.values()].filter((r) => r.perLookup != null).sort((a, b) => a.perLookup! - b.perLookup! || (a.composite ?? Infinity) - (b.composite ?? Infinity));
  const top = lookups[0];
  if (top) {
    const nextRow = lookups.find((r) => r.target.provider !== top.target.provider);
    const perLookupScorer: Scorer = (dist, tests, row) => perLookupOf(dist, tests, row.target.cache_hits ?? null, row.cdn, hitShare);
    const l = lead(top, top.perLookup, nextRow, nextRow?.perLookup, perLookupScorer);
    const noCdn = lookups.every((r) => r.cdnTerm == null);
    const t = perLookupTerms(top.dist, run.config.tests, top.target.cache_hits ?? null, top.cdn, hitShare)!;
    const parts: [string, string][] = [];
    if (t.hit != null) parts.push(['Cache hit', t.hit < 0.005 ? '≈0 ms' : ms(t.hit)]);
    if (t.miss != null) parts.push(['Fresh lookup', ms(t.miss)]);
    if (t.hit != null && t.miss != null) parts.push(['Mix', `${pct(hitShare, 0)} cached`]);
    if (top.target.cache_hits != null) parts.push(['Measured hit rate', pct(top.target.cache_hits, 0)]);
    if (t.failures > 0) parts.push(['Timeouts', `+${ms(t.failures)}`]);
    if (t.cdn != null) parts.push(['CDN edges', `+${ms(t.cdn)}`]);
    // The bar shares out the total: the two speed terms at the mix, then what timeouts and CDN edges add.
    const both = t.hit != null && t.miss != null;
    const terms: Extract<PickGraph, { kind: 'stack' }>['terms'] = [];
    if (t.hit != null) terms.push({ label: 'Cache hit', v: both ? t.hit * hitShare : t.hit, c: 'cached' });
    if (t.miss != null) terms.push({ label: 'Fresh lookup', v: both ? t.miss * (1 - hitShare) : t.miss, c: 'uncached' });
    if (t.failures > 0) terms.push({ label: 'Timeouts', v: t.failures, c: 'neutral' });
    if (t.cdn != null) terms.push({ label: 'CDN edges', v: t.cdn, c: 'tld' });
    picks.push({
      goal: 'Best from here',
      resolverId: top.target.resolver_id,
      name: nameOfRow(top),
      figure: ms(top.perLookup),
      note: [`${mixLabel(hitShare)} mix`, l.short, noCdn ? 'no CDN term' : ''].filter(Boolean).join(', '),
      evidence: `${ms(top.perLookup)} per lookup at the ${mixLabel(hitShare)} mix${l.text}${noCdn ? ' (no CDN term)' : ''}`,
      verified: true,
      parts,
      graph: { kind: 'stack', terms },
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
  const near = per.get(picks.find((p) => p.goal === GOAL.cdn)?.resolverId ?? '');
  if (near && near.target.resolver_id !== best.target.resolver_id && near.cdn != null && best.cdn != null && best.cdn - near.cdn >= 1 && near.composite != null && best.composite != null) {
    const slower = tie && near.target.resolver_id === runnerUp?.target.resolver_id ? '' : ` answers ${ms(Math.max(0, near.composite - best.composite))} slower but`;
    detail += ` ${nameOfRow(near)}${slower} sends you to CDN edges ${ms(best.cdn - near.cdn)} nearer, and every new connection pays that at least ${CDN_ROUND_TRIPS} times.`;
  }
  const also = picks.slice(1).filter((p) => p.resolverId === best.target.resolver_id).map((p) => p.goal[0].toLowerCase() + p.goal.slice(1));
  if (also.length) detail += ` It is also the pick for ${list(also)}.`;

  return { best, sentence, detail, picks, caveats };
}
