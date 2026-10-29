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

export interface Cloud {
  cdn: string;
  name: string;
  /** The content groups the CDN's traffic lands in. */
  ids: number[];
}

/** The CDNs you reach both at an exchange and past one, over a carrier or your ISP's own link: every box of such a CDN
 *  shares one cloud, so two boxes read as one network reached two ways. */
export function clouds(run: MapRun): Cloud[] {
  const ix = new Set(run.groups.filter((g) => g.role === 'exchange').map((g) => g.id));
  // How traffic gets to a group: through an exchange, or past every exchange, following each link in. A group with no
  // link in is your home, which everything starts from.
  const ways = (id: number, stack: number[]): Set<'ix' | 'past'> => {
    if (ix.has(id)) return new Set(['ix']);
    const ins = run.edges.filter((l) => l.to === id && l.from !== id && !stack.includes(l.from));
    const out = new Set<'ix' | 'past'>(ins.length ? [] : ['past']);
    for (const l of ins) for (const w of l.kind === 'exchange' ? ['ix' as const] : ways(l.from, [...stack, id])) out.add(w);
    return out;
  };
  const cdnOf = new Map(run.traces.map((t) => [t.target.id, t.target]));
  const out: Cloud[] = [];
  for (const g of run.groups) {
    if (g.role !== 'content') continue;
    for (const t of g.targets.map((id) => cdnOf.get(id)).filter((t) => t?.cdn)) {
      const c = out.find((c) => c.cdn === t!.cdn) ?? out[out.push({ cdn: t!.cdn!, name: t!.name, ids: [] }) - 1];
      if (!c.ids.includes(g.id)) c.ids.push(g.id);
    }
  }
  return out.filter((c) => {
    const w = new Set(c.ids.flatMap((id) => [...ways(id, [])]));
    return w.has('ix') && w.has('past');
  });
}

/** Whether a link is settlement-free peering (onto or out of an exchange, or a listed peer) or paid transit (onto a
 *  carrier, or a listed transit or customer pair); null for a direct link nothing classifies, and for the link out
 *  of your home. */
export function linkWay(l: NetLink, groups: NetGroup[]): 'peer' | 'transit' | null {
  if (l.rel === 'transit' || l.rel === 'customer') return 'transit';
  if (l.kind === 'exchange' || l.rel === 'peer' || groups.some((g) => g.id === l.from && g.role === 'exchange')) return 'peer';
  if (l.kind === 'via_transit') return 'transit';
  return null;
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
