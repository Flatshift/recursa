<script lang="ts">
  // Where a resolver's time goes, on one log-ms axis: the wire floor (best TCP handshake), the cached median and
  // the uncached median, with the resolver's and recursion's shares bracketed between them. One strip per transport.
  import type { PathInfo } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { TRANSPORT_LABEL, ms } from '../lib/format';
  import { logScale, logTicks, tickLabel } from '../lib/scale';
  import { sentence, split, type Split } from '../lib/floor';

  let { path, rows, name }: { path: PathInfo; rows: Row[]; name: string } = $props();

  const STRIP = 34;
  const AXIS = 18;
  const TOP = 12;

  let w = $state(320);
  const multi = $derived(rows.length > 1);
  const x0 = $derived(multi ? 40 : 6);
  const x1 = $derived(w - 8);

  type Strip = { y: number; label: string; s: Split };
  const strips = $derived.by<Strip[]>(() => {
    const wire = path.rtt_ms;
    const out: Strip[] = [];
    for (const r of rows) {
      const s = split(wire, r.dist.cached.p50, r.dist.uncached.p50);
      if (s) out.push({ y: TOP + out.length * STRIP + STRIP / 2, label: TRANSPORT_LABEL[r.target.transport], s });
    }
    return out;
  });
  const height = $derived(TOP + strips.length * STRIP + AXIS);

  const domain = $derived.by<[number, number]>(() => {
    const wire = path.rtt_ms ?? 1;
    let hi = wire * 1.5;
    for (const { s } of strips) hi = Math.max(hi, s.cached ?? 0, s.uncached ?? 0);
    return [wire / 1.6, hi * 1.12];
  });
  const x = $derived(logScale(domain, x0, x1));
  const ticks = $derived(logTicks(domain, x1 - x0));

  // Brackets under a strip: resolver (floor → cached) and recursion (cached → uncached), labelled when they fit.
  function brackets(s: Split): { a: number; b: number; text: string }[] {
    const out: { a: number; b: number; text: string }[] = [];
    const seg = (from: number, to: number, v: number | null) => {
      if (v == null || to <= from) return;
      out.push({ a: x(from), b: x(to), text: `+${ms(v, false)}` });
    };
    if (s.cached != null) seg(s.wire, s.cached, s.resolver);
    if (s.uncached != null) seg(s.cached ?? s.wire, s.uncached, s.recursion);
    return out;
  }

  // The floor label sits to the right of its tick unless that runs into the marks or the edge.
  function wireAnchor(s: Split): 'start' | 'end' {
    const near = Math.min(s.cached ?? Infinity, s.uncached ?? Infinity);
    return x(near) - x(s.wire) < 56 || x1 - x(s.wire) < 56 ? 'end' : 'start';
  }

  let tip = $state<{ x: number; y: number; s: Split; label: string } | null>(null);
  let tipW = $state(0);
  const tipX = $derived(tip ? Math.min(Math.max(tip.x, tipW / 2 + 8), window.innerWidth - tipW / 2 - 8) : 0);
  function hover(e: PointerEvent) {
    const svg = e.currentTarget as SVGSVGElement;
    const rect = svg.getBoundingClientRect();
    const py = e.clientY - rect.top;
    const st = strips.find((s) => Math.abs(py - s.y) <= STRIP / 2) ?? strips[0];
    tip = st ? { x: e.clientX, y: rect.top + st.y - 10, s: st.s, label: st.label } : null;
  }

  const summary = $derived(strips.length ? sentence(name, strips[0].s) : '');
</script>

{#if strips.length}
  <figure class="floor" bind:clientWidth={w} aria-label={summary}>
    <svg width={w} {height} onpointermove={hover} onpointerleave={() => (tip = null)} role="img">
      {#each ticks as t (t)}
        <line class="grid" x1={x(t)} x2={x(t)} y1={TOP} y2={height - AXIS} />
        <text class="xl" x={x(t)} y={height - 4} text-anchor="middle">{tickLabel(t)}</text>
      {/each}
      {#each strips as { y, label, s } (label)}
        {#if multi}<text class="lane" x="0" y={y + 3.5}>{label}</text>{/if}
        <line class="rule" x1={x0} x2={x1} y1={y} y2={y} />
        {#each brackets(s) as br (br.a)}
          <path class="br" d="M{br.a} {y + 7} v4 H{br.b} v-4" />
          {#if br.b - br.a >= 30}<text class="brl" x={(br.a + br.b) / 2} y={y + 20} text-anchor="middle">{br.text}</text>{/if}
        {/each}
        <rect class="wire" x={x(s.wire) - 0.75} y={y - 8} width="1.5" height="16" rx="0.75" />
        <text class="wl" x={x(s.wire) + (wireAnchor(s) === 'start' ? 4 : -4)} y={y - 10} text-anchor={wireAnchor(s)}>wire {ms(s.wire, false)}</text>
        {#if s.uncached != null}<circle class="unc" cx={x(s.uncached)} cy={y} r="3" />{/if}
        {#if s.cached != null}
          <circle class="halo" cx={x(s.cached)} cy={y} r="5.5" />
          <circle class="answer" cx={x(s.cached)} cy={y} r="3.5" />
        {/if}
      {/each}
    </svg>
    <figcaption>
      <p class="sum">{summary}</p>
      <p class="legend">
        <span class="mark wire-k"></span>wire · <span class="mark ans-k"></span>cached median{#if strips[0].s.uncached != null} · <span class="mark unc-k"></span>uncached median{/if}
      </p>
    </figcaption>
  </figure>
{/if}
{#if tip}
  <div class="tip" style:left="{tipX}px" style:top="{tip.y}px" role="tooltip" bind:clientWidth={tipW}>
    <div class="tt-h">{tip.label ? `${tip.label} · ` : ''}TCP handshake to port {path.port}</div>
    <div class="tt-v num">{path.samples.map((v: number) => ms(v, false)).join(' · ')} ms</div>
    <div class="tt-s">
      cached median {ms(tip.s.cached)} · uncached median {ms(tip.s.uncached)}
    </div>
  </div>
{/if}

<style>
  .floor {
    margin: 6px 0 0;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }
  .rule {
    stroke: var(--ink-3);
    stroke-width: 0.75;
    opacity: 0.6;
  }
  .xl,
  .lane,
  .wl,
  .brl {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variation-settings: 'MONO' 1;
  }
  .lane {
    font-variation-settings: normal;
  }
  .wl {
    fill: var(--ink-2);
  }
  .br {
    fill: none;
    stroke: var(--ink-3);
    stroke-width: 1;
  }
  .wire {
    fill: var(--ink);
  }
  .unc {
    fill: var(--ink);
  }
  .halo {
    fill: var(--surface);
  }
  .answer {
    fill: var(--brand-answer);
  }
  figcaption {
    margin-top: 4px;
  }
  .sum {
    margin: 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .legend {
    margin: 2px 0 0;
    font-size: var(--t-2xs);
    color: var(--ink-3);
  }
  .mark {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    margin-right: 4px;
    vertical-align: 0;
  }
  .wire-k {
    width: 2px;
    height: 9px;
    border-radius: 1px;
    background: var(--ink);
    vertical-align: -1px;
  }
  .ans-k {
    background: var(--brand-answer);
  }
  .unc-k {
    background: var(--ink);
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
</style>
