<script lang="ts">
  // Source list: the app's places, then its runs, newest first.
  import { app } from '../lib/store.svelte';
  import { desktop, mac, openSettings } from '../lib/api';
  import { shortcut } from '../lib/keys';
  import { menu, SEPARATOR } from '../lib/menu.svelte';
  import { ms, shortWhen } from '../lib/format';
  import type { RunRow } from '../lib/types';
  import Icon from './Icon.svelte';
  import Mark from './Mark.svelte';

  const places = [
    { href: '/', label: 'Benchmark', icon: 'play', match: 'bench', key: '1' },
    { href: '/resolvers', label: 'Resolvers', icon: 'list', match: 'resolvers', key: '2' },
    { href: '/reliability', label: 'Reliability', icon: 'pulse', match: 'reliability', key: '3' },
    { href: '/time', label: 'Time', icon: 'clock', match: 'time', key: '5' },
    { href: '/method', label: 'Method', icon: 'book', match: 'method', key: '4' },
  ] as const;

  const runs = $derived(app.boot?.runs ?? []);
  const liveId = $derived(app.boot?.active_run ?? (app.run?.status === 'running' ? app.run.id : null));
  const current = $derived(app.route.name === 'run' ? app.route.id : null);
  const monitoring = $derived(app.boot?.monitor ?? null);

  function go(e: MouseEvent, href: string) {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    e.preventDefault();
    app.navigate(href);
  }

  function runMenu(e: MouseEvent, r: RunRow) {
    const done = r.status !== 'running' && r.id !== liveId;
    menu.show(
      e,
      [
        { id: 'open', label: 'Open' },
        SEPARATOR,
        { id: 'csv', label: 'Export Samples as CSV…', enabled: done },
        { id: 'json', label: 'Export Run as JSON…', enabled: done },
        SEPARATOR,
        { id: 'save', label: r.saved ? 'Unsave Run' : 'Save Run', enabled: done },
        { id: 'delete', label: 'Delete Run…', enabled: done },
        SEPARATOR,
        { id: 'clear', label: 'Clear All Runs…' },
      ],
      (id) => {
        if (id === 'delete') app.deleteRun(r.id);
        else if (id === 'save') app.setSaved(r.id, !r.saved);
        else if (id === 'clear') app.clearRuns();
        else if (id === 'csv' || id === 'json') app.exportRun(id, r.id);
        else if (current !== r.id) app.navigate(`/run/${r.id}`);
      },
    );
  }

  function settings(e: MouseEvent) {
    if (desktop) openSettings();
    else go(e, '/settings');
  }
</script>

<nav class="side chrome" aria-label="Sidebar">
  <div class="top" data-tauri-drag-region="deep">
    {#snippet mark()}
      <Mark />
      <span>Recursa</span>
    {/snippet}
    <!-- In the Mac app this row belongs to the traffic lights and is the window's drag handle, as in
         any Mac app; the brand lives in the Dock and the menu bar there. -->
    {#if !mac}
      <a class="brand" href="/" onclick={(e) => go(e, '/')} aria-label="Recursa">{@render mark()}</a>
    {/if}
  </div>

  <ul class="places">
    {#each places as p}
      {@const on = app.route.name === p.match || (p.match === 'resolvers' && app.route.name === 'deepdive')}
      <li>
        <a href={p.href} class="item" class:on aria-current={on ? 'page' : undefined} onclick={(e) => go(e, p.href)} title="{p.label} ({shortcut(`cmd+${p.key}`)})">
          <Icon name={p.icon} size={15} />
          <span>{p.label}</span>
        </a>
      </li>
    {/each}
  </ul>

  <div class="runs-h">
    <span>Runs</span>
    {#if runs.length}<span class="num count">{runs.length}</span>{/if}
  </div>
  <ol class="runs scroll" aria-label="Runs">
    {#each runs as r (r.id)}
      {@const live = r.id === liveId}
      {@const on = r.id === current}
      <li>
        <a
          href="/run/{r.id}"
          class="item run"
          class:on
          aria-current={on ? 'page' : undefined}
          onclick={(e) => go(e, `/run/${r.id}`)}
          oncontextmenu={(e) => runMenu(e, r)}
        >
          <span class="rt">
            {#if live}<span class="dot" aria-hidden="true"></span>{/if}
            <span>{shortWhen(r.started_at)}</span>
            {#if r.saved}<span class="kept" title="Saved: kept when runs are cleared"><Icon name="bookmark" size={10} /><span class="visually-hidden">Saved</span></span>{/if}
          </span>
          <span class="rb">
            {#if live}
              Measuring…
            {:else if r.summary.best?.tied_with}
              <span title="Tied for fastest: too close for this run to call">{r.summary.best.label} or {r.summary.best.tied_with}</span>
            {:else if r.summary.best}
              <span>{r.summary.best.label}</span> <span class="num" title="Speed score">{ms(r.summary.best.composite_ms)}</span>
            {:else}
              <span class="faint">{r.status === 'cancelled' ? 'Stopped' : 'No reliable pick'}</span>
            {/if}
          </span>
        </a>
      </li>
    {:else}
      <li class="empty">Finished benchmarks appear here.</li>
    {/each}
  </ol>

  <div class="foot">
    <a
      class="mon"
      href="/reliability"
      onclick={(e) => go(e, '/reliability')}
      class:on={monitoring?.enabled}
      title={monitoring?.enabled ? `Background monitoring runs every ${monitoring.interval_min} min. Open Reliability.` : 'Background monitoring is off. Open Reliability to switch it on.'}
    >
      <span class="dot" aria-hidden="true"></span>
      <span class="ml">
        {#if monitoring?.enabled}Monitoring <span class="sub">{monitoring.interval_min} min</span>{:else}Monitoring <span class="sub">off</span>{/if}
      </span>
      <Icon name="chevron-right" size={12} />
    </a>
    <button class="gear" aria-label="Settings" title="Settings{desktop ? ` (${shortcut('cmd+,')})` : ''}" onclick={settings}><Icon name="gear" size={15} /></button>
  </div>
</nav>

<style>
  .side {
    display: grid;
    grid-template-rows: var(--bar-h) auto auto minmax(0, 1fr) auto;
    /* minmax(0, …): the footer's label must not widen the column past the slot. */
    grid-template-columns: minmax(0, 1fr);
    height: 100%;
    min-height: 0;
    background: var(--side);
    border-right: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .top {
    display: flex;
    align-items: center;
    padding: 0 16px;
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    color: var(--ink);
    font-size: var(--t-base);
    font-weight: 680;
    letter-spacing: -0.015em;
    text-decoration: none;
  }
  ul,
  ol {
    list-style: none;
    margin: 0;
    padding: 0 8px;
  }
  .places {
    padding-top: 2px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 9px;
    min-height: 28px;
    padding: 0 10px;
    border-radius: var(--r-md);
    color: var(--ink);
    text-decoration: none;
    cursor: default;
  }
  .item :global(svg) {
    color: var(--ink-3);
    flex: none;
  }
  .item:hover {
    background: color-mix(in oklch, var(--side-sel) 45%, transparent);
  }
  .item.on {
    background: var(--side-sel);
  }
  .item.on :global(svg) {
    color: var(--accent-text);
  }
  .runs-h {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 18px 18px 4px;
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-3);
  }
  .count {
    font-weight: 500;
  }
  .runs {
    padding-bottom: 8px;
  }
  .run {
    display: grid;
    gap: 0;
    padding: 5px 10px;
    line-height: 1.3;
  }
  .rt {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .rb {
    font-size: var(--t-xs);
    color: var(--ink);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rb .num {
    color: var(--ink-2);
  }
  .faint {
    color: var(--ink-3);
  }
  .item.on .rt {
    color: var(--ink-2);
  }
  .kept {
    display: inline-flex;
    margin-left: auto;
    color: var(--ink-3);
  }
  .item.on .kept {
    color: var(--ink-2);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--s-uncached);
    animation: pulse 1.4s var(--ease-out) infinite;
  }
  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 color-mix(in oklch, var(--s-uncached) 55%, transparent);
    }
    100% {
      box-shadow: 0 0 0 6px transparent;
    }
  }
  .empty {
    padding: 6px 10px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px 10px;
    border-top: 1px solid var(--rule);
  }
  /* A button, not a caption: a raised surface with a hairline, a status dot and a chevron that says it goes somewhere. */
  .mon {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: 28px;
    padding: 0 8px 0 10px;
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    background: var(--surface);
    font-size: var(--t-xs);
    color: var(--ink);
    text-decoration: none;
    cursor: default;
    transition: background-color 120ms ease, border-color 120ms ease;
  }
  .mon:hover {
    background: var(--surface-2);
    border-color: color-mix(in oklch, var(--ink-3) 45%, var(--rule));
    text-decoration: none;
  }
  .mon:active {
    background: var(--surface-3);
  }
  .mon :global(svg) {
    flex: none;
    color: var(--ink-3);
  }
  .mon:hover :global(svg) {
    color: var(--ink);
  }
  .ml {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-weight: 560;
  }
  .sub {
    font-weight: 400;
    color: var(--ink-2);
  }
  .mon .dot {
    flex: none;
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--ink-3);
    animation: none;
  }
  .mon.on .dot {
    background: var(--good);
    box-shadow: none;
  }
  @media (prefers-reduced-motion: reduce) {
    .mon {
      transition: none;
    }
    .dot {
      animation: none;
    }
  }
  .gear {
    display: grid;
    place-items: center;
    width: 28px;
    height: 26px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--ink-2);
  }
  .gear:hover {
    background: color-mix(in oklch, var(--side-sel) 45%, transparent);
    color: var(--ink);
  }
</style>
