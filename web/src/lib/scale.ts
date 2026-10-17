// Shared log-millisecond scale for every latency chart, so axes agree across views.

const NICE = [0.1, 0.2, 0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000];
const DECADES = [0.1, 1, 10, 100, 1000, 10000];

// One tuple per domain: a chart redrawn every frame while a run is live keeps the same scale object, so its axis and
// dots aren't redrawn unless the domain really changed.
const DOMAINS = new Map<string, [number, number]>();

/** Nice bounds around observed [lo, hi] (pass Infinity/0 when nothing is observed). */
export function logDomain(lo: number, hi: number): [number, number] {
  let l = 1;
  let h = 100;
  if (Number.isFinite(lo) && hi > 0) {
    l = [...NICE].reverse().find((n) => n <= lo * 0.9) ?? NICE[0];
    h = Math.max(NICE.find((n) => n >= hi * 1.08) ?? NICE[NICE.length - 1], l * 10);
  }
  const k = `${l}:${h}`;
  let d = DOMAINS.get(k);
  if (!d) DOMAINS.set(k, (d = [l, h]));
  return d;
}

/** 1-2-5 ticks when there's room, decades only when narrow. */
export function logTicks(domain: [number, number], width: number): number[] {
  const inside = NICE.filter((t) => t >= domain[0] && t <= domain[1]);
  const perDecade = width / Math.log10(domain[1] / domain[0]);
  return perDecade < 110 ? inside.filter((t) => DECADES.includes(t)) : inside;
}

/** Map a value in ms to [x0, x1], clamped to the domain. */
export function logScale(domain: [number, number], x0: number, x1: number): (v: number) => number {
  const a = Math.log10(domain[0]);
  const span = Math.log10(domain[1]) - a;
  return (v) => {
    const t = (Math.log10(Math.max(v, domain[0])) - a) / span;
    return x0 + Math.min(1, Math.max(0, t)) * (x1 - x0);
  };
}

/** Inverse of logScale: pixel → ms. */
export function logInvert(domain: [number, number], x0: number, x1: number, px: number): number {
  const a = Math.log10(domain[0]);
  const span = Math.log10(domain[1]) - a;
  return Math.pow(10, a + ((px - x0) / (x1 - x0)) * span);
}

export function tickLabel(t: number): string {
  return t >= 1000 ? `${t / 1000}s` : String(t);
}
