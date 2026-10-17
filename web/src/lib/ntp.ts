import type { NtpEntry, NtpResult } from './types';

export const MAX_CUSTOM = 4;
export const num = (v: number | null | undefined, d = 1) => (v == null ? '—' : v.toFixed(d));
export const sgn = (v: number | null | undefined, d = 2) => (v == null ? '—' : `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(d)}`);
export const idMap = (catalog: NtpEntry[]) => new Map(catalog.map((e) => [e.id, e]));
export const nameOf = (ids: Map<string, NtpEntry>, id: string) => ids.get(id)?.name ?? id;
/** Addresses of a server that answered. */
export const addrs = (r: NtpResult) => r.addrs.filter((a) => a.ok > 0).length;
