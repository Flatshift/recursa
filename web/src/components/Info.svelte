<script lang="ts">
  // An (i) button that explains something in a popover, so explanations stay one click away
  // instead of filling the screen.
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  let { label = 'About this', children }: { label?: string; children: Snippet } = $props();

  let btn: HTMLButtonElement | undefined = $state();
  let pop: HTMLDivElement | undefined = $state();
  const id = `info-${Math.random().toString(36).slice(2, 9)}`;

  function place() {
    if (!btn || !pop) return;
    const b = btn.getBoundingClientRect();
    const w = pop.offsetWidth;
    const h = pop.offsetHeight;
    const left = Math.max(8, Math.min(b.left + b.width / 2 - w / 2, innerWidth - w - 8));
    const below = b.bottom + 6 + h < innerHeight - 8;
    pop.style.left = `${left}px`;
    pop.style.top = `${below ? b.bottom + 6 : Math.max(8, b.top - h - 6)}px`;
  }
</script>

<button bind:this={btn} class="info" type="button" aria-label={label} title={label} popovertarget={id}>
  <Icon name="info" size={14} />
</button>
<div bind:this={pop} {id} class="pop" popover="auto" ontoggle={(e) => (e as ToggleEvent).newState === 'open' && place()}>
  {@render children()}
</div>

<style>
  .info {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--ink-3);
    vertical-align: middle;
  }
  .info:hover {
    color: var(--ink);
    background: var(--surface-2);
  }
  .pop {
    position: fixed;
    inset: auto;
    margin: 0;
    width: min(380px, calc(100vw - 16px));
    padding: 12px 14px;
    border: 0;
    border-radius: var(--r-lg);
    background: var(--surface);
    color: var(--ink-2);
    box-shadow: var(--shadow-pop);
    font-size: var(--t-xs);
    line-height: 1.5;
    text-align: left;
    font-weight: 400;
  }
  .pop :global(p + p) {
    margin-top: 8px;
  }
  .pop :global(b) {
    color: var(--ink);
  }
</style>
