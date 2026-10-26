import type { Resolver, Transport, TestKind } from './types';
import { checksOf } from './phases';

export const TEST_HINT: Record<TestKind, string> = {
  cached: 'Popular names the resolver already holds: network round trip plus front-end time.',
  uncached: 'Random names under wildcard zones: forces the resolver to ask the authoritative servers.',
  tld: 'Random names under .com, .org and .uk that cannot exist: the resolver must ask that TLD’s servers.',
};

export const TRANSPORT_LABEL: Record<Transport, string> = { udp: 'UDP', tcp: 'TCP', dot: 'DoT', doh: 'DoH' };

/** Milliseconds with precision that matches magnitude: 0.42 · 3.18 · 17.9 · 214 */
export function ms(v: number | null | undefined, unit = true): string {
  if (v == null || !Number.isFinite(v)) return '—';
  const s = v < 10 ? v.toFixed(2) : v < 100 ? v.toFixed(1) : Math.round(v).toString();
  return unit ? `${s} ms` : s;
}

export function us(v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return '—';
  return v < 100 ? `${v.toFixed(1)} µs` : `${Math.round(v)} µs`;
}

export function pct(v: number | null | undefined, digits = 1): string {
  if (v == null || !Number.isFinite(v)) return '—';
  const p = v * 100;
  if (p === 100) return '100%';
  return `${p.toFixed(p > 99 ? 2 : digits)}%`;
}

export function signedMs(v: number | null | undefined, unit = false): string {
  if (v == null || !Number.isFinite(v)) return '—';
  if (Math.abs(v) < 0.05) return unit ? '±0 ms' : '±0';
  return `${v > 0 ? '+' : '−'}${ms(Math.abs(v), unit)}`;
}

export function when(ts: number | string | null | undefined): string {
  if (ts == null) return '—';
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return '—';
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
}

export function day(ts: number | string | null | undefined): string {
  if (ts == null) return '—';
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return String(ts);
  return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export function ago(ts: number | string | null | undefined): string {
  if (ts == null) return 'never';
  const t = typeof ts === 'number' ? ts : new Date(ts).getTime();
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

export function duration(msTotal: number): string {
  const s = Math.round(msTotal / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return r ? `${m} m ${r} s` : `${m} m`;
}

/** How long a run should take: the Setup estimate, and the yardstick for time left while one runs. */
export function runEstimateMs(
  targets: number,
  resolvers: number,
  c: { tests: unknown[]; samples: number; mode: 'accurate' | 'quick'; probes: boolean; cdn: boolean; live: boolean },
): number {
  const queries = targets * (c.tests.length * c.samples + 11);
  let msTotal: number;
  if (c.mode === 'accurate') {
    // Sends are 4 ms apart across resolvers; each resolver also needs a round trip per sample.
    msTotal = Math.max(queries * 4, c.tests.length * c.samples * 38) + 4000;
  } else {
    const conc = Math.min(8, Math.max(1, targets));
    msTotal = (queries * 35) / conc + 4000;
  }
  return msTotal + Object.values(checkMs(resolvers, c)).reduce((a, b) => a + b, 0);
}

/** How long each check after the latency rounds should take, in run order: the yardstick once the checks begin. */
export function checkMs(resolvers: number, c: { probes: boolean; cdn: boolean; live: boolean }): Record<string, number> {
  const each: Record<string, number> = {
    path: 1500,
    probes: Math.ceil(resolvers / 8) * 2500 + 3000,
    cdn: 5000 + resolvers * 300,
    intel: 2000,
    // The feeds and the sample of hosts that still resolve, then each resolver's lookups, eight at a time.
    live: 15000 + Math.ceil(resolvers / 8) * 3500,
  };
  return Object.fromEntries(checksOf(c).map((id) => [id, each[id]]));
}

/** How many distinct resolvers a run's targets cover (a resolver may have several transports). */
export const resolverCount = (targets: { resolver_id: string }[]): number => new Set(targets.map((t) => t.resolver_id)).size;

/** Resolvers the catalog documents, so published claims apply. */
export const documented = (r: Resolver) => r.group === 'public' || r.group === 'legacy';

export function label(provider: string, variant: string): string {
  return variant ? `${provider} ${variant}` : provider;
}

/** A resolver's name from its id, in the given list; the id itself if the list doesn't have it. */
export function resolverName(resolvers: Resolver[], id: string): string {
  const r = resolvers.find((x) => x.id === id);
  return r ? label(r.provider, r.variant) : id;
}

/** Scores and short latencies: one decimal under 10, whole numbers above. */
export function score(v: number): string {
  return v < 10 ? v.toFixed(1) : Math.round(v).toString();
}

/** "a", "a and b", "a, b and c". */
export function list(xs: string[]): string {
  return xs.length < 3 ? xs.join(' and ') : `${xs.slice(0, -1).join(', ')} and ${xs.at(-1)}`;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

/** Strip RIPE holder prefixes like "CLOUDFLARENET - Cloudflare, Inc." → "Cloudflare, Inc." */
export function holder(h: string | null | undefined): string {
  if (!h) return '';
  const i = h.indexOf(' - ');
  return i > 0 ? h.slice(i + 3) : h;
}

/** Compact timestamp for lists: "Today 9:48 PM", "Yesterday 3:10 PM", "Oct 6, 9:48 PM". */
export function shortWhen(ts: number | null | undefined): string {
  if (ts == null) return '—';
  const d = new Date(ts);
  const time = d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  const days = Math.round((new Date().setHours(0, 0, 0, 0) - new Date(ts).setHours(0, 0, 0, 0)) / 86_400_000);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Yesterday ${time}`;
  return when(ts);
}
