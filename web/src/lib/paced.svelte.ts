import { untrack } from 'svelte';
import { RERANK_GAP, orderOf, reduceMotion } from './motion';

/** A ranked list's displayed order. It follows `ids`, but while `live` re-ranks at most once per `RERANK_GAP`, so each
 *  slide finishes before the next one starts and near-tied rows don't trade places on every refresh. A change in which
 *  rows are listed applies at once, and rows that left drop out at once. The order is the same array until it really
 *  changes (`orderOf`). */
export function pacedOrder(ids: () => number[], live: () => boolean) {
  const keepWant = orderOf();
  const want = $derived(keepWant(ids()));
  let shown = $state.raw(untrack(() => want));
  let at = 0;
  const show = (next: number[]) => {
    shown = next;
    at = performance.now();
  };
  $effect.pre(() => {
    const next = want;
    const pace = live() && !reduceMotion;
    let timer: ReturnType<typeof setTimeout> | undefined;
    untrack(() => {
      if (next === shown) return;
      const same = next.length === shown.length && next.every((k) => shown.includes(k));
      const wait = at + RERANK_GAP - performance.now();
      if (!pace || !same || wait <= 0) show(next);
      else timer = setTimeout(() => show(next), wait);
    });
    return () => clearTimeout(timer);
  });
  const keepShown = orderOf();
  const current = $derived.by(() => {
    const listed = new Set(want);
    return keepShown(shown.filter((k) => listed.has(k)));
  });
  return {
    get current() {
      return current;
    },
  };
}
