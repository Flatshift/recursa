<script lang="ts">
  // One pick's graph: what that goal was decided on, small enough to sit in the sheet's row.
  import type { PickGraph } from '../lib/verdict';
  import type { Sample } from '../lib/types';
  import { logDomain, logScale, logTicks, tickLabel } from '../lib/scale';
  import { jitterOf } from '../lib/motion';
  import { ms } from '../lib/format';

  let {
    g,
    samples,
    domain,
    axis = false,
  }: {
    g: PickGraph;
    /** The winner's answered queries (dots graphs only). */
    samples: Sample[];
    /** The log axis every dots graph in the sheet shares. */
    domain: [number, number];
    /** Label the shared log axis under this dots graph (the sheet's last speed row). */
    axis?: boolean;
  } = $props();

  let w = $state(180);
  // Height comes from the sheet (--pg-h), so wide sheets can give the graphs more room.
  let H = $state(16);
  const R = 2;
  // Room under the marks for an axis's labels.
  const AX = 13;
  const strip = $derived(g.kind === 'edges' || g.kind === 'spread');
  const P = $derived(axis || strip ? H - AX : H);
  const x = $derived(logScale(domain, R + 1, w - R - 1));
  const ticks = $derived([0.1, 1, 10, 100, 1000].filter((t) => t > domain[0] && t < domain[1]));
  // Tick labels, dropping any that would crowd the one before.
  function labels(ts: number[], at: (v: number) => number, unit = '') {
    const out: { x: number; t: string; a: string }[] = [];
    for (const t of ts) {
      const tx = at(t);
      if (out.length && tx - out[out.length - 1].x < 44) continue;
      out.push({ x: tx, t: unit + (t >= 1000 ? tickLabel(t) : `${t} ms`), a: tx < 20 ? 'start' : tx > w - 20 ? 'end' : 'middle' });
    }
    return out;
  }
  const tickLabels = $derived(labels(ticks, x));
  // Dots as one string of circles, as in the ranking: the same stable jitter, so a dot sits the same way everywhere.
  const dots = $derived(
    g.kind === 'dots'
      ? samples
          .filter((s) => s.t === g.target && s.o === 'ok' && g.tests.includes(s.k))
          .map((s) => `<circle class="dot ${s.k}" cx="${x(s.us / 1000).toFixed(1)}" cy="${(P / 2 + jitterOf(s) * (P / 2 - R - 1)).toFixed(1)}" r="${R}"/>`)
          .join('')
      : '',
  );
  // Edges and spread: every candidate a tick on one linear axis from zero, the winner the dot; nearer zero wins.
  // Candidates on the same pixel share a tick and a tooltip.
  const cands = $derived(g.kind === 'edges' || g.kind === 'spread' ? g.cands : []);
  const vmax = $derived(Math.max(0.1, ...cands.map((c) => c.v)));
  // Distance from the edges is linear from zero; spread is on a log axis, as one slow resolver would squash the rest.
  const sdom = $derived(logDomain(Math.min(...cands.map((c) => c.v)), vmax));
  const sx = $derived(g.kind === 'spread' ? logScale(sdom, 5, w - 5) : (v: number) => 5 + (v / vmax) * (w - 10));
  const sTicks = $derived(g.kind === 'spread' ? logTicks(sdom, w).filter((t) => t > sdom[0] && t < sdom[1]) : []);
  const sLabels = $derived(labels(sTicks, sx, '±'));
  const marks = $derived.by(() => {
    const at = new Map<number, { x: number; v: number; names: string[]; win: boolean }>();
    cands.forEach((c, i) => {
      const mx = Math.round(sx(c.v));
      const m = at.get(mx);
      if (m) m.names.push(c.name);
      else at.set(mx, { x: mx, v: c.v, names: [c.name], win: i === 0 });
    });
    const out = [...at.values()].sort((a, b) => a.x - b.x);
    // Each tick answers the pointer from halfway to its neighbours.
    return out.map((m, i) => ({ ...m, l: i ? (out[i - 1].x + m.x) / 2 : 0, r: i < out.length - 1 ? (m.x + out[i + 1].x) / 2 : w }));
  });
  const edgeV = (v: number) => (v < 0.05 ? 'best edges' : `+${ms(v)}`);
  const stackSum = $derived(g.kind === 'stack' ? g.terms.reduce((a, t) => a + t.v, 0) : 0);
  const bar = $derived(Math.round(H / 2));
  // Checks: one group per property across the width, one pip per resolver in each; named when there is room.
  const GG = 14;
  const gw = $derived(g.kind === 'checks' ? (w - GG * (g.rows.length - 1)) / g.rows.length : 0);
  const named = $derived(gw >= 110 && H >= 20);
  const pip = $derived(g.kind === 'checks' && g.cols.length ? Math.max(2, Math.min(14, (gw - 2 * (g.cols.length - 1)) / g.cols.length)) : 0);
  // Square pips under the names; a short strip when there is no room for them.
  const pipH = $derived(named ? Math.min(pip, H - 13) : Math.min(8, H));
  const label = $derived(
    g.kind === 'dots'
      ? `Every answered query on a log axis, the score at ${ms(g.at)}`
      : g.kind === 'edges'
        ? `Distance from the best CDN edges: ` + g.cands.map((c) => `${c.name} ${edgeV(c.v)}`).join(', ')
        : g.kind === 'spread'
          ? `Answer-time spread: ` + g.cands.map((c) => `${c.name} ±${ms(c.v)}`).join(', ')
          : g.kind === 'checks'
            ? `Rows: ${g.rows.join(', ')}. ` + g.cols.map((c) => `${c.name}: ${c.on.filter(Boolean).length} of ${g.rows.length}`).join('; ')
            : g.terms.map((t) => `${t.label} ${ms(t.v)}`).join(', '),
  );
</script>

<!-- Parts carry data-h/-v/-s: the sheet's tooltip shows them in place of the row's. -->
<span class="pg" class:axis class:strip bind:clientWidth={w} bind:clientHeight={H}>
  <svg width={w} height={H} role="img" aria-label={label}>
    {#if g.kind === 'dots'}
      {#each ticks as t}<line class="grid-l" x1={x(t)} x2={x(t)} y1="0" y2={P} />{/each}
      {@html dots}
      {#if g.at != null}<rect class="mark" x={x(g.at) - 1} y="0" width="2" height={P} rx="1" />{/if}
      {#if axis}{#each tickLabels as l}<text class="cap" x={l.x} y={H - 2} text-anchor={l.a}>{l.t}</text>{/each}{/if}
    {:else if g.kind === 'edges' || g.kind === 'spread'}
      {#each sTicks as t}<line class="grid-l" x1={sx(t)} x2={sx(t)} y1="0" y2={P} />{/each}
      <line class="base" x1="0" x2={w} y1={P / 2} y2={P / 2} />
      {#each marks as m}
        <g
          class:win={m.win}
          data-h={m.names.join(', ')}
          data-v={g.kind === 'edges' ? edgeV(m.v) : `±${ms(m.v)}`}
          data-s={g.kind === 'edges' ? 'per new connection, from the best CDN edges' : 'answer-time standard deviation'}
        >
          <rect class="hit" x={m.l} y="0" width={m.r - m.l} height={P} />
          {#if !m.win}<rect class="tick" x={m.x - 0.75} y={P / 2 - 5} width="1.5" height="10" />{/if}
        </g>
      {/each}
      {#each marks.filter((m) => m.win) as m}<circle class="end" cx={m.x} cy={P / 2} r="4.5" />{/each}
      {#if g.kind === 'edges'}
        <text class="cap" x="0" y={H - 2}>best edges</text>
        <text class="cap" x={w} y={H - 2} text-anchor="end">+{ms(vmax)}</text>
      {:else}
        {#each sLabels as l}<text class="cap" x={l.x} y={H - 2} text-anchor={l.a}>{l.t}</text>{/each}
      {/if}
    {:else if g.kind === 'checks'}
      {#each g.rows as row, j}
        {#if named}<text class="cap" x={j * (gw + GG)} y="10">{row}</text>{/if}
        {#each g.cols as c, i}
          <rect
            class="cell"
            class:on={c.on[j]}
            class:win={c.win}
            x={j * (gw + GG) + i * (pip + 2)}
            y={named ? H - pipH : (H - pipH) / 2}
            width={pip}
            height={pipH}
            rx="1.5"
            data-h={c.name}
            data-v={c.on[j] ? 'Yes' : 'No'}
            data-s={row}
          />
        {/each}
      {/each}
    {:else}
      {#each g.terms as t, i}
        {@const x0 = g.terms.slice(0, i).reduce((a, u) => a + u.v, 0)}
        <rect class="seg {t.c}" data-h={t.label} data-v={ms(t.v)} x={(w * x0) / (stackSum || 1)} y={(H - bar) / 2} width={Math.max(0, (w * t.v) / (stackSum || 1) - 1)} height={bar} />
      {/each}
    {/if}
  </svg>
</span>

<style>
  .pg {
    display: block;
    min-width: 0;
    height: var(--pg-h, 16px);
    align-self: center;
  }
  .pg.axis {
    height: calc(var(--pg-h, 16px) + 13px);
  }
  /* A strip is the same height everywhere: the marks and the labels under them. */
  .pg.strip {
    height: 30px;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .grid-l {
    stroke: var(--rule);
    stroke-width: 1;
  }
  .pg :global(.dot) {
    opacity: 0.55;
  }
  .pg :global(.dot.cached),
  .seg.cached {
    fill: var(--s-cached);
  }
  .pg :global(.dot.uncached),
  .seg.uncached {
    fill: var(--s-uncached);
  }
  .pg :global(.dot.tld),
  .seg.tld {
    fill: var(--s-tld);
  }
  .seg.neutral {
    fill: var(--s-neutral);
  }
  .mark {
    fill: var(--ink);
  }
  .base {
    stroke: var(--rule);
  }
  .hit {
    fill: transparent;
  }
  .tick {
    fill: var(--ink-3);
  }
  .end {
    fill: var(--ink);
    stroke: var(--surface);
    stroke-width: 1.5;
  }
  .cap {
    font-size: 11px;
    fill: var(--ink-3);
  }
  .cell {
    fill: var(--rule);
  }
  .cell.on {
    fill: var(--ink-3);
  }
  .cell.on.win {
    fill: var(--ink);
  }
</style>
