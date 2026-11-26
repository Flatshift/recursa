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

/** A share cut to `d` decimals, never rounded up: 99.896 reads 99.89, on the side of the line it is on. */
export const cut = (v: number, d: number) => (Math.floor(v * 10 ** d + 1e-9) / 10 ** d).toFixed(d);

export type AnsweredState ='good' | 'fair' | 'poor' | 'none';
export interface AnsweredCell {
  /** Start and end of the period (Unix ms, UTC boundaries; the end is exclusive). */
  start: number;
  end: number;
  sent: number;
  ok: number;
  state: AnsweredState;
}

/** How a period's answered share reads: 99.9% and up, 99% and up, below that, or no checks at all. */
export function answeredState(ok: number, sent: number): AnsweredState {
  if (sent <= 0) return 'none';
  if (ok * 1000 >= sent * 999) return 'good';
  if (ok * 100 >= sent * 99) return 'fair';
  return 'poor';
}

/**
 * One cell per UTC hour for a one-day window and per UTC day otherwise, from the period that holds the window's start
 * to the one that holds `until`: one more cell than the window has periods, the first and last of them part periods.
 */
export function answeredCells(points: { ts: number; sent: number; ok: number }[], until: number, days: number): AnsweredCell[] {
  const hourly = days <= 1;
  const unit = hourly ? 3_600_000 : 86_400_000;
  const n = (hourly ? 24 : days) + 1;
  const first = (Math.floor(until / unit) - (n - 1)) * unit;
  const cells = Array.from({ length: n }, (_, i): AnsweredCell => ({ start: first + i * unit, end: first + (i + 1) * unit, sent: 0, ok: 0, state: 'none' }));
  for (const p of points) {
    const i = Math.floor((p.ts - first) / unit);
    if (i < 0 || i >= n) continue;
    cells[i].sent += p.sent;
    cells[i].ok += p.ok;
  }
  for (const c of cells) c.state = answeredState(c.ok, c.sent);
  return cells;
}

/** Y ticks symmetric around zero: the smallest 1-2-5 step that covers `maxAbs` within three steps either side. */
export function symmetricTicks(maxAbs: number): number[] {
  const m = Math.max(maxAbs, 0.05);
  // The decade a third of the range falls in, and never finer than 0.1: one of its steps, or the next decade, covers it.
  const e = Math.max(-1, Math.floor(Math.log10(m / 3)));
  const step = [1, 2, 5, 10].map((k) => k * 10 ** e).find((k) => Math.ceil(m / k - 1e-9) <= 3) ?? 10 ** (e + 1);
  const k = Math.ceil(m / step - 1e-9);
  return Array.from({ length: 2 * k + 1 }, (_, i) => +((i - k) * step).toFixed(6));
}

/** One line on a Time chart: a name, a CSS colour, and a value (null where a check got no answer) at each time. */
export interface TimeSeries {
  name: string;
  color: string;
  dash?: string;
  pts: { ts: number; v: number | null }[];
}
