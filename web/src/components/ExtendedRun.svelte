<script lang="ts">
  // An Extended run, live or finished: the standings, the chart of every 15-second bin, and how the run was made.
  // The engine decides the order, the gaps and the calls; this page only lays them out.
  import type { RunState } from '../lib/types';
  import { label, ms, plural, timeOfDay } from '../lib/format';
  import { CALL_LABEL, GAP_LABEL, GROUP_LABEL, chartOf, gapSeconds, groupsOf, minutesLeft, rangeOf, sentOf, verdictOf } from '../lib/extended';
  import OverTime from './OverTime.svelte';
  import ExtEcdf from './ExtEcdf.svelte';
  import ResolverMark from './ResolverMark.svelte';

  let { run }: { run: RunState } = $props();

  const ext = $derived(run.extended);
  const res = $derived(ext?.result);
  const running = $derived(run.status === 'running');
  const final = $derived(!!res?.final);
  const names = $derived(run.targets.map((t) => label(t.provider, t.variant)));
  const isCurrent = (t: number) => run.resolvers.find((r) => r.id === run.targets[t]?.resolver_id)?.group === 'system';
  const clock = (t: number) => timeOfDay(run.started_at + t);
  const f1 = (v: number | null | undefined) => (v == null ? '—' : v.toFixed(1));
  const sent = $derived(ext ? sentOf(ext) : 0);
  const pct = $derived(ext && ext.total ? Math.round((sent / ext.total) * 100) : 0);
  const left = $derived(ext ? minutesLeft(ext) : 0);

  const chart = $derived(ext ? chartOf(ext, names) : null);
  const verdict = $derived(res ? verdictOf(res.rows, res.no_verdict) : null);
  const rows = $derived(res?.rows ?? []);
  const lead = $derived(rows[0]);
  const next = $derived(rows[1]);
  const hasCalls = $derived(rows.some((r) => r.call));
  const sleepS = $derived(ext ? gapSeconds(ext.gaps, 'sleep') : 0);
  const lastBin = $derived(ext?.bins.at(-1));
  const removedFrom = $derived(ext?.bins.find((b) => b.removed));

  let cursor = $state<number | null>(null);
  let binsOpen = $state(false);

  const gap = (v: { diff: number; lo: number; hi: number } | null) => (v ? `${v.diff > 0 ? '+' : ''}${f1(v.diff)}` : '');
  const mins = (s: number) => (s >= 90 ? `${Math.round(s / 60)} minutes` : `${s} seconds`);
  // Beaten resolvers, as the verdict's second line names them.
  const beaten = $derived(rows.filter((r) => r.call === 'slower' && r.vs));
  const SOURCE = $derived(
    !ext
      ? ''
      : ext.names.source === 'bundled'
        ? 'Recursa’s own list stood in, because the Tranco list could not be reached'
        : `the Tranco list ${ext.names.list_id}, dated ${ext.names.list_date}, ${ext.names.source === 'tranco' ? 'downloaded when the run started' : 'kept from an earlier run'}`,
  );
</script>

{#snippet who(t: number)}
  <span class="who"><ResolverMark {t} /> <b>{names[t]}</b>{#if isCurrent(t)} <span class="chip">Current</span>{/if}</span>
{/snippet}

{#snippet legend()}
  <span class="legend">{#each names as _, t}<span>{@render who(t)}</span>{/each}</span>
{/snippet}

{#if !ext || !res}
  <div class="page"><p class="muted" role="status">Waiting for the first results.</p></div>
{:else if running}
  <div class="page">
    <div class="runhead">
      {#if run.phase === 'extended'}
        <h1 class="h">About {left} min left</h1>
        <div>
          <div class="progress" role="progressbar" aria-label="Lookups sent" aria-valuemin="0" aria-valuemax="100" aria-valuenow={pct}><i style:width="{pct}%"></i></div>
          <p class="count"><span class="num">{sent.toLocaleString()} of {ext.total.toLocaleString()}</span> lookups sent to each resolver, <span class="num">{pct}%</span>. Started <span class="num">{timeOfDay(run.started_at)}</span>.</p>
        </div>
      {:else}
        <h1 class="h">Getting ready</h1>
        <p class="count">The first bins appear here once the lookups start.</p>
      {/if}
    </div>

    {#if rows.length}
      <div class="sheet">
        <table>
          <thead>
            <tr><th>Resolver</th><th class="n">Median so far, ms</th><th class="n">Behind the lowest so far, ms</th><th class="n">Answered</th><th class="n">Lost</th></tr>
          </thead>
          <tbody>
            {#each rows as r}
              <tr>
                <td data-l="Resolver">{@render who(r.t)}</td>
                <td class="n" data-l="Median so far">{f1(r.p50)}</td>
                <td class="n" data-l="Behind the lowest so far">{#if r.vs}<span class="num">{gap(r.vs)}</span> <span class="sub">({rangeOf(r.vs)})</span>{:else}—{/if}</td>
                <td class="n" data-l="Answered">{(r.sent - r.lost).toLocaleString()}</td>
                <td class="n" data-l="Lost">{r.lost.toLocaleString()}</td>
              </tr>
            {/each}
          </tbody>
        </table>
        <p class="foot">
          Not final.
          {#if next?.vs}
            {names[lead.t]} leads {names[next.t]} by <span class="num">{f1(next.vs.diff)} ms</span> so far (<span class="num">{rangeOf(next.vs)}</span>), and the order can still change.
          {:else}
            The gap and its range appear once 10 bins are in.
          {/if}
          No call is made until the run ends.
        </p>
      </div>
    {/if}

    <div class="card">
      <header><h2>The run so far</h2><span class="how"><span class="num">{ext.bin_s}</span>-second bins, <span class="num">{ext.per_bin}</span> queries to each resolver in every bin</span>{@render legend()}</header>
      {#if chart}<OverTime series={chart.series} gaps={chart.gaps} bind:cursor formatTime={clock} unit="{ext.bin_s}-second bin" />{/if}
    </div>
  </div>
{:else}
  <div class="page">
    {#if final && verdict}
      <div class="verdict">
        <div>
          <p class="kind">{verdict.kind}</p>
          <h1 class="h">{#if verdict.kind === 'No verdict'}Run it again{:else if verdict.head.length === 1}{names[verdict.head[0]]}{:else}{names[verdict.head[0]]} or {names[verdict.head[1]]}{/if}</h1>
        </div>
        <div>
          {#if verdict.kind === 'No verdict'}
            <p>About {mins(res.removed_s)} were removed because nothing answered, which is too much of the run to call.</p>
          {:else if lead && next?.vs}
            {@const fewer = lead.lost <= next.lost ? lead : next}
            {@const more = fewer === lead ? next : lead}
            {#if verdict.kind === 'Practical tie for first'}
              <p>
                Their medians are <span class="num">{f1(Math.abs(next.vs.diff))} ms</span> apart (<span class="num">{rangeOf(next.vs)}</span>), under the <span class="num">{ms(res.margin)}</span> this test counts as a real difference.{#if fewer.lost !== more.lost}
                  {names[fewer.t]} loses fewer queries: <span class="num">{fewer.lost}</span> against <span class="num">{more.lost}</span>.{/if}
              </p>
            {:else if verdict.kind === 'Lowest median'}
              <p>
                {names[lead.t]} has the lowest median, <span class="num">{f1(Math.abs(next.vs.diff))} ms</span> ahead of the next (<span class="num">{rangeOf(next.vs)}</span>){#if res.loss_flag}, but it lost <span class="num">{lead.loss_pct.toFixed(1)}%</span> of queries against <span class="num">{next.loss_pct.toFixed(1)}%</span> for {names[next.t]}.{:else}.{/if}
                Differences under {ms(res.margin)} count as a tie.
              </p>
            {:else}
              <p>Cannot separate {names[lead.t]} and {names[next.t]}: the gap could be <span class="num">{rangeOf(next.vs)} ms</span>, and {ms(res.margin)} is the line. Run it longer.</p>
            {/if}
            <p>
              {#if beaten.length}{verdict.kind === 'Lowest median' ? names[lead.t] + ' beat' : 'Both beat'} {#each beaten as r, i}{i ? ' and ' : ''}{names[r.t]}{#if isCurrent(r.t)}, your current resolver,{/if} by about <span class="num">{Math.round(r.vs?.diff ?? 0)} ms</span>{/each} a lookup.{' '}{/if}Measured from this network between <span class="num">{clock(0)}</span> and <span class="num">{clock(lastBin?.end ?? 0)}</span>{#if sleepS}, apart from the {mins(sleepS)} the computer was asleep{/if}{#if res.removed_s}{sleepS ? ' and' : ', apart from the'} {mins(res.removed_s)} when most queries were lost{/if}; another hour or network can order them differently.
            </p>
          {/if}
        </div>
      </div>
    {:else}
      <div class="verdict">
        <div>
          <p class="kind">Not final</p>
          <h1 class="h">Stopped early</h1>
        </div>
        <div>
          <p>This run was stopped after <span class="num">{sent.toLocaleString()} of {ext.total.toLocaleString()}</span> lookups to each resolver, so no call is made. The figures below are what was measured so far, and the order can change with the rest of the run.</p>
        </div>
      </div>
    {/if}

    <div class="sheet">
      <table>
        <thead>
          <tr>
            <th>Resolver</th><th class="n">Median</th><th class="n">9 in 10 within</th><th class="n">95 in 100 within</th><th class="n">Lost</th><th class="n">Median behind the lowest</th>{#if hasCalls}<th>Call</th>{/if}
          </tr>
        </thead>
        <tbody>
          {#each rows as r}
            <tr>
              <td data-l="Resolver">{@render who(r.t)}</td>
              <td class="n" data-l="Median">{f1(r.p50)}</td>
              <td class="n" data-l="9 in 10 within">{f1(r.p90)}</td>
              <td class="n" data-l="95 in 100 within">{f1(r.p95)}</td>
              <td class="n" data-l="Lost">{r.lost.toLocaleString()} <span class="sub">of {r.sent.toLocaleString()}</span></td>
              <td class="n" data-l="Median behind {names[lead?.t ?? 0]}">{#if r.vs}<span class="num">{gap(r.vs)}</span> <span class="sub">({rangeOf(r.vs)})</span>{:else}—{/if}</td>
              {#if hasCalls}<td data-l="Call">{#if r.call}<span class="pill {r.call}">{CALL_LABEL[r.call]}</span>{/if}</td>{/if}
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="foot">
        Times in milliseconds. A lost query counts as slower than any answer. The range beside each gap holds about 9 times in 10.
        {#if !final}Not final: this run was stopped, so no call is made.{/if}
        {#if res.removed_s && removedFrom}The {res.removed_s} seconds from {clock(removedFrom.at)} are left out for every resolver: each lost half or more of its queries then, which points at the link.{/if}
        {#each ext.gaps.filter((g) => g[2] === 'sleep') as g}The computer was asleep from {clock(g[0])} to {clock(g[1])}. Nothing was sent then, and that stretch is in no figure.{/each}
      </p>
    </div>

    <div class="card">
      <header><h2>Response time over the run</h2><span class="how"><span class="num">{ext.bin_s}</span>-second bins, <span class="num">{ext.per_bin}</span> queries to each resolver in every bin · log scale</span>{@render legend()}</header>
      {#if chart}<OverTime series={chart.series} gaps={chart.gaps} bind:cursor formatTime={clock} unit="{ext.bin_s}-second bin" />{/if}
    </div>

    <div class="two">
      <div class="card">
        <header><h2>How fast answers came back</h2><span class="how">Share of queries answered within the time below, in ms</span>{@render legend()}</header>
        <ExtEcdf ecdf={res.ecdf} {names} />
        <p class="hint">A curve further left is faster; a steeper one is steadier. A lost query never comes back, so each curve stops short of 100% by its losses.</p>
      </div>

      <div class="card">
        <header><h2>How this run was made</h2></header>
        <dl>
          <dt>Resolvers</dt><dd>{names.join(', ')}</dd>
          <dt>Transport</dt><dd>Plain DNS over UDP, IPv4</dd>
          <dt>Pace</dt>
          <dd>Each name goes to every resolver in the same <span class="num">{ext.gap_ms} ms</span> slot, in a random order each time: about <span class="num">{(1000 / ext.gap_ms).toFixed(1)}</span> queries a second to each resolver</dd>
          <dt>No answer</dt><dd>After <span class="num">{ext.timeout_ms / 1000} s</span> a query counts as lost, and is ranked as slower than any answer</dd>
          <dt>Names</dt>
          <dd><span class="num">{ext.names.common}</span> common names, each asked three times, and <span class="num">{ext.names.tail.toLocaleString()}</span> less common sites, from {SOURCE}</dd>
          <dt>Seed</dt><dd><span class="num">{ext.names.seed}</span>, which fixes the names and their order</dd>
          <dt>Comparison</dt>
          <dd>The gap between two medians. Its range comes from the gap in ten equal stretches of the run and holds about <span class="num">9</span> times in <span class="num">10</span>. Gaps under <span class="num">{ms(res.margin)}</span> count as a tie</dd>
          <dt>Left out</dt>
          <dd>
            {#if res.removed_s && removedFrom}<span class="num">{res.removed_s} s</span> from <span class="num">{clock(removedFrom.at)}</span>, when every resolver lost half or more of its queries{:else}Nothing{/if}
          </dd>
          {#each ext.gaps as g}
            <dt>{GAP_LABEL[g[2]]}</dt><dd><span class="num">{clock(g[0])} to {clock(g[1])}</span>. Nothing was sent then</dd>
          {/each}
          {#if res.dropped_names}<dt>Dropped names</dt><dd>{plural(res.dropped_names, 'name')} that no resolver answered, left out of every figure</dd>{/if}
          {#if rows.some((r) => r.blocked)}
            <dt>Blocked names</dt><dd>{#each rows.filter((r) => r.blocked) as r, i}{i ? ', ' : ''}{names[r.t]} <span class="num">{r.blocked}</span>{/each}: answered with a block or an error another resolver did not give, so left out of that resolver’s timing only</dd>
          {/if}
          <dt>Bins</dt><dd><span class="num">{ext.bins.length}</span> bins of <span class="num">{ext.bin_s} s</span>, <span class="num">{ext.per_bin}</span> queries to each resolver in every bin</dd>
        </dl>
      </div>
    </div>

    {#if groupsOf(res.groups).length}
      <div class="card">
        <header><h2>By kind of name</h2><span class="how">Median in ms, and queries lost</span></header>
        <div class="scroll">
          <table>
            <thead>
              <tr><th>Kind</th><th class="n">Names</th>{#each rows as r}<th class="n">{@render who(r.t)}</th>{/each}</tr>
            </thead>
            <tbody>
              {#each groupsOf(res.groups) as g}
                <tr>
                  <td>{GROUP_LABEL[g.key] ?? g.key}</td>
                  <td class="n">{g.count.toLocaleString()}</td>
                  {#each rows as r}<td class="n"><span class="num">{f1(g.r[r.t]?.p50)}</span> <span class="sub">· {g.r[r.t]?.lost ?? 0} lost</span></td>{/each}
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </div>
    {/if}

    <details class="card" bind:open={binsOpen}>
      <summary>Every bin as a table<span><span class="num">{ext.bins.length}</span> rows, times in milliseconds</span></summary>
      {#if binsOpen}
        <div class="scroll">
          <table>
            <thead>
              <tr><th>Bin starts</th>{#each rows as r}<th class="n">{names[r.t]} median</th><th class="n">95 in 100</th><th class="n">Lost</th>{/each}</tr>
            </thead>
            <tbody>
              {#each ext.bins as b}
                <tr>
                  <td class="num">{clock(b.at)}{b.removed ? ', left out' : ''}</td>
                  {#each rows as r}<td class="n">{f1(b.r[r.t]?.p50)}</td><td class="n">{f1(b.r[r.t]?.p95)}</td><td class="n">{b.r[r.t]?.lost ?? 0}</td>{/each}
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </details>
  </div>
{/if}

<style>
  .page {
    display: grid;
    gap: 20px;
    padding: 4px 0 32px;
    max-width: 62rem;
  }
  .muted,
  .count,
  .hint {
    margin: 0;
    font-size: var(--t-sm);
    color: var(--ink-2);
    line-height: 1.5;
  }
  .h {
    margin: 0;
    font-size: var(--t-xl);
    font-weight: 620;
    color: var(--ink);
  }
  .runhead {
    display: grid;
    gap: 10px;
  }
  .progress {
    height: 6px;
    border-radius: 3px;
    background: var(--rule);
    overflow: hidden;
    margin-bottom: 6px;
  }
  .progress i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 300ms var(--ease-out);
  }
  .verdict {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr);
    gap: 24px;
    padding: 18px 20px;
    border: 1px solid var(--rule);
    border-radius: 10px;
    background: var(--surface);
  }
  .verdict p {
    margin: 0 0 8px;
    font-size: var(--t-base);
    color: var(--ink-2);
    line-height: 1.55;
  }
  .verdict .kind {
    font-size: var(--t-sm);
    font-weight: 600;
    color: var(--ink-2);
  }
  .sheet,
  .card {
    border: 1px solid var(--rule);
    border-radius: 10px;
    background: var(--surface);
    padding: 14px 16px;
  }
  .card header {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 14px;
    margin-bottom: 10px;
  }
  .card h2 {
    margin: 0;
    font-size: var(--t-md);
    font-weight: 620;
    color: var(--ink);
  }
  .how {
    flex: 1 1 auto;
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    font-size: var(--t-sm);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--t-sm);
  }
  th,
  td {
    padding: 8px 10px;
    text-align: left;
    border-bottom: 1px solid var(--rule);
    color: var(--ink);
    vertical-align: baseline;
  }
  th {
    font-weight: 600;
    color: var(--ink-2);
  }
  th.n,
  td.n {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .sub {
    color: var(--ink-3);
  }
  .foot {
    margin: 10px 0 0;
    font-size: var(--t-sm);
    color: var(--ink-2);
    line-height: 1.5;
  }
  .who {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
  }
  .who b {
    font-weight: 600;
    color: var(--ink);
  }
  .chip {
    font-size: var(--t-xs);
    padding: 1px 6px;
    border: 1px solid var(--rule);
    border-radius: 4px;
    color: var(--ink-2);
  }
  .pill {
    display: inline-block;
    padding: 1px 8px;
    border: 1px solid var(--rule);
    border-radius: 999px;
    font-size: var(--t-xs);
    color: var(--ink);
  }
  .pill.lead {
    font-weight: 600;
    border-color: var(--ink-2);
  }
  .pill.slower {
    color: var(--ink-2);
  }
  .pill.unclear {
    border-style: dashed;
  }
  .two {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(22rem, 1fr));
    gap: 20px;
    align-items: start;
  }
  dl {
    margin: 0;
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 8px 16px;
    font-size: var(--t-sm);
  }
  dt {
    color: var(--ink-2);
  }
  dd {
    margin: 0;
    color: var(--ink);
    line-height: 1.45;
  }
  summary {
    cursor: pointer;
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: var(--t-md);
    font-weight: 620;
    color: var(--ink);
  }
  summary span {
    font-size: var(--t-sm);
    font-weight: 400;
    color: var(--ink-2);
  }
  summary:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  details .scroll {
    margin-top: 10px;
    overflow: auto;
    max-height: 28rem;
  }
  @media (max-width: 640px) {
    .verdict {
      grid-template-columns: 1fr;
      gap: 10px;
    }
    .two {
      grid-template-columns: 1fr;
    }
    dl {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    dd {
      margin-bottom: 8px;
    }
  }
</style>
