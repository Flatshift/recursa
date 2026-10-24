<script lang="ts">
  // Where a resolver's time goes, in milliseconds beyond the wire: zero is the wire floor (best TCP handshake), the
  // cached median and the uncached median sit at what they add to it, with the resolver's and recursion's shares
  // bracketed between them. Linear, so a 3 ms share reads as a third of a 9 ms one. One strip per transport.
  import type { PathInfo } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { TRANSPORT_LABEL, ms } from '../lib/format';
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

  // Milliseconds beyond the wire. The axis starts at zero (or just below it when cached answers beat the wire) and
  // runs past the slowest mark, never shorter than 2 ms so a resolver that adds nothing still reads as a chart.
  const domain = $derived.by<[number, number]>(() => {
    let lo = 0;
    let hi = 2;
    for (const { s } of strips) {
      for (const v of [s.cached, s.uncached]) {
        if (v == null) continue;
        lo = Math.min(lo, v - s.wire);
        hi = Math.max(hi, v - s.wire);
      }
    }
    const span = hi - lo;
    return [lo < 0 ? lo - span * 0.05 : 0, hi + span * 0.1];
  });
  const x = $derived.by(() => {
    const [a, b] = domain;
    const k = (x1 - x0) / (b - a);
    return (v: number) => x0 + (v - a) * k;
  });
  // Round tick steps (1, 2, 5 × 10ⁿ) about 70 px apart.
  const ticks = $derived.by(() => {
    const [a, b] = domain;
    const raw = (b - a) / Math.max(1, (x1 - x0) / 70);
    const mag = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 5, 10].map((m) => m * mag).find((t) => t >= raw) ?? 10 * mag;
    const out: number[] = [];
    for (let t = Math.ceil(a / step) * step; t <= b; t += step) out.push(Math.abs(t) < step / 2 ? 0 : +t.toFixed(6));
    return out;
  });
  const tickText = (t: number) => (t === 0 ? 'wire' : `${t > 0 ? '+' : '−'}${ms(Math.abs(t), false)}`);
  // Labels at either end hang inward so they aren't clipped.
  const tickAnchor = (t: number) => (x(t) - x0 < 14 ? 'start' : x1 - x(t) < 14 ? 'end' : 'middle');

  // Brackets under a strip: resolver (floor → cached) and recursion (cached → uncached), labelled when they fit.
  function brackets(s: Split): { a: number; b: number; text: string }[] {
    const out: { a: number; b: number; text: string }[] = [];
    const seg = (from: number, to: number, v: number | null) => {
      if (v == null || to <= from) return;
      out.push({ a: x(from - s.wire), b: x(to - s.wire), text: `+${ms(v, false)}` });
    };
    if (s.cached != null) seg(s.wire, s.cached, s.resolver);
    if (s.uncached != null) seg(s.cached ?? s.wire, s.uncached, s.recursion);
    return out;
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
        <text class="xl" x={x(t)} y={height - 4} text-anchor={tickAnchor(t)}>{tickText(t)}</text>
      {/each}
      {#each strips as { y, label, s } (label)}
        {#if multi}<text class="lane" x="0" y={y + 3.5}>{label}</text>{/if}
        <line class="rule" x1={x0} x2={x1} y1={y} y2={y} />
        {#each brackets(s) as br (br.a)}
          <path class="br" d="M{br.a} {y + 7} v4 H{br.b} v-4" />
          {#if br.b - br.a >= 30}<text class="brl" x={(br.a + br.b) / 2} y={y + 20} text-anchor="middle">{br.text}</text>{/if}
        {/each}
        <rect class="wire" x={x(0) - 0.75} y={y - 8} width="1.5" height="16" rx="0.75" />
        {#if s.uncached != null}<circle class="unc" cx={x(s.uncached - s.wire)} cy={y} r="3" />{/if}
        {#if s.cached != null}
          <circle class="halo" cx={x(s.cached - s.wire)} cy={y} r="5.5" />
          <circle class="answer" cx={x(s.cached - s.wire)} cy={y} r="3.5" />
        {/if}
      {/each}
    </svg>
    <figcaption>
      <p class="sum">{summary}</p>
      <p class="legend">
        <span class="mark wire-k"></span>wire {ms(strips[0].s.wire, false)} ms · <span class="mark ans-k"></span>cached median{#if strips[0].s.uncached != null} · <span class="mark unc-k"></span>uncached median{/if}
      </p>
    </figcaption>
  </figure>
{/if}
{#if tip}
  <div class="chart-tip" style:left="{tipX}px" style:top="{tip.y}px" role="tooltip" bind:clientWidth={tipW}>
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
  .brl {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variation-settings: 'MONO' 1;
  }
  .lane {
    font-variation-settings: normal;
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
</style>
