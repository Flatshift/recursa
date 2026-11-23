<script lang="ts">
  // The share of each resolver's queries answered within a time, on a shared log axis. A curve further left is faster;
  // a lost query never comes back, so a curve ends short of 100% by its losses.
  import type { ExtResult } from '../lib/types';
  import { colourOf, ecdfPoints, markOf, markPath } from '../lib/extended';
  import { logDomain, logScale, logTicks, tickLabel } from '../lib/scale';

  let { ecdf, names }: { ecdf: ExtResult['ecdf']; names: string[] } = $props();
  const height = 200;
  const padL = 40;
  const padR = 12;
  const padT = 8;
  const padB = 24;

  let w = $state(320);
  const xs = $derived(ecdf.x.filter((v) => v > 0));
  const domain = $derived(logDomain(Math.min(...xs), Math.max(...xs)));
  const x = $derived(logScale(domain, padL, w - padR));
  const y = (p: number) => padT + (1 - p) * (height - padT - padB);
  const ticks = $derived(logTicks(domain, w - padL - padR));
  const lines = $derived(
    names.map((name, t) => {
      const pts = ecdfPoints(ecdf, t).filter(([v]) => v > 0);
      // A few marks along each line, staggered by resolver so they do not stack.
      const at = pts.filter((_, i) => (i + t * 3) % 12 === 0);
      return { t, name, d: pts.map(([v, s], i) => `${i ? 'L' : 'M'}${x(v).toFixed(1)},${y(s).toFixed(1)}`).join(''), at: at.map(([v, s]) => markPath(markOf(t), x(v), y(s), 3.5)) };
    }),
  );
</script>

<div class="ecdf" bind:clientWidth={w}>
  <svg width={w} {height} role="img" aria-label="Share of queries answered within each time, for {names.join(', ')}.">
    {#each [0, 0.25, 0.5, 0.75, 1] as p}
      <line class="grid" x1={padL} x2={w - padR} y1={y(p)} y2={y(p)} />
      <text class="yl" x={padL - 6} y={y(p) + 3.5} text-anchor="end">{p * 100}%</text>
    {/each}
    {#each ticks as t}
      <text class="xl" x={x(t)} y={height - 7} text-anchor="middle">{tickLabel(t)}</text>
    {/each}
    {#each lines as l}
      <path d={l.d} fill="none" stroke={colourOf(l.t)} stroke-width="1.75" stroke-linejoin="round" />
      {#each l.at as m}<path d={m} fill={colourOf(l.t)} />{/each}
    {/each}
  </svg>
</div>

<style>
  .ecdf {
    width: 100%;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }
  text {
    font-size: var(--t-xs);
    fill: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
</style>
