import { describe, expect, it } from 'vitest';
import { FEED, POUR, feedDip, feedDots, feedPath, jitterOf, pourAt, sinFrac, wander } from './motion';
import type { Sample, TestKind } from './types';
import { warmJitter } from './warm';

const sample = (t: number, i: number, k: TestKind): Sample => ({ t, i, k, us: 1000, o: 'ok', at: 0, h: 0 });

// Pinned from the hash as it was written out in each view, so the placement of dots, rings and ghosts stays put.
describe('sinFrac', () => {
  it('is the fraction of a scaled sine', () => {
    expect(sinFrac(12.9898)).toBe(0.9216903898159217);
    expect(sinFrac(77.9388)).toBe(0.626086772987037);
    expect(sinFrac(299.648)).toBe(0.9064690540690208);
    expect(sinFrac(586.3062)).toBe(0.41706695510220015);
  });
  it('takes the multiplier', () => {
    expect(sinFrac(406.26, 24634.6345)).toBe(0.5487084472333663);
    expect(sinFrac(816.34, 24634.6345)).toBe(0.576368986048692);
    expect(sinFrac(2186.91, 24634.6345)).toBe(0.022308664070806117);
  });
});

describe('jitterOf', () => {
  it('places a dot by its target, index and test', () => {
    expect(jitterOf(sample(0, 0, 'cached'))).toBe(-0.12463258554089407);
    expect(jitterOf(sample(3, 7, 'uncached'))).toBe(-0.6182320048756083);
    expect(jitterOf(sample(12, 41, 'tld'))).toBe(0.6905934405367589);
  });
});

describe('warmJitter', () => {
  it('places a warm-up ring by its target, test and index', () => {
    expect(warmJitter(0, 'cached', 0)).toBe(-0.9547647957151639);
    expect(warmJitter(3, 'uncached', 2)).toBe(0.5055564944486832);
    expect(warmJitter(12, 'tld', 9)).toBe(-0.47224733195616864);
  });
});

describe('pourAt', () => {
  const g = { w: 800, floor: 180, level: 180, left: 44, right: 120, top: 30, end: 790 };
  const ys = (t: number) => pourAt(t, g).surface.slice(1).split('L').map((p) => Number(p.split(' ')[1]));
  it('starts as the full vessel', () => {
    const f = pourAt(0, g);
    expect(f.d.startsWith('M0.0 180L0.0 0.5')).toBe(true);
    expect(f.d.endsWith('L800.0 180Z')).toBe(true);
    expect(f.clip).toEqual({ x: 0, w: 800, rx: 10 });
    expect(f.frame).toMatchObject({ w: 799, op: 1 });
    expect(f.wall).toMatchObject({ x: 120, y: 180, op: 0 });
  });
  it('tips to a slope whose foot is on the floor while it runs', () => {
    const y = ys(0.25);
    expect(y.at(-1)).toBe(180);
    expect(y[0]).toBeLessThan(60);
    expect(y.every((v, i) => i === 0 || v >= y[i - 1])).toBe(true);
  });
  it('ends level in the sliver, inside the plot', () => {
    const f = pourAt(1, g);
    expect(ys(1).every((y) => y === 30)).toBe(true);
    expect(f.d.startsWith('M44.0 180L44.0 30.0')).toBe(true);
    expect(f.d.endsWith('L120.0 180Z')).toBe(true);
    expect(f.frame).toMatchObject({ x: 44.5, w: 746, op: 0 });
    expect(f.wall).toMatchObject({ x: 120, y: 30, op: 1 });
  });
  it('sloshes without a jump where the liquid lands', () => {
    // The shoulder, where the level surface ends: it reaches the far side as the liquid lands.
    const y = (t: number) => ys(t)[20];
    const at = POUR.arrive;
    expect(Math.abs(y(at - 0.0001) - y(at + 0.0001))).toBeLessThan(0.5);
    // Released from rest: the same speed on both sides of the handover.
    expect(Math.abs(y(at - 0.002) - y(at) - (y(at) - y(at + 0.002)))).toBeLessThan(0.5);
    const spread = (t: number) => Math.max(...ys(t)) - Math.min(...ys(t));
    expect(spread(at + 0.1)).toBeGreaterThan(3);
    expect(spread(0.99)).toBeLessThan(0.5);
  });
  it('carries a point with the liquid, from the vessel to its place in the sliver', () => {
    expect(pourAt(0, g).at(0.5, 0.5)).toEqual([400, 90.25]);
    expect(pourAt(1, g).at(0.5, 0.5)).toEqual([82, 105]);
    expect(pourAt(1, g).at(0, 1)).toEqual([44, 30]);
    // Never under the floor, never above the surface.
    for (const t of [0.1, 0.3, 0.5, 0.7, 0.9]) {
      const f = pourAt(t, g);
      const [, top] = f.at(0.4, 1);
      const [, mid] = f.at(0.4, 0.5);
      expect(mid).toBeGreaterThanOrEqual(top);
      expect(mid).toBeLessThanOrEqual(180);
    }
  });
  it('swings what it carries toward the high side as it leans, and never through a wall', () => {
    // Landed and leaning on the axis: the middle of the surface has moved toward it, the floor has not.
    const f = pourAt(POUR.arrive, g);
    expect(f.at(0.5, 1)[0]).toBeLessThan(80);
    expect(f.at(0.5, 0)[0]).toBe(82);
    for (const t of [0.2, 0.4, POUR.arrive, 0.6, 0.7, 0.8, 0.9])
      for (const u of [0, 0.02, 0.1, 0.5, 0.9, 0.98, 1]) {
        const p = pourAt(t, g);
        const [x] = p.at(u, 1);
        expect(x).toBeGreaterThanOrEqual(p.at(0, 1)[0]);
        expect(x).toBeLessThanOrEqual(p.at(1, 1)[0]);
      }
  });
});

describe('wander', () => {
  it('is a closed lap, small and slower sideways than the drift', () => {
    for (let i = 0; i < 40; i++) {
      const w = wander(i);
      expect(w.path.at(-1)![0]).toBeCloseTo(w.path[0][0], 9);
      expect(w.path.at(-1)![1]).toBeCloseTo(w.path[0][1], 9);
      expect(w.path.every(([x, y]) => Math.abs(x) <= 3.5 && Math.abs(y) <= 3)).toBe(true);
      const step = w.ms / 24 / 1000;
      const fastest = Math.max(...w.path.slice(1).map(([x], k) => Math.abs(x - w.path[k][0]) / step));
      expect(fastest).toBeLessThan(FEED.v);
      expect(w.p).toBeGreaterThanOrEqual(0);
      expect(w.p).toBeLessThan(1);
    }
  });
  it('gives each dot its own lap', () => {
    expect(wander(0)).not.toEqual(wander(1));
    expect(wander(3).ms).not.toBe(wander(4).ms);
  });
});

describe('feedDots', () => {
  const f = feedDots(44, 154, 30, 150);
  it('rounds from behind the axis to the edge, then out', () => {
    expect(f.from).toBe(40);
    expect(f.travel).toBe(((154 - 40) / FEED.v) * 1000);
    expect(f.total).toBe(f.travel + FEED.out);
  });
  it('starts every dot inside the warm-up, where its round puts it', () => {
    expect(f.dots.length).toBeGreaterThan(20);
    for (const d of f.dots) {
      expect(d.x).toBeGreaterThanOrEqual(40);
      expect(d.x).toBeLessThan(154);
      expect(d.y).toBeGreaterThan(30);
      expect(d.y).toBeLessThan(180);
      expect(d.x).toBeCloseTo(40 + (d.p * f.total * FEED.v) / 1000, 6);
      expect(d.p * f.total).toBeLessThan(f.travel);
    }
  });
  it('keeps one dot in a sliver too narrow for a row', () => {
    const n = feedDots(44, 47, 30, 150);
    expect(n.dots.every((d) => d.x >= 40 && d.x < 47)).toBe(true);
    expect(new Set(n.dots.map((d) => d.y)).size).toBe(n.dots.length);
  });
});

describe('feedDip', () => {
  it('leaves a dot level until its bend and on its line at the edge', () => {
    expect(feedDip(60, 40, 154, 20)).toBe(0);
    expect(feedDip(154 - 32, 40, 154, 20)).toBe(0);
    expect(feedDip(154 - 16, 40, 154, 20)).toBe(10);
    expect(feedDip(154, 40, 154, 20)).toBe(20);
    expect(feedDip(154, 40, 154, -35)).toBe(-35);
  });
  it('bends toward a far line from further back, as far as the axis', () => {
    expect(feedDip(154 - 60, 40, 154, 50)).toBeGreaterThan(0);
    expect(feedDip(154 - 60, 40, 154, 10)).toBe(0);
    expect(feedDip(40, 40, 154, 140)).toBe(0);
    expect(feedDip(41, 40, 154, 140)).toBeGreaterThan(0);
  });
});

describe('feedPath', () => {
  const f = feedDots(44, 154, 30, 150);
  const share = f.travel / f.total;
  it('runs level, bends onto the line by the edge, then fades along it', () => {
    const p = feedPath(f, 154, 20, [[157, 20], [163, 14], [169, 14], [400, 14]]);
    expect(p[0]).toEqual({ at: 0, x: 40, y: 0, o: 1 });
    expect(p.every((s, k) => k === 0 || s.at > p[k - 1].at)).toBe(true);
    expect(p.every((s, k) => k === 0 || s.x > p[k - 1].x)).toBe(true);
    const edge = p[11];
    expect(edge).toMatchObject({ x: 154, y: 20, o: 1 });
    expect(edge.at).toBeCloseTo(share, 9);
    // On the line's own points, then as far as it reaches along the last stretch.
    expect(p.slice(12, 15).map((s) => [s.x, s.y])).toEqual([[157, 20], [163, 14], [169, 14]]);
    const end = p.at(-1)!;
    expect(end).toMatchObject({ at: 1, y: 14, o: 0 });
    expect(end.x).toBeCloseTo(169 + FEED.reach - 3 - Math.hypot(6, 6) - 6, 6);
  });
  it('drifts at an even speed up to the edge and leaves it no faster', () => {
    const p = feedPath(f, 154, 20, [[400, 20]]);
    for (const s of p.slice(0, 12)) expect(s.at * f.total).toBeCloseTo(((s.x - f.from) / FEED.v) * 1000, 6);
    const near = feedPath(f, 154, 20, [[154.1, 20], [400, 20]])[12];
    expect(0.1 / (((near.at - share) * f.total) / 1000)).toBeCloseTo(FEED.v, 0);
  });
  it('stops where the line does, fading there', () => {
    const p = feedPath(f, 154, 20, [[157, 20]]);
    expect(p.at(-2)).toMatchObject({ x: 157, y: 20 });
    expect(p.at(-2)!.at).toBeLessThan(1);
    expect(p.at(-1)).toEqual({ at: 1, x: 157, y: 20, o: 0 });
    expect(feedPath(f, 154, 0, []).at(-1)).toEqual({ at: 1, x: 154, y: 0, o: 0 });
  });
  it('starts its bend at the axis when the line is far off', () => {
    const p = feedPath(f, 154, 140, []);
    expect(p[0]).toEqual({ at: 0, x: 40, y: 0, o: 1 });
    expect(p[1].at).toBeGreaterThan(0);
  });
});
