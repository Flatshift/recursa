<script lang="ts">
  import type { Resolver, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { shortcut } from '../lib/keys';
  import { label } from '../lib/format';
  import { status } from '../lib/status.svelte';
  import Icon from './Icon.svelte';
  import ResolverDetail from './ResolverDetail.svelte';
  import { app } from '../lib/store.svelte';
  import { mobile } from '../lib/api';

  let {
    resolver,
    run = null,
    rows = [],
    onclose,
  }: { resolver: Resolver; run?: RunState | null; rows?: Row[]; onclose: () => void } = $props();

  // The inspector follows the selection; it loads the status feed it shows.
  $effect(() => {
    status.load();
  });

  // Phone: a sheet over the run. A downward drag on its header follows the finger and closes it past a third of
  // its height or on a quick flick; anything less springs back.
  let sheet = $state<HTMLElement>();
  let pull = $state<number | null>(null);
  let pullFrom: { y: number; t: number } | null = null;
  function pullStart(e: TouchEvent) {
    e.stopPropagation();
    if (e.touches.length === 1) pullFrom = { y: e.touches[0].clientY, t: e.timeStamp };
  }
  function pullMove(e: TouchEvent) {
    e.stopPropagation();
    if (!pullFrom) return;
    pull = Math.max(0, e.touches[0].clientY - pullFrom.y);
  }
  function pullEnd(e: TouchEvent) {
    e.stopPropagation();
    if (!pullFrom) return;
    const dy = pull ?? 0;
    const fast = dy > 40 && dy / Math.max(1, e.timeStamp - pullFrom.t) > 0.5;
    pullFrom = null;
    pull = null;
    if (fast || dy > (sheet?.offsetHeight ?? 600) / 3) onclose();
  }


  const id = $derived(resolver.id);
  const open = () => app.navigate(`/resolver/${id}`);
</script>

{#if mobile}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="sheet-scrim" onclick={onclose}></div>
{/if}
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<aside
  class="inspector"
  class:pulling={pull !== null}
  style:transform={pull ? `translateY(${pull}px)` : undefined}
  aria-labelledby="sheet-title"
  bind:this={sheet}
  ontouchstart={(e) => e.stopPropagation()}
>
  <div class="inner scroll">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header class="head" ontouchstart={mobile ? pullStart : undefined} ontouchmove={mobile ? pullMove : undefined} ontouchend={mobile ? pullEnd : undefined} ontouchcancel={mobile ? pullEnd : undefined}>
      {#if mobile}<span class="grabber" aria-hidden="true"></span>{/if}
      <div>
        <p class="kicker">
          {resolver.group === 'system' ? 'Your network' : resolver.group === 'custom' ? 'Custom resolver' : resolver.group === 'legacy' ? 'Legacy resolver' : 'Public resolver'}{resolver.published.jurisdiction ? ` · ${resolver.published.jurisdiction}` : ''}
        </p>
        <h2 id="sheet-title">{label(resolver.provider, resolver.variant)}</h2>
        <p class="addrs mono">
          {[...resolver.ipv4, ...resolver.ipv6].join(' · ')}
        </p>
        {#if resolver.dot || resolver.doh}
          <p class="addrs mono">
            {#if resolver.dot}DoT {resolver.dot}{/if}{#if resolver.dot && resolver.doh}&nbsp;·&nbsp;{/if}{#if resolver.doh}DoH {resolver.doh.replace('https://', '')}{/if}
          </p>
        {/if}
      </div>
      <div class="acts">
        <button class="tb-icon close" aria-label="Open in page" title="Open in page ({shortcut('cmd+Enter')})" onclick={open}><Icon name="external" size={14} /></button>
        {#if mobile}
          <button class="done" onclick={onclose}>Done</button>
        {:else}
          <button class="tb-icon close" aria-label="Close inspector" title="Close inspector ({shortcut('alt+cmd+I')})" onclick={onclose}><Icon name="close" size={14} /></button>
        {/if}
      </div>
    </header>

    <ResolverDetail {resolver} {run} {rows} runAllButton />
  </div>
</aside>

<style>
  /* Beside the shortlist it takes the rest of the window; the text stays within a readable measure. */
  .inspector {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    height: 100%;
    background: var(--surface);
    color: var(--ink);
    display: grid;
  }
  /* Phone: a sheet that rises over the run and leaves its top edge in view, with its latency first; the operator's
     notes and the deep dive follow the measurements. */
  :global(html.mobile) .inspector {
    position: fixed;
    inset: calc(env(safe-area-inset-top) + 10px) 0 0 0;
    z-index: var(--z-sheet);
    max-width: none;
    height: auto;
    border-left: 0;
    border-radius: 14px 14px 0 0;
    box-shadow: var(--shadow-sheet);
    animation: rise 320ms var(--ease-out);
    transition: transform 260ms var(--ease-out);
  }
  :global(html.mobile) .inspector.pulling {
    transition: none;
  }
  .sheet-scrim {
    position: fixed;
    inset: 0;
    z-index: var(--z-backdrop);
    background: var(--backdrop);
    animation: fadein 320ms var(--ease-out);
  }
  @keyframes rise {
    from {
      transform: translateY(100%);
    }
  }
  @keyframes fadein {
    from {
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    :global(html.mobile) .inspector {
      animation: fadein 160ms linear;
      transition: none;
    }
  }
  :global(html.mobile) .inner {
    display: flex;
    flex-direction: column;
    padding: 0 16px calc(32px + env(safe-area-inset-bottom));
    overscroll-behavior: contain;
  }
  :global(html.mobile) .inner > :global(*) {
    order: 3;
  }
  :global(html.mobile) .inner > .head {
    order: 0;
    position: sticky;
    padding: 22px 0 12px;
    border-radius: 14px 14px 0 0;
    touch-action: none;
  }
  .grabber {
    position: absolute;
    top: 7px;
    left: 50%;
    width: 36px;
    height: 5px;
    margin-left: -18px;
    border-radius: 3px;
    background: var(--rule-2);
  }
  .done {
    align-self: flex-start;
    min-height: 44px;
    margin: -10px -8px 0 0;
    padding: 0 8px;
    border: 0;
    background: none;
    color: var(--ink);
    font-size: var(--t-base);
    font-weight: 620;
  }
  .inner {
    padding: 0 28px 40px;
    height: 100%;
    max-width: 816px;
  }
  .head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    position: sticky;
    top: 0;
    padding: 16px 0 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--rule);
    z-index: 1;
  }
  .kicker {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  h2 {
    font-size: var(--t-lg);
    font-weight: 680;
  }
  .addrs {
    font-size: var(--t-xs);
    color: var(--ink-3);
    margin-top: 2px;
    overflow-wrap: anywhere;
  }
  .close {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 26px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--ink-2);
  }
  .close:hover {
    background: var(--surface-2);
    color: var(--ink);
  }
  .acts {
    display: flex;
    gap: 2px;
    flex: none;
  }
  @media (max-width: 560px) {
    .inner {
      padding: 18px 16px 40px;
    }
  }
</style>
