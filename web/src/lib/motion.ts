// Motion shared across views. Motion conveys state only, and every animation collapses under reduced motion.

import { cubicInOut, quintOut } from 'svelte/easing';
import type { TransitionConfig } from 'svelte/transition';
import type { Sample } from './types';

export const reduceMotion = typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/** Memo for the keys of a ranked list: the same array until the order really changes. An `animate:flip` each block
 *  over it reconciles (and re-measures every row) only on a re-rank, not on every frame of the live glide, which would
 *  restart each swap mid-flight. */
export function orderOf() {
  let last: number[] = [];
  return (keys: number[]) => (keys.length === last.length && keys.every((k, i) => k === last[i]) ? last : (last = keys));
}

/** How a re-ranked row slides to its new place (`animate:flip`): longer the farther it goes, easing in and out so the
 *  whole move is visible rather than spent in its first tenth. */
export const RERANK = { duration: (d: number) => (reduceMotion ? 0 : Math.min(1100, 650 + d * 0.8)), easing: cubicInOut };

/** The least time between two re-ranks of a live list (`pacedOrder`): longer than the longest slide, so one finishes
 *  before the next begins. */
export const RERANK_GAP = 1800;

/** The latency axis easing to new bounds while a run is live (the `Tween` options; log10 space). */
export const ZOOM = { duration: reduceMotion ? 0 : 600, easing: quintOut };

/** A live chart's newest stretch drawn in over about one update's time, at an even pace (the `Tween` options; slots). */
export const REVEAL = { duration: reduceMotion ? 0 : 3000 };

/** A gridline or axis label entering or leaving as the axis zooms. */
export const TICK_FADE = reduceMotion ? 0 : 200;

/** A row entering the ranked list, once its target has measured answers (`rise`). */
const ENTER = { duration: 400, y: 8, easing: quintOut };

/** A rank numeral rolling in after its row moved (`roll`), and the climbed row's tint fading back (CSS, 1.2 s). */
const ROLL = { duration: 280, easing: quintOut };

/** Fade and rise a row in. `on` is false for a finished run and on first render, so those don't animate. */
export function rise(_: Element, { on }: { on: boolean }): TransitionConfig {
  return {
    duration: on && !reduceMotion ? ENTER.duration : 0,
    easing: ENTER.easing,
    css: (t, u) => `opacity: ${t}; transform: translateY(${u * ENTER.y}px)`,
  };
}

/** A live chart or key arriving mid-run (`unfold`): opens its own height so what's below glides down, then fades in. */
const UNFOLD = 700;

/** Open a block in place: the row grows from nothing as the content fades up. The node needs `display: grid` and a
 *  single child with `min-height: 0` (the `.unfold` class), so content of any height opens smoothly. */
export function unfold(_: Element, { on }: { on: boolean }): TransitionConfig {
  return {
    duration: on && !reduceMotion ? UNFOLD : 0,
    easing: cubicInOut,
    css: (t) => `grid-template-rows: ${t}fr; opacity: ${Math.max(0, (t - 0.4) / 0.6)}; overflow: hidden`,
  };
}

/** The filled vessel leaving as the first repeat chart takes its place: lifted out of the flow so the chart opens
 *  beneath it, then faded, uncovering the chart's first frame, which looks the same. */
export function lift(node: HTMLElement, { on }: { on: boolean }): TransitionConfig {
  if (!on || reduceMotion) return { duration: 0 };
  Object.assign(node.style, { position: 'absolute', top: '0', left: '0', right: '0', zIndex: '1' });
  return { duration: 320, css: (t) => `opacity: ${t}` };
}

/** The vessel's liquid pouring into the warm-up sliver, in ms: a hold while the vessel fades, the pour itself, the chart
 *  growing to its own height through it, and the first lines drawn on from the sliver as the liquid lands. `arrive` is
 *  the share of the pour spent getting there; the rest is the slosh settling. */
export const POUR = { hold: 300, ms: 2300, arrive: 0.52, growAt: 0.11, grow: 1100, draw: 900 };
/** Swings of the slosh as it settles. */
const SWINGS = 1.5;
/** How much shallower the liquid lies while it is spread out on its way. */
const THIN = 0.25;
/** How far what hangs in the liquid swings sideways, as a share of the surface's lean. */
const SWAY = 0.5;

const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
/** 0 before `a`, 1 after `b`, eased between. */
export const ramp = (t: number, a: number, b: number) => {
  const u = clamp01((t - a) / (b - a));
  return u * u * (3 - 2 * u);
};

export interface PourFrame {
  /** The liquid: floor, sloshing surface, and walls. */
  d: string;
  /** Its surface alone, for the waterline. */
  surface: string;
  /** The vessel's rounded corners, closing to square as it narrows. */
  clip: { x: number; w: number; rx: number };
  /** The vessel's border, becoming the plot's as it fades. */
  frame: { x: number; y: number; w: number; h: number; rx: number; op: number };
  /** The warm-up's dashed edge, rising behind the liquid as it comes in. */
  wall: { x: number; y: number; op: number };
  /** Where something carried in the liquid is: `u` across it from the axis and `d` up it from the floor, both 0 to 1. */
  at: (u: number, d: number) => [number, number];
}

/** The pour at `t` (0 to 1). The vessel becomes the plot: its lid comes down to `top`, its left wall in to `left` and its
 *  right edge to `end`. Its liquid runs to the axis in one motion: the surface draws down from the far wall, leaving a
 *  level stretch that ends at a shoulder and a hollow slope from there to a foot on the floor. The foot runs in, slowly
 *  at first, to the sliver between `left` and `right`, and as it closes the shoulder moves out to meet it, so the tail
 *  feeds the sliver and its face stands up at the sliver's edge. Then it sloshes, each swing smaller, until it lies
 *  level at `top`. `w` is the vessel's width, `floor` the shared bottom, `level` the liquid's height above it at first. */
export function pourAt(
  t: number,
  g: { w: number; floor: number; level: number; left: number; right: number; top: number; end: number },
): PourFrame {
  const sp = clamp01(t / POUR.arrive);
  const e = cubicInOut(sp);
  const s = clamp01((t - POUR.arrive) / (1 - POUR.arrive));
  const L = g.left * e;
  const fy = 0.5 + (g.top - 0.5) * e;
  const X = g.w + (g.right - g.w) * ramp(sp ** 1.3, 0, 1);
  // The shoulder, as a share of the way from the axis to the foot: at the foot while the liquid is a block.
  const sh = 1 - ramp(sp, 0, 0.35) + ramp(sp, 0.6, 1);
  const land = ramp(sp, 0.75, 1);
  const hb = (g.level + (g.floor - g.top - g.level) * e) * (1 - THIN * ramp(sp, 0, 0.4) * (1 - ramp(sp, 0.55, 1)));
  // It lands leaning on the axis, which is where the slosh starts, so the surface never jerks.
  const A = Math.min(20, 0.22 * Math.max(0, g.right - g.left));
  const lean = t < POUR.arrive ? A * Math.sin((Math.PI / 2) * sp) ** 2 : A * (1 - s) ** 2 * (1 + 2 * s);
  const a = lean * Math.cos(2 * Math.PI * SWINGS * s);
  // A full vessel's liquid stays under the lid, so while it leans it sits low enough for its high side to meet it.
  const level = Math.min(hb, g.floor - fy - lean);
  const height = (u: number) => {
    const v = sh < 1 ? clamp01((u - sh) / (1 - sh)) : 0;
    return Math.max(0, (level + a * Math.cos(Math.PI * clamp01(u))) * (1 - v ** 1.5) ** 2.5);
  };
  // What hangs in the liquid swings with it: toward the high side as it leans, most in the middle and near the surface.
  const at = (u: number, d: number): [number, number] => [
    L + (X - L) * u - SWAY * a * d * Math.sin(Math.PI * clamp01(u)),
    g.floor - height(u) * d,
  ];
  // Points along the level stretch, then along the slope, however short either is.
  const us = Array.from({ length: 21 }, (_, k) => (sh * k) / 20);
  if (sh < 1) for (let k = 1; k <= 32; k++) us.push(sh + ((1 - sh) * k) / 32);
  const pts = us.map((u) => `${(L + (X - L) * u).toFixed(1)} ${(g.floor - height(u)).toFixed(1)}`);
  return {
    d: `M${L.toFixed(1)} ${g.floor}L${pts.join('L')}L${X.toFixed(1)} ${g.floor}Z`,
    surface: `M${pts.join('L')}`,
    clip: { x: L, w: g.w - L, rx: 10 * (1 - e) },
    frame: {
      x: L + 0.5,
      y: fy,
      w: Math.max(0, g.w - 1 + (g.end - g.left - g.w + 1) * e),
      h: g.floor - 0.5 - fy,
      rx: 9.5 * (1 - e),
      op: 1 - ramp(sp, 0.3, 0.85),
    },
    wall: { x: g.right, y: g.floor - (g.floor - g.top) * land, op: land },
    at,
  };
}

/** The warm-up's dots: one tile, 48 by 144, of centre, radius and tint (`c` a common name's first ask, `u` a name asked once). */
export const BUBBLES: [number, number, number, '' | 'c' | 'u'][] = [
  [6, 8, 1.25, ''],
  [27, 18, 1.5, ''],
  [40, 31, 1.5, 'c'],
  [14, 42, 1.25, ''],
  [33, 55, 1.5, ''],
  [21, 67, 1.5, 'u'],
  [44, 79, 1.25, ''],
  [9, 90, 1.5, ''],
  [30, 102, 1.5, 'c'],
  [17, 114, 1.25, ''],
  [38, 126, 1.5, ''],
  [4, 136, 1.25, ''],
];

/** The dots feeding a live chart from its warm-up: how fast they drift across it (px a second), how long one takes to
 *  run along its line past the edge (ms) and how far it gets (px), the shortest run over which one bends onto its line,
 *  and about how far apart they follow each other. */
export const FEED = { v: 4, out: 1800, reach: 36, bend: 30, gap: 44 };

export interface FeedDot {
  /** Where it rests, and where it is as the flow starts. */
  x: number;
  y: number;
  r: number;
  tint: '' | 'c' | 'u';
  /** How far into its round it starts, 0 to 1. */
  p: number;
}

/** The warm-up's dots laid out for the flow, between `x0` and `x1` from `top` down `h`. Each keeps to its own height:
 *  it comes in at the axis, drifts to the warm-up's edge in `travel` ms, leaves into the plot and starts again, a
 *  round of `total` ms. They start spread along the way, none of them mid-leave. */
export function feedDots(x0: number, x1: number, top: number, h: number): { dots: FeedDot[]; from: number; travel: number; total: number } {
  const from = x0 - 4;
  const travel = ((x1 - from) / FEED.v) * 1000;
  const total = travel + FEED.out;
  const n = Math.max(1, Math.round((x1 - from) / FEED.gap));
  const dots: FeedDot[] = [];
  for (let row = 0; row * 144 < h; row++)
    for (const [cx, cy, r, tint] of BUBBLES) {
      const y = row * 144 + cy;
      if (y > h - 3) continue;
      for (let j = 0; j < n; j++) {
        const p = (cx / 48) * (travel / total - (n - 1) / n) + j / n;
        dots.push({ x: from + (p * total * FEED.v) / 1000, y: top + y, r, tint, p });
      }
    }
  return { dots, from, travel, total };
}

/** How far a feeding dot at `x` has left its row for a line `dy` away: nothing until its bend, an eased curve through
 *  it, all of `dy` at the warm-up's edge. A line further off is bent toward from further back, as far as `from`. */
export function feedDip(x: number, from: number, edge: number, dy: number): number {
  const bend = Math.min(edge - from, Math.max(FEED.bend, 1.6 * Math.abs(dy)));
  const u = clamp01((x - (edge - bend)) / bend);
  return dy * u * u * (3 - 2 * u);
}

/** One feeding dot's round as steps (`at` is the share of the round): level from `from`, bending onto its line by the
 *  warm-up's `edge`, then along the line's points past it (`along`, each an x and how far it is from the dot's row),
 *  picking up speed from its drift and fading. It stops where the line does. */
export function feedPath(
  f: { from: number; travel: number; total: number },
  edge: number,
  dy: number,
  along: [number, number][],
): { at: number; x: number; y: number; o: number }[] {
  const steps = 10;
  const share = f.travel / f.total;
  const bend = Math.min(edge - f.from, Math.max(FEED.bend, 1.6 * Math.abs(dy)));
  const pts = bend < edge - f.from ? [{ at: 0, x: f.from, y: 0, o: 1 }] : [];
  for (let k = 0; k <= steps; k++) {
    const x = edge - bend * (1 - k / steps);
    pts.push({ at: ((x - f.from) / (edge - f.from)) * share, x, y: feedDip(x, f.from, edge, dy), o: 1 });
  }
  // How far along the line after `t` of the run: off at the drift's speed, gathering to cover the reach.
  const lead = (FEED.v * FEED.out) / 1000;
  const gain = FEED.reach - lead;
  const when = (run: number) => Math.min(1, (-lead + Math.sqrt(lead * lead + 4 * gain * run)) / (2 * gain));
  let [px, py] = [edge, dy];
  let run = 0;
  for (const [x, y] of along) {
    const seg = Math.hypot(x - px, y - py);
    if (seg === 0) continue;
    const cut = Math.min(1, (FEED.reach - run) / seg);
    run += seg * cut;
    [px, py] = [px + (x - px) * cut, py + (y - py) * cut];
    const t = when(run);
    pts.push({ at: share + (1 - share) * t, x: px, y: py, o: 1 - t });
    if (cut < 1) break;
  }
  const last = pts[pts.length - 1];
  if (last.at < 1 - 1e-9) pts.push({ at: 1, x: last.x, y: last.y, o: 0 });
  else Object.assign(last, { at: 1, o: 0 });
  return pts;
}

/** A dot's own slow wander about its place, so it hangs in the liquid instead of riding a rail: a closed lap of offsets
 *  (px), how long the lap takes (ms) and how far into it the dot starts (0 to 1). Each dot's reach, pace and shape are
 *  its own. Its sideways speed stays under `FEED.v`, so a drifting dot lingers and surges but never backs up. */
export function wander(i: number): { path: [number, number][]; ms: number; p: number } {
  const f = (k: number) => ((i + 1) * k) % 1;
  const ax = 2 + 1.5 * f(0.618034);
  const ay = 1.5 + 1.5 * f(0.754878);
  const turn = 2 * Math.PI * f(0.56984);
  const path = Array.from({ length: 25 }, (_, k): [number, number] => {
    const th = (Math.PI * k) / 12;
    return [ax * Math.sin(th), ay * Math.sin(2 * th + turn)];
  });
  return { path, ms: 8000 + 6000 * f(0.341), p: f(0.4301) };
}

/** Slide a rank numeral in from below when its row climbed (`dir` 1) or from above when it dropped (-1). */
export function roll(_: Element, { dir }: { dir: number }): TransitionConfig {
  return {
    duration: dir && !reduceMotion ? ROLL.duration : 0,
    easing: ROLL.easing,
    css: (t, u) => `opacity: ${t}; transform: translateY(${u * dir * 100}%)`,
  };
}

/** Rank changes during a live run: remembers each target's previous rank and last move (kept while the rank holds, so
 *  the cue isn't cut short by a later refresh). Call it with the ranked rows; it returns climbed (1) or dropped (-1)
 *  for the eligible rows that moved to their current rank. Plain maps: bookkeeping only. */
export function rankTracker() {
  const lastRank = new Map<number, number>();
  const lastMove = new Map<number, { rank: number; dir: 1 | -1 }>();
  return (rows: { target: { idx: number }; eligible: boolean }[], live: boolean) => {
    const out = new Map<number, 1 | -1>();
    rows.forEach((r, i) => {
      const idx = r.target.idx;
      if (!r.eligible) {
        lastRank.delete(idx);
        return;
      }
      const prev = lastRank.get(idx);
      lastRank.set(idx, i + 1);
      if (prev != null && prev !== i + 1) lastMove.set(idx, { rank: i + 1, dir: i + 1 < prev ? 1 : -1 });
      const mv = lastMove.get(idx);
      if (live && mv?.rank === i + 1) out.set(idx, mv.dir);
    });
    return out;
  };
}

/** A stable pseudo-random fraction in 0..1 from a sine hash of `x`: the same input always lands in the same place. */
export function sinFrac(x: number, mul = 43758.5453) {
  const h = Math.sin(x) * mul;
  return h - Math.floor(h);
}

/** A dot's stable vertical offset in -1..1, the same in the Latency view and the ranked list. */
export function jitterOf(s: Sample) {
  return sinFrac((s.t + 1) * 12.9898 + (s.i + 1) * 78.233 + (s.k === 'cached' ? 1 : s.k === 'uncached' ? 2 : 3) * 37.719) * 2 - 1;
}
