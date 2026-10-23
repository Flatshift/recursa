<script lang="ts">
  import { app } from '../lib/store.svelte';
  import { selectKey } from '../lib/select';
  import { menu } from '../lib/menu.svelte';
  import { api, mobile } from '../lib/api';
  import { shortcut } from '../lib/keys';
  import type { Resolver, RunState } from '../lib/types';
  import { capCell } from '../lib/cells';
  import { PUBLISHED_ONLY } from '../lib/caps';
  import { HIT_SHARE, buildRows } from '../lib/verdict';
  import { status, stateLabel, stateGlyph, localNote, elsewhereNote } from '../lib/status.svelte';
  import { ago } from '../lib/format';
  import Cell from '../components/Cell.svelte';
  import Glyph from '../components/Glyph.svelte';
  import Provenance from '../components/Provenance.svelte';
  import Inspector from '../components/Inspector.svelte';
  import Shortlist, { type Pick } from '../components/Shortlist.svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import { onResolverContext } from '../lib/resolverMenu';

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
    for (const [key, lbl] of [
      ['dot', 'DoT'],
      ['doh', 'DoH'],
    ] as const) {
      const c = capCell(run, r, key);
      if (c.status === 'pass') out.push({ label: lbl, verified: c.prov === 'verified' });
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
  const name = (r: Resolver) => (r.group === 'public' || r.group === 'legacy' ? r.variant || r.provider : `${r.provider} ${r.variant}`);
  const picks = $derived<[string, Pick[]][]>(groups.map(([g, rs]) => [g, rs.map((r) => ({ id: r.id, name: name(r), sub: r.ipv4[0] ?? r.ipv6[0] ?? r.name }))]));
  function onKey(e: KeyboardEvent) {
    if ((e.target as HTMLElement)?.tagName === 'INPUT' || menu.open) return;
    if (e.key === 'Escape' && app.inspect) app.inspect = null;
    else selectKey(e);
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
    {#if split && open}
      <div class="split">
        <Shortlist groups={picks} selected={app.inspect} back="All resolvers" collapsed={narrow} onselect={(id) => (app.inspect = id)} onback={() => (app.inspect = null)} />
        {#key open.id}
          <Inspector resolver={open} run={last && inLast(open) ? last : null} rows={last && inLast(open) ? lastRows : []} onclose={() => (app.inspect = null)} />
        {/key}
      </div>
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
                <button class="mrow" class:sel={app.inspect === r.id} data-resolver={r.id} onclick={() => (app.inspect = r.id)}>
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
                  </span>
                </button>
              {/each}
            {/each}
          </div>
        {:else}
        <div class="tablewrap panel">
          <table class="sticky-head">
            <thead>
              <tr>
                <th class="first">Resolver</th>
                {#each caps as c}<th>{capLabels[c]}</th>{/each}
                <th title="Encrypted transports: DoT and DoH are checked by probes; DoQ, DNSCrypt and ODoH are as published">Encrypted</th>
                <th title="The operator's status page, where one is published">Status</th>
              </tr>
            </thead>
            {#each groups as [g, rs]}
              <tbody>
                <tr class="grp"><th colspan={3 + caps.length} class="first">{g}</th></tr>
                {#each rs as r (r.id)}
                  {@const feed = status.feedFor(r.status_feed)}
                  <tr data-resolver={r.id} class:sel={app.inspect === r.id}>
                    <th class="first" scope="row">
                      <button onclick={() => (app.inspect = r.id)}>
                        <span class="nm">
                          <b>{name(r)}</b>
                          {#if inLast(r)}<span class="v" title="Verified in your latest run"><Provenance kind="verified" /></span>{/if}
                        </span>
                        <span class="mono addr">{r.ipv4[0] ?? r.ipv6[0] ?? r.name}{#if !r.udp}<span class="enc"> · encrypted only</span>{/if}</span>
                      </button>
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
                      {:else if r.group === 'public' || r.group === 'legacy'}
                        <span class="muted" title="This operator publishes no status page">none</span>
                      {:else}
                        <span class="muted">—</span>
                      {/if}
                    </td>
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
    {#if open && mobile}
      {#key open.id}
        <Inspector resolver={open} run={last && inLast(open) ? last : null} rows={last && inLast(open) ? lastRows : []} onclose={() => (app.inspect = null)} />
      {/key}
    {/if}
  </div>
</div>

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
  .split {
    flex: 1;
    display: flex;
    min-height: 0;
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
  .mrow {
    display: grid;
    gap: 4px;
    width: 100%;
    padding: 12px 2px;
    border: 0;
    border-bottom: 1px solid var(--rule);
    background: none;
    text-align: left;
    color: var(--ink);
  }
  .mrow:active {
    background: var(--surface-2);
  }
  .mrow.sel {
    background: var(--sel);
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
