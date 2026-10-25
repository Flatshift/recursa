import { score } from './format';

// Where a resolver's time goes: the wire (best TCP handshake to its address), what the resolver adds for an answer
// it already holds (cached median − floor), and what recursion adds (uncached median − cached median).

export interface Split {
  wire: number;
  cached: number | null;
  uncached: number | null;
  /** Cached median − wire. Negative when answers beat the floor: something nearer answered. */
  resolver: number | null;
  /** Uncached median − cached median, or − wire when no cached test ran. */
  recursion: number | null;
}

export function split(wire: number | null | undefined, cached: number | null | undefined, uncached: number | null | undefined): Split | null {
  if (wire == null || !Number.isFinite(wire)) return null;
  const c = cached ?? null;
  const u = uncached ?? null;
  const base = c ?? wire;
  return { wire, cached: c, uncached: u, resolver: c == null ? null : c - wire, recursion: u == null ? null : u - base };
}

/** One sentence under the strip. `name` is the resolver as the user knows it. */
export function sentence(name: string, s: Split): string {
  const wire = `${score(s.wire)} ms wire`;
  if (s.resolver != null && s.resolver < -0.5) {
    return `${name}'s cached answers arrived ${score(-s.resolver)} ms under the ${wire}: something nearer answered them.`;
  }
  const parts: string[] = [];
  if (s.resolver != null) parts.push(`${name} adds ${score(Math.max(0, s.resolver))} ms to a ${wire} for cached answers`);
  if (s.recursion != null) {
    parts.push(s.resolver != null ? `recursion adds ${score(Math.max(0, s.recursion))} ms more` : `${name} adds ${score(Math.max(0, s.recursion))} ms to a ${wire} for uncached answers`);
  }
  if (!parts.length) return `The wire to ${name} is ${score(s.wire)} ms.`;
  return parts.join('; ') + '.';
}
