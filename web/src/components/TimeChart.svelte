<script lang="ts">
  // A line chart for the Time page: offsets in ms on a shared axis centred on zero, time across. Drawn at the width it
  // is given, so labels keep their size. Hover or arrow keys read the nearest check.
  import { linePath, timeTicks } from '../lib/overtime';
  import { symmetricTicks, type TimeSeries } from '../lib/ntp';
  import { signedMs } from '../lib/format';

  let { series, since, until, height = 180, label, legend = true }: { series: TimeSeries[]; since: number; until: number; height?: number; label: string; legend?: boolean } = $props();

  const ML = 52;
  const MR = 10;
  const MT = 8;
  const MB = 22;
  let width = $state(600);
  let cur = $state<number | null>(null);

  const span = $derived(Math.max(1, until - since));
  const pw = $derived(Math.max(1, width - ML - MR));
  const ph = $derived(height - MT - MB);
  const x = (ts: number) => ML + ((ts - since) / span) * pw;
  const ticks = $derived.by(() => {
    const m = Math.max(0, ...series.flatMap((s) => s.pts.map((p) => (p.v == null ? 0 : Math.abs(p.v)))));
    return symmetricTicks(m);
  });
  const top = $derived(ticks[ticks.length - 1]);
  const y = (v: number) => MT + ph / 2 - (v / top) * (ph / 2);
  const long = $derived(span > 2 * 86_400_000);
  const xticks = $derived(timeTicks(since, until, Math.max(2, Math.floor(pw / (long ? 70 : 60)))));
  const tickText = (t: number) => {
    const d = new Date(t);
    if (long || (d.getHours() === 0 && d.getMinutes() === 0)) return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
    return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  };
  const stamp = (t: number) => new Date(t).toLocaleString(undefined, { weekday: 'short', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

  const paths = $derived(series.map((s) => linePath(s.pts.map((p) => x(p.ts)), s.pts.map((p) => (p.v == null ? null : y(p.v))), 1.5)));
  const times = $derived([...new Set(series.flatMap((s) => s.pts.map((p) => p.ts)))].sort((a, b) => a - b));
  const byTs = $derived(series.map((s) => new Map(s.pts.map((p) => [p.ts, p.v] as const))));

  // The check the cursor is on, by its time: the same check after the data reloads, or none once it has left the window.
  const at = $derived(cur != null && times.includes(cur) ? cur : null);
  const range = $derived.by(() => {
    const vs = series.flatMap((s) => s.pts.flatMap((p) => (p.v == null ? [] : [p.v])));
    return vs.length ? `, from ${signedMs(Math.min(...vs), true)} to ${signedMs(Math.max(...vs), true)}` : '';
  });

  function nearest(px: number) {
    if (!times.length) return null;
    const t = since + ((px - ML) / pw) * span;
    let best = 0;
    for (let i = 1; i < times.length; i++) if (Math.abs(times[i] - t) < Math.abs(times[best] - t)) best = i;
    return times[best];
  }
  function move(ev: PointerEvent) {
    const r = (ev.currentTarget as SVGElement).getBoundingClientRect();
    cur = nearest(ev.clientX - r.left);
  }
  function key(ev: KeyboardEvent) {
    if (!times.length) return;
    if (ev.key === 'Escape') cur = null;
    const step = ev.key === 'ArrowLeft' ? -1 : ev.key === 'ArrowRight' ? 1 : 0;
    if (!step) return;
    ev.preventDefault();
    const i = at == null ? -1 : times.indexOf(at);
    cur = times[i < 0 ? (step < 0 ? times.length - 1 : 0) : Math.max(0, Math.min(times.length - 1, i + step))];
  }
</script>

<div class="tc">
  {#if legend}
    <ul class="series-key">
      {#each series as s (s.name)}
        <li>
          <svg width="18" height="8" aria-hidden="true"><path d="M0 4H18" stroke={s.color} stroke-width="2" stroke-dasharray={s.dash} fill="none" /></svg>
          {s.name}
        </li>
      {/each}
    </ul>
  {/if}
  <p class="read num" aria-live="polite">
    {#if at != null}
      <span>{stamp(at)}</span>
      {#each series as s, i (s.name)}
        {@const v = byTs[i].get(at)}
        <span>
          {#if legend}<svg width="10" height="8" aria-hidden="true"><path d="M0 4H10" stroke={s.color} stroke-width="2" fill="none" /></svg>{s.name}{/if}
          {v == null ? (byTs[i].has(at) ? 'no answer' : 'no check') : signedMs(v, true)}
        </span>
      {/each}
    {:else}
      <span class="hint">Point at the chart, or focus it and use the arrow keys, to read a check.</span>
    {/if}
  </p>
  <div class="box" bind:clientWidth={width}>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <svg
      viewBox="0 0 {width} {height}"
      {width}
      {height}
      role="img"
      aria-label="{label}{range}"
      tabindex="0"
      onpointermove={move}
      onpointerleave={() => (cur = null)}
      onkeydown={key}
      onblur={() => (cur = null)}
    >
      {#each ticks as t (t)}
        {#if t !== 0}<path d="M{ML} {y(t).toFixed(1)}H{width - MR}" stroke="var(--rule)" stroke-width="1" />{/if}
        <text x={ML - 6} y={y(t)} dy="0.32em" text-anchor="end" class="lab">{signedMs(t, false)}</text>
      {/each}
      <path d="M{ML} {y(0).toFixed(1)}H{width - MR}" stroke="var(--ink-3)" stroke-width="1" />
      {#each xticks as t (t)}
        <path d="M{x(t).toFixed(1)} {MT}V{MT + ph + 4}" stroke="var(--rule)" stroke-width="1" />
        <text x={x(t)} y={height - 5} text-anchor="middle" class="lab">{tickText(t)}</text>
      {/each}
      {#each series as s, i (s.name)}
        <path d={paths[i]} fill="none" stroke={s.color} stroke-width="1.5" stroke-dasharray={s.dash} stroke-linejoin="round" />
      {/each}
      {#if at != null}
        <path d="M{x(at).toFixed(1)} {MT}V{MT + ph}" stroke="var(--ink-2)" stroke-width="1" />
        {#each series as s, i (s.name)}
          {@const v = byTs[i].get(at)}
          {#if v != null}<circle cx={x(at)} cy={y(v)} r="3" fill={s.color} stroke="var(--surface)" stroke-width="1" />{/if}
        {/each}
      {/if}
    </svg>
  </div>
</div>

<style>
  .tc {
    min-width: 0;
  }
  .series-key {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
    list-style: none;
    margin: 0 0 6px;
    padding: 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .series-key li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .read {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 16px;
    min-height: 1.5em;
    margin: 0 0 4px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .read svg {
    margin-right: 4px;
  }
  .hint {
    color: var(--ink-3);
  }
  .box {
    width: 100%;
  }
  .box svg {
    display: block;
    touch-action: pan-y;
  }
  .lab {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
</style>
