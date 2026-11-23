<script lang="ts">
  // Response time over time on one shared axis: the medians together on a log scale, then one small panel per series
  // with its median line, middle half (p25 to p75) and nine in ten (p5 to p95), and under each a strip of lost queries.
  // Gaps and left-out stretches are hatched across every panel and named. Pointer and arrow keys share one cursor.
  import type { OverTimeGap, OverTimeMark, OverTimeOutage, OverTimeSeries } from '../lib/overtime';
  import { bandPath, fitSpans, lastFilled, linePath, lossClass, lossHeight, slotAt, slotGrid, spreadLabels, timeTicks } from '../lib/overtime';
  import { logDomain, logScale, logTicks, tickLabel } from '../lib/scale';
  import { ms } from '../lib/format';

  let {
    series,
    gaps,
    marks = [],
    outages = [],
    cursor = $bindable(null),
    formatTime,
    unit,
  }: {
    series: OverTimeSeries[];
    gaps: OverTimeGap[];
    marks?: OverTimeMark[];
    outages?: OverTimeOutage[];
    /** The slot under the pointer or the keyboard, shared with whatever else lists these times. */
    cursor?: number | null;
    formatTime: (t: number) => string;
    /** What one cell is called: "15-second bin", "hour". */
    unit: string;
  } = $props();

  const uid = $props.id();
  const PAD_L = 44;
  const OV_TOP = 30;
  const OV_H = 84;
  const OV_BOTTOM = OV_TOP + OV_H;

  let W = $state(0);
  let sel = $state<number | null>(null);
  let tip = $state<{ x: number; y: number; text: string } | null>(null);
  let live = $state('');

  const grid = $derived(slotGrid(series));
  const N = $derived(grid?.n ?? 0);
  const wide = $derived(W >= 620);
  const padR = $derived(wide ? 128 : 10);
  const pw = $derived(Math.max(40, W - PAD_L - padR));
  const slotW = $derived(N ? pw / N : pw);
  // More than four series: nothing is coloured by identity; the panels carry the names.
  const many = $derived(series.length > 4);
  const colour = (i: number) => (many ? 'var(--ink)' : `var(--r-${i + 1})`);
  const panelH = $derived(many ? 44 : 58);
  const pitch = $derived(panelH + 38);
  const first = $derived(OV_BOTTOM + 34);
  const rowTop = (i: number) => first + i * pitch;
  const lastBottom = $derived(series.length ? rowTop(series.length - 1) + panelH + 16 : OV_BOTTOM);
  const H = $derived(lastBottom + 30);

  const xb = (i: number) => PAD_L + i * slotW;
  const xm = (i: number) => xb(i) + slotW / 2;
  const startOf = (i: number) => (grid ? grid.base + i * grid.width : 0);
  const tx = (t: number) => (grid ? Math.min(PAD_L + pw, Math.max(PAD_L, PAD_L + ((t - grid.base) / grid.width) * slotW)) : PAD_L);

  function domainOf(pick: (c: NonNullable<OverTimeSeries['cells'][number]>) => (number | null)[]): [number, number] {
    let lo = Infinity;
    let hi = 0;
    for (const s of series)
      for (const c of s.cells) if (c) for (const v of pick(c)) if (v != null && v > 0) (lo = Math.min(lo, v)), (hi = Math.max(hi, v));
    return logDomain(lo, hi);
  }
  const domOv = $derived(domainOf((c) => [c.p50]));
  const domP = $derived(domainOf((c) => [c.p5 ?? c.p50, c.p95 ?? c.p50]));
  const yOv = $derived(logScale(domOv, OV_BOTTOM, OV_TOP));
  const ticksOv = $derived(logTicks(domOv, OV_H));
  const ticksP = $derived(logTicks(domP, panelH));

  const hwLine = $derived(Math.min(slotW / 2, 4));
  const slotXs = $derived(Array.from({ length: N }, (_, i) => xm(i)));
  const cellsOf = (s: OverTimeSeries) => Array.from({ length: N }, (_, i) => s.cells[i] ?? null);

  const together = $derived(
    series.map((s, i) => {
      const cells = cellsOf(s);
      const ys = cells.map((c) => (c?.p50 != null ? yOv(c.p50) : null));
      const last = lastFilled(ys);
      return { i, d: linePath(slotXs, ys, hwLine), last, ly: last >= 0 ? ys[last]! : 0 };
    }),
  );
  // End-of-line names, pushed apart so close lines keep separate labels.
  const ends = $derived.by(() => {
    const seen = together.filter((t) => t.last >= 0);
    if (many) return [];
    const ys = spreadLabels(seen.map((t) => t.ly), 16, OV_TOP + 6, OV_BOTTOM - 4);
    return seen.map((t, k) => ({ i: t.i, x: xm(t.last), y: t.ly, labelY: ys[k] }));
  });
  const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, Math.max(1, n - 1))}…` : s);

  const panels = $derived(
    series.map((s, i) => {
      const top = rowTop(i);
      const bottom = top + panelH;
      const y = logScale(domP, bottom, top);
      const cells = cellsOf(s);
      const ys = (pick: 'p5' | 'p25' | 'p50' | 'p75' | 'p95') => cells.map((c) => (c?.[pick] != null ? y(c[pick]!) : null));
      const bars: { x: number; h: number; cls: string }[] = [];
      const thin: number[] = [];
      const empty: [number, number][] = [];
      let lost = 0;
      let from = -1;
      cells.forEach((c, k) => {
        if (!c || c.sent === 0) {
          if (from < 0) from = k;
        } else if (from >= 0) {
          empty.push([from, k]);
          from = -1;
        }
        if (!c) return;
        lost += c.lost;
        const cls = lossClass(c);
        if (cls === 'thin') thin.push(k);
        else if (cls) bars.push({ x: xb(k), h: lossHeight(c, cls) * 8, cls });
      });
      if (from >= 0) empty.push([from, N]);
      return {
        i,
        top,
        bottom,
        bandOuter: bandPath(slotXs, ys('p95'), ys('p5'), slotW / 2),
        bandInner: bandPath(slotXs, ys('p75'), ys('p25'), slotW / 2),
        median: linePath(slotXs, ys('p50'), hwLine),
        bars,
        thin,
        empty,
        lost,
        outs: outages
          .filter((o) => o.id === s.id)
          .map((o) => ({ x0: tx(o.t0), x1: tx(o.t1), shared: o.shared }))
          .filter((o) => o.x1 > o.x0),
        marks: marks
          .filter((m) => m.id === s.id && grid && m.t >= grid.base && m.t < grid.base + N * grid.width)
          .map((m) => ({ x: tx(m.t), t: m.t, title: m.title })),
        y,
      };
    }),
  );

  // Gaps named above the first panel; one that would run into another name stays unnamed (its tooltip still says it).
  const gapBoxes = $derived.by(() => {
    const boxes = gaps.map((g) => ({ g, x0: tx(g.t0), x1: tx(g.t1) })).filter((b) => b.x1 > b.x0);
    const wOf = (s: string) => s.length * 6.2;
    const placed = boxes.map((b) => {
      const w = wOf(b.g.label);
      const end = b.x0 - 6 - w >= PAD_L;
      return { ...b, anchor: end ? 'end' : 'start', tx: end ? b.x0 - 6 : b.x0 + 4, span: (end ? [b.x0 - 6 - w, b.x0 - 6] : [b.x0 + 4, b.x0 + 4 + w]) as [number, number] };
    });
    const fit = fitSpans([[PAD_L, PAD_L + 112], ...placed.map((p) => p.span)]).slice(1);
    return placed.map((p, k) => ({ ...p, named: fit[k] }));
  });

  const cur = $derived(cursor != null && cursor >= 0 && cursor < N ? cursor : null);
  const pillText = $derived(cur == null ? '' : formatTime(startOf(cur)));
  const pillW = $derived(pillText.length * 6.4 + 16);
  const pillX = $derived(cur == null ? 0 : Math.max(PAD_L + pillW / 2, Math.min(PAD_L + pw - pillW / 2, xm(cur))));

  const axis = $derived.by(() => {
    if (!grid) return [];
    const ts = timeTicks(grid.base, grid.base + N * grid.width, wide ? 9 : 4);
    const items = ts.map((t) => ({ t, x: tx(t), label: formatTime(t) })).filter((it, k, a) => !k || it.label !== a[k - 1].label);
    const spans = items.map((it) => [it.x - it.label.length * 3.2, it.x + it.label.length * 3.2] as [number, number]);
    const ok = fitSpans(spans, 10);
    return items.filter((it, k) => ok[k] && spans[k][1] <= W);
  });

  const gapAt = (i: number) => gaps.find((g) => g.t1 > startOf(i) && g.t0 < startOf(i + 1));
  function summary(si: number, i: number): string {
    const s = series[si];
    const c = s.cells[i] ?? null;
    const t0 = c?.t0 ?? startOf(i);
    const t1 = c?.t1 ?? startOf(i + 1);
    const gap = gapAt(i);
    const parts = [`${formatTime(t0)} – ${formatTime(t1)}`];
    if (!c || c.sent === 0) parts.push(gap?.label ?? 'no checks');
    else {
      parts.push(`${c.sent - c.lost} of ${c.sent} answered`, `median ${ms(c.p50)}`, `p95 ${ms(c.p95)}`);
      if (gap) parts.push(gap.label);
      if (c.note) parts.push(c.note);
    }
    const out = outages.find((o) => o.id === s.id && !o.shared && o.t1 > t0 && o.t0 < t1);
    if (out) parts.push(out.label);
    return parts.join(' · ');
  }
  /** One series at one slot, for the read-out line. */
  function readOf(si: number, i: number): string {
    const c = series[si].cells[i] ?? null;
    if (!c || c.sent === 0) return gapAt(i)?.label ?? 'no checks';
    const spread = c.p5 != null && c.p95 != null ? `, 9 in 10 between ${ms(c.p5, false)} and ${ms(c.p95)}` : '';
    return `${ms(c.p50)}${spread}, ${c.lost} lost`;
  }
  const shownSeries = $derived(many ? (sel == null ? [] : [sel]) : series.map((_, k) => k));
  const plain = (i: number) => `${formatTime(startOf(i))}. ${shownSeries.map((k) => `${series[k].name}: ${readOf(k, i)}`).join('; ')}`;

  function panelAt(py: number): number | null {
    for (let i = 0; i < series.length; i++) if (py >= rowTop(i) - 22 && py <= rowTop(i) + panelH + 14) return i;
    return null;
  }
  function hover(e: PointerEvent) {
    const box = (e.currentTarget as SVGElement).getBoundingClientRect();
    const px = e.clientX - box.left;
    const py = e.clientY - box.top;
    const i = slotAt(px, PAD_L, pw, N);
    if (i == null) return leave();
    cursor = i;
    const p = panelAt(py);
    sel = p;
    if (p == null) tip = null;
    else {
      const m = panels[p].marks.find((k) => Math.abs(k.x - px) < 6 && py - panels[p].top < 10);
      tip = { x: e.clientX, y: box.top + panels[p].top, text: m ? `Operator reported, ${formatTime(m.t)}: ${m.title}` : summary(p, i) };
    }
  }
  function leave() {
    tip = null;
    cursor = null;
    sel = null;
  }
  const lastSlot = $derived(series.reduce((m, s) => Math.max(m, lastFilled(s.cells)), 0));
  function key(e: KeyboardEvent) {
    if (!N) return;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight' || e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      const at = cur ?? lastSlot;
      cursor = e.key === 'Home' ? 0 : e.key === 'End' ? lastSlot : Math.min(N - 1, Math.max(0, at + (e.key === 'ArrowLeft' ? -1 : 1)));
      live = plain(cursor);
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      const d = e.key === 'ArrowUp' ? -1 : 1;
      sel = Math.min(series.length - 1, Math.max(0, (sel ?? (d > 0 ? -1 : series.length)) + d));
      cursor ??= lastSlot;
      live = plain(cursor);
    }
  }
  function focus() {
    cursor ??= lastSlot;
    live = plain(cursor);
  }
  const label = $derived(
    `Response time over time, one ${unit} per step. ${series.map((s) => `${s.name}${s.sub ? `, ${s.sub}` : ''}`).join('; ')}. Left and right arrows read each ${unit}; up and down arrows pick a series.`,
  );
</script>

{#snippet shape(i: number, x: number, y: number)}
  {#if i % 4 === 0}<circle class="m" cx={x} cy={y} r="3.6" />
  {:else if i % 4 === 1}<rect class="m" x={x - 3.3} y={y - 3.3} width="6.6" height="6.6" />
  {:else if i % 4 === 2}<path class="m" d="M{x},{y - 4.2}L{x + 4.2},{y + 3.4}L{x - 4.2},{y + 3.4}Z" />
  {:else}<path class="m" d="M{x},{y - 4.4}L{x + 4.4},{y}L{x},{y + 4.4}L{x - 4.4},{y}Z" />{/if}
{/snippet}

<div class="ot" bind:clientWidth={W}>
  {#if W > 0 && grid && series.length}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <svg
      width={W}
      height={H}
      viewBox="0 0 {W} {H}"
      role="group"
      tabindex="0"
      aria-label={label}
      onpointermove={hover}
      onpointerleave={leave}
      onkeydown={key}
      onfocus={focus}
    >
      <defs>
        <pattern id="{uid}-hatch" width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
          <rect width="5" height="5" class="hatch-bg" />
          <line x1="0" y1="0" x2="0" y2="5" class="hatch" />
        </pattern>
      </defs>

      {#if sel != null && panels[sel]}
        <rect class="sel" x="0" y={panels[sel].top - 22} width={W} height={panelH + 36} />
      {/if}

      {#each gapBoxes as b}
        <rect class="gap" x={b.x0} y={OV_TOP} width={Math.max(3, b.x1 - b.x0)} height={lastBottom - OV_TOP} fill="url(#{uid}-hatch)" />
        <line class="ref" x1={b.x0} x2={b.x0} y1={OV_TOP} y2={lastBottom} />
        <line class="ref" x1={b.x1} x2={b.x1} y1={OV_TOP} y2={lastBottom} />
        {#if b.named}
          <line class="ref" x1={b.x0} x2={b.x0} y1={OV_TOP - 14} y2={OV_TOP} />
          <text class="note" x={b.tx} y={OV_TOP - 6} text-anchor={b.anchor}>{b.g.label}</text>
        {/if}
      {/each}

      <!-- Medians together -->
      <text class="rowname" x={PAD_L} y="12">Medians together</text>
      <text class="tick" x={PAD_L - 8} y={OV_TOP - 6} text-anchor="end">ms</text>
      {#each ticksOv as g}
        <line class="grid" x1={PAD_L} x2={PAD_L + pw} y1={yOv(g)} y2={yOv(g)} />
        <text class="tick" x={PAD_L - 8} y={yOv(g) + 3.5} text-anchor="end">{tickLabel(g)}</text>
      {/each}
      {#each together as t (t.i)}
        {#if t.d && !(many && sel === t.i)}
          <path class="ln" class:neutral={many} style:--c={many ? 'var(--s-neutral)' : colour(t.i)} d={t.d} />
        {/if}
      {/each}
      {#if many && sel != null && together[sel]?.d}<path class="ln pick" d={together[sel].d} />{/if}
      {#each ends as e (e.i)}
        <g style:--c={colour(e.i)}>
          {#if wide}
            {@render shape(e.i, e.x + 10, e.labelY)}
            <text class="endlabel" x={e.x + 20} y={e.labelY + 4}>{clip(series[e.i].name, Math.floor((padR - 24) / 6.4))}</text>
          {:else}
            {@render shape(e.i, e.x, e.y)}
          {/if}
        </g>
      {/each}

      <!-- One panel per series -->
      {#each panels as p (p.i)}
        {@const s = series[p.i]}
        <g style:--c={colour(p.i)}>
          {#if !many}{@render shape(p.i, PAD_L + 4, p.top - 14)}{/if}
          <text class="rowname" x={PAD_L + (many ? 0 : 14)} y={p.top - 10}>{s.name}{#if s.sub}<tspan class="sub" dx="8">{clip(s.sub, Math.max(8, Math.floor(pw / 6.2) - s.name.length - 4))}</tspan>{/if}</text>
          {#each ticksP as g}
            <line class="grid" x1={PAD_L} x2={PAD_L + pw} y1={p.y(g)} y2={p.y(g)} />
            <text class="tick" x={PAD_L - 8} y={p.y(g) + 3.5} text-anchor="end">{tickLabel(g)}</text>
          {/each}
          {#each p.empty as [a, z]}<rect class="empty" x={xb(a)} y={p.top} width={(z - a) * slotW} height={panelH} />{/each}
          {#if p.bandOuter}<path class="band2" d={p.bandOuter} />{/if}
          {#if p.bandInner}<path class="band1" d={p.bandInner} />{/if}
          {#each p.outs as o}
            <rect class="out" class:shared={o.shared} x={o.x0} y={p.top} width={o.x1 - o.x0} height={panelH} />
            <line class="out-edge" class:shared={o.shared} x1={o.x0} x2={o.x0} y1={p.top} y2={p.bottom} />
          {/each}
          {#if p.median}
            <path class="med-ring" d={p.median} />
            <path class="med" d={p.median} />
          {/if}
          {#each p.thin as k}<rect class="thin" x={xb(k)} y={p.top} width={slotW} height={panelH + 14} fill="url(#{uid}-hatch)" />{/each}
          {#each p.marks as m}<circle class="mark" cx={m.x} cy={p.top + 5} r="3.5" />{/each}
          <rect class="rail" x={PAD_L} y={p.bottom + 6} width={pw} height="8" />
          <text class="tick" x={PAD_L - 8} y={p.bottom + 13.5} text-anchor="end">lost</text>
          {#each p.bars as b}<rect class="loss {b.cls}" x={b.x + 0.5} y={p.bottom + 14 - b.h} width={Math.max(1.5, slotW - 1)} height={b.h} />{/each}
          {#if wide}<text class="railnote" x={PAD_L + pw + 10} y={p.bottom + 14}>{p.lost.toLocaleString()} lost</text>{/if}
        </g>
      {/each}

      <!-- Shared time axis -->
      {#each axis as a}
        <line class="axis" x1={a.x} x2={a.x} y1={lastBottom} y2={lastBottom + 4} />
        {#if cur == null || Math.abs(a.x - pillX) > pillW / 2 + a.label.length * 3.2 + 4}
          <text class="tick" x={a.x} y={lastBottom + 18} text-anchor="middle">{a.label}</text>
        {/if}
      {/each}
      <line class="axis" x1={PAD_L} x2={PAD_L + pw} y1={lastBottom} y2={lastBottom} />

      {#if cur != null}
        <rect class="cur" x={xb(cur)} y={OV_TOP} width={Math.max(2, slotW)} height={lastBottom - OV_TOP} />
        <rect class="pill" x={pillX - pillW / 2} y={lastBottom + 6} width={pillW} height="18" rx="4" />
        <text class="pilltext" x={pillX} y={lastBottom + 19} text-anchor="middle">{pillText}</text>
      {/if}
    </svg>

    <p class="read" aria-hidden="true">
      {#if cur != null}
        <b class="num">{formatTime(startOf(cur))}</b>
        {#each shownSeries as k (k)}
          <span class="rd" style:--c={colour(k)}>
            {#if !many}<svg class="sw" width="12" height="12" viewBox="-6 -6 12 12">{@render shape(k, 0, 0)}</svg>{/if}<span class="who">{series[k].name}</span>
            <span class="num">{readOf(k, cur)}</span>
          </span>
        {/each}
        {#if many && sel == null}<span class="rd">Point at a panel to read it.</span>{/if}
      {:else}
        Point at the chart, or use the arrow keys, to read one {unit}.
      {/if}
    </p>
    <p class="visually-hidden" aria-live="polite">{live}</p>
  {/if}
</div>

{#if tip}
  <div class="tip chart-tip" style:left="{tip.x}px" style:top="{tip.y}px" role="tooltip">{tip.text}</div>
{/if}

<style>
  .ot {
    width: 100%;
  }
  svg[role='group'] {
    display: block;
    overflow: visible;
    border-radius: var(--r-sm);
  }
  svg[role='group']:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 4px;
  }
  text {
    fill: var(--ink);
    font-family: inherit;
  }
  .tick,
  .railnote,
  .note {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
  .note {
    fill: var(--ink-2);
  }
  .rowname {
    font-size: var(--t-sm);
    font-weight: 600;
  }
  .sub {
    font-size: var(--t-xs);
    font-weight: 400;
    fill: var(--ink-3);
  }
  .endlabel {
    font-size: var(--t-xs);
    font-weight: 600;
  }
  .m {
    fill: var(--c);
    stroke: var(--surface);
    stroke-width: 1;
  }
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }
  .axis {
    stroke: var(--rule-2);
    stroke-width: 1;
  }
  .ref {
    stroke: var(--ink-3);
    stroke-width: 1;
    stroke-opacity: 0.6;
  }
  .sel {
    fill: var(--surface-2);
    fill-opacity: 0.6;
  }
  .hatch-bg {
    fill: var(--surface-2);
  }
  .hatch {
    stroke: var(--ink-3);
    stroke-width: 1;
    stroke-opacity: 0.55;
  }
  .gap {
    pointer-events: none;
  }
  .empty {
    fill: var(--surface-2);
  }
  .thin {
    opacity: 0.85;
    pointer-events: none;
  }
  .ln,
  .med,
  .med-ring {
    fill: none;
    stroke-linejoin: round;
    stroke-linecap: round;
    shape-rendering: geometricPrecision;
  }
  .ln {
    stroke: var(--c);
    stroke-width: 1.75;
  }
  .ln.neutral {
    stroke-width: 1;
    stroke-opacity: 0.7;
  }
  .ln.pick {
    stroke: var(--ink);
    stroke-width: 2;
  }
  .med-ring {
    stroke: var(--surface);
    stroke-width: 4;
  }
  .med {
    stroke: var(--c);
    stroke-width: 1.5;
  }
  .band2 {
    fill: var(--c);
    fill-opacity: 0.13;
  }
  .band1 {
    fill: var(--c);
    fill-opacity: 0.26;
  }
  .out {
    fill: var(--critical);
    fill-opacity: 0.16;
  }
  .out-edge {
    stroke: var(--critical);
    stroke-width: 1.5;
  }
  .out.shared {
    fill: var(--ink-3);
  }
  .out-edge.shared {
    stroke: var(--ink-3);
  }
  .mark {
    fill: var(--surface);
    stroke: var(--ink-2);
    stroke-width: 1.5;
    pointer-events: none;
  }
  .rail {
    fill: var(--surface-2);
  }
  .loss {
    shape-rendering: crispEdges;
  }
  .loss.up {
    fill: var(--rule-2);
  }
  .loss.some {
    fill: var(--warning);
  }
  .loss.most {
    fill: var(--serious);
  }
  .loss.down {
    fill: var(--critical);
  }
  .loss.shared {
    fill: var(--ink-3);
  }
  .cur {
    fill: var(--ink);
    fill-opacity: 0.09;
    pointer-events: none;
  }
  .pill {
    fill: var(--ink);
  }
  .pilltext {
    fill: var(--surface);
    font-size: var(--t-2xs);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .read {
    margin-top: 6px;
    min-height: 2.6em;
    font-size: var(--t-xs);
    color: var(--ink-2);
    display: flex;
    flex-wrap: wrap;
    gap: 2px 16px;
    align-items: center;
  }
  .read b {
    color: var(--ink);
  }
  .rd {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .rd .who {
    color: var(--ink);
    font-weight: 600;
  }
  .sw {
    flex: none;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .tip {
    color: var(--ink);
    padding: 5px 9px;
  }
</style>
