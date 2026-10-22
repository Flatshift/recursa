<script lang="ts">
  // The route to a resolver: a spine of hops on the left, grouped into the networks that carry them, and every round's
  // round trip on a shared log axis to the right. Ink throughout; only the resolver's answer wears the brand colour.
  import type { Trace, TraceHop } from '../lib/types';
  import { holder, ms } from '../lib/format';
  import { logScale, logTicks, tickLabel } from '../lib/scale';
  import { reduceMotion } from '../lib/motion';

  let { trace, busy = false, rtt = null }: { trace: Trace | null; busy?: boolean; rtt?: number | null } = $props();

  const ROW = 21;
  const BAND = 15;
  const AXIS = 20;
  const STAGGER = reduceMotion ? 0 : 45;
  const ttlX = 22;
  const spineX = 34;
  const textX = 46;
  const rail = 6;

  let w = $state(360);
  const axX0 = $derived(Math.max(196, Math.round(w * 0.56)));
  const axX1 = $derived(w - rail - 10);

  type Row =
    | { kind: 'mac'; y: number }
    | { kind: 'band'; y: number; label: string }
    | { kind: 'hop'; y: number; hop: TraceHop; i: number; last: boolean }
    | { kind: 'end'; y: number; after: number };

  // Consecutive hops in one network form a band; a silent TTL stays in the band it interrupts.
  function bandKey(h: TraceHop, prev: string | null): string | null {
    if (!h.addrs.length) return prev;
    if (h.private) return 'private';
    return h.asn != null ? `as${h.asn}` : 'unknown';
  }
  function bandLabel(h: TraceHop): string {
    if (h.private) return 'your network';
    if (h.asn == null) return 'network not announced';
    const name = holder(h.holder);
    return name ? `AS${h.asn} ${name}` : `AS${h.asn}`;
  }

  const rows = $derived.by((): Row[] => {
    const out: Row[] = [{ kind: 'mac', y: 0 }];
    let y = ROW;
    if (!trace) {
      // Waiting: a short dashed spine to an unanswered ring.
      out.push({ kind: 'end', y: y + ROW * 2, after: 0 });
      return out;
    }
    let key: string | null = null;
    trace.hops.forEach((h, i) => {
      const k = bandKey(h, key);
      if (k && k !== key) {
        out.push({ kind: 'band', y, label: bandLabel(h) });
        y += BAND;
        key = k;
      }
      out.push({ kind: 'hop', y, hop: h, i, last: i === trace.hops.length - 1 });
      y += ROW;
    });
    if (!trace.reached) out.push({ kind: 'end', y, after: trace.hops.length });
    return out;
  });
  const height = $derived(rows[rows.length - 1].y + ROW + AXIS);
  const cy = (r: Row) => r.y + (r.kind === 'band' ? BAND : ROW) / 2;
  const hopRows = $derived(rows.filter((r) => r.kind === 'hop') as Extract<Row, { kind: 'hop' }>[]);

  // The axis spans this trace alone, from a little under its fastest reply, as the dive's timeline does.
  const domain = $derived.by((): [number, number] | null => {
    const v = (trace?.hops ?? []).flatMap((h) => h.samples_ms.filter((s): s is number => s != null && s > 0));
    if (!v.length) return null;
    const lo = Math.min(...v) / 1.6;
    return [lo, Math.max(Math.max(...v) * 1.05, lo * 2)];
  });
  const x = $derived(domain ? logScale(domain, axX0, axX1) : null);
  const ticks = $derived(domain ? logTicks(domain, axX1 - axX0) : []);

  const frac = (n: number) => n - Math.floor(n);
  const jitter = (ttl: number, round: number) => (frac(Math.sin(ttl * 12.9898 + (round + 1) * 78.233) * 43758.5453) * 2 - 1) * 4;
  function median(h: TraceHop): number | null {
    const v = h.samples_ms.filter((s): s is number => s != null).sort((a, b) => a - b);
    return v.length ? v[Math.floor((v.length - 1) / 2)] : null;
  }
  function lossClass(h: TraceHop): string | null {
    if (!h.sent || h.recv >= h.sent) return null;
    if (!h.recv) return 'down';
    return h.recv / h.sent <= 0.5 ? 'most' : 'some';
  }
  // Band labels sit in ink-3 at the 2xs size, about 6 px a glyph; no SVG ellipsis, so they are cut by hand.
  function fit(s: string, px: number): string {
    const n = Math.floor(px / 6);
    return s.length <= n ? s : `${s.slice(0, Math.max(3, n - 1)).trimEnd()}…`;
  }

  // Spine segments: dashed through a silent TTL or into the unanswered end.
  const spine = $derived.by(() => {
    const pts = rows.filter((r) => r.kind !== 'band');
    const segs: { y1: number; y2: number; gap: boolean }[] = [];
    for (let i = 1; i < pts.length; i++) {
      const a = pts[i - 1], b = pts[i];
      const silent = (r: Row) => r.kind === 'end' || (r.kind === 'hop' && !r.hop.addrs.length);
      segs.push({ y1: cy(a), y2: cy(b), gap: silent(a) || silent(b) });
    }
    return segs;
  });

  // The POST returns the whole route; the drawing then plays it: spine first, rows down the page, dots in from the
  // left, the answer last.
  function draw(el: SVGGElement, on: boolean) {
    if (!on || reduceMotion) return;
    el.animate([{ clipPath: 'inset(0 0 100% 0)' }, { clipPath: 'inset(0 0 0 0)' }], { duration: 420, easing: 'cubic-bezier(0.22, 1, 0.36, 1)', fill: 'backwards' });
  }
  function stream(el: SVGCircleElement, { dx, delay }: { dx: number; delay: number }) {
    if (reduceMotion) return;
    el.animate(
      [
        { transform: `translateX(${-dx}px)`, opacity: 0, easing: 'cubic-bezier(0.16, 1, 0.3, 1)' },
        { transform: 'translateX(0)', opacity: 1, offset: 0.62 },
        {},
      ],
      { duration: 420 / 0.62, delay, fill: 'backwards' },
    );
  }
  function contract(el: SVGCircleElement, delay: number) {
    if (reduceMotion) return;
    el.animate([{ opacity: 0, transform: 'scale(2.4)' }, {}], { duration: 520, delay, easing: 'cubic-bezier(0.22, 1, 0.36, 1)', fill: 'backwards' });
  }
  const delayOf = (i: number) => 180 + i * STAGGER;

  // Hover: the row lifts and a tooltip lists every address and round; over the axis a crosshair reads its value.
  let hover = $state<number | null>(null);
  let tip = $state<{ x: number; y: number; hop: TraceHop } | null>(null);
  let tipW = $state(0);
  // Centred on the pointer, but kept inside the window: the Inspector is narrow.
  const tipX = $derived(tip ? Math.min(Math.max(tip.x, tipW / 2 + 8), window.innerWidth - tipW / 2 - 8) : 0);
  let cross = $state<number | null>(null);
  function move(e: PointerEvent) {
    const svg = e.currentTarget as SVGSVGElement;
    const r = svg.getBoundingClientRect();
    const px = e.clientX - r.left, py = e.clientY - r.top;
    const row = hopRows.find((h) => py >= h.y && py < h.y + ROW) ?? null;
    hover = row?.i ?? null;
    tip = row ? { x: e.clientX, y: r.top + row.y, hop: row.hop } : null;
    cross = x && px >= axX0 && px <= axX1 ? px : null;
  }
  function leave() {
    hover = null;
    tip = null;
    cross = null;
  }
  const crossMs = $derived.by(() => {
    if (cross == null || !domain) return null;
    const a = Math.log10(domain[0]);
    return Math.pow(10, a + ((cross - axX0) / (axX1 - axX0)) * (Math.log10(domain[1]) - a));
  });

  const summary = $derived.by(() => {
    if (!trace) return null;
    const last = trace.hops[trace.hops.length - 1];
    if (!trace.reached || !last) {
      const answered = trace.hops.filter((h) => h.addrs.length).length;
      return answered ? `No reply after hop ${trace.hops.length}. The resolver's network may not answer probes.` : 'Nothing on the path answered.';
    }
    const med = median(last);
    let s = `Hop ${last.ttl} answered ${ms(med)} median of ${trace.rounds}.`;
    if (rtt != null) s += ` Your query's TCP round trip measured ${ms(rtt)}.`;
    return s;
  });
  function aria(): string {
    if (!trace) return 'Tracing the route';
    return `Route: ${trace.hops.map((h) => `hop ${h.ttl} ${h.addrs[0] ?? 'no reply'}${h.asn != null ? ` AS${h.asn}` : ''} ${ms(median(h))}`).join(', ')}`;
  }
</script>

<figure class="route" class:busy bind:clientWidth={w}>
  {#key trace}
    <svg width={w} {height} role="img" aria-label={aria()} onpointermove={trace ? move : undefined} onpointerleave={leave}>
      <!-- Networks: alternating washes behind the rows, the holder once per band. -->
      {#each rows as r, j (j)}
        {#if r.kind === 'band'}
          {@const next = rows.slice(j + 1).findIndex((n) => n.kind === 'band')}
          {@const end = next < 0 ? rows[rows.length - 1].y + (rows[rows.length - 1].kind === 'end' ? 0 : ROW) : rows[j + 1 + next].y}
          <rect class="band" class:alt={rows.slice(0, j).filter((n) => n.kind === 'band').length % 2 === 1} x="0" y={r.y} width={w} height={end - r.y} />
          <text class="holder" x={textX} y={r.y + BAND - 4}>{fit(r.label, axX0 - textX - 8)}</text>
        {/if}
      {/each}
      {#if hover != null}
        {@const hr = hopRows[hover]}
        <rect class="lift" x="0" y={hr.y} width={w} height={ROW} />
      {/if}

      <!-- Axis: hairlines at the decades, labels below. -->
      {#if x}
        {#each ticks as t (t)}
          <line class="grid" x1={x(t)} x2={x(t)} y1={ROW} y2={height - AXIS} />
          <text class="xl" x={x(t)} y={height - 6} text-anchor={x(t) > axX1 - 18 ? 'end' : 'middle'}>{tickLabel(t)}</text>
        {/each}
      {/if}

      <!-- Spine, drawn downward. -->
      <g use:draw={!!trace}>
        {#each spine as s, i (i)}
          <line class="spine" class:gap={s.gap} x1={spineX} x2={spineX} y1={s.y1} y2={s.y2} />
        {/each}
      </g>

      {#each rows as r, j (j)}
        {#if r.kind === 'mac'}
          <circle class="ring" cx={spineX} cy={cy(r)} r="3.5" />
          <text class="label" x={textX} y={cy(r) + 3.5}>this Mac</text>
        {:else if r.kind === 'end'}
          <circle class="ring gap" cx={spineX} cy={cy(r)} r="3.5" />
          <text class="label" x={textX} y={cy(r) + 3.5}>{trace ? `no reply after hop ${r.after}` : 'tracing…'}</text>
        {:else if r.kind === 'hop'}
          {@const h = r.hop}
          {@const y = cy(r)}
          {@const med = median(h)}
          {@const loss = lossClass(h)}
          <g class="hop" style:--d="{delayOf(r.i)}ms">
            <text class="ttl" x={ttlX} y={y + 3.5} text-anchor="end">{h.ttl}</text>
            {#if !h.addrs.length}
              <circle class="ring gap" cx={spineX} cy={y} r="3.5" />
              <text class="label" x={textX} y={y + 3.5}>no reply</text>
            {:else}
              {#if r.last && trace?.reached}
                <circle class="halo" cx={spineX} cy={y} r="5.5" />
                <circle class="answer" cx={spineX} cy={y} r="3.5" use:contract={delayOf(r.i) + 200} />
              {:else}
                <circle class="dot" cx={spineX} cy={y} r="3" />
              {/if}
              <text class="addr" x={textX} y={y + 3.5}>
                {h.addrs[0]}{#if h.addrs.length > 1}<tspan class="more"> +{h.addrs.length - 1}</tspan>{/if}{#if loss}<tspan class="more"> · {h.recv}/{h.sent}</tspan>{/if}
              </text>
            {/if}
            {#if x}
              {#each h.samples_ms as s, k}
                {#if s != null}
                  <circle class="rt" cx={x(s)} cy={y + jitter(h.ttl, k)} r="1.8" use:stream={{ dx: x(s) - axX0, delay: delayOf(r.i) + k * 40 }} />
                {/if}
              {/each}
              {#if med != null}
                <rect class="med-ring" x={x(med) - 2.25} y={y - 7.5} width="4.5" height="15" rx="2.25" />
                <rect class="med" x={x(med) - 0.75} y={y - 6.5} width="1.5" height="13" rx="0.75" />
              {/if}
            {/if}
            {#if loss}
              <rect class="loss {loss}" x={w - rail} y={r.y + 2} width={rail} height={ROW - 4} rx="1" />
            {/if}
          </g>
        {/if}
      {/each}

      {#if cross != null && crossMs != null}
        <line class="cross" x1={cross} x2={cross} y1={ROW} y2={height - AXIS} />
        <g class="pill" transform="translate({Math.min(Math.max(cross, axX0 + 22), axX1 - 22)}, {ROW / 2})">
          <rect x="-22" y="-7" width="44" height="14" rx="7" />
          <text y="3.5" text-anchor="middle">{ms(crossMs)}</text>
        </g>
      {/if}
    </svg>
  {/key}
  <figcaption>
    {#if summary}<span class="sum">{summary}</span>{/if}
    <span class="credit">Networks named by Team Cymru. Trace by Trippy.</span>
  </figcaption>
</figure>

{#if tip}
  <div class="tip" style:left="{tipX}px" style:top="{tip.y}px" role="tooltip" bind:clientWidth={tipW}>
    <div class="tt-h">Hop {tip.hop.ttl}{#if tip.hop.addrs.length} · <span class="mono">{tip.hop.addrs.join(', ')}</span>{/if}</div>
    {#if tip.hop.addrs.length}
      <div class="tt-s">{tip.hop.private ? 'Your network' : tip.hop.asn != null ? `AS${tip.hop.asn} ${holder(tip.hop.holder)}` : 'Network not announced'}</div>
    {/if}
    <div class="tt-v num">{tip.hop.samples_ms.map((s) => (s == null ? '—' : ms(s, false))).join(' · ')} ms</div>
    <div class="tt-s">{tip.hop.recv} of {tip.hop.sent} probes answered{tip.hop.best_ms != null ? ` · best ${ms(tip.hop.best_ms)}` : ''}</div>
  </div>
{/if}

<style>
  .route {
    margin: 10px 0 0;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .band {
    fill: transparent;
  }
  .band.alt {
    fill: var(--surface-2);
  }
  .lift {
    fill: var(--sel);
  }
  .holder,
  .label,
  .ttl,
  .xl {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
  }
  .ttl,
  .xl {
    font-variation-settings: 'MONO' 1;
  }
  .label {
    font-size: var(--t-xs);
  }
  .addr {
    font-size: var(--t-xs);
    fill: var(--ink);
    font-variation-settings: 'MONO' 1;
  }
  .more {
    fill: var(--ink-3);
  }
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }
  .spine {
    stroke: var(--ink-3);
    stroke-width: 1.2;
  }
  .spine.gap {
    stroke-dasharray: 2 3;
  }
  .ring {
    fill: var(--surface);
    stroke: var(--ink-3);
    stroke-width: 1.25;
  }
  .ring.gap {
    stroke-dasharray: 2 2;
  }
  .dot {
    fill: var(--ink);
  }
  .halo {
    fill: var(--surface);
  }
  .answer {
    fill: var(--brand-answer);
    transform-box: fill-box;
    transform-origin: center;
  }
  .rt {
    fill: var(--ink-2);
  }
  .med-ring {
    fill: var(--surface);
  }
  .med {
    fill: var(--ink);
  }
  .loss {
    shape-rendering: crispEdges;
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
  .cross {
    stroke: var(--ink-3);
    stroke-width: 1;
  }
  .pill rect {
    fill: var(--ink);
  }
  .pill text {
    fill: var(--surface);
    font-size: var(--t-2xs);
    font-variation-settings: 'MONO' 1;
  }
  /* Rows appear top to bottom; each median tick fades in after its dots have landed. */
  .hop {
    animation: appear 200ms var(--ease-out) var(--d) both;
  }
  .hop .med,
  .hop .med-ring {
    animation: appear 200ms var(--ease-out) calc(var(--d) + 560ms) both;
  }
  @keyframes appear {
    from {
      opacity: 0;
    }
  }
  .busy svg {
    opacity: 0.7;
  }
  figcaption {
    display: grid;
    gap: 2px;
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--rule);
    font-size: var(--t-xs);
  }
  .sum {
    color: var(--ink-2);
  }
  .credit {
    color: var(--ink-3);
  }
  .tip {
    position: fixed;
    z-index: var(--z-tooltip);
    transform: translate(-50%, calc(-100% - 8px));
    pointer-events: none;
    background: var(--surface);
    border-radius: var(--r-md);
    box-shadow: var(--shadow-pop);
    padding: 7px 10px;
    font-size: var(--t-xs);
    white-space: nowrap;
  }
  .tt-h {
    color: var(--ink-2);
  }
  .tt-v {
    font-size: var(--t-md);
    font-weight: 620;
  }
  .tt-s {
    color: var(--ink-3);
  }
  @media (prefers-reduced-motion: reduce) {
    .hop,
    .hop .med,
    .hop .med-ring {
      animation: none;
    }
  }
</style>
