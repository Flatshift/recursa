<script lang="ts">
  import { untrack } from 'svelte';
  import { app } from './lib/store.svelte';
  import { desktop, mobile, shell } from './lib/api';
  import { isMac, shortcut } from './lib/keys';
  import Sidebar from './components/Sidebar.svelte';
  import Toolbar from './components/Toolbar.svelte';
  import InstrumentStrip from './components/InstrumentStrip.svelte';
  import MenuHost from './components/MenuHost.svelte';
  import Tooltip from './components/Tooltip.svelte';
  import Setup from './components/Setup.svelte';
  import RunView from './routes/RunView.svelte';
  import Resolvers from './routes/Resolvers.svelte';
  import DeepDive from './routes/DeepDive.svelte';
  import Reliability from './routes/Reliability.svelte';
  import Time from './routes/Time.svelte';
  import Method from './routes/Method.svelte';
  import Settings from './routes/Settings.svelte';
  import Icon from './components/Icon.svelte';

  app.init();

  const titles: Record<string, string> = {
    bench: 'Benchmark',
    run: 'Results',
    resolvers: 'Resolvers',
    deepdive: 'Deep Dive',
    reliability: 'Reliability',
    time: 'Time',
    method: 'Method',
    settings: 'Settings',
  };
  $effect(() => {
    document.title = `${titles[app.route.name]} · Recursa`;
  });

  // In the desktop app, Settings is its own window with no sidebar or status bar.
  const standalone = shell && app.route.name === 'settings';

  // Narrow windows show the sidebar as an overlay, closed until asked for.
  let width = $state(innerWidth);
  const narrow = $derived(width < 860);
  $effect(() => {
    if (narrow) app.setSidebar(false, false);
    else app.restoreSidebar();
  });
  $effect(() => {
    app.route;
    if (untrack(() => narrow)) app.setSidebar(false, false);
  });

  // Phone: the sidebar is a drawer that follows the finger in from the left edge and back out.
  // `drag` is how much of it shows while a finger has it; between touches CSS owns the position.
  const EDGE = 24;
  let drawer = $state<HTMLElement>();
  let drag = $state<number | null>(null);
  let touch: { x: number; y: number; t: number; open: boolean; w: number; axis: 'x' | 'y' | null } | null = null;

  function tStart(e: TouchEvent) {
    if (!mobile || e.touches.length !== 1 || !drawer) return;
    const t = e.touches[0];
    const open = app.sidebar;
    if (!open && t.clientX > EDGE) return;
    touch = { x: t.clientX, y: t.clientY, t: performance.now(), open, w: drawer.offsetWidth, axis: null };
  }
  function tMove(e: TouchEvent) {
    if (!touch) return;
    const t = e.touches[0];
    const dx = t.clientX - touch.x;
    const dy = t.clientY - touch.y;
    if (!touch.axis) {
      if (Math.abs(dx) < 6 && Math.abs(dy) < 6) return;
      touch.axis = Math.abs(dx) > Math.abs(dy) ? 'x' : 'y';
      // A vertical move is a scroll, not the drawer.
      if (touch.axis === 'y') return (touch = null);
    }
    drag = Math.max(0, Math.min(touch.w, (touch.open ? touch.w : 0) + dx));
  }
  function tEnd(e: TouchEvent) {
    if (!touch) return;
    const { open, w, x, t } = touch;
    touch = null;
    if (drag === null) return;
    const dx = e.changedTouches[0].clientX - x;
    const flick = Math.abs(dx) > 30 && performance.now() - t < 300;
    app.setSidebar(flick ? dx > 0 : drag > w / 2, false);
    drag = null;
  }

  const onRun = $derived(app.route.name === 'run' && app.run);
  const stripEnv = $derived(onRun ? app.run!.env : app.env);

  // A settings window changing the theme updates this one too.
  function onStorage(e: StorageEvent) {
    if (e.key === 'recursa.theme' && e.newValue) app.setTheme(JSON.parse(e.newValue));
  }

  // The app has no "Reload / Inspect Element" menu: keep the WebKit default only where it
  // edits or copies text.
  function onContext(e: MouseEvent) {
    if (!desktop || e.defaultPrevented) return;
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, [contenteditable]') || String(getSelection() ?? '').trim()) return;
    e.preventDefault();
  }
</script>

<svelte:window bind:innerWidth={width} onstorage={onStorage} oncontextmenu={onContext} />

{#if standalone}
  <Settings />
{:else}
  <a class="skip" href="#main">Skip to content</a>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="shell" class:noside={!app.sidebar || narrow} ontouchstart={tStart} ontouchmove={tMove} ontouchend={tEnd} ontouchcancel={tEnd}>
    {#if mobile}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div
        class="scrim fade"
        class:on={app.sidebar}
        class:dragging={drag !== null}
        style:opacity={drag === null ? undefined : drag / (drawer?.offsetWidth || 1)}
        onclick={() => app.setSidebar(false, false)}
      ></div>
      <div
        class="side-slot drawer"
        class:open={app.sidebar}
        class:dragging={drag !== null}
        style:transform={drag === null ? undefined : `translateX(${drag - (drawer?.offsetWidth || 0)}px)`}
        bind:this={drawer}
        inert={!app.sidebar && drag === null}
      >
        <Sidebar />
      </div>
      <div class="edge"></div>
    {:else if app.sidebar}
      <div class="side-slot" class:overlay={narrow}>
        <Sidebar />
      </div>
      {#if narrow}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div class="scrim" onclick={() => app.setSidebar(false, false)}></div>
      {/if}
    {/if}
    <div class="main">
      <main id="main" class="route">
        {#if app.bootError}
          <div class="view-root">
            <Toolbar title="Recursa" />
            <div class="content boot">
              <h2>Recursa can't reach its engine</h2>
              <p class="ink-2">{app.bootError}</p>
              {#if desktop}
                <p class="ink-2">Quit Recursa ({isMac ? shortcut('cmd+Q') : 'Recursa › Exit, or from the tray icon'}) and open it again.</p>
              {:else}
                <p class="ink-2">Start it from a terminal with <code>recursa</code>, then reload this page.</p>
              {/if}
              <button class="btn primary" onclick={() => app.init()}><Icon name="refresh" size={14} />Try again</button>
            </div>
          </div>
        {:else if !app.boot || !app.draft}
          <div class="view-root">
            <Toolbar title="Recursa" />
            <div class="content boot"><p class="muted">Starting…</p></div>
          </div>
        {:else if app.route.name === 'bench'}
          <Setup />
        {:else if app.route.name === 'run'}
          {#key app.route.id}<RunView />{/key}
        {:else if app.route.name === 'resolvers'}
          <Resolvers />
        {:else if app.route.name === 'deepdive'}
          {#key app.route.id}<DeepDive id={app.route.id} />{/key}
        {:else if app.route.name === 'reliability'}
          <Reliability />
        {:else if app.route.name === 'time'}
          <Time />
        {:else if app.route.name === 'method'}
          <Method />
        {:else if app.route.name === 'settings'}
          <Settings />
        {/if}
      </main>
      <InstrumentStrip env={stripEnv} now={app.env} link={onRun ? app.run!.link : undefined} frozen={!!onRun && app.run?.status !== 'running'} />
    </div>
  </div>
{/if}

<MenuHost />
<Tooltip />

{#if app.toast}
  <div class="toast {app.toast.tone}" role={app.toast.tone === 'error' ? 'alert' : 'status'}>
    <Icon name={app.toast.tone === 'error' ? 'warn' : 'check'} size={15} />
    <span>{app.toast.text}</span>
    <button class="btn ghost sm" aria-label="Dismiss" onclick={() => (app.toast = null)}><Icon name="close" size={13} /></button>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: var(--side-w) minmax(0, 1fr);
    height: 100%;
    position: relative;
  }
  .shell.noside {
    grid-template-columns: minmax(0, 1fr);
  }
  .side-slot {
    min-height: 0;
    height: 100%;
  }
  .side-slot.overlay {
    position: absolute;
    inset: 0 auto 0 0;
    width: var(--side-w);
    z-index: var(--z-sheet);
    box-shadow: var(--shadow-sheet);
    animation: slide-in var(--dur) var(--ease-out);
  }
  @keyframes slide-in {
    from {
      transform: translateX(-12px);
      opacity: 0;
    }
  }
  .scrim {
    position: absolute;
    inset: 0;
    z-index: var(--z-backdrop);
    background: var(--backdrop);
  }
  /* The phone drawer: off screen to the left, slid in by a finger or the toolbar button. */
  .side-slot.drawer {
    position: absolute;
    inset: 0 auto 0 0;
    width: var(--side-w);
    z-index: var(--z-sheet);
    transform: translateX(-100%);
    transition: transform var(--dur) var(--ease-out);
    touch-action: pan-y;
  }
  .side-slot.drawer.open {
    transform: none;
    box-shadow: var(--shadow-sheet);
  }
  .side-slot.drawer.dragging {
    transition: none;
    box-shadow: var(--shadow-sheet);
  }
  .scrim.fade {
    opacity: 0;
    pointer-events: none;
    transition: opacity var(--dur) var(--ease-out);
  }
  .scrim.fade.on {
    opacity: 1;
    pointer-events: auto;
  }
  .scrim.fade.dragging {
    transition: none;
  }
  /* The strip of screen the drawer swipes in from, below the toolbar so its button stays tappable. */
  .edge {
    position: absolute;
    inset: calc(env(safe-area-inset-top) + var(--bar-h)) auto 0 0;
    width: 24px; /* EDGE */
    z-index: var(--z-backdrop);
    touch-action: none;
  }
  @media (prefers-reduced-motion: reduce) {
    .side-slot.drawer,
    .scrim.fade {
      transition: none;
    }
  }
  .main {
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto;
    min-width: 0;
    min-height: 0;
    height: 100%;
  }
  .route {
    min-height: 0;
    min-width: 0;
  }
  .skip {
    position: absolute;
    left: -9999px;
    top: 8px;
    z-index: var(--z-toast);
    background: var(--surface);
    padding: 8px 12px;
    border-radius: var(--r-md);
  }
  .skip:focus {
    left: 12px;
  }
  .boot {
    display: grid;
    gap: 12px;
    justify-items: start;
    align-content: start;
    padding-top: 64px;
  }
  .boot h2 {
    font-size: var(--t-xl);
  }
  .toast {
    position: fixed;
    left: 50%;
    bottom: calc(var(--status-h) + 18px + env(safe-area-inset-bottom));
    transform: translateX(-50%);
    z-index: var(--z-toast);
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 8px 8px 14px;
    background: var(--ink);
    color: var(--bg);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    font-size: var(--t-sm);
    max-width: min(560px, calc(100vw - 32px));
    animation: rise var(--dur) var(--ease-out);
  }
  .toast .btn {
    color: inherit;
  }
  .toast.error :global(svg:first-child) {
    color: var(--serious);
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translate(-50%, 8px);
    }
  }
</style>
