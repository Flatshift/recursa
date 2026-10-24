<script lang="ts">
  // The monitoring record: one row per resolver on a shared time axis. Each cell's answers are a column of
  // log-ms bins shaded by how many landed there, so a resolver's spread and its second mode (a forwarder's cache
  // hits) show, not just a median. The median runs through it as a ringed line; unanswered shares stand below as
  // status bars; outages are struck through in red; operator-reported incidents sit on top as hollow marks.
  import type { MonitorCell, MonitorHist, MonitorHistory } from '../lib/types';
  import { logDomain } from '../lib/scale';
  import { ms, pct, plural, when } from '../lib/format';

  export interface Mark {
    t: number;
    title: string;
  }

  let {
    history,
    intervalMin,
    nameOf,
    marks,
    cursor = $bindable(null),
  }: {
    history: MonitorHistory;
    intervalMin: number;
    nameOf: (id: string) => string;
    marks: (id: string) => Mark[];
    /** The cell under the pointer or the keyboard, shared by every row. */
    cursor?: number | null;
  } = $props();

  const LEVELS = [0.1, 0.2, 0.32, 0.46, 0.62, 0.8];

  const cellH = $derived(history.cell / 3_600_000);
  const N = $derived(Math.ceil(history.hours / cellH));
  const now = $derived.by(() => {
    void history;
    return Date.now();
  });
  const toX = (t: number) => (t - history.since) / history.cell;
  const nowX = $derived(Math.min(N, toX(now)));

  /** [bin, count] pairs of a cell's histogram. */
  function bins(h: number[] | undefined): [number, number][] {
    const out: [number, number][] = [];
    if (h) for (let k = 1; k < h.length; k++) if (h[k]) out.push([h[0] + k - 1, h[k]]);
    return out;
  }
  const edge = (b: number) => 10 ** ((b - 4) / 8);

  // One scale for every row, framing the middle 99% of all answers in the window; the rest pile onto the edges.
  const domain = $derived.by(() => {
    const tot = new Map<number, number>();
    let n = 0;
    for (const r of history.resolvers)
      for (const c of r.buckets)
        for (const [b, k] of bins(c[5])) {
          tot.set(b, (tot.get(b) ?? 0) + k);
          n += k;
        }
    if (!n) return logDomain(Infinity, 0);
    const keys = [...tot.keys()].sort((a, b) => a - b);
    let acc = 0;
    let lo: number | null = null;
    let hi = keys[keys.length - 1];
    for (const b of keys) {
      acc += tot.get(b)!;
      if (lo == null && acc >= n * 0.005) lo = b;
      if (acc >= n * 0.995) {
        hi = b;
        break;
      }
    }
    return logDomain(edge(lo ?? keys[0]), edge(hi + 1));
  });
  const loB = $derived(Math.floor(8 * Math.log10(domain[0]) + 1e-9) + 4);
  const hiB = $derived(Math.ceil(8 * Math.log10(domain[1]) - 1e-9) + 3);
  const B = $derived(hiB - loB + 1);
  const yOf = (v: number) => Math.min(B, Math.max(0, hiB + 1 - (8 * Math.log10(v) + 4)));
  const gridY = $derived([0.1, 1, 10, 100, 1000, 10000].filter((t) => t >= domain[0] && t <= domain[1]));

  function cellClass(c: MonitorCell | undefined): 'none' | 'up' | 'some' | 'most' | 'down' {
    if (!c || c[1] === 0) return 'none';
    const r = c[2] / c[1];
    return r >= 0.99 ? 'up' : r >= 0.9 ? 'some' : r > 0 ? 'most' : 'down';
  }

  interface Out {
    x0: number;
    x1: number;
    start: number;
    until: number;
    cycles: number;
    recovered: number | null;
  }
  interface Row {
    h: MonitorHist;
    name: string;
    by: Map<number, MonitorCell>;
    heat: string[];
    median: string;
    gaps: string;
    loss: Record<'up' | 'some' | 'most' | 'down', string>;
    outs: Out[];
    marks: (Mark & { x: number })[];
    marginal: string;
  }

  const rows: Row[] = $derived(
    history.resolvers.map((h) => {
      const by = new Map<number, MonitorCell>();
      for (const c of h.buckets) by.set(Math.round(toX(c[0])), c);
      const heat = LEVELS.map(() => '');
      const total = new Map<number, number>();
      const loss = { up: '', some: '', most: '', down: '' };
      let median = '';
      let gaps = '';
      let pen = false;
      let gapFrom: number | null = null;
      const end = Math.ceil(nowX);
      for (let i = 0; i <= N; i++) {
        const c = by.get(i);
        const has = !!c && c[1] > 0 && i < end;
        if (!has && i < end) gapFrom ??= i;
        else if (gapFrom != null) {
          gaps += `M${gapFrom} 0H${i}V${B}H${gapFrom}Z`;
          gapFrom = null;
        }
        if (!c || !has) {
          pen = false;
          continue;
        }
        // Clamp into the frame, then shade each bin against the cell's fullest one, so every column shows its own shape.
        const col = new Map<number, number>();
        for (const [b, k] of bins(c[5])) {
          const cb = Math.min(hiB, Math.max(loB, b));
          col.set(cb, (col.get(cb) ?? 0) + k);
          total.set(cb, (total.get(cb) ?? 0) + k);
        }
        const max = Math.max(...col.values());
        for (const [b, k] of col) {
          const l = Math.min(LEVELS.length - 1, Math.floor(Math.sqrt(k / max) * LEVELS.length));
          heat[l] += `M${i} ${hiB - b}h1v1h-1z`;
        }
        if (c[3] != null) {
          median += `${pen ? 'L' : 'M'}${i + 0.5} ${yOf(c[3]).toFixed(3)}`;
          pen = true;
        } else pen = false;
        const k = cellClass(c);
        const lost = 1 - c[2] / c[1];
        const lh = k === 'up' ? 0.14 : Math.min(1, 0.32 + lost * 6);
        loss[k as 'up'] += `M${i} ${1 - lh}h1V1h-1z`;
      }
      const tmax = Math.max(1, ...total.values());
      let marginal = 'M0 0';
      for (let b = hiB; b >= loB; b--) marginal += `H${((total.get(b) ?? 0) / tmax).toFixed(3)}V${hiB - b + 1}`;
      marginal += 'H0Z';
      const outs = h.outages.map((o) => {
        const until = o.recovered ?? Math.min(now, o.end + intervalMin * 60_000);
        return {
          x0: Math.max(0, toX(o.start)),
          x1: Math.min(nowX, Math.max(toX(o.start), toX(until))),
          start: o.start,
          until,
          cycles: o.cycles,
          recovered: o.recovered,
        };
      });
      return {
        h,
        name: nameOf(h.resolver_id),
        by,
        heat,
        median,
        gaps,
        loss,
        outs,
        marks: marks(h.resolver_id)
          .map((m) => ({ ...m, x: toX(m.t) }))
          .filter((m) => m.x >= 0 && m.x <= nowX),
        marginal,
      };
    }),
  );

  // Time axis: midnights (or every 6 hours in a day's window), thinned to about eight labels.
  const ticks = $derived.by(() => {
    const step = history.hours <= 24 ? 6 : 24;
    const end = history.since + N * history.cell;
    const d = new Date(history.since);
    d.setMinutes(0, 0, 0);
    d.setHours(step === 24 ? 24 : Math.ceil((d.getHours() + 1) / 6) * 6);
    const out: { x: number; label: string | null }[] = [];
    for (; d.getTime() < end; d.setHours(d.getHours() + step)) {
      const midnight = d.getHours() === 0;
      out.push({
        x: toX(d.getTime()),
        label:
          step === 6 && !midnight
            ? d.toLocaleTimeString(undefined, { hour: 'numeric' })
            : d.toLocaleDateString(undefined, history.hours > 24 * 8 ? { month: 'short', day: 'numeric' } : { weekday: 'short', day: 'numeric' }),
      });
    }
    const every = Math.ceil(out.length / 8);
    return out.map((t, i) => (i % every ? { ...t, label: null } : t));
  });

  const startOf = (i: number) => history.since + i * history.cell;
  function span(i: number): string {
    const t = startOf(i);
    const a = when(t);
    const b = new Date(t + history.cell).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
    return `${a} – ${b}`;
  }
  function summary(r: Row, i: number): string {
    const c = r.by.get(i);
    const out = r.outs.find((o) => o.x1 > i && o.x0 < i + 1);
    const parts = [span(i)];
    if (!c || c[1] === 0) parts.push(i < nowX ? 'no checks' : 'not yet');
    else parts.push(`${c[2]} of ${c[1]} answered`, `median ${ms(c[3])}`, `p95 ${ms(c[4])}`);
    if (out) parts.push(`down from ${when(out.start)}, ${plural(out.cycles, 'missed cycle')}`);
    return parts.join(' · ');
  }

  // Pointer and keyboard share one cursor; the tooltip follows the pointer only.
  let tip = $state<{ x: number; y: number; text: string } | null>(null);
  let live = $state('');
  const cellAt = (e: PointerEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    return Math.min(N - 1, Math.max(0, Math.floor(((e.clientX - r.left) / r.width) * N)));
  };
  function hover(e: PointerEvent, r: Row) {
    const i = cellAt(e);
    cursor = i;
    const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const mark = r.marks.find((m) => Math.abs(((m.x / N) * box.width + box.left) - e.clientX) < 6 && e.clientY - box.top < 12);
    tip = { x: e.clientX, y: box.top, text: mark ? `Operator reported, ${when(mark.t)}: ${mark.title}` : summary(r, i) };
  }
  function leave() {
    tip = null;
    cursor = null;
  }
  const lastCell = (r: Row) => Math.max(0, ...[...r.by.keys()].filter((i) => i < N));
  function key(e: KeyboardEvent, r: Row, idx: number) {
    const move: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1 };
    if (e.key in move || e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      const cur = cursor ?? lastCell(r);
      cursor = e.key === 'Home' ? 0 : e.key === 'End' ? lastCell(r) : Math.min(N - 1, Math.max(0, cur + move[e.key]));
      live = `${r.name}: ${summary(r, cursor)}`;
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      const rowsEl = (e.currentTarget as HTMLElement).parentElement?.querySelectorAll<HTMLElement>('.row');
      const next = rowsEl?.[idx + (e.key === 'ArrowUp' ? -1 : 1)];
      if (next) {
        next.focus();
        const nr = rows[idx + (e.key === 'ArrowUp' ? -1 : 1)];
        if (cursor != null) live = `${nr.name}: ${summary(nr, cursor)}`;
      }
    }
  }
  function focusRow(r: Row) {
    cursor ??= lastCell(r);
    live = `${r.name}: ${summary(r, cursor)}`;
  }

  function outLabel(r: Row): string {
    if (!r.outs.length) return 'No outages';
    return plural(r.outs.length, 'outage');
  }
  const pill = $derived(cursor == null ? null : { x: ((cursor + 0.5) / N) * 100, text: span(cursor) });
</script>

<div class="record">
  <div class="head" aria-hidden="true">
    <span class="h-name">Resolver</span>
    <div class="h-axis">
      {#each ticks as t}
        {#if t.label}<span class="tick" class:hid={pill && Math.abs(pill.x - (t.x / N) * 100) < 9} style:left="{(t.x / N) * 100}%">{t.label}</span>{/if}
      {/each}
      {#if pill}<span class="pill" style:left="clamp(60px, {pill.x}%, calc(100% - 60px))">{pill.text}</span>{/if}
    </div>
    <span class="h-y">ms</span>
    <span class="h-num">Answered</span>
    <span class="h-num">Median</span>
    <span class="h-num">p95</span>
  </div>

  <div class="rows">
    {#each rows as r, idx (r.h.resolver_id)}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
      <div
        class="row"
        role="group"
        tabindex="0"
        aria-label="{r.name}: {pct(r.h.query_success)} of queries answered, median {ms(r.h.p50)}, p95 {ms(r.h.p95)}, {outLabel(r)}. Left and right arrows read each {cellH === 1 ? 'hour' : `${cellH} hours`}."
        onkeydown={(e) => key(e, r, idx)}
        onfocus={() => focusRow(r)}
      >
        <div class="name">
          <span class="nm">{r.name}</span>
          <span class="sub" class:bad={r.outs.length > 0}>{outLabel(r)}</span>
        </div>
        <!-- svelte-ignore a11y_no_static_element_interactions (pointer detail only; the row's label and arrow keys carry it) -->
        <div class="plot" onpointermove={(e) => hover(e, r)} onpointerleave={leave}>
          <svg class="heat" viewBox="0 0 {N} {B}" preserveAspectRatio="none" aria-hidden="true">
            {#if r.gaps}<path class="gap" d={r.gaps} />{/if}
            {#each gridY as g}<line class="grid" x1="0" x2={N} y1={yOf(g)} y2={yOf(g)} vector-effect="non-scaling-stroke" />{/each}
            {#each ticks as t}<line class="grid day" x1={t.x} x2={t.x} y1="0" y2={B} vector-effect="non-scaling-stroke" />{/each}
            {#each r.heat as d, l}{#if d}<path class="bin" d={d} fill-opacity={LEVELS[l]} />{/if}{/each}
            {#each r.outs as o}
              <rect class="out" x={o.x0} width={Math.max(0, o.x1 - o.x0)} y="0" height={B} />
              <line class="out-edge" x1={o.x0} x2={o.x0} y1="0" y2={B} vector-effect="non-scaling-stroke" />
            {/each}
            {#if r.median}
              <path class="med-ring" d={r.median} vector-effect="non-scaling-stroke" />
              <path class="med" d={r.median} vector-effect="non-scaling-stroke" />
            {/if}
            {#if nowX < N}<line class="now" x1={nowX} x2={nowX} y1="0" y2={B} vector-effect="non-scaling-stroke" />{/if}
          </svg>
          <svg class="loss" viewBox="0 0 {N} 1" preserveAspectRatio="none" aria-hidden="true">
            {#each ['up', 'some', 'most', 'down'] as k}{#if r.loss[k as 'up']}<path class={k} d={r.loss[k as 'up']} />{/if}{/each}
          </svg>
          {#each r.marks as m}<span class="mark" style:left="{(m.x / N) * 100}%" aria-hidden="true"></span>{/each}
          {#if cursor != null}<span class="cur" style:left="{(cursor / N) * 100}%" style:width="max(1px, {100 / N}%)" aria-hidden="true"></span>{/if}
        </div>
        <div class="yax" aria-hidden="true">
          <svg class="marg" viewBox="0 0 1 {B}" preserveAspectRatio="none"><path d={r.marginal} /></svg>
          {#each gridY as g}<span class="ylab" style:top="{(yOf(g) / B) * 100}%">{g >= 1000 ? `${g / 1000}s` : g}</span>{/each}
        </div>
        <span class="num" class:bad={(r.h.query_success ?? 1) < 0.99}>{pct(r.h.query_success)}</span>
        <span class="num">{ms(r.h.p50)}</span>
        <span class="num">{ms(r.h.p95)}</span>
      </div>
    {/each}
  </div>
  <p class="visually-hidden" aria-live="polite">{live}</p>
</div>

{#if tip}
  <div class="tip chart-tip" style:left="{tip.x}px" style:top="{tip.y}px" role="tooltip">{tip.text}</div>
{/if}

<style>
  .record {
    container: record / inline-size;
    --cols: 196px minmax(0, 1fr) 46px 64px 64px 64px;
  }
  .head,
  .row {
    display: grid;
    grid-template-columns: var(--cols);
    column-gap: 14px;
    align-items: center;
  }
  .head {
    height: 28px;
    font-size: var(--t-2xs);
    color: var(--ink-3);
    border-bottom: 1px solid var(--rule);
  }
  .h-num {
    text-align: right;
  }
  .h-y {
    padding-left: 18px;
  }
  .h-axis {
    position: relative;
    height: 100%;
  }
  .tick,
  .pill {
    position: absolute;
    top: 50%;
    transform: translate(-50%, -50%);
    white-space: nowrap;
  }
  .tick {
    transform: translate(4px, -50%);
  }
  .tick.hid {
    opacity: 0;
  }
  .pill {
    background: var(--ink);
    color: var(--surface);
    font-weight: 600;
    padding: 1px 7px;
    border-radius: var(--r-sm);
    font-variant-numeric: tabular-nums;
  }
  .row {
    padding: 10px 0;
    border-bottom: 1px solid var(--rule);
    border-radius: var(--r-sm);
  }
  .row:last-child {
    border-bottom: 0;
  }
  .row:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  .name {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .nm {
    font-size: var(--t-sm);
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sub {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .sub.bad::before {
    content: '';
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-right: 6px;
    vertical-align: 1px;
    background: var(--critical);
    transform: rotate(45deg);
  }
  .plot {
    position: relative;
    display: grid;
    grid-template-rows: 56px 9px;
    row-gap: 3px;
  }
  .plot svg {
    display: block;
    width: 100%;
    height: 100%;
    overflow: visible;
  }
  .heat {
    shape-rendering: crispEdges;
  }
  .gap {
    fill: var(--surface-2);
  }
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }
  .grid.day {
    stroke-dasharray: 2 3;
  }
  .bin {
    fill: var(--ink);
  }
  .out {
    fill: var(--critical);
    fill-opacity: 0.16;
  }
  .out-edge {
    stroke: var(--critical);
    stroke-width: 1.5;
  }
  .med-ring,
  .med {
    fill: none;
    stroke-linejoin: round;
    stroke-linecap: round;
    shape-rendering: geometricPrecision;
  }
  .med-ring {
    stroke: var(--surface);
    stroke-width: 3.5;
  }
  .med {
    stroke: var(--ink);
    stroke-width: 1.25;
  }
  .now {
    stroke: var(--ink-3);
    stroke-width: 1;
  }
  .loss {
    shape-rendering: crispEdges;
  }
  .loss .up {
    fill: var(--rule-2);
  }
  .loss .some {
    fill: var(--warning);
  }
  .loss .most {
    fill: var(--serious);
  }
  .loss .down {
    fill: var(--critical);
  }
  .mark {
    position: absolute;
    top: -4px;
    width: 7px;
    height: 7px;
    margin-left: -3.5px;
    border: 1.5px solid var(--ink-2);
    border-radius: 50%;
    background: var(--surface);
    pointer-events: none;
  }
  .cur {
    position: absolute;
    top: -6px;
    bottom: -6px;
    background: color-mix(in oklch, var(--ink) 9%, transparent);
    box-shadow: inset 1px 0 0 color-mix(in oklch, var(--ink) 30%, transparent);
    pointer-events: none;
  }
  .yax {
    position: relative;
    align-self: start;
    height: 56px;
  }
  .marg {
    position: absolute;
    inset: 0 auto 0 0;
    width: 14px;
    height: 100%;
  }
  .marg path {
    fill: var(--ink);
    fill-opacity: 0.4;
  }
  .ylab {
    position: absolute;
    left: 18px;
    transform: translateY(-50%);
    font-size: var(--t-2xs);
    color: var(--ink-3);
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .num {
    text-align: right;
    font-size: var(--t-sm);
  }
  .bad.num {
    color: var(--critical);
    font-weight: 600;
  }
  .tip {
    color: var(--ink);
    padding: 5px 9px;
  }
  @container record (max-width: 760px) {
    .head,
    .row {
      --cols: minmax(0, 1fr) 46px 60px 60px 60px;
    }
    .h-name {
      display: none;
    }
    .name {
      grid-column: 1 / -1;
      grid-template-columns: auto auto;
      justify-content: start;
      align-items: baseline;
      column-gap: 10px;
      margin-bottom: 6px;
    }
  }
</style>
