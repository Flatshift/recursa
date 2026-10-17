<script lang="ts">
  // Empirical CDF of answered queries per test, on a shared log axis. A curve
  // further left is faster; a steep curve is consistent.
  import type { Dist } from '../lib/verdict';
  import type { TestKind } from '../lib/types';
  import { TEST_LABEL, ms } from '../lib/format';
  import { logInvert, logScale, logTicks } from '../lib/scale';
  import { mobile } from '../lib/api';

  let { dists, domain }: { dists: Partial<Record<TestKind, Dist>>; domain: [number, number] } = $props();
  const height = 120;

  let w = $state(320);
  // The phone's larger axis labels need wider gutters, or "100%" and the last tick clip.
  const padL = mobile ? 40 : 30;
  const padR = mobile ? 14 : 8;
  const padT = 6;
  const padB = 20;
  const x = $derived(logScale(domain, padL, w - padR));
  const y = (p: number) => padT + (1 - p) * (height - padT - padB);
  const ticks = $derived(logTicks(domain, w - padL - padR));
  const entries = $derived(Object.entries(dists).filter(([, d]) => d && d.n > 0) as [TestKind, Dist][]);

  function path(d: Dist): string {
    const n = d.values.length;
    let s = `M${x(d.values[0])},${y(0)}`;
    d.values.forEach((v, i) => {
      s += `L${x(v)},${y(i / n)}L${x(v)},${y((i + 1) / n)}`;
    });
    return s;
  }

  let hx = $state<number | null>(null);
  const hoverMs = $derived(hx == null ? null : logInvert(domain, padL, w - padR, hx));
  const share = (d: Dist, v: number) => {
    let i = 0;
    while (i < d.values.length && d.values[i] <= v) i++;
    return i / d.values.length;
  };
</script>

<div class="ecdf" bind:clientWidth={w}>
  <svg
    width={w}
    {height}
    role="img"
    aria-label="Cumulative distribution of response times: {entries.map(([k, d]) => `${TEST_LABEL[k]} median ${ms(d.p50)}`).join(', ')}"
    onpointermove={(e) => {
      const r = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
      const px = e.clientX - r.left;
      hx = px >= padL && px <= w - padR ? px : null;
    }}
    onpointerleave={() => (hx = null)}
  >
    {#each [0, 0.5, 1] as p}
      <line class="grid" x1={padL} x2={w - padR} y1={y(p)} y2={y(p)} />
      <text class="yl" x={padL - 6} y={y(p) + 3.5} text-anchor="end">{p * 100}%</text>
    {/each}
    {#each ticks as t}
      <text class="xl" x={x(t)} y={height - 5} text-anchor="middle">{t}</text>
    {/each}
    {#each entries as [k, d]}
      <path class="line {k}" d={path(d)} />
    {/each}
    {#if hx != null}
      <line class="cross" x1={hx} x2={hx} y1={padT} y2={height - padB} />
    {/if}
  </svg>
  {#if hoverMs != null}
    <div class="read num">
      ≤ {ms(hoverMs)}:
      {#each entries as [k, d]}<span><i class="key {k}"></i>{Math.round(share(d, hoverMs) * 100)}%</span>{/each}
    </div>
  {/if}
</div>

<style>
  .ecdf {
    position: relative;
  }
  svg {
    display: block;
  }
  .grid {
    stroke: var(--rule);
  }
  .yl,
  .xl {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variation-settings: 'MONO' 1;
  }
  .line {
    fill: none;
    stroke-width: 2;
    stroke-linejoin: round;
  }
  .line.cached {
    stroke: var(--s-cached);
  }
  .line.uncached {
    stroke: var(--s-uncached);
  }
  .line.tld {
    stroke: var(--s-tld);
  }
  .cross {
    stroke: var(--ink-3);
    stroke-width: 1;
  }
  .read {
    position: absolute;
    top: 0;
    right: 8px;
    font-size: var(--t-xs);
    color: var(--ink-2);
    display: flex;
    gap: 8px;
    background: var(--surface);
    padding: 0 4px;
  }
  .read span {
    display: inline-flex;
    gap: 4px;
    align-items: center;
  }
</style>
