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

/** A dot's stable vertical offset in -1..1, the same in the Latency view and the ranked list. */
export function jitterOf(s: Sample) {
  const h = Math.sin((s.t + 1) * 12.9898 + (s.i + 1) * 78.233 + (s.k === 'cached' ? 1 : s.k === 'uncached' ? 2 : 3) * 37.719) * 43758.5453;
  return (h - Math.floor(h)) * 2 - 1;
}
