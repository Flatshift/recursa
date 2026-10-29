// What the map says in words, shared by the graph, the table and the claim cards so the three never disagree.
import type { MapRun, NetEvidence, NetGroup, NetLink, NetClaim, NetTarget, Trace } from './types';

/** How a hand-off crossed, as the edge label and the table cell read it. */
export function linkLabel(l: NetLink, groups: NetGroup[]): string {
  if (l.kind === 'exchange') {
    // The link into the exchange node lands on the name already; only a hand-off drawn elsewhere needs to say which.
    if (groups.some((g) => g.id === l.to && g.role === 'exchange')) return 'at the exchange';
    const ix = groups.find((g) => g.ix_id === l.ix_id && g.ix_name);
    return ix?.ix_name ?? 'at an exchange';
  }
  if (l.rel) return l.rel === 'transit' ? 'transit' : l.rel === 'peer' ? 'peering' : 'their customer';
  return l.kind === 'direct' ? 'direct' : l.kind === 'via_transit' ? 'via transit' : 'unknown';
}

/** "October 2026" from a CAIDA file name like `20261001.as-rel2.txt.bz2`. */
export function asrelMonth(source: string): string | null {
  const m = /^(\d{4})(\d{2})\d{2}\./.exec(source);
  if (!m || +m[2] < 1 || +m[2] > 12) return null;
  return `${new Date(Date.UTC(+m[1], +m[2] - 1, 1)).toLocaleString('en-US', { month: 'long', timeZone: 'UTC' })} ${m[1]}`;
}

/** The provenance mark for a piece of evidence: seen here, read from a list, or worked out. */
export const markOf = (e: NetEvidence) => (e.kind === 'observed' ? 'verified' : e.kind === 'listed' ? 'published' : 'unknown') as 'verified' | 'published' | 'unknown';

export const evidenceLabel = (e: NetEvidence) => (e.kind === 'observed' ? 'observed' : e.kind === 'listed' ? e.body.source : 'inferred');

/** One piece of evidence as the words under its chip. */
export const evidenceText = (e: NetEvidence) => (e.kind === 'listed' ? `${e.body.text}${e.body.updated ? ` (${e.body.source}, edited ${e.body.updated})` : ` (${e.body.source})`}` : e.body);

/** The round trip to the end of a trace: the best of the last hop that answered. */
export function endMs(t: Trace): number | null {
  for (let i = t.hops.length - 1; i >= 0; i--) if (t.hops[i].best_ms != null) return t.hops[i].best_ms;
  return null;
}

/** The roll call's name for a destination: "Cloudflare (IAD)". */
export const siteName = (t: NetTarget) => (t.site ? `${t.name} (${t.site})` : t.name);

export const isV6 = (t: NetTarget) => t.family === 6;

/** Whether every one of these target ids belongs to an IPv6 target: what only IPv6 traffic crosses. */
export function v6Only(ids: string[], run: MapRun): boolean {
  // A run saved before IPv6 tracing has no target lists on its links.
  return !!ids?.length && ids.every((id) => run.traces.some((t) => t.target.id === id && isV6(t.target)));
}

export interface Dest {
  target: NetTarget;
  trace: Trace;
  /** For an IPv6 trace: whether its route is the IPv4 one's. */
  same_route: boolean | null;
  claim: NetClaim | null;
  link: NetLink | null;
  /** The group the traffic leaves your ISP from, and the one it enters. */
  near: NetGroup | null;
  far: NetGroup | null;
}

/** One entry per destination traced, with its hand-off when the map could say one. */
export function destinations(run: MapRun): Dest[] {
  return run.traces.map(({ target, trace, same_route }) => {
    const claim = run.claims.find((c) => c.target === target.id) ?? null;
    const link = claim ? (run.edges[claim.link] ?? null) : null;
    return {
      target,
      trace,
      same_route,
      claim,
      link,
      near: link ? (run.groups.find((g) => g.id === link.from) ?? null) : null,
      far: link ? (run.groups.find((g) => g.id === link.to) ?? null) : null,
    };
  });
}

/** Where the last ISP router sits: a city only when the map passed the floor check, else its address. */
export function leavesAt(near: NetGroup | null): string {
  if (!near) return '';
  if (near.city_hint) return `consistent with ${near.city_hint}`;
  const last = [...near.members].sort((a, b) => b.ttl - a.ttl)[0];
  return last?.addr ?? '';
}
