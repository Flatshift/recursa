// Arrow keys walk the resolvers on screen, in the order they are drawn, and move the selection the Inspector follows.
import { app } from './store.svelte';

function stepInspect(dir: 1 | -1) {
  const ids: string[] = [];
  for (const el of document.querySelectorAll<HTMLElement>(
    '[data-resolver]:not(.rrow)',
  )) {
    const id = el.dataset.resolver;
    if (id && el.getClientRects().length && !ids.includes(id)) ids.push(id);
  }
  if (!ids.length) return;
  const i = app.inspect ? ids.indexOf(app.inspect) : -1;
  app.inspect =
    i < 0
      ? ids[dir > 0 ? 0 : ids.length - 1]
      : ids[Math.min(ids.length - 1, Math.max(0, i + dir))];
}

/** ↑↓ move the selection. True when handled. */
export function selectKey(e: KeyboardEvent): boolean {
  if (
    (e.key === 'ArrowDown' || e.key === 'ArrowUp') &&
    !e.metaKey &&
    !e.ctrlKey &&
    !e.altKey
  ) {
    e.preventDefault();
    stepInspect(e.key === 'ArrowDown' ? 1 : -1);
    return true;
  }
  return false;
}
