<script lang="ts">
  // The run in time: every query where it was sent, so a slow or failing stretch that hit every resolver at once
  // reads as the network from here, not as one resolver.
  import type { Sample, TestKind } from '../lib/types';
  import { logDomain, logScale, logTicks, tickLabel } from '../lib/scale';
  import { TEST_LABEL } from '../lib/format';

  let {
    samples,
    target,
    name,
  }: {
    samples: Sample[];
    /** The fastest pick's target: its queries are drawn by test, the rest in grey. */
    target: number;
    name: string;
  } = $props();

  const FAILED = new Set(['timeout', 'unreachable', 'neterr', 'servfail', 'refused', 'rcode']);
  // Same window the engine excuses failures over.
  const BLIP_MS = 2000;
  const H = 136;
  const L = 34;
  const BAND = 8;
  const BOTTOM = H - 16;

  let w = $state(800);
  // From the first query, not the run's start: setup and warm-up send nothing that is kept.
  const start = $derived(samples.reduce((m, s) => Math.min(m, s.at), Infinity));
  const end = $derived(Math.max(start + 1, samples.reduce((m, s) => Math.max(m, s.at), 0)));
  const domain = $derived.by(() => {
    let lo = Infinity;
    let hi = 0;
    for (const s of samples) {
      if (s.o !== 'ok') continue;
      lo = Math.min(lo, s.us / 1000);
      hi = Math.max(hi, s.us / 1000);
    }
    return logDomain(lo, hi);
  });
  const y = $derived(logScale(domain, BOTTOM, BAND + 6));
  const x = (at: number) => L + ((at - start) / (end - start)) * (w - L - 2);
  const yTicks = $derived(logTicks(domain, (BOTTOM - BAND) * 0.6));
  const STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1200];
  const step = $derived((STEPS.find((s) => ((end - start) / 1000 / s) * 70 <= w - L) ?? 1800) * 1000);
  const xTicks = $derived.by(() => {
    const out: number[] = [];
    for (let t = Math.ceil(start / step) * step; t <= end; t += step) out.push(t);
    return out;
  });
  const xLabel = (t: number) => (t >= 60000 && t % 60000 === 0 ? `${t / 60000} min` : `${t / 1000} s`);

  // Drawn as strings, as the ranking's dots are: a run holds thousands of queries.
  const dots = $derived.by(() => {
    let rest = '';
    let pick = '';
    for (const s of samples) {
      if (s.o !== 'ok') continue;
      const c = `cx="${x(s.at).toFixed(1)}" cy="${y(s.us / 1000).toFixed(1)}"`;
      if (s.t === target) pick += `<circle class="dot ${s.k}" ${c} r="2"/>`;
      else rest += `<circle class="dot rest" ${c} r="1.2"/>`;
    }
    return rest + pick;
  });
  const fails = $derived(
    samples
      .filter((s) => FAILED.has(s.o))
      .map((s) => `<rect class="fail${s.t === target ? ' own' : ''}" x="${(x(s.at) - 0.5).toFixed(1)}" y="0" width="1" height="${BAND}"/>`)
      .join(''),
  );
  const blips = $derived([...new Set(samples.filter((s) => s.x).map((s) => Math.floor(s.at / BLIP_MS)))]);
  const tests = $derived([...new Set(samples.filter((s) => s.t === target && s.o === 'ok').map((s) => s.k))] as TestKind[]);
  const label = $derived(
    `Every query by the time it was sent, over ${Math.round((end - start) / 1000)} seconds. ${name} by test, the other resolvers in grey.` +
      (blips.length ? ` ${blips.length} moments failed across several resolvers at once.` : ''),
  );
</script>

<figure class="tl">
  <figcaption>
    <span>Every query as it was sent. {name}’s by test, the others in grey; failures mark the top edge.</span>
    <span class="legend">
      {#each tests as k}<span><i class="sw {k}"></i>{TEST_LABEL[k]}</span>{/each}
      {#if blips.length}<span><i class="sw blip"></i>Network blip</span>{/if}
    </span>
  </figcaption>
  <div class="plot" bind:clientWidth={w}>
    <svg width={w} height={H} role="img" aria-label={label}>
      {#each blips as b}<rect class="blip" x={x(b * BLIP_MS)} y="0" width={Math.max(2, x((b + 1) * BLIP_MS) - x(b * BLIP_MS))} height={BOTTOM} />{/each}
      {#each yTicks as t}
        <line class="grid-l" x1={L} x2={w} y1={y(t)} y2={y(t)} />
        <text class="ax" x={L - 6} y={y(t) + 3} text-anchor="end">{tickLabel(t)}</text>
      {/each}
      <text class="ax" x={L - 6} y={BAND + 2} text-anchor="end">ms</text>
      {#each xTicks as t}<text class="ax" x={x(t)} y={H - 2} text-anchor="middle">{xLabel(t)}</text>{/each}
      <line class="base" x1={L} x2={w} y1={BAND + 0.5} y2={BAND + 0.5} />
      {@html fails}
      {@html dots}
    </svg>
  </div>
</figure>

<style>
  .tl {
    margin: 0;
    min-width: 0;
  }
  figcaption {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 4px 16px;
    margin-bottom: 8px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .legend {
    display: flex;
    gap: 14px;
    color: var(--ink-3);
  }
  .sw {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: 5px;
    border-radius: 1.5px;
  }
  .sw.cached {
    background: var(--s-cached);
  }
  .sw.uncached {
    background: var(--s-uncached);
  }
  .sw.tld {
    background: var(--s-tld);
  }
  .sw.blip {
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--rule-2);
  }
  svg {
    display: block;
    overflow: visible;
  }
  .grid-l,
  .base {
    stroke: var(--rule);
  }
  .ax {
    font-size: 10px;
    fill: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
  .blip {
    fill: var(--surface-2);
  }
  .plot :global(.fail) {
    fill: var(--ink-3);
    opacity: 0.6;
  }
  .plot :global(.fail.own) {
    fill: var(--ink);
    opacity: 1;
  }
  .plot :global(.dot.rest) {
    fill: var(--ink-3);
    opacity: 0.3;
  }
  .plot :global(.dot.cached) {
    fill: var(--s-cached);
  }
  .plot :global(.dot.uncached) {
    fill: var(--s-uncached);
  }
  .plot :global(.dot.tld) {
    fill: var(--s-tld);
  }
</style>
