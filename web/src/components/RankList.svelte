<script lang="ts">
  import TransportChip from './TransportChip.svelte';
  import BaselineChip from './BaselineChip.svelte';
  // The ranking at a glance, under the verdict: one line per target with a small scatter of
  // every answered query on a shared log axis, the speed score it is sorted by and the answer
  // rate. The full ladder is one view over (Latency).
  import { app } from '../lib/store.svelte';
  import { RELIABLE, WEIGHTS, type Row } from '../lib/verdict';
  import type { Sample, TestKind, WarmSample } from '../lib/types';
  import { TEST_LABEL, label, ms, pct, plural } from '../lib/format';
  import { logDomain, logScale, tickLabel } from '../lib/scale';
  import { warmByTarget, warmJitter } from '../lib/warm';
  import { flip } from 'svelte/animate';
  import { onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import { RERANK, jitterOf, orderOf, rankTracker, rise, roll } from '../lib/motion';
  import { pacedOrder } from '../lib/paced.svelte';

  let {
    rows,
    samples,
    tests,
    multi,
    live = false,
    warm = [],
    placeholders = 0,
    onopen,
    onmore,
  }: {
    rows: Row[];
    samples: Sample[];
    tests: TestKind[];
    multi: boolean;
    /** The run is measuring: targets still waiting for answers hold ghost lines. */
    live?: boolean;
    /** Warm-up answers so far: hollow rings on a ghost line until its target has measured answers. */
    warm?: WarmSample[];
    /** Ghost lines to draw while the run hasn't listed its targets yet. */
    placeholders?: number;
    onopen: (id: string) => void;
    onmore: () => void;
  } = $props();

  const LIMIT = 10;
  // A CDN column only when the run mapped CDNs. It is for reading across; it doesn't move the order.
  const withCdn = $derived(rows.some((r) => r.cdn != null));
  const shown = $derived(rows.filter((r) => r.measured).slice(0, LIMIT));
  const hidden = $derived(rows.filter((r) => r.measured).length - shown.length);
  const pending = $derived(live ? rows.filter((r) => !r.measured && (r.target.state === 'pending' || r.target.state === 'ready')).slice(0, Math.max(0, LIMIT - shown.length)) : []);
  // Ranked lines, then waiting ones, in one keyed list, so a line that gets its first answers slides up into its rank.
  // The order changes only on a re-rank; each line reads its gliding values by key.
  const keepOrder = orderOf();
  const byIdx = $derived(new Map([...shown, ...pending].map((r) => [r.target.idx, r])));
  const want = $derived(keepOrder([...shown, ...pending].map((r) => r.target.idx)));
  // While live, re-ranks are paced so each slide completes; rows that left are dropped at once.
  const paced = pacedOrder(() => want, () => live);
  const keepShown = orderOf();
  const order = $derived(keepShown(paced.current.filter((k) => byIdx.has(k))));
  // Rank numbers count only what can be picked: a forwarder on your network keeps its place as a reference line, unnumbered,
  // so #1 is the resolver the verdict names.
  const rankNo = $derived.by(() => {
    const m = new Map<number, number>();
    let n = 0;
    for (const k of order) {
      const r = byIdx.get(k);
      if (r?.measured && r.eligible && !r.baseline) m.set(k, ++n);
    }
    return m;
  });
  const ghosts = $derived(live && !rows.length ? Math.min(placeholders, LIMIT) : 0);
  const ghostBand = (seed: number) => {
    const h = Math.sin((seed + 1) * 12.9898) * 43758.5453;
    const f = h - Math.floor(h);
    return { left: 15 + f * 25, width: 18 + ((f * 7) % 1) * 22 };
  };

  // Warm-up answers per target. They never count; until something is measured they set the axis.
  const warmBy = $derived(live ? warmByTarget(warm, tests) : new Map<number, Partial<Record<TestKind, WarmSample[]>>>());
  const warmAxis = $derived(!shown.length && warmBy.size > 0);
  const warmN = (idx: number) => tests.reduce((n, k) => n + (warmBy.get(idx)?.[k]?.length ?? 0), 0);

  // One log axis for every strip, as in the Latency view; while live it also spans the warm-up answers, so it holds
  // when measuring starts.
  const domain = $derived.by(() => {
    let lo = Infinity;
    let hi = 0;
    for (const r of shown)
      for (const k of tests) {
        const d = r.dist[k];
        if (d.min != null) lo = Math.min(lo, d.min);
        if (d.max != null) hi = Math.max(hi, d.max);
      }
    if (live)
      for (const w of warm) {
        if (!tests.includes(w.k)) continue;
        lo = Math.min(lo, w.us / 1000);
        hi = Math.max(hi, w.us / 1000);
      }
    return logDomain(lo, hi);
  });
  let stripW = $state(200);
  // Lane height and dot size sit between the ladder's compact and roomy settings: a scatter, not a smear.
  const H = 24;
  const R = 2.4;
  const x = $derived(logScale(domain, 3, stripW - 3));
  const ticks = $derived([0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000].filter((t) => t > domain[0] && t < domain[1]));
  // Label every other tick, so labels never collide in a narrow strip.
  const labelled = $derived(ticks.filter((t) => [1, 10, 100, 1000].includes(t)).length >= 2 ? ticks.filter((t) => [1, 10, 100, 1000].includes(t)) : ticks.filter((_, i) => i % 2 === 0));

  // Rows enter, and numerals roll, only for changes after the first render and only while live.
  let ready = $state(false);
  onMount(() => (ready = true));
  const riseOn = () => ({ on: live && ready });
  // Rank changes during a live run: climbed (1) or dropped (-1) per target.
  const rankOf = rankTracker();
  const rankDir = $derived(rankOf(shown, live));

  const byTarget = $derived.by(() => {
    const m = new Map<number, Sample[]>();
    for (const s of samples) {
      if (s.o !== 'ok' || !tests.includes(s.k)) continue;
      let a = m.get(s.t);
      if (!a) m.set(s.t, (a = []));
      a.push(s);
    }
    return m;
  });
  // The same stable jitter as the Latency view, so a dot sits the same way in both.
  // Each strip's dots as one string of circles: no per-dot bookkeeping, and these dots don't
  // animate, so redrawing them as a block while a run is live changes nothing visible.
  const strip = (list: Sample[] | undefined) =>
    (list ?? [])
      .map((s) => `<circle class="dot ${s.k}" cx="${x(s.us / 1000).toFixed(1)}" cy="${(H / 2 + jitterOf(s) * (H / 2 - R - 1)).toFixed(1)}" r="${R}"/>`)
      .join('');
  // Warm-up answers as hollow rings, in one strip per target (no lanes here, as with the dots).
  const rings = (idx: number) => {
    const byLane = warmBy.get(idx) ?? {};
    return tests
      .flatMap((k) => (byLane[k] ?? []).map((w, i) => ({ w, i })))
      .map(({ w, i }) => `<circle class="ring ${w.k}" cx="${x(w.us / 1000).toFixed(1)}" cy="${(H / 2 + warmJitter(w.t, w.k, i) * (H / 2 - R - 2)).toFixed(1)}" r="${R}"/>`)
      .join('');
  };
</script>

<section class="rank" aria-labelledby="rank-h">
  <header>
    <h2 id="rank-h">By speed score</h2>
    <button class="more" onclick={onmore}>Full chart in Latency{hidden > 0 ? `, ${hidden} more` : ''}</button>
  </header>
  <ol class:withcdn={withCdn}>
    <li class="axis" aria-hidden="true">
      <span></span><span></span>
      <span class="strip" bind:clientWidth={stripW}>
        <svg width={stripW} height="12">
          {#each shown.length || warmAxis ? labelled : [] as t}<text x={x(t)} y="10" text-anchor="middle">{tickLabel(t)}</text>{/each}
        </svg>
      </span>
      <span class="h" title="Speed score, in ms: cached, uncached and TLD lookups weighted together, each test's mean after its slowest tenth is pulled down to its p90. The list is sorted by it.">score</span>{#if withCdn}<span class="h cdnh" title="CDN edges compared with the nearest any resolver found">CDN</span>{/if}<span class="h">answered</span>
    </li>
    {#each order as idx (idx)}
      {@const r = byIdx.get(idx)!}
      {@const wn = warmN(idx)}
      <li animate:flip={RERANK} in:rise={riseOn()}>
        {#if r.measured}
          <button
            in:fade={{ duration: live && ready ? 350 : 0 }}
            class="row"
            class:sel={app.inspect === r.target.resolver_id}
            class:weak={!r.eligible}
            data-resolver={r.target.resolver_id}
            onclick={() => onopen(r.target.resolver_id)}
          >
            <span class="n num"
              >{#if rankNo.has(idx)}{#key rankNo.get(idx)}<span class="rn" class:up={rankDir.get(idx) === 1} in:roll={{ dir: rankDir.get(idx) ?? 0 }}>{rankNo.get(idx)}</span>{/key}{:else}·{/if}</span
            >
            <span class="who">
              {#if r.baseline}<BaselineChip what={r.forwarder} lead />{/if}<b>{r.target.provider}</b>
              {r.target.variant}{#if multi}<TransportChip t={r.target.transport} />{/if}
              {#if r.intercepted}<span class="flag">answered locally, not ranked</span>{/if}
            </span>
            <svg class="strip" width={stripW} height={H} aria-hidden="true">
              {#each ticks as t}<line class="grid-l" x1={x(t)} x2={x(t)} y1="0" y2={H} />{/each}
              {@html strip(byTarget.get(idx))}
              {#if r.composite != null}<rect class="median" x={x(r.composite) - 1.25} y="1" width="2.5" height={H - 2} rx="1.25" />{/if}
            </svg>
            <span class="v num">{ms(r.composite)}</span>
            {#if withCdn}<span class="cdn num" title={r.cdn == null ? 'No CDN result for this resolver' : `CDN edges ${r.cdn < 0.05 ? 'as near as any resolver found' : `${ms(r.cdn)} farther than the nearest`}; this has its own pick and doesn’t move the order`}
                >{r.cdn == null ? '—' : r.cdn < 0.05 ? 'best' : `+${ms(r.cdn, false)}`}</span
              >{/if}
            <span class="ok num" class:bad={r.success < RELIABLE}>{pct(r.success)}</span>
          </button>
        {:else}
          <div
            class="row ghost-line"
            role="img"
            aria-label={wn
              ? `${label(r.target.provider, r.target.variant)}: warming up, ${plural(wn, 'warm-up answer')}, not recorded`
              : `${label(r.target.provider, r.target.variant)}: waiting for answers`}
          >
            <span class="n num">·</span>
            <span class="who"><b>{r.target.provider}</b> {r.target.variant}{#if multi}<TransportChip t={r.target.transport} />{/if}</span>
            {#if wn}
              <svg class="strip" width={stripW} height={H} aria-hidden="true">
                {#each ticks as t}<line class="grid-l" x1={x(t)} x2={x(t)} y1="0" y2={H} />{/each}
                {@html rings(idx)}
              </svg>
              <span class="gv wn" style:grid-column="span {withCdn ? 3 : 2}">{plural(wn, 'warm-up answer')}</span>
            {:else}
              {@render ghostCells(idx)}
            {/if}
          </div>
        {/if}
      </li>
    {/each}
    {#each { length: ghosts } as _, i}
      <li aria-hidden="true">
        <div class="row ghost-line">
          <span class="n num">·</span>
          <span class="ghost" style:width="{110 + ((i * 37) % 60)}px" style:height="10px"></span>
          {@render ghostCells(i)}
        </div>
      </li>
    {/each}
  </ol>

  {#snippet ghostCells(seed: number)}
    {@const b = ghostBand(seed)}
    <span class="strip gstrip"><span class="ghost" style:margin-left="{b.left}%" style:width="{b.width}%"></span></span>
    <span class="ghost gv" style:width="44px"></span>
    {#if withCdn}<span class="ghost gv" style:width="32px"></span>{/if}
    <span class="ghost gv" style:width="34px"></span>
  {/snippet}
  <p class="legend-note">
    {#each tests as k}<span><i class="key {k}"></i>{TEST_LABEL[k]}</span>{/each}
    <span>every answered query, log ms · tick: speed score (cached {pct(WEIGHTS.cached, 0)}, uncached {pct(WEIGHTS.uncached, 0)}, TLD {pct(WEIGHTS.tld, 0)}, each test's mean after its slowest tenth is pulled down to its p90)</span>
  </p>
</section>

<style>
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
  }
  h2 {
    font-size: var(--t-sm);
    font-weight: 650;
  }
  .more {
    border: 0;
    background: none;
    padding: 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
    text-decoration: underline;
    text-decoration-color: var(--rule-2);
    text-underline-offset: 3px;
  }
  .more:hover {
    color: var(--ink);
    text-decoration-color: currentColor;
  }
  /* One grid for every row (rows are subgrids), so names get the width they need and the
     strips share the rest, aligned. */
  ol {
    list-style: none;
    margin: 0 -8px;
    padding: 0;
    display: grid;
    grid-template-columns: 22px minmax(100px, max-content) minmax(80px, 1fr) 62px 54px;
  }
  ol.withcdn {
    grid-template-columns: 22px minmax(100px, max-content) minmax(80px, 1fr) 62px 54px 54px;
  }
  ol {
    column-gap: 16px;
    align-items: center;
  }
  /* A real box on the subgrid (not display: contents), so a re-ranked line has something for FLIP to measure and move. */
  li {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: subgrid;
  }
  .axis,
  .row {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: subgrid;
    align-items: center;
  }
  .axis {
    padding: 0 8px 4px;
    font-size: var(--t-2xs);
    color: var(--ink-3);
    user-select: none;
    -webkit-user-select: none;
  }
  .axis text {
    fill: var(--ink-3);
    font-size: var(--t-2xs);
    font-variation-settings: 'MONO' 1;
  }
  .axis .h {
    text-align: right;
  }
  .strip {
    display: block;
    min-width: 0;
    overflow: hidden;
  }
  .row {
    width: 100%;
    padding: 5px 8px;
    border: 0;
    border-radius: var(--r-md);
    background: none;
    text-align: left;
    font-size: var(--t-sm);
  }
  .row:hover {
    background: var(--surface-2);
  }
  .row.sel {
    background: var(--sel);
  }
  .row.weak .who,
  .row.weak .v {
    opacity: 0.66;
  }
  .n {
    color: var(--ink-3);
    font-size: var(--t-xs);
    text-align: right;
    /* Clips the numeral as it rolls in, so the column never changes size. */
    overflow: hidden;
  }
  .rn {
    display: block;
  }
  /* A row that climbed: its numeral starts in full ink at a heavier weight and settles back. */
  .rn.up {
    animation: climb 1200ms var(--ease-out);
  }
  @keyframes climb {
    from {
      color: var(--ink);
      font-weight: 760;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .rn.up {
      animation: none;
    }
  }
  .who {
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .who b {
    font-weight: 600;
  }
  .flag {
    margin-left: 8px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .grid-l {
    stroke: var(--rule);
  }
  /* Global: the dots are inserted as markup, which carries no scoping class. */
  .strip :global(.dot) {
    opacity: 0.6;
  }
  .strip :global(.dot.cached) {
    fill: var(--s-cached);
  }
  .strip :global(.dot.uncached) {
    fill: var(--s-uncached);
  }
  .strip :global(.dot.tld) {
    fill: var(--s-tld);
  }
  .strip :global(.ring) {
    fill: none;
    stroke-width: 1.25;
    opacity: 0.85;
  }
  .strip :global(.ring.cached) {
    stroke: var(--s-cached);
  }
  .strip :global(.ring.uncached) {
    stroke: var(--s-uncached);
  }
  .strip :global(.ring.tld) {
    stroke: var(--s-tld);
  }
  .median {
    fill: var(--ink);
    stroke: var(--surface);
    stroke-width: 1;
    paint-order: stroke;
  }
  .v,
  .cdn,
  .ok {
    text-align: right;
    font-size: var(--t-xs);
  }
  .ok {
    color: var(--ink-3);
  }
  .ok.bad {
    color: var(--ink);
    font-weight: 600;
  }
  .ghost-line,
  .ghost-line:hover {
    background: none;
    min-height: 34px;
  }
  .ghost-line .who {
    opacity: 0.7;
  }
  .gstrip {
    display: flex;
    align-items: center;
    height: 24px;
  }
  .gstrip .ghost {
    height: 6px;
    border-radius: 3px;
  }
  .gv {
    justify-self: end;
    height: 9px;
  }
  .wn {
    height: auto;
    font-size: var(--t-xs);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .legend-note {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    margin-top: 12px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .legend-note span {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  @media (max-width: 640px) {
    ol {
      grid-template-columns: 18px minmax(0, 1fr) 0 64px 48px;
    }
    ol.withcdn {
      grid-template-columns: 18px minmax(0, 1fr) 0 64px 48px 48px;
    }
    .strip {
      visibility: hidden;
    }
  }
  /* Phone: no strip (the Latency view has the full chart), no CDN column (it has its own view), so the
     name gets the row and wraps rather than truncates. */
  :global(html.mobile) ol,
  :global(html.mobile) ol.withcdn {
    grid-template-columns: 18px minmax(0, 1fr) 0 56px max-content;
    column-gap: 8px;
    margin: 0 -4px;
  }
  :global(html.mobile) .cdn,
  :global(html.mobile) .cdnh {
    display: none;
  }
  :global(html.mobile) .axis {
    padding: 0 4px 4px;
  }
  :global(html.mobile) .row {
    padding: 7px 4px;
    min-height: 40px;
  }
  :global(html.mobile) .row:hover {
    background: none;
  }
  :global(html.mobile) .row:active {
    background: var(--surface-2);
  }
  :global(html.mobile) .row.sel {
    background: var(--sel);
  }
  :global(html.mobile) .who {
    white-space: normal;
    line-height: 1.25;
  }
  /* Names wrap on the phone, so the baseline mark sits on its own line above the name instead of splitting it. */
  :global(html.mobile) .who :global(.bchip.lead) {
    display: table;
    margin: 0 0 3px;
  }
  :global(html.mobile) .flag {
    display: block;
    margin-left: 0;
  }
  :global(html.mobile) .v,
  :global(html.mobile) .ok {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .legend-note {
    display: none;
  }
</style>
