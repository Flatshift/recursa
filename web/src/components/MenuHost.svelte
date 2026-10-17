<script lang="ts">
  // The browser's stand-in for a native menu. The desktop app never renders it.
  import { menu } from '../lib/menu.svelte';
  import { mobile } from '../lib/api';

  let el: HTMLDivElement | undefined = $state();
  let pos = $state({ x: 0, y: 0 });

  $effect(() => {
    const o = menu.open;
    if (!o || !el) return;
    // Keep the menu inside the window, flipping up or left when it would overflow.
    const r = el.getBoundingClientRect();
    pos = {
      x: Math.max(4, Math.min(o.x, innerWidth - r.width - 4)),
      y: o.y + r.height > innerHeight - 4 ? Math.max(4, o.y - r.height) : o.y,
    };
    el.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
  });

  function choose(id: string) {
    const o = menu.open;
    menu.close();
    o?.pick(id);
  }

  function onKey(e: KeyboardEvent) {
    if (!menu.open) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      menu.close();
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const items = [...(el?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    items[(i + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus();
  }
</script>

<svelte:window onkeydown={onKey} onblur={() => menu.close()} />

{#if menu.open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions (Escape closes it) -->
  <div class="scrim" onclick={() => menu.close()} oncontextmenu={(e) => (e.preventDefault(), menu.close())}></div>
  <!-- On the phone it is an action sheet at the bottom; the position effect still runs, harmlessly. -->
  <div class="menu chrome" class:sheet={mobile} role="menu" bind:this={el} style:left={mobile ? undefined : `${pos.x}px`} style:top={mobile ? undefined : `${pos.y}px`}>
    {#each menu.open.items as item}
      {#if item.id === '-'}
        <hr />
      {:else}
        <button role="menuitem" disabled={item.enabled === false} onclick={() => choose(item.id)}>{item.label}</button>
      {/if}
    {/each}
    {#if mobile}
      <button class="cancel" onclick={() => menu.close()}>Cancel</button>
    {/if}
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: var(--z-dropdown);
  }
  .menu {
    position: fixed;
    z-index: var(--z-dropdown);
    min-width: 180px;
    padding: 4px;
    background: var(--surface);
    border-radius: var(--r-md);
    box-shadow: var(--shadow-pop);
    display: grid;
    font-size: var(--t-sm);
  }
  button {
    text-align: left;
    padding: 4px 10px;
    border: 0;
    border-radius: var(--r-sm);
    background: none;
    color: var(--ink);
  }
  button:hover,
  button:focus-visible {
    background: var(--accent);
    color: var(--accent-ink);
    outline: none;
  }
  button:disabled {
    color: var(--ink-3);
    background: none;
  }
  hr {
    border: 0;
    border-top: 1px solid var(--rule);
    margin: 4px 6px;
  }
  /* The phone's action sheet: above the drawer it was opened from, rows at finger size. */
  :global(html.mobile) .scrim {
    z-index: var(--z-toast);
    background: var(--backdrop);
  }
  .sheet {
    inset: auto 10px calc(10px + env(safe-area-inset-bottom));
    z-index: var(--z-toast);
    padding: 6px;
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-sheet);
    font-size: var(--t-base);
    animation: rise var(--dur) var(--ease-out);
  }
  .sheet button {
    min-height: 50px;
    padding: 0 16px;
    text-align: center;
    border-radius: var(--r-md);
  }
  .sheet button:hover {
    background: none;
    color: var(--ink);
  }
  .sheet button:active {
    background: var(--surface-2);
  }
  .sheet .cancel {
    margin-top: 6px;
    border-top: 1px solid var(--rule);
    border-radius: 0;
    font-weight: 620;
  }
  @keyframes rise {
    from {
      transform: translateY(16px);
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .sheet {
      animation: none;
    }
  }
</style>
