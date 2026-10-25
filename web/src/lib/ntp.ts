import type { NtpEntry, NtpResult } from './types';

export const MAX_CUSTOM = 4;
export const MAX_SERVERS = 8;
export const num = (v: number | null | undefined, d = 1) => (v == null ? '—' : v.toFixed(d));
export const sgn = (v: number | null | undefined, d = 2) => (v == null ? '—' : `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(d)}`);
export const idMap = (catalog: NtpEntry[]) => new Map(catalog.map((e) => [e.id, e]));
export const nameOf = (ids: Map<string, NtpEntry>, id: string) => ids.get(id)?.name ?? id;
/** How far a server's offset can be from UTC: its root distance, or the path bound alone for a row saved without one. */
export const reachMs = (r: NtpResult) => r.distance_ms ?? r.bound_ms;
/** Addresses of a server that answered. */
export const addrs = (r: NtpResult) => r.addrs.filter((a) => a.ok > 0).length;
