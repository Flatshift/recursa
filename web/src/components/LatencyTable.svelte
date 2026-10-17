<script lang="ts">
  import TransportChip from './TransportChip.svelte';
  import BaselineChip from './BaselineChip.svelte';
  import { app } from '../lib/store.svelte';
  import { mobile } from '../lib/api';
  import type { Row } from '../lib/verdict';
  import { CDN_ROUND_TRIPS, CONNECTION_SHARE, HIT_SHARE, MIX_LABEL, TESTS, TIMEOUT_COST_MS, WEIGHTS, timeoutCost } from '../lib/verdict';
  import type { TestKind } from '../lib/types';
  import { TEST_LABEL, ms, pct } from '../lib/format';

  const NAME: Record<TestKind, string> = { cached: 'cached', uncached: 'uncached', tld: 'TLD' };
  const weightsText = TESTS.map((k) => `${NAME[k]} ${Math.round(WEIGHTS[k] * 100)}%`).join(', ');

  const perLookupTitle = $derived(
    `What one lookup costs on average, in ms, at the ${MIX_LABEL[app.mix]} mix: ${pct(HIT_SHARE[app.mix], 0)} of lookups answered from the resolver's cache, the cached figure corrected for answers that were really fresh lookups, the timeouts' cost, and the CDN edge penalty × ${CDN_ROUND_TRIPS} round trips on the ${CONNECTION_SHARE === 0.5 ? 'half' : pct(CONNECTION_SHARE, 0)} of lookups that open a new connection. Change the mix in Settings.`,
  );

  let { rows, tests, onopen }: { rows: Row[]; tests: TestKind[]; onopen: (id: string) => void } = $props();

  type Key = 'name' | 'composite' | 'perlookup' | `${TestKind}-p50` | `${TestKind}-p95` | 'iqr' | 'success' | 'setup';
  let sortKey = $state<Key>('composite');
  let asc = $state(true);

  const ks = $derived(TESTS.filter((k) => tests.includes(k)));
  const val = (r: Row, k: Key): number | string => {
    if (k === 'name') return `${r.target.provider} ${r.target.variant} ${r.target.transport}`;
    if (k === 'composite') return r.composite ?? Infinity;
    if (k === 'perlookup') return r.perLookup ?? Infinity;
    if (k === 'success') return r.success;
    if (k === 'setup') return r.target.setup_ms ?? Infinity;
    if (k === 'iqr') {
      const d = r.dist.uncached.n ? r.dist.uncached : r.dist.cached;
      return d.p75 != null && d.p25 != null ? d.p75 - d.p25 : Infinity;
    }
    const [t, p] = k.split('-') as [TestKind, 'p50' | 'p95'];
    return r.dist[t][p] ?? Infinity;
  };
  const sorted = $derived(
    rows
      .filter((r) => r.measured)
      .sort((a, b) => {
        const x = val(a, sortKey);
        const y = val(b, sortKey);
        const c = typeof x === 'string' ? x.localeCompare(y as string) : (x as number) - (y as number);
        return asc ? c : -c;
      }),
  );
  // Connection setup only exists for TCP, DoT and DoH; a UDP-only run would show a column of dashes.
  const hasSetup = $derived(sorted.some((r) => r.target.setup_ms != null));
  function sort(k: Key) {
    if (sortKey === k) asc = !asc;
    else {
      sortKey = k;
      asc = k !== 'success';
    }
  }
  const aria = (k: Key) => (sortKey === k ? (asc ? 'ascending' : 'descending') : 'none');
  const iqr = (r: Row) => {
    const d = r.dist.uncached.n ? r.dist.uncached : r.dist.cached;
    return d.p75 != null && d.p25 != null ? d.p75 - d.p25 : null;
  };
  // Phone: the sort keys a row card shows, as chips.
  const sortKeys = $derived<[Key, string][]>([
    ['composite', 'Score'],
    ['perlookup', 'Per lookup'],
    ...ks.map((k): [Key, string] => [`${k}-p50`, TEST_LABEL[k]]),
    ['iqr', 'Jitter'],
    ['success', 'Answered'],
  ]);
</script>

{#snippet head(k: Key, text: string, title = '')}
  <th scope="col" aria-sort={aria(k)} {title}>
    <button onclick={() => sort(k)}>{text}{#if sortKey === k}<span class="arrow">{asc ? '↑' : '↓'}</span>{/if}</button>
  </th>
{/snippet}

{#if mobile}
  <!-- Phone: one card per target, the columns folded into a small grid, sorted by a wrapping row of chips. -->
  <div class="sortbar" role="group" aria-label="Sort by">
    {#each sortKeys as [k, text] (k)}
      <button class="chip" class:on={sortKey === k} aria-pressed={sortKey === k} onclick={() => sort(k)}>{text}{#if sortKey === k}<span class="arrow">{asc ? '↑' : '↓'}</span>{/if}</button>
    {/each}
  </div>
  <ol class="cards">
    {#each sorted as r (r.target.idx)}
      <li class:flag={r.intercepted} class:sel={app.inspect === r.target.resolver_id} data-resolver={r.target.resolver_id}>
        <button class="card" onclick={() => onopen(r.target.resolver_id)}>
          <span class="cname">
            <b>{r.target.provider}</b> {r.target.variant}
            <TransportChip t={r.target.transport} v6={r.target.family === 'v6'} />
            {#if r.baseline}<BaselineChip what={r.forwarder} />{/if}
            {#if r.intercepted}<span class="warn">answered locally</span>{/if}
          </span>
          <span class="cscore num" title={timeoutCost(r) >= 0.05 ? `Includes ${ms(timeoutCost(r))} for timeouts` : undefined}>{ms(r.composite, false)}<small>score</small></span>
          <span class="stats">
            <span><span class="k">Per lookup</span><span class="v num">{ms(r.perLookup, false)}</span></span>
            {#each ks as k}
              <span><span class="k">{TEST_LABEL[k]}</span><span class="v num">{ms(r.dist[k].p50, false)} <span class="dim">/ {ms(r.dist[k].p95, false)}</span></span></span>
            {/each}
            <span><span class="k">Jitter</span><span class="v num">{ms(iqr(r), false)}</span></span>
            <span><span class="k">Answered</span><span class="v num" class:bad={r.success < 0.98}>{pct(r.success)}</span></span>
            {#if hasSetup}<span><span class="k">Connect</span><span class="v num dim">{r.target.setup_ms != null ? ms(r.target.setup_ms, false) : '—'}</span></span>{/if}
            <span><span class="k">Hops</span><span class="v num" class:bad={r.intercepted && (r.target.hops ?? 99) <= 1}>{r.target.hops ?? '—'}</span></span>
          </span>
        </button>
      </li>
    {/each}
  </ol>
  <p class="foot">All times in milliseconds; each test shows its median / p95. Medians and percentiles use answered queries only; timeouts, errors and a filter’s block answers count against “Answered”.</p>
{:else}
<!-- svelte-ignore a11y_no_noninteractive_tabindex (scrollable region must be keyboard-reachable) -->

<div class="wrap" role="region" aria-label="Latency by target" tabindex="0">
  <table>
    <thead>
      <tr>
        {@render head('name', 'Target')}
        {@render head('composite', 'Score', `Speed score, in ms: ${weightsText}, each median pulled halfway toward its p95, plus a quarter of its standard deviation, plus ${TIMEOUT_COST_MS / 1000} s for each timeout spread over every query`)}
        {@render head('perlookup', 'Per lookup', perLookupTitle)}
        {#each ks as k}
          {@render head(`${k}-p50`, `${TEST_LABEL[k]} p50`)}
          {@render head(`${k}-p95`, 'p95')}
        {/each}
        {@render head('iqr', 'Jitter', 'Interquartile range (p75 − p25) of uncached lookups')}
        {@render head('success', 'Answered')}
        {#if hasSetup}{@render head('setup', 'Connect', 'Time to open the connection (TCP connect, plus the TLS handshake for DoT/DoH). Not part of query latency.')}{/if}
        <th scope="col" title="Router hops the plain-DNS answers crossed, from their IP TTL. 0–1 for a public resolver means a device on your network answered.">Hops</th>
      </tr>
    </thead>
    <tbody>
      {#each sorted as r (r.target.idx)}
        <tr class:flag={r.intercepted} data-resolver={r.target.resolver_id} class:sel={app.inspect === r.target.resolver_id}>
          <th scope="row">
            <button onclick={() => onopen(r.target.resolver_id)}>
              <b>{r.target.provider}</b> {r.target.variant}
              <TransportChip t={r.target.transport} v6={r.target.family === 'v6'} />
              {#if r.baseline}<BaselineChip what={r.forwarder} />{/if}
              {#if r.intercepted}<span class="warn">answered locally</span>{/if}
            </button>
          </th>
          <td class="num strong" title={timeoutCost(r) >= 0.05 ? `Includes ${ms(timeoutCost(r))} for timeouts` : undefined}>{ms(r.composite, false)}</td>
          <td
            class="num"
            title={r.cdnTerm == null ? "No CDN term: edges weren't mapped for this resolver" : r.cdnTerm >= 0.05 ? `Includes ${ms(r.cdnTerm)} for CDN edges` : undefined}>{ms(r.perLookup, false)}</td>
          {#each ks as k}
            <td class="num" title={k === 'cached' && r.target.cache_hits != null ? `${pct(r.target.cache_hits, 0)} of these answers came from the resolver's cache` : undefined}>{ms(r.dist[k].p50, false)}</td>
            <td class="num dim">{ms(r.dist[k].p95, false)}</td>
          {/each}
          <td class="num">{ms(iqr(r), false)}</td>
          <td class="num" class:bad={r.success < 0.98}>{pct(r.success)}</td>
          {#if hasSetup}<td class="num dim">{r.target.setup_ms != null ? ms(r.target.setup_ms, false) : '—'}</td>{/if}
          <td class="num" class:bad={r.intercepted && (r.target.hops ?? 99) <= 1}>{r.target.hops ?? '—'}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
<p class="foot">All times in milliseconds. Medians and percentiles use answered queries only; timeouts, errors and a filter’s block answers count against “Answered”. Uncached answers that came from a cache are left out.</p>
{/if}

<style>
  /* Its own scroll region (both ways), so the header row stays put. */
  .wrap {
    flex: 0 1 auto;
    min-height: 200px;
    overflow: auto;
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--surface);
  }
  table {
    width: 100%;
    border-collapse: separate;
    border-spacing: 0;
    font-size: var(--t-xs);
  }
  th,
  td {
    padding: 7px 12px;
    border-bottom: 1px solid var(--rule);
    white-space: nowrap;
    text-align: right;
  }
  th[scope='row'],
  thead th:first-child {
    text-align: left;
    position: sticky;
    left: 0;
    background: var(--surface);
    border-right: 1px solid var(--rule);
  }
  thead th {
    background: var(--surface-2);
    color: var(--ink-3);
    font-weight: 600;
  }
  thead th:first-child {
    background: var(--surface-2);
    z-index: 1;
  }
  thead button {
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    cursor: default;
  }
  .arrow {
    margin-left: 3px;
  }
  tbody tr:last-child > * {
    border-bottom: 0;
  }
  tbody tr:hover > * {
    background: var(--surface-2);
  }
  tbody tr.sel > * {
    background: var(--sel);
  }
  tr.flag > * {
    background: color-mix(in oklab, var(--warn-soft) 55%, var(--surface));
  }
  th[scope='row'] button {
    padding: 0;
    border: 0;
    background: none;
    cursor: default;
    font-weight: 400;
    color: var(--ink-2);
    font-size: var(--t-xs);
  }
  th[scope='row'] b {
    color: var(--ink);
    font-weight: 600;
  }
  .warn {
    margin-left: 6px;
    padding: 0 6px;
    border-radius: 8px;
    background: var(--warn-soft);
    color: var(--ink);
  }
  .strong {
    font-weight: 620;
  }
  .dim {
    color: var(--ink-3);
  }
  .bad {
    color: var(--critical);
    font-weight: 600;
  }
  .foot {
    margin-top: 8px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  /* Every sort key in view: the chips wrap rather than scroll sideways. */
  .sortbar {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0 0 10px;
  }
  .chip {
    flex: none;
    min-height: 36px;
    padding: 0 12px;
    border: 1px solid var(--rule);
    border-radius: 18px;
    background: var(--surface);
    color: var(--ink-2);
    font-size: var(--t-sm);
    white-space: nowrap;
  }
  .chip.on {
    border-color: transparent;
    background: var(--side-sel);
    color: var(--ink);
    font-weight: 620;
  }
  .cards {
    flex: none;
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .card {
    width: 100%;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px 12px;
    padding: 10px 12px 12px;
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--surface);
    text-align: left;
    cursor: default;
  }
  .card:active {
    background: var(--surface-2);
  }
  li.sel .card {
    background: var(--sel);
  }
  li.flag .card {
    background: color-mix(in oklab, var(--warn-soft) 55%, var(--surface));
  }
  .cname {
    min-width: 0;
    font-size: var(--t-base);
    color: var(--ink-2);
    line-height: 1.3;
  }
  .cname b {
    color: var(--ink);
    font-weight: 620;
  }
  .cname .warn {
    display: inline-block;
    margin: 2px 0 0;
  }
  .cscore {
    align-self: start;
    display: grid;
    justify-items: end;
    font-size: var(--t-lg);
    font-weight: 620;
    line-height: 1.1;
  }
  .cscore small {
    font-size: var(--t-2xs);
    font-weight: 500;
    color: var(--ink-3);
    letter-spacing: 0.02em;
  }
  .stats {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 6px 8px;
    margin: 0;
  }
  .stats .k {
    display: block;
    font-size: var(--t-2xs);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .stats .v {
    display: block;
    margin: 1px 0 0;
    font-size: var(--t-sm);
    white-space: nowrap;
  }
</style>
