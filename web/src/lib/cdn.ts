import type { CdnInfo, CdnTiming, Edge, Mapping, Reference } from './types';
import { holder } from './format';

/** The edge addresses a resolver returned for a CDN, across its hostnames. */
export const edgeIps = (m: Mapping | undefined): string[] => [...new Set(Object.values(m?.hosts ?? {}).flatMap((h) => h.ips))];

/** The edges that never answered a handshake: listed as silent and still without a time. */
export const silentEdges = (t: Pick<CdnTiming, 'best' | 'silent'> | undefined): string[] => (t?.silent ?? []).filter((ip) => t?.best?.[ip] == null);

export interface EdgeNet {
  key: string;
  name: string;
  n: number;
  you: boolean;
}

/** The networks the edges sit in, with how many edges each holds. `ranked` puts your ISP first, then the largest. */
export function edgeNets(edges: Edge[], ranked = false): EdgeNet[] {
  const m = new Map<string, EdgeNet>();
  for (const e of edges) {
    if (e.asn == null) continue;
    const key = String(e.asn);
    const x = m.get(key) ?? { key, name: holder(e.holder) || `AS${e.asn}`, n: 0, you: false };
    x.n++;
    x.you ||= e.on_net;
    m.set(key, x);
  }
  const out = [...m.values()];
  return ranked ? out.sort((a, b) => Number(b.you) - Number(a.you) || b.n - a.n) : out;
}

/** The penalty a cell shows. A CDN needs two usable hostnames for its own; an anycast control with one hostname shows that one. */
export function cellPenalty(c: CdnInfo, m: Mapping | undefined): number | null {
  if (!m) return null;
  return m.penalty_ms ?? (c.role === 'control' ? (m.worst_host?.[1] ?? null) : null);
}

/** Every usable hostname sits within the noise of the best: the cell is level with it. */
export function level(m: Mapping | undefined): boolean {
  const ok = Object.values(m?.hosts ?? {}).filter((h) => h.status === 'ok' && h.same != null);
  return ok.length > 0 && ok.every((h) => h.same);
}

export const median = (v: number[]) => {
  const s = [...v].sort((a, b) => a - b);
  return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2;
};

/** The CDN's own answer for this computer, as one figure per CDN: the median over its hostnames. */
export function youOf(c: CdnInfo): Reference | null {
  const asked = c.hosts.flatMap((h) => (h.you ? [h.you] : []));
  const timed = asked.filter((y) => y.rtt_ms != null);
  if (!timed.length) return asked.find((y) => y.error) ? { name: c.name, server: '', ips: [], rtt_ms: null, penalty_ms: null, error: asked.find((y) => y.error)!.error } : null;
  const pens = timed.flatMap((y) => (y.penalty_ms != null ? [y.penalty_ms] : []));
  return {
    name: `${timed.length} of ${c.hosts.length} hostnames`,
    server: '',
    ips: timed.flatMap((y) => y.ips),
    rtt_ms: median(timed.map((y) => y.rtt_ms!)),
    penalty_ms: pens.length ? median(pens) : null,
    error: null,
  };
}

/**
 * The client subnet each echoing authority saw, by name, or null where it answered without one. Runs from before
 * the per-authority map carry Google's view and, later, Akamai's. Empty when no authority answered.
 */
export function ecsByAuthority(
  r: { ecs: string | null; ecs_akamai?: string | null },
  p: { ecs_akamai?: string | null; ecs_by_authority?: Record<string, string | null> } | null | undefined,
): Record<string, string | null> {
  if (p?.ecs_by_authority) return p.ecs_by_authority;
  const akamai = r.ecs_akamai ?? p?.ecs_akamai;
  return { Google: r.ecs, ...(akamai !== undefined ? { Akamai: akamai } : {}) };
}
