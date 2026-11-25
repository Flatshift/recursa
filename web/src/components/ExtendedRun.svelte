<script lang="ts">
  // An Extended run, live or finished: the standings, the chart of every 15-second bin, and how the run was made.
  // The engine decides the order, the gaps and the calls; this page only lays them out.
  import type { ExtendedState, RunState } from '../lib/types';
  import type { SlowKind } from '../lib/overtime';
  import { SLOW_FAMILY } from '../lib/overtime';
  import { isPrivate, label, ms, plural, timeOfDay } from '../lib/format';
  import { CALL_LABEL, GAP_LABEL, GROUP_LABEL, REGION_LABEL, asksOf, chartOf, chartsOf, gapSeconds, groupsOf, minutesLeft, plannedSlots, rangeOf, sentOf, slowsOf, verdictOf } from '../lib/extended';
  import { isMac } from '../lib/keys';
  import { mobile } from '../lib/api';
  import OverTime from './OverTime.svelte';
  import ExtEcdf from './ExtEcdf.svelte';
  import ResolverMark from './ResolverMark.svelte';

  let { run }: { run: RunState } = $props();

  const ext = $derived(run.extended);
  const res = $derived(ext?.result);
  const region = $derived(REGION_LABEL[ext?.names.region ?? '']);
  const running = $derived(run.status === 'running');
  const final = $derived(!!res?.final);
  // Read through one step, so the answer count, which changes several times a second, leaves the charts alone.
  const targets = $derived(run.targets);
  const link = $derived(run.link);
  const names = $derived(targets.map((t) => label(t.provider, t.variant)));
  const isCurrent = (t: number) => run.resolvers.find((r) => r.id === run.targets[t]?.resolver_id)?.group === 'system';
  const clock = (t: number) => timeOfDay(run.started_at + t);
  const f1 = (v: number | null | undefined) => (v == null ? '—' : v.toFixed(1));
  const live = $derived(running && run.phase === 'extended' ? run : undefined);
  const sent = $derived(ext ? sentOf(ext, live) : 0);
  const pct = $derived(ext && ext.total ? Math.round((sent / ext.total) * 100) : 0);
  const left = $derived(ext ? minutesLeft(ext, live) : 0);
  // While it runs, both charts are drawn to the whole planned span, so the warm-up stays a sliver at the start.
  const perCold = $derived(ext?.cold_bin_s ? ext.cold_bin_s / ext.bin_s : 1);
  const coldSlots = $derived(ext && live ? Math.ceil(plannedSlots(ext) / perCold) : undefined);

  // Runs from before the split charts have the one chart of every lookup.
  const charts = $derived(ext ? chartsOf(ext, names) : null);
  const split = $derived(!!charts);
  const chart = $derived(ext && !charts ? chartOf(ext, names) : null);
  const hasRepeat = $derived(!!charts?.repeat.series.some((s) => s.cells.some((c) => c)));
  const hasCold = $derived(!!charts?.cold.series.some((s) => s.cells.some((c) => c)));
  // The caches fill over the first eighth of the run: how far, and the minutes to go at the run's own pace.
  const fillN = $derived(ext ? ext.total / 8 : 1);
  const fill = $derived(Math.min(1, sent / fillN));
  const fillLeft = $derived(ext ? Math.max(1, Math.ceil(((fillN - fill * fillN) * ext.bin_s) / ext.per_bin / 60)) : 1);
  // The liquid's surface: four swells over twice the vessel's width, so sliding it by half loops without a seam.
  const WAVE = 'M0 4 Q12.5 -2 25 4 T50 4 T75 4 T100 4 T125 4 T150 4 T175 4 T200 4';
  // A few bubbles rising through the liquid, spread by the golden ratio, each over its own stretch of the depth so
  // they mix through the body: a stretch of a third to two thirds of it, whose foot falls anywhere from that far down
  // to the floor. One in four is tinted by the kind of name being asked: blue for a common name's first ask, orange for
  // a name asked once, in the warm-up's own proportion. While the liquid is shallow fewer show, so they don't crowd
  // into a line; all of them by the time it is half full.
  const onceTinted = $derived(ext && ext.total ? Math.max(1, Math.round((4 * ext.names.tail) / ext.total)) : 1);
  const bubbleCount = $derived(Math.min(16, Math.max(4, Math.round(fill * 32))));
  const bubbles = $derived(
    Array.from({ length: 16 }, (_, i) => {
      const tint = i % 4 === 3 ? ((i - 3) / 4 < onceTinted ? 'uncached' : 'cached') : '';
      const len = 35 + ((i * 0.56984) % 1) * 30;
      const foot = len + ((i * 0.754878) % 1) * (100 - len);
      return {
        x: 3 + ((i * 0.618034) % 1) * 94,
        top: foot - len,
        bot: 100 - foot,
        size: tint ? 4 : 3 + (i % 2),
        dur: 6 + ((i * 7) % 6),
        delay: -((i * 2.3) % 9),
        sway: 2 + (i % 3),
        tint,
      };
    }),
  );
  const shownGaps = $derived(charts?.repeat.gaps ?? chart?.gaps ?? []);
  // Stretches where the resolvers slowed together: marked on both charts, and still counted. While the run sends, its
  // last bin is still filling and too thin to call slow.
  const slows = $derived(
    ext && split
      ? slowsOf(running ? { ...ext, bins: ext.bins.slice(0, -1) } : ext, targets.map((t) => isPrivate(t.ip)), link && !('state' in link) ? link : undefined)
      : [],
  );
  const slowFamilies = $derived([...new Set(slows.map((s) => SLOW_FAMILY[s.kind]))]);
  const machine = mobile ? 'iPhone' : isMac ? 'Mac' : 'computer';
  const SLOW_KEY = { net: 'Wi-Fi or local network', host: `this ${machine}`, up: 'internet link', any: 'cause not known' };
  const SLOW_WHY: Record<SlowKind, string> = {
    busy: `Likely cause: this ${machine} was busy.`,
    traffic: `Likely cause: other traffic on this ${machine}.`,
    wifi: 'Likely cause: AirDrop or another Apple device feature using the Wi-Fi radio.',
    local: 'Likely cause: the Wi-Fi or the local network.',
    upstream: 'The resolver on this network kept its pace, so the likely cause is the internet link.',
    shared: 'Nothing this run recorded says why.',
  };
  const verdict = $derived(res ? verdictOf(res.rows, res.no_verdict) : null);
  const rows = $derived(res?.rows ?? []);
  const lead = $derived(rows[0]);
  const next = $derived(rows[1]);
  const hasCalls = $derived(rows.some((r) => r.call));
  // The once-asked names alone; runs from before the cold ranking have none.
  const cold = $derived(res?.cold ?? []);
  const coldLead = $derived(cold[0]);
  const coldHasCalls = $derived(cold.some((r) => r.call));
  const coldVerdict = $derived(coldHasCalls ? verdictOf(cold, false) : null);
  // Once-asked names in 10 of the lookups: most of a run before the cold ranking, a fifth after it.
  const tailTenths = $derived(ext ? Math.round((ext.names.tail / ext.total) * 10) : 0);
  // One kind of name's median for a resolver, shown beside the median of everything.
  const kindP50 = (key: string, t: number) => res?.groups.find((g) => g.key === key)?.r[t]?.p50;
  // What each chart reads out before a bin is picked: the medians the tables use.
  const spanLabel = $derived(running ? 'So far' : final ? 'Whole run' : 'Until it stopped');
  const repeatTotals = $derived(names.map((_, t) => kindP50('common_later', t)));
  const coldTotals = $derived(names.map((_, t) => cold.find((r) => r.t === t)?.p50));
  const allTotals = $derived(names.map((_, t) => rows.find((r) => r.t === t)?.p50));
  const sleepS = $derived(ext ? gapSeconds(ext.gaps, 'sleep') : 0);
  const lastBin = $derived(ext?.bins.at(-1));
  const removedFrom = $derived(ext?.bins.find((b) => b.removed));

  let cursor = $state<number | null>(null);
  let cursorCold = $state<number | null>(null);
  let binsOpen = $state(false);

  const gap = (v: { diff: number; lo: number; hi: number } | null) => (v ? `${v.diff > 0 ? '+' : ''}${f1(v.diff)}` : '');
  const mins = (s: number) => (s >= 90 ? `${Math.round(s / 60)} minutes` : `${s} seconds`);
  // Beaten resolvers, as the verdict's second line names them.
  const beaten = $derived(rows.filter((r) => r.call === 'slower' && r.vs));
  // Left-out bins come in stretches: the chart has already merged neighbours.
  const removedRuns = $derived(shownGaps.filter((g) => g.kind === 'removed').length);
  const endNotes = $derived(run.notes.filter((n) => n.level === 'warn'));
</script>

{#snippet who(t: number)}
  <span class="who"><ResolverMark {t} /> <b>{names[t]}</b>{#if isCurrent(t)} <span class="chip">Current</span>{/if}</span>
{/snippet}

{#snippet legend()}
  <span class="legend">{#each names as _, t}<span>{@render who(t)}</span>{/each}</span>
{/snippet}

{#snippet key()}
  <p class="chartkey" aria-hidden="true">
    <span><i class="sw-med"></i>Median</span>
    <span><i class="sw-band"></i>Middle half, nine in ten</span>
    <span><i class="sw-loss"><b class="w"></b><b class="s"></b><b class="c"></b></i>Lost queries in a bin: a few, 1 in 10 or more, 1 in 4 or more</span>
    {#if shownGaps.some((g) => g.kind !== 'warmup')}<span><i class="sw-gap"></i>In no figure: left out, asleep or stalled</span>{/if}
    {#if split}<span><i class="sw-warm"></i>Cache filling: the first ask of each common name, not ranked</span>{/if}
    {#each slowFamilies as f}<span><i class="sw-slow"><b class={f}></b></i>Slow together, still counted: {SLOW_KEY[f]}</span>{/each}
  </p>
{/snippet}

{#snippet chartBody(ext: ExtendedState)}
  {#if charts}
    <h3 class="sh">Repeat lookups</h3>
    <p class="how note">common names after their first ask · {ext.bin_s}-second bins</p>
    {#if hasRepeat}
      <OverTime series={charts.repeat.series} gaps={charts.repeat.gaps} {slows} slots={coldSlots != null ? coldSlots * perCold : undefined} bind:cursor formatTime={clock} unit="{ext.bin_s}-second bin" totals={repeatTotals} totalsLabel={spanLabel} />
    {:else}
      <div class="vessel" role="progressbar" aria-label="Caches filling" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(fill * 100)}>
        <div class="liquid" style:transform="translateY({(1 - fill) * 100}%)">
          <div class="bubbles" class:moving={running} style:height="max(0px, calc({fill * 100}% - 10px))" aria-hidden="true">
            {#each bubbles as b, i}
              <span class="b" style:left="{b.x}%" style:top="{b.top}%" style:bottom="{b.bot}%" style:--d="{b.dur}s" style:--delay="{b.delay}s"><i class="d {b.tint}" class:off={i >= bubbleCount} style:--s="{b.size}px" style:--sw="{b.sway}s"></i></span>
            {/each}
          </div>
          <svg class="crest" class:moving={running} viewBox="0 0 200 8" preserveAspectRatio="none" aria-hidden="true">
            <path class="body" d="{WAVE} V8 H0 Z" />
            <path class="line" d={WAVE} vector-effect="non-scaling-stroke" />
          </svg>
        </div>
        <div class="vtext">
          <p class="vh">{running ? 'Caches are filling' : 'Stopped while the caches filled'}</p>
          {#if running}
            <p class="muted">Every common name is asked once before its repeats count. <span class="num">{Math.round(fill * 100)}%</span> asked; repeat lookups start in about {plural(fillLeft, 'minute')}.</p>
            <p class="vkey muted"><span><i class="key cached"></i>Common names, first ask</span><span><i class="key uncached"></i>Names asked once</span></p>
          {:else}
            <p class="muted">The run stopped before the caches filled, so it has no repeat lookups.</p>
          {/if}
        </div>
      </div>
    {/if}
    {#if hasCold}
      <h3 class="sh">Cold lookups</h3>
      <p class="how note">names asked once · {ext.cold_bin_s}-second bins, wider because there are fewer of them</p>
      <OverTime series={charts.cold.series} gaps={charts.cold.gaps} {slows} slots={coldSlots} bind:cursor={cursorCold} formatTime={clock} unit="{ext.cold_bin_s}-second bin" totals={coldTotals} totalsLabel={spanLabel} />
    {/if}
    {#if hasRepeat || hasCold}{@render key()}{/if}
  {:else if chart}
    <OverTime series={chart.series} gaps={chart.gaps} bind:cursor formatTime={clock} unit="{ext.bin_s}-second bin" totals={allTotals} totalsLabel={spanLabel} />{@render key()}
  {/if}
{/snippet}

{#if !ext || !res}
  <div class="page">
    {#if running}
      <p class="muted" role="status">Waiting for the first results.</p>
    {:else}
      {#each endNotes as n}<p class="muted" role="status">{n.text}</p>{:else}<p class="muted" role="status">This run ended before its first results.</p>{/each}
    {/if}
  </div>
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
      <header><h2>The run so far</h2>{#if split}<span class="how">both charts share the time axis</span>{:else}<span class="how"><span class="num">{ext.bin_s}</span>-second bins, <span class="num">{ext.per_bin}</span> queries to each resolver in every bin</span>{/if}{@render legend()}</header>
      {@render chartBody(ext)}
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
            {#if res.no_verdict}
              <p>About {mins(res.removed_s)} were left out because most queries were lost, which is too much of the run to call.</p>
            {:else}
              <p>Too few lookups were answered to compare the resolvers.</p>
            {/if}
          {:else if lead && next?.vs}
            {@const fewer = lead.lost <= next.lost ? lead : next}
            {@const more = fewer === lead ? next : lead}
            {#if verdict.kind === 'Practical tie for first'}
              <p>
                Their medians are <span class="num">{f1(Math.abs(next.vs.diff))} ms</span> apart (<span class="num">{rangeOf(next.vs)}</span>), under the <span class="num">{ms(res.margin)}</span> this test counts as a real difference.{#if fewer.lost !== more.lost}
                  {names[fewer.t]} loses fewer queries: <span class="num">{fewer.lost.toLocaleString()}</span> against <span class="num">{more.lost.toLocaleString()}</span>.{/if}
              </p>
            {:else if verdict.kind === 'Lowest median'}
              <p>
                {names[lead.t]} has the lowest median, <span class="num">{f1(Math.abs(next.vs.diff))} ms</span> ahead of the next (<span class="num">{rangeOf(next.vs)}</span>){#if res.loss_flag}, but it lost <span class="num">{lead.loss_pct.toFixed(1)}%</span> of queries against <span class="num">{next.loss_pct.toFixed(1)}%</span> for {names[next.t]}.{:else}.{/if}
                Differences under {ms(res.margin)} count as a tie.
              </p>
            {:else}
              <p>Cannot separate {names[lead.t]} and {names[next.t]}: the gap could be <span class="num">{rangeOf(next.vs)} ms</span>, and {ms(res.margin)} is the line. {ext.total < 5000 ? 'Run it longer.' : 'Run it again.'}</p>
            {/if}
            <p>
              {#if beaten.length}{verdict.kind === 'Lowest median' ? names[lead.t] + ' beat' : 'Both beat'} {#each beaten as r, i}{i ? ' and ' : ''}{names[r.t]}{#if isCurrent(r.t)}, your current resolver,{/if} by about <span class="num">{Math.round(r.vs?.diff ?? 0)} ms</span>{/each} a lookup.{' '}{/if}Measured from this network between <span class="num">{clock(0)}</span> and <span class="num">{clock(lastBin?.end ?? 0)}</span>{#if sleepS}, apart from the {mins(sleepS)} the {machine} was asleep{/if}{#if res.removed_s}{sleepS ? ' and' : ', apart from the'} {mins(res.removed_s)} when most queries were lost{/if}; another hour or network can order them differently.
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
      <table class="nine">
        <thead>
          <tr>
            <th>Resolver</th><th class="n">Median</th><th class="n">Asked again</th><th class="n">9 in 10 within</th><th class="n">95 in 100 within</th><th class="n">Lost</th><th class="n">Median behind the lowest</th>{#if hasCalls}<th>Call</th>{/if}
          </tr>
        </thead>
        <tbody>
          {#each rows as r}
            <tr>
              <td data-l="Resolver">{@render who(r.t)}</td>
              <td class="n" data-l="Median">{f1(r.p50)}</td>
              <td class="n" data-l="Asked again">{f1(kindP50('common_later', r.t))}</td>
              <td class="n" data-l="9 in 10 within">{f1(r.p90)}</td>
              <td class="n" data-l="95 in 100 within">{f1(r.p95)}</td>
              <td class="n under" data-l="Lost">{r.lost.toLocaleString()} <span class="sub">of&nbsp;{r.sent.toLocaleString()}</span></td>
              <td class="n under" data-l="Median behind {names[lead?.t ?? 0]}">{#if r.vs}<span class="num">{gap(r.vs)}</span> <span class="sub">({rangeOf(r.vs)})</span>{:else}—{/if}</td>
              {#if hasCalls}<td data-l="Call">{#if r.call}<span class="pill {r.call}">{CALL_LABEL[r.call]}</span>{/if}</td>{/if}
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="foot">
        Times in milliseconds. A lost query counts as slower than any answer. The range beside each gap holds about 19 times in 20.
        {#if tailTenths > 5}
          Names asked once are the less common sites, about <span class="num">{tailTenths}</span> in 10 of the lookups, so the median leans toward them.
        {:else}
          This run is a typical mix: about <span class="num">{10 - tailTenths}</span> in 10 lookups are names asked before and <span class="num">{tailTenths}</span> in 10 are less common sites asked once. Cold lookups are ranked on their own below.
        {/if}
        {#if split}The first ask of each common name fills the caches and is not ranked.{/if}
        Names asked again are the common names on every ask after the first.
        {#if !final}Not final: this run was stopped, so no call is made.{/if}
        {#if res.removed_s && removedFrom}{#if removedRuns > 1}{mins(res.removed_s)} in {removedRuns} stretches, the first at {clock(removedFrom.at)}, are{:else}The {mins(res.removed_s)} from {clock(removedFrom.at)} are{/if} left out for every resolver: each lost half or more of its queries then, which points at the link.{/if}
        {#each slows as s, i}{i ? ' ' : ''}From {clock(s.t0)} to {clock(s.t1)} the resolvers slowed together, which points away from any one of them. {SLOW_WHY[s.kind]} Those lookups still count in every figure.{/each}
        {#each ext.gaps.filter((g) => g[2] === 'sleep') as g, i}{i ? ' ' : ''}The {machine} was asleep from {clock(g[0])} to {clock(g[1])}. Nothing was sent then, and that stretch is in no figure.{/each}
      </p>
    </div>

    {#if cold.length}
      <div class="card">
        <header><h2>Cold lookups</h2><span class="how">The <span class="num">{ext.names.tail.toLocaleString()}</span> less common sites, each asked once, so a resolver has to look most of them up</span></header>
        <table>
          <thead>
            <tr><th>Resolver</th><th class="n">Median</th><th class="n">9 in 10 within</th><th class="n">Lost</th><th class="n">Median behind {names[coldLead.t]}</th>{#if coldHasCalls}<th>Call</th>{/if}</tr>
          </thead>
          <tbody>
            {#each cold as r}
              <tr>
                <td data-l="Resolver">{@render who(r.t)}</td>
                <td class="n" data-l="Median">{f1(r.p50)}</td>
                <td class="n" data-l="9 in 10 within">{f1(r.p90)}</td>
                <td class="n under" data-l="Lost">{r.lost.toLocaleString()} <span class="sub">of&nbsp;{r.sent.toLocaleString()}</span></td>
                <td class="n under" data-l="Median behind {names[coldLead.t]}">{#if r.vs}<span class="num">{gap(r.vs)}</span> <span class="sub">({rangeOf(r.vs)})</span>{:else}—{/if}</td>
                {#if coldHasCalls}<td data-l="Call">{#if r.call}<span class="pill {r.call}">{CALL_LABEL[r.call]}</span>{/if}</td>{/if}
              </tr>
            {/each}
          </tbody>
        </table>
        {#if coldVerdict && coldLead && cold[1]?.vs}
          <p class="foot">
            {#if coldVerdict.kind === 'Lowest median'}
              {names[coldLead.t]} has the lowest median on cold lookups, <span class="num">{f1(Math.abs(cold[1].vs.diff))} ms</span> ahead of {names[cold[1].t]} (<span class="num">{rangeOf(cold[1].vs)}</span>).
            {:else if coldVerdict.kind === 'Practical tie for first'}
              Practical tie on cold lookups: {names[coldLead.t]} or {names[cold[1].t]}.
            {:else}
              Too close to call on cold lookups: {names[coldLead.t]} or {names[cold[1].t]}.
            {/if}
          </p>
        {/if}
      </div>
    {/if}

    <div class="card">
      <header>
        <h2>Response time over the run</h2>
        {#if split}<span class="how">log scale · both charts share the time axis</span>{:else}<span class="how"><span class="num">{ext.bin_s}</span>-second bins, <span class="num">{ext.per_bin}</span> queries to each resolver in every bin · log scale</span>{/if}
        {@render legend()}
      </header>
      {@render chartBody(ext)}
    </div>

    <div class="card">
      <header><h2>How fast answers came back</h2><span class="how">Share of queries answered within the time below, in ms</span>{@render legend()}</header>
      <ExtEcdf ecdf={res.ecdf} {names} />
      <p class="hint">A curve further left is faster; a steeper one is steadier. A lost query never comes back, so each curve stops short of 100% by its losses.</p>
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
                  <td>{GROUP_LABEL[g.key] ?? g.key}{#if split && g.key === 'common_first'} <span class="sub">· cache filling, not ranked</span>{/if}</td>
                  <td class="n">{g.count.toLocaleString()}</td>
                  {#each rows as r}<td class="n"><span class="num">{f1(g.r[r.t]?.p50)}</span> <span class="sub">· {g.r[r.t]?.lost ?? 0} lost</span></td>{/each}
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </div>
    {/if}

    <details class="card">
      <summary>How this run was made<span>{plural(names.length, 'resolver')} · plain DNS over UDP · seed <span class="num">{ext.names.seed}</span></span></summary>
        <dl>
          <dt>Resolvers</dt><dd>{names.join(', ')}</dd>
          <dt>Transport</dt><dd>Plain DNS over UDP, IPv4</dd>
          <dt>Pace</dt>
          <dd>Each name goes to every resolver in the same <span class="num">{ext.gap_ms} ms</span> slot, in a random order each time: about <span class="num">{(1000 / ext.gap_ms).toFixed(1)}</span> queries a second to each resolver</dd>
          <dt>No answer</dt><dd>After <span class="num">{ext.timeout_ms / 1000} s</span> a query counts as lost, and is ranked as slower than any answer</dd>
          <dt>Names</dt>
          <dd><span class="num">{ext.names.common}</span> common names, each asked <span class="num">{asksOf(ext.names, ext.total)}</span> times, and <span class="num">{ext.names.tail.toLocaleString()}</span> less common sites{#if region}{#if (ext.names.regional ?? 0) < ext.names.tail}, <span class="num">{(ext.names.regional ?? 0).toLocaleString()}</span> of them{:else}, all{/if} from {region}{/if}, from Recursa’s own list, dated {ext.names.list_date}</dd>
          <dt>Seed</dt><dd><span class="num">{ext.names.seed}</span>, which fixes the names and their order</dd>
          <dt>Comparison</dt>
          <dd>The gap between two medians. Its range comes from the gap in ten equal stretches of the run and holds about <span class="num">19</span> times in <span class="num">20</span>. Gaps under <span class="num">{ms(res.margin)}</span> count as a tie</dd>
          <dt>Left out</dt>
          <dd>
            {#if res.removed_s && removedFrom}<span class="num">{res.removed_s} s</span>{#if removedRuns > 1} in <span class="num">{removedRuns}</span> stretches, the first{/if} from <span class="num">{clock(removedFrom.at)}</span>, when every resolver lost half or more of its queries{:else}Nothing{/if}
          </dd>
          {#each ext.gaps as g}
            <dt>{GAP_LABEL[g[2]]}</dt><dd><span class="num">{clock(g[0])} to {clock(g[1])}</span>. Nothing was sent then</dd>
          {/each}
          {#each slows as s}
            <dt>{s.label}</dt><dd><span class="num">{clock(s.t0)} to {clock(s.t1)}</span>. The resolvers slowed together; still counted in every figure</dd>
          {/each}
          {#if res.dropped_names}<dt>Dropped names</dt><dd>{plural(res.dropped_names, 'name')} that no resolver answered, left out of every figure</dd>{/if}
          {#if rows.some((r) => r.blocked)}
            <dt>Blocked names</dt><dd>{#each rows.filter((r) => r.blocked) as r, i}{i ? ', ' : ''}{names[r.t]} <span class="num">{r.blocked}</span>{/each}: answered with a block or an error another resolver did not give, so left out of that resolver’s timing only</dd>
          {/if}
          <dt>Bins</dt><dd><span class="num">{ext.bins.length}</span> bins of <span class="num">{ext.bin_s} s</span>, <span class="num">{ext.per_bin}</span> queries to each resolver in every bin{#if split}; cold lookups are charted in bins of <span class="num">{ext.cold_bin_s} s</span>{/if}</dd>
        </dl>
    </details>

    <details class="card" bind:open={binsOpen}>
      <summary>Every bin as a table<span><span class="num">{ext.bins.length}</span> rows, {split ? 'every lookup, ' : ''}times in milliseconds</span></summary>
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
  /* The caches filling before any repeat lookup: a vessel the height of the chart that replaces it, filled to the true
     share, with a few bubbles rising through it. The surface drifts and the bubbles rise only while the run sends. */
  .vessel {
    position: relative;
    height: 180px;
    border: 1px solid var(--rule);
    border-radius: 10px;
    background: var(--surface);
    overflow: hidden;
    isolation: isolate;
  }
  .liquid {
    position: absolute;
    inset: 0;
    background: color-mix(in oklch, var(--ink) 5%, var(--surface));
    transition: transform 900ms var(--ease-out);
  }
  /* The liquid below its surface, kept clear of the swell so no bubble crosses the line. Each bubble rides its own
     column within it, fading in at the column's foot and out before its head; it sways on a clock of its own. */
  .bubbles {
    position: absolute;
    top: 10px;
    left: 0;
    right: 0;
  }
  .b {
    position: absolute;
    width: 0;
    animation: rise var(--d) linear var(--delay) infinite;
  }
  .d {
    position: absolute;
    top: 0;
    left: calc(var(--s) / -2);
    width: var(--s);
    height: var(--s);
    border: 1px solid color-mix(in oklch, var(--ink-3) 70%, transparent);
    border-radius: 50%;
    animation: sway var(--sw) var(--ease-in-out) var(--delay) infinite alternate;
    transition: opacity 1.2s var(--ease-out);
  }
  .d.off {
    opacity: 0;
  }
  .d.cached {
    border: 0;
    background: var(--s-cached);
  }
  .d.uncached {
    border: 0;
    background: var(--s-uncached);
  }
  .bubbles:not(.moving) .b,
  .bubbles:not(.moving) .d {
    animation-play-state: paused;
  }
  @keyframes rise {
    from {
      transform: translateY(100%);
      opacity: 0;
    }
    20%,
    60% {
      opacity: 1;
    }
    to {
      transform: translateY(0);
      opacity: 0;
    }
  }
  @keyframes sway {
    from {
      transform: translateX(-3px);
    }
    to {
      transform: translateX(3px);
    }
  }
  .vkey {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    margin: 2px 0 0;
    font-size: var(--t-sm);
  }
  .vkey span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .crest {
    position: absolute;
    left: 0;
    top: -4px;
    width: 200%;
    height: 8px;
    overflow: visible;
  }
  .crest.moving {
    animation: drift 8s linear infinite;
  }
  @keyframes drift {
    to {
      transform: translateX(-50%);
    }
  }
  .crest .body {
    fill: color-mix(in oklch, var(--ink) 5%, var(--surface));
  }
  .crest .line {
    fill: none;
    stroke: var(--ink-3);
    stroke-width: 1;
  }
  .vtext {
    position: relative;
    display: grid;
    gap: 4px;
    max-width: 34rem;
    padding: 16px 20px;
  }
  .vh {
    margin: 0;
    font-size: var(--t-base);
    font-weight: 620;
    color: var(--ink);
  }
  @media (prefers-reduced-motion: reduce) {
    .crest.moving {
      animation: none;
    }
    .liquid {
      transition: none;
    }
    .b,
    .d {
      animation-play-state: paused;
    }
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
  .chartkey {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    margin-top: 8px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .chartkey span {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .chartkey i {
    display: inline-block;
    flex: none;
    width: 14px;
    height: 10px;
  }
  .chartkey .sw-med {
    height: 1.5px;
    background: var(--ink);
  }
  .sw-band {
    background: linear-gradient(
      to bottom,
      color-mix(in oklch, var(--ink) 13%, transparent) 0 25%,
      color-mix(in oklch, var(--ink) 26%, transparent) 25% 75%,
      color-mix(in oklch, var(--ink) 13%, transparent) 75%
    );
  }
  .chartkey .sw-loss {
    display: inline-flex;
    gap: 1px;
    align-items: end;
  }
  .sw-loss b {
    flex: 1;
  }
  .sw-loss .w {
    height: 50%;
    background: var(--warning);
  }
  .sw-loss .s {
    height: 75%;
    background: var(--serious);
  }
  .sw-loss .c {
    height: 100%;
    background: var(--critical);
  }
  .sw-gap {
    background: repeating-linear-gradient(135deg, var(--ink-3) 0 1px, transparent 1px 4px), var(--surface-2);
    outline: 1px solid var(--rule);
    outline-offset: -1px;
  }
  .sw-warm {
    --ring: color-mix(in oklch, var(--ink-3) 70%, transparent);
    background:
      radial-gradient(circle at 3.5px 6px, transparent 0.9px, var(--ring) 1px 1.8px, transparent 1.9px),
      radial-gradient(circle at 10.5px 3.5px, transparent 0.7px, var(--ring) 0.8px 1.5px, transparent 1.6px),
      radial-gradient(circle at 9px 7.5px, var(--s-cached) 1.3px, transparent 1.6px);
    outline: 1px solid var(--rule);
    outline-offset: -1px;
  }
  .sw-slow {
    background: color-mix(in oklch, var(--ink) 6%, transparent);
  }
  .sw-slow b {
    display: block;
    height: 4px;
    background: var(--ink-2);
  }
  .sw-slow .host {
    background: repeating-linear-gradient(90deg, var(--ink-2) 0 1px, transparent 1px 4px);
  }
  .sw-slow .up {
    box-sizing: border-box;
    background: none;
    border: 1px solid var(--ink-2);
  }
  .sw-slow .any {
    height: 1px;
  }
  .card h3.sh {
    margin: 12px 0 0;
    font-size: var(--t-sm);
    font-weight: 620;
    color: var(--ink);
  }
  .how.note {
    margin: 2px 0 8px;
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
  /* The final table is nine columns wide: a cell's second figure goes under its first, and its figures sit closer. */
  .under .sub {
    display: block;
  }
  .nine .n {
    padding-inline: 6px;
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
    padding: 2px 9px;
    border: 1px solid var(--rule);
    border-radius: 999px;
    font-size: var(--t-xs);
    line-height: 1.35;
    white-space: nowrap;
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
  dl {
    margin: 12px 0 0;
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
    dl {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    dd {
      margin-bottom: 8px;
    }
  }
</style>
