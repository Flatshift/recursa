<script lang="ts">
  // The window's toolbar, which is also its title bar in the desktop app: a title on the
  // left, the page's view switcher in the middle, its actions on the right.
  import type { Snippet } from 'svelte';
  import { app } from '../lib/store.svelte';
  import { shortcut } from '../lib/keys';
  import { mobile } from '../lib/api';
  import Icon from './Icon.svelte';

  let {
    title,
    subtitle = '',
    center,
    actions,
  }: { title: string; subtitle?: string; center?: Snippet; actions?: Snippet } = $props();

  // On the phone a pushed page (a run, a resolver, settings) gets a Back chevron instead of
  // the drawer button; the drawer is still a swipe away.
  const parent: Record<string, string> = { run: '/', resolver: '/resolvers', settings: '/' };
  const back = $derived(mobile ? parent[app.route.name] : undefined);
  function goBack() {
    if (history.length > 1) history.back();
    else app.navigate(back!);
  }
</script>

<header class="toolbar chrome" class:bare={!app.sidebar} data-tauri-drag-region="deep">
  {#if back}
    <button class="tb-icon" aria-label="Back" onclick={goBack}><Icon name="chevron-left" size={22} /></button>
  {:else}
    <button
      class="tb-icon"
      aria-label={app.sidebar ? 'Hide sidebar' : 'Show sidebar'}
      aria-pressed={app.sidebar}
      title={mobile ? undefined : `${app.sidebar ? 'Hide' : 'Show'} sidebar (${shortcut('ctrl+cmd+S')})`}
      onclick={() => app.setSidebar(!app.sidebar, !mobile)}><Icon name={mobile ? 'menu' : 'sidebar'} size={mobile ? 20 : 16} /></button
    >
  {/if}
  <div class="title">
    <h1>{title}</h1>
    {#if subtitle}<p>{subtitle}</p>{/if}
  </div>
  {#if center}<div class="center">{@render center()}</div>{/if}
  {#if actions}<div class="actions">{@render actions()}</div>{/if}
</header>

<style>
  .toolbar {
    container: toolbar / inline-size;
    display: flex;
    align-items: center;
    gap: 14px;
    height: var(--bar-h);
    padding: 0 14px 0 10px;
    border-bottom: 1px solid var(--rule);
    background: var(--bg);
    min-width: 0;
  }
  /* Mac app with the sidebar hidden: the traffic lights now sit over the toolbar. The app
     measures where they end (--lights), and they're gone in full screen. */
  :global(html.mac) .toolbar.bare {
    padding-left: calc(var(--lights, 78px) + 10px);
  }
  :global(html.mobile) .toolbar {
    box-sizing: content-box;
    padding-top: env(safe-area-inset-top);
    padding-left: calc(10px + env(safe-area-inset-left));
    padding-right: calc(14px + env(safe-area-inset-right));
    gap: 6px;
  }
  :global(html.mobile) .toolbar :global(.tb-icon) {
    width: 44px;
    height: 44px;
    margin-left: -8px;
    color: var(--ink);
  }
  :global(html.mobile) h1 {
    font-size: var(--t-md);
  }
  :global(html.mobile) .title p {
    display: none;
  }
  /* Touch has no hover: the button tints only while pressed. */
  :global(html.mobile) .toolbar :global(.tb-icon:hover) {
    background: transparent;
  }
  :global(html.mobile) .toolbar :global(.tb-icon:active) {
    background: var(--surface-2);
  }
  /* A page with its own view switcher gives it the room; the chevron already says where we are. */
  :global(html.mobile) .toolbar:has(:global(.center > *)) .title {
    display: none;
  }
  /* Full screen: the title bar slides down over the window's top edge when the pointer
     reaches it, and the toolbar rides down with it, as a native one does. The app reports
     where the toolbar should end up (--reveal) as the title bar starts to move, and the
     toolbar eases there on AppKit's timing. */
  :global(html.fullscreen) .toolbar {
    position: relative;
    z-index: var(--z-sticky);
    transform: translateY(var(--reveal, 0px));
    transition:
      transform 260ms cubic-bezier(0.25, 0.1, 0.25, 1),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  :global(html.revealed) .toolbar {
    box-shadow: var(--shadow-pop);
  }
  @media (prefers-reduced-motion: reduce) {
    :global(html.fullscreen) .toolbar {
      transition: none;
    }
  }
  .title {
    min-width: 0;
    flex: 0 1 auto;
  }
  h1 {
    font-size: var(--t-base);
    font-weight: 660;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title p {
    font-variant-numeric: tabular-nums;
    font-size: var(--t-xs);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.3;
  }
  .center {
    flex: 1 1 auto;
    display: flex;
    justify-content: center;
    min-width: 0;
  }
  .actions {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .center + .actions {
    margin-left: 0;
  }
  .toolbar :global(.tb-icon) {
    display: grid;
    place-items: center;
    flex: none;
    width: 30px;
    height: 28px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--ink-2);
  }
  .toolbar :global(.tb-icon:hover) {
    background: var(--surface-2);
    color: var(--ink);
  }
  .toolbar :global(.tb-icon[aria-pressed='true']) {
    color: var(--ink);
  }
  .toolbar :global(.tb-icon:disabled) {
    opacity: 0.4;
    background: transparent;
  }
  .toolbar :global(.btn) {
    height: 28px;
    padding: 0 11px;
    font-size: var(--t-xs);
  }
  /* Tight windows: the subtitle goes first; pages swap wide controls for compact ones. */
  @container toolbar (max-width: 1000px) {
    .title p {
      display: none;
    }
  }
  @container toolbar (max-width: 560px) {
    .title {
      display: none;
    }
    /* The phone's navigation bar keeps its title; the page's own controls make room. */
    :global(html.mobile) .title {
      display: block;
    }
  }
</style>
