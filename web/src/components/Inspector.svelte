<script lang="ts">
  // One resolver beside the list or the run: its name and addresses, then its six views as tabs. The chosen view stays
  // when the selection moves to another resolver.
  import { untrack } from 'svelte';
  import type { Resolver, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { shortcut } from '../lib/keys';
  import { label } from '../lib/format';
  import { status } from '../lib/status.svelte';
  import Icon from './Icon.svelte';
  import ResolverDetail, { RESOLVER_VIEW_LABEL, VIEW_CHECK } from './ResolverDetail.svelte';
  import { app, RESOLVER_VIEWS, type ResolverView } from '../lib/store.svelte';
  import { mobile } from '../lib/api';
  import { RUN_ALL_TITLE, allKinds, blocker, isRunning, runAll, runningAll } from '../lib/checks.svelte';

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
  const stop = $derived(blocker());
  const setView = (v: ResolverView) => (app.resolverView = v);
  // A view whose check is running carries the live dot.
  const working = (v: ResolverView) => {
    const kind = VIEW_CHECK[v];
    return !!kind && isRunning(kind, id);
  };
  // ⌥⌘1–6 pick a view on the Resolvers page; in a run they pick the run's views.
  const keyed = $derived(app.route.name === 'resolvers');

  let scroller = $state<HTMLElement>();
  $effect(() => {
    app.resolverView;
    untrack(() => scroller?.scrollTo({ top: 0 }));
  });
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
  <div class="inner scroll" bind:this={scroller}>
    <div class="bar">
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <header class="head" ontouchstart={mobile ? pullStart : undefined} ontouchmove={mobile ? pullMove : undefined} ontouchend={mobile ? pullEnd : undefined} ontouchcancel={mobile ? pullEnd : undefined}>
        {#if mobile}<span class="grabber" aria-hidden="true"></span>{/if}
        <div class="who">
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
          <button class="btn sm runall" disabled={!!stop} title={stop ?? RUN_ALL_TITLE} onclick={() => runAll(id, allKinds(resolver))}>
            <Icon name="play" size={12} />{runningAll(id) ? 'Running…' : 'Run all checks'}
          </button>
          {#if mobile}
            <button class="done" onclick={onclose}>Done</button>
          {:else}
            <button class="tb-icon close" aria-label="Close inspector" title="Close inspector ({shortcut('alt+cmd+I')})" onclick={onclose}><Icon name="close" size={14} /></button>
          {/if}
        </div>
      </header>
      <div class="segmented tabs" role="tablist" aria-label="Resolver view">
        {#each RESOLVER_VIEWS as view, i}
          <button
            role="tab"
            aria-selected={app.resolverView === view}
            aria-controls="inspector-panel"
            title={keyed ? `${RESOLVER_VIEW_LABEL[view]} (${shortcut(`alt+cmd+${i + 1}`)})` : undefined}
            onclick={() => setView(view)}
            >{RESOLVER_VIEW_LABEL[view]}{#if working(view)}<span class="live-dot" aria-hidden="true"></span><span class="visually-hidden">, in progress</span>{/if}</button
          >
        {/each}
      </div>
    </div>

    <div role="tabpanel" id="inspector-panel" aria-label="{RESOLVER_VIEW_LABEL[app.resolverView]}, {label(resolver.provider, resolver.variant)}">
      <ResolverDetail {resolver} {run} {rows} view={app.resolverView} onview={setView} />
    </div>
  </div>
</aside>

<style>
  /* Beside the shortlist it takes the rest of the window; tables use the width, prose keeps its measure in ResolverDetail. */
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
  :global(html.mobile) .bar {
    border-radius: 14px 14px 0 0;
  }
  :global(html.mobile) .head {
    padding: 22px 0 12px;
    touch-action: none;
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
  }
  /* The name and the view tabs stay in view while the view scrolls under them. */
  .bar {
    position: sticky;
    top: 0;
    z-index: 1;
    padding-bottom: 10px;
    background: var(--surface);
    border-bottom: 1px solid var(--rule);
  }
  /* The name keeps at least 180px; in a narrow inspector the buttons drop to a row of their own rather than squeeze it. */
  .head {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 8px 12px;
    padding: 16px 0 12px;
  }
  /* Six views across the pane; a narrow pane or a phone scrolls them sideways rather than squeezing them. */
  .tabs {
    display: flex;
    width: 100%;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabs button {
    flex: 1 0 auto;
    justify-content: center;
  }
  .kicker {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .who {
    flex: 1 1 180px;
    min-width: 0;
  }
  h2 {
    font-size: var(--t-lg);
    font-weight: 680;
    overflow-wrap: anywhere;
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
    align-items: center;
    gap: 2px;
    flex: none;
    margin-left: auto;
  }
  :global(html.mobile) .runall {
    align-self: flex-start;
    margin: -10px 0 0;
  }
  @media (max-width: 560px) {
    .inner {
      padding: 18px 16px 40px;
    }
  }
</style>
