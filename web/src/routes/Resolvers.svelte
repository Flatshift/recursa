<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import { app, RESOLVER_VIEWS } from '../lib/store.svelte';
  import { selectKey } from '../lib/select';
  import { menu } from '../lib/menu.svelte';
  import { SEPARATOR, api, mobile } from '../lib/api';
  import { shortcut } from '../lib/keys';
  import type { Resolver, RunState } from '../lib/types';
  import { capCell } from '../lib/cells';
  import { PUBLISHED_ONLY } from '../lib/caps';
  import { HIT_SHARE, buildRows } from '../lib/verdict';
  import { status, stateLabel, stateGlyph, localNote, elsewhereNote } from '../lib/status.svelte';
  import { TRANSPORT_LABEL, ago, documented, ms, pct } from '../lib/format';
  import Cell from '../components/Cell.svelte';
  import Glyph from '../components/Glyph.svelte';
  import Provenance from '../components/Provenance.svelte';
  import Inspector from '../components/Inspector.svelte';
  import Shortlist, { type Pick } from '../components/Shortlist.svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import { faults } from '../components/BehaviourChecks.svelte';
  import { onResolverContext } from '../lib/resolverMenu';
  import { type CheckKind, batchProgress, blocker, canRun, confirmLive, errorOf, isRunning, results, runBatch, stopBatch } from '../lib/checks.svelte';

  let last = $state<RunState | null>(null);
  $effect(() => {
    status.load();
    const done = app.boot?.runs.find((r) => r.status === 'done');
    if (done && last?.id !== done.id) api.get<RunState>(`/api/runs/${done.id}`).then((r) => (last = r)).catch(() => {});
  });

  let filter = $state('');
  const list = $derived(
    app.resolvers.filter((r) => {
      const f = filter.trim().toLowerCase();
      return !f || [r.provider, r.variant, ...r.ipv4, ...r.published.filtering, r.published.jurisdiction ?? ''].join(' ').toLowerCase().includes(f);
    }),
  );
  const groups = $derived.by(() => {
    const order = ['system', 'custom', 'public', 'legacy'];
    const m = new Map<string, Resolver[]>();
    for (const r of list) {
      const g = r.group === 'system' || r.group === 'custom' ? 'Your network & custom' : r.provider;
      if (!m.has(g)) m.set(g, []);
      m.get(g)!.push(r);
    }
    return [...m.entries()].sort((a, b) => order.indexOf(a[1][0].group) - order.indexOf(b[1][0].group));
  });

  const inLast = (r: Resolver) => !!last?.probes[r.id];
  const lastRows = $derived(last ? buildRows(last, app.boot?.resolvers ?? [], HIT_SHARE[app.mix]) : []);
  const open = $derived(app.inspect ? app.resolvers.find((r) => r.id === app.inspect) : undefined);
  const caps = ['dnssec', 'ecs', 'qmin', 'filter_malware', 'filter_ads'];
  const capLabels: Record<string, string> = {
    dnssec: 'DNSSEC',
    ecs: 'ECS',
    qmin: 'QNAME min',
    filter_malware: 'Malware',
    filter_ads: 'Ads',
  };

  /** Encrypted transports offered, measured ones marked: one column instead of five. */
  function encrypted(r: Resolver): { label: string; verified: boolean }[] {
    const run = last && inLast(r) ? last : null;
    const out: { label: string; verified: boolean }[] = [];
    for (const key of ['dot', 'doh'] as const) {
      const c = capCell(run, r, key);
      if (c.status === 'pass') out.push({ label: TRANSPORT_LABEL[key], verified: c.prov === 'verified' });
    }
    for (const p of PUBLISHED_ONLY) if (p.get(r.published)) out.push({ label: p.key === 'dnscrypt' ? 'DNSCrypt' : p.key.replace('doq', 'DoQ').replace('odoh', 'ODoH'), verified: false });
    return out;
  }

  let filterEl: HTMLInputElement | undefined = $state();
  $effect(() => {
    if (app.findRev) filterEl?.focus();
  });
  // Inspecting a resolver splits the view: the shortlist on the left, its detail on the right. Phones use a sheet.
  const split = $derived(!!open && !mobile);
  let bodyW = $state(1200);
  const narrow = $derived(bodyW < 720);
  const name = (r: Resolver) => (documented(r) ? r.variant || r.provider : `${r.provider} ${r.variant}`);
  const picks = $derived<[string, Pick[]][]>(groups.map(([g, rs]) => [g, rs.map((r) => ({ id: r.id, name: name(r), sub: r.ipv4[0] ?? r.ipv6[0] ?? r.name }))]));
  function onKey(e: KeyboardEvent) {
    if ((e.target as HTMLElement)?.tagName === 'INPUT' || menu.open) return;
    // ⌥⌘1–6 (Ctrl+Shift+1–6 on Windows) pick the Inspector's view; the desktop menu sends the same commands.
    if (app.inspect && ((e.altKey && e.metaKey) || (e.ctrlKey && e.shiftKey)) && /^Digit[1-6]$/.test(e.code)) {
      e.preventDefault();
      app.resolverView = RESOLVER_VIEWS[Number(e.code.slice(5)) - 1];
    } else if (e.key === 'Escape' && app.inspect) app.inspect = null;
    else selectKey(e);
  }

  // Rows ticked for a batch of checks. Ticks outlast the filter, but only listed rows are checked.
  const picked = new SvelteSet<string>();
  const ordered = $derived(groups.flatMap(([, rs]) => rs));
  const chosen = $derived(ordered.filter((r) => picked.has(r.id)));
  const allPicked = $derived(ordered.length > 0 && chosen.length === ordered.length);
  function pickAll() {
    // allPicked recomputes as the set changes, so read it once.
    const all = allPicked;
    for (const r of ordered) all ? picked.delete(r.id) : picked.add(r.id);
  }
  const togglePick = (id: string) => (picked.has(id) ? picked.delete(id) : picked.add(id));

  // The ticked rows, or every listed one when none are ticked, one resolver at a time.
  const targets = $derived(chosen.length ? chosen : ordered);
  const batchLabel = $derived(chosen.length ? `Check ${chosen.length} selected` : filter.trim() ? `Check ${ordered.length} shown` : 'Check all');
  const progress = $derived(batchProgress());
  const busyName = $derived.by(() => {
    const r = progress?.rid ? app.resolvers.find((x) => x.id === progress.rid) : undefined;
    return r ? name(r) : '';
  });
  const stop = $derived(blocker());
  function batchMenu(e: MouseEvent) {
    const rs = targets;
    menu.show(
      e,
      [
        { id: 'all', label: 'Run All Checks' },
        SEPARATOR,
        ...(app.env?.trace !== 'unsupported' ? [{ id: 'trace', label: 'Trace Path', enabled: rs.some((r) => canRun('trace', r)) }] : []),
        { id: 'dive', label: 'Sample 18 Queries' },
        { id: 'checks', label: 'Check Behaviour' },
        { id: 'live', label: 'Check Live Feeds', enabled: rs.some((r) => canRun('live', r)) },
      ],
      async (pick) => {
        if (pick === 'live' && !(await confirmLive())) return;
        void runBatch(rs, pick === 'all' ? ['trace', 'dive', 'checks'] : [pick as CheckKind]);
      },
      e.currentTarget as HTMLElement,
    );
  }

  // A column for each check that has run on a listed resolver this session.
  const RESULT_COLS: { kind: CheckKind; label: string; title: string }[] = [
    { kind: 'dive', label: 'Median', title: 'Median round trip of the 18-query sample' },
    { kind: 'trace', label: 'Path', title: 'Router hops to the resolver, from Trace path' },
    { kind: 'checks', label: 'Behaviour', title: 'Faults found by Check behaviour' },
    { kind: 'live', label: 'Live threats', title: 'Live malicious hosts blocked, from Check live feeds' },
  ];
  const touched = (kind: CheckKind, id: string) => !!results[kind][id] || !!errorOf(kind, id) || isRunning(kind, id);
  const resultCols = $derived(RESULT_COLS.filter((c) => ordered.some((r) => touched(c.kind, r.id))));
  /** One check's result as a list cell, with the full reading for its tooltip. */
  function outcome(kind: CheckKind, id: string): { text: string; title?: string; bad?: boolean } | null {
    if (kind === 'dive') {
      const d = results.dive[id];
      return d && { text: d.latency.samples_ms.length ? ms(d.latency.stats.p50) : 'no answers' };
    }
    if (kind === 'trace') {
      const t = results.trace[id];
      const last = t?.hops[t.hops.length - 1];
      return t && (t.reached && last ? { text: `${last.ttl} hops` } : { text: 'stopped', title: `Trace stopped after hop ${t.hops.length}` });
    }
    if (kind === 'checks') {
      const c = results.checks[id];
      const f = c ? faults(c) : [];
      return c && (f.length ? { text: f.length === 1 ? '1 fault' : `${f.length} faults`, title: f.join(', '), bad: true } : { text: 'no faults' });
    }
    const cats = Object.values(results.live[id]?.categories ?? {});
    const tested = cats.reduce((a, c) => a + c.tested, 0);
    const blocked = cats.reduce((a, c) => a + c.blocked, 0);
    return results.live[id] ? (tested ? { text: pct(blocked / tested, 0), title: `Blocked ${blocked} of ${tested} live threats` } : { text: 'none tested' }) : null;
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="view-root">
  <Toolbar title="Resolvers" subtitle={last ? `Verified in your latest run, ${ago(last.started_at)}` : 'Published claims until you run a benchmark'}>
    {#snippet actions()}
      {#if !mobile}
        <label class="search">
          <Icon name="search" size={13} />
          <span class="visually-hidden">Filter</span>
          <input bind:this={filterEl} type="search" bind:value={filter} placeholder="Filter" />
        </label>
      {/if}
      {#if progress}
        <span class="batch" aria-live="polite"
          >{progress.stopping ? 'Stopping after this check…' : `Checking ${Math.min(progress.done + 1, progress.total)} of ${progress.total}${busyName ? ` · ${busyName}` : ''}…`}</span
        >
        <button class="btn sm" disabled={progress.stopping} onclick={stopBatch}>Stop</button>
      {:else}
        <button class="btn sm" disabled={!!stop || !targets.length} title={stop ?? 'Run checks on each resolver in turn'} onclick={batchMenu}
          ><Icon name="play" size={12} />{batchLabel}</button
        >
      {/if}
      <button
        class="tb-icon"
        aria-label={app.inspect ? 'Hide inspector' : 'Show inspector'}
        aria-pressed={!!app.inspect}
        title="{app.inspect ? 'Hide' : 'Show'} inspector ({shortcut('alt+cmd+I')})"
        onclick={() => app.toggleInspector()}><Icon name="inspector" size={16} /></button
      >
    {/snippet}
  </Toolbar>

  <div class="view-body" bind:clientWidth={bodyW}>
    {#if split}
      <Shortlist groups={picks} selected={app.inspect} back="All resolvers" collapsed={narrow} onselect={(id) => (app.inspect = id)} onback={() => (app.inspect = null)} />
    {:else}
      <!-- svelte-ignore a11y_no_static_element_interactions (right-click menu for the rows inside) -->
      <div class="content scroll" oncontextmenu={onResolverContext}>
        {#if mobile}
          <!-- Phone: the filter spans the page, and each resolver is one row that opens the inspector. -->
          <label class="search msearch">
            <Icon name="search" size={15} />
            <span class="visually-hidden">Filter</span>
            <input bind:this={filterEl} type="search" bind:value={filter} placeholder="Filter by name, address or blocking" />
          </label>
          <div class="mres">
            {#each groups as [g, rs]}
              <h3 class="mgrp">{g}</h3>
              {#each rs as r (r.id)}
                {@const feed = status.feedFor(r.status_feed)}
                {@const enc = encrypted(r)}
                <div class="mitem" class:sel={app.inspect === r.id} data-resolver={r.id}>
                  <button class="mrow" onclick={() => (app.inspect = r.id)}>
                    <span class="mtop">
                      <span class="nm">
                        <b>{name(r)}</b>
                        {#if inLast(r)}<span class="v"><Provenance kind="verified" /></span>{/if}
                      </span>
                      {#if feed}<span class="pub"><Glyph status={stateGlyph(feed.state)} size={12} />{stateLabel(feed)}</span>{/if}
                    </span>
                    <span class="mono addr">{r.ipv4[0] ?? r.ipv6[0] ?? r.name}{#if !r.udp}<span class="enc"> · encrypted only</span>{/if}</span>
                    <span class="mcaps">
                      {#each caps as c}<span class="mc"><span class="mk">{capLabels[c]}</span><Cell c={capCell(last && inLast(r) ? last : null, r, c)} compact /></span>{/each}
                      {#if enc.length}<span class="mc"><span class="mk">Encrypted</span><span class="t">{enc.map((t) => t.label).join(' · ')}</span></span>{/if}
                      {#each resultCols as c}{#if touched(c.kind, r.id)}<span class="mc"><span class="mk">{c.label}</span>{@render result(c.kind, r.id)}</span>{/if}{/each}
                    </span>
                  </button>
                  <label class="pick"><input type="checkbox" checked={picked.has(r.id)} onchange={() => togglePick(r.id)} /><span class="visually-hidden">Select {name(r)}</span></label>
                </div>
              {/each}
            {/each}
          </div>
        {:else}
        <div class="tablewrap panel">
          <table class="sticky-head">
            <thead>
              <tr>
                <th class="first">
                  <span class="who">
                    <input
                      type="checkbox"
                      checked={allPicked}
                      indeterminate={chosen.length > 0 && !allPicked}
                      aria-label="Select all listed resolvers"
                      onchange={pickAll}
                    />Resolver
                  </span>
                </th>
                {#each caps as c}<th>{capLabels[c]}</th>{/each}
                <th title="Encrypted transports: DoT and DoH are checked by probes; DoQ, DNSCrypt and ODoH are as published">Encrypted</th>
                <th title="The operator's status page, where one is published">Status</th>
                {#each resultCols as c}<th title={c.title}>{c.label}</th>{/each}
              </tr>
            </thead>
            {#each groups as [g, rs]}
              <tbody>
                <tr class="grp"><th colspan={3 + caps.length + resultCols.length} class="first">{g}</th></tr>
                {#each rs as r (r.id)}
                  {@const feed = status.feedFor(r.status_feed)}
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions (the first-column button is the keyboard target) -->
                  <tr data-resolver={r.id} class:sel={app.inspect === r.id} onclick={() => (app.inspect = r.id)}>
                    <th class="first" scope="row">
                      <span class="who">
                        <input
                          type="checkbox"
                          checked={picked.has(r.id)}
                          aria-label="Select {name(r)}"
                          onclick={(e) => e.stopPropagation()}
                          onchange={() => togglePick(r.id)}
                        />
                        <button>
                          <span class="nm">
                            <b>{name(r)}</b>
                            {#if inLast(r)}<span class="v" title="Verified in your latest run"><Provenance kind="verified" /></span>{/if}
                          </span>
                          <span class="mono addr">{r.ipv4[0] ?? r.ipv6[0] ?? r.name}{#if !r.udp}<span class="enc"> · encrypted only</span>{/if}</span>
                        </button>
                      </span>
                    </th>
                    {#each caps as c}<td><Cell c={capCell(last && inLast(r) ? last : null, r, c)} compact /></td>{/each}
                    <td>
                      <span class="tx">
                        {#each encrypted(r) as t}<span class="t">{t.label}{#if t.verified}<Provenance kind="verified" />{/if}</span>{:else}<span
                            class="muted">—</span
                          >{/each}
                      </span>
                    </td>
                    <td>
                      {#if feed}
                        <span class="pub" title={localNote(feed) ?? undefined}><Glyph status={stateGlyph(feed.state)} size={12} />{stateLabel(feed)}</span
                        >{#if elsewhereNote(feed)}<span class="away">{elsewhereNote(feed)}</span>{/if}
                      {:else if r.status_feed && !status.data}
                        <span class="muted">{status.error ? 'unavailable' : 'checking…'}</span>
                      {:else if documented(r)}
                        <span class="muted" title="This operator publishes no status page">none</span>
                      {:else}
                        <span class="muted">—</span>
                      {/if}
                    </td>
                    {#each resultCols as c}<td>{@render result(c.kind, r.id)}</td>{/each}
                  </tr>
                {/each}
              </tbody>
            {/each}
          </table>
        </div>
        {/if}
        <p class="legend">
          <span><Provenance kind="verified" /> measured from your network</span>
          <span><Provenance kind="published" /> operator's documentation</span>
          <span><Provenance kind="unknown" /> not documented, not yet measured</span>
          <span>Add an ISP or custom resolver under Benchmark → Custom.</span>
        </p>
      </div>

    {/if}
    <!-- One mount for both layouts: beside the shortlist on the desktop, a sheet over the list on the phone. -->
    {#if open}
      {#key open.id}
        <Inspector resolver={open} run={last && inLast(open) ? last : null} rows={last && inLast(open) ? lastRows : []} onclose={() => (app.inspect = null)} />
      {/key}
    {/if}
  </div>
</div>

<!-- A check's cell: a dot while it runs, its error on hover, or what it found. -->
{#snippet result(kind: CheckKind, id: string)}
  {@const o = outcome(kind, id)}
  {@const err = errorOf(kind, id)}
  {#if isRunning(kind, id)}
    <span class="live-dot" title="Checking…"></span><span class="visually-hidden">Checking</span>
  {:else if err}
    <span class="muted" title={err}>failed</span>
  {:else if o}
    <span class:bad={o.bad} title={o.title}>{o.text}</span>
  {:else}
    <span class="muted">—</span>
  {/if}
{/snippet}

<style>
  .search {
    position: relative;
    display: flex;
    align-items: center;
    color: var(--ink-3);
  }
  .search :global(svg) {
    position: absolute;
    left: 8px;
    pointer-events: none;
  }
  .search input {
    width: 220px;
    height: 28px;
    padding-left: 26px;
    font-size: var(--t-xs);
  }
  /* A narrow window keeps the Check button and the inspector toggle in view. */
  @media (max-width: 560px) {
    .search input {
      width: 130px;
    }
  }
  .tablewrap {
    overflow-x: auto;
    /* Scroll shadow: an edge shade that appears only while columns remain off to the right.
       The local layer travels with the content and covers the shade at the end. */
    background:
      linear-gradient(to left, var(--surface) 40%, transparent) right / 48px 100% no-repeat local,
      linear-gradient(to left, color-mix(in oklch, var(--ink) 14%, transparent), transparent) right / 18px 100% no-repeat scroll,
      var(--surface);
  }
  table {
    width: 100%;
    border-collapse: separate;
    border-spacing: 0;
    font-size: var(--t-xs);
  }
  th,
  td {
    padding: 7px 9px;
    border-bottom: 1px solid var(--rule);
    text-align: left;
    white-space: nowrap;
  }
  thead th {
    background: var(--surface-2);
    color: var(--ink-3);
    font-weight: 600;
    position: sticky;
    top: 0;
  }
  .first {
    position: sticky;
    left: 0;
    z-index: 1;
    background: var(--surface);
    border-right: 1px solid var(--rule);
    min-width: 150px;
  }
  thead .first {
    background: var(--surface-2);
    z-index: 2;
  }
  .grp th {
    background: var(--surface-2);
    font-size: var(--t-sm);
    font-weight: 650;
    color: var(--ink);
    padding-top: 10px;
  }
  tbody tr:not(.grp):hover > * {
    background: var(--surface-2);
  }
  .first button {
    display: grid;
    justify-items: start;
    padding: 0;
    border: 0;
    background: none;
    cursor: default;
    font-size: var(--t-sm);
  }
  .first button b {
    font-weight: 560;
  }
  .first button:hover b {
    color: var(--accent-text);
  }
  .nm {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .addr {
    color: var(--ink-3);
    font-size: var(--t-xs);
  }
  .enc {
    font-family: var(--font);
    font-variation-settings: 'MONO' 0;
    color: var(--accent-text);
  }
  .tx {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 8px;
    max-width: 132px;
    white-space: normal;
  }
  .t {
    display: inline-flex;
    gap: 3px;
    align-items: center;
  }
  tbody tr.sel > * {
    background: var(--sel);
  }
  .pub {
    display: inline-flex;
    gap: 5px;
    align-items: center;
  }
  .who {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .bad {
    color: var(--critical);
  }
  .batch {
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .away {
    display: block;
    padding-left: 17px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .legend {
    display: flex;
    gap: 18px;
    flex-wrap: wrap;
    margin-top: 12px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .legend span {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  /* Phone list. WebKit draws its own magnifier in a search field; ours is enough. */
  .msearch {
    margin-bottom: 8px;
  }
  .msearch input {
    width: 100%;
    height: 44px;
    padding-left: 34px;
    -webkit-appearance: none;
    appearance: none;
  }
  .msearch input::-webkit-search-decoration {
    -webkit-appearance: none;
  }
  .msearch :global(svg) {
    left: 12px;
  }
  .mres {
    display: grid;
  }
  .mgrp {
    padding: 18px 0 6px;
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-2);
    border-bottom: 1px solid var(--rule);
  }
  /* A phone row: the resolver's button, and beside it the tick that selects it for a batch of checks. */
  .mitem {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    border-bottom: 1px solid var(--rule);
  }
  .mitem:active {
    background: var(--surface-2);
  }
  .mitem.sel {
    background: var(--sel);
  }
  .pick {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
  }
  .mrow {
    display: grid;
    gap: 4px;
    width: 100%;
    padding: 12px 2px;
    border: 0;
    background: none;
    text-align: left;
    color: var(--ink);
  }
  .mtop {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .mrow .nm b {
    font-size: var(--t-base);
    font-weight: 620;
  }
  .mcaps {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    margin-top: 4px;
    font-size: var(--t-xs);
  }
  .mc {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .mk {
    color: var(--ink-3);
  }
</style>
