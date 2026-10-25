<script lang="ts">
  import type { NtpResult } from '../lib/types';
  import { addrs, num, reachMs, sgn } from '../lib/ntp';

  let { r }: { r: NtpResult } = $props();


  const offset = $derived(r.offset_ms ?? 0);
  const delay = $derived(r.delay_min_ms ?? 0);
  const bound = $derived(r.bound_ms ?? delay / 2);
  const distance = $derived(reachMs(r) ?? bound);
  const rootDelay = $derived(r.root_delay_ms ?? null);
  const rootDisp = $derived(r.root_dispersion_ms ?? null);
  const server = $derived(Math.max(0, distance - bound));
  const nAddr = $derived(addrs(r));

  // The scale is symmetric around this computer's clock (0), wide enough for the whole error interval.
  const range = $derived(Math.max(Math.abs(offset) + distance, 1) * 1.12);
  const pct = (v: number) => 50 + (v / range) * 50;
  const band = (centre: number, half: number) => `left:${pct(centre - half)}%;width:${(half / range) * 100}%`;
  // A 1-2-5 step giving about six marks; bounded so a wildly-off clock can't stall the page.
  const tick = $derived.by(() => {
    if (!Number.isFinite(range) || range <= 0) return 1;
    const raw = range / 3;
    const pow = 10 ** Math.floor(Math.log10(raw));
    const f = raw / pow;
    return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * pow;
  });
  const marks = $derived.by(() => {
    const out: number[] = [];
    if (!Number.isFinite(range) || !Number.isFinite(tick)) return out;
    for (let v = -Math.floor(range / tick) * tick, i = 0; v <= range && i < 20; v += tick, i++) out.push(v);
    return out;
  });

  const half = $derived(delay / 2);
</script>

<div class="work">
  <div class="fig">
    <h3>One query</h3>
    <svg viewBox="0 0 360 150" role="img" aria-label="A query: you send at T1, the server receives at T2 and replies at T3, you receive at T4.">
      <defs>
        <marker id="ah-{r.id}" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M0 0 L8 4 L0 8 z" fill="currentColor" />
        </marker>
      </defs>
      <line class="lane" x1="14" y1="34" x2="346" y2="34" />
      <line class="lane" x1="14" y1="108" x2="346" y2="108" />
      <text class="lab" x="14" y="22">your computer</text>
      <text class="lab" x="14" y="128">the server</text>
      <line class="leg" x1="70" y1="34" x2="164" y2="108" marker-end="url(#ah-{r.id})" />
      <line class="leg" x1="176" y1="108" x2="270" y2="34" marker-end="url(#ah-{r.id})" />
      <circle cx="70" cy="34" r="3.5" /><circle cx="164" cy="108" r="3.5" /><circle cx="176" cy="108" r="3.5" /><circle cx="270" cy="34" r="3.5" />
      <text class="t" x="70" y="52" text-anchor="middle">T1</text>
      <text class="t" x="270" y="52" text-anchor="middle">T4</text>
      <text class="t" x="164" y="96" text-anchor="end">T2</text>
      <text class="t" x="176" y="96" text-anchor="start">T3</text>
      <path class="brace" d="M70 10 H270" />
      <text class="lab c" x="170" y="6" text-anchor="middle">round trip {num(delay)} ms</text>
      <text class="lab" x="70" y="144">each leg drawn as {num(half, 2)} ms</text>
    </svg>
    <p class="cap">
      T1 you send, T2 it arrives, T3 it replies, T4 you receive. T2 and T3 are read off the server's clock, T1 and T4 off yours. Latency is T4 − T1 less the time the server held the packet (T3 − T2).
    </p>
  </div>

  <div class="fig">
    <h3>Where its clock could be</h3>
    <div class="scale" role="img" aria-label="Its clock is {sgn(offset)} ms from yours, give or take {num(bound)} ms from the path and {num(distance)} ms from true time.">
      <div class="zero" style="left:50%"></div>
      <div class="row">
        <span class="cap2">can differ from true time by up to {num(distance)} ms</span>
        <i class="b wide" style={band(offset, distance)}></i>
      </div>
      <div class="row">
        <span class="cap2">the path alone: ± {num(bound)} ms</span>
        <i class="b narrow" style={band(offset, bound)}></i>
      </div>
      <div class="at" style="left:{pct(offset)}%"><span>{sgn(offset)} ms</span></div>
      <div class="axis">
        {#each marks as v (v)}<span style="left:{pct(v)}%">{v === 0 ? '0 · your clock' : sgn(v, 0)}</span>{/each}
      </div>
    </div>
    <p class="cap">
      Each bar runs from the offset less its bound to the offset plus it. {#if Math.abs(offset) <= bound}Your clock (0) sits inside the narrow bar, so this server alone can't tell you it is any different.{:else}Your clock (0) sits outside the narrow bar, so the difference is more than path asymmetry can explain.{/if}
    </p>
  </div>

  <ol class="steps">
    <li>
      <b>Offset {sgn(offset)} ms</b>
      <span><code>((T2 − T1) + (T3 − T4)) / 2</code>, the server's clock less yours, taking each leg as half the trip. It comes from the query with the smallest round trip, the one least held up on the way.{#if nAddr > 1} The row shows the median of its {nAddr} addresses.{/if}</span>
    </li>
    <li>
      <b>± {num(bound)} ms</b>
      <span>Half the smallest round trip, {num(delay)} ÷ 2. If one leg was really slower than the other the offset is off by up to this much, and nothing in the packets says which.</span>
    </li>
    <li>
      <b>Error ≤ {num(distance)} ms</b>
      <span>
        {#if rootDelay != null && rootDisp != null}
          Half the server's own root delay ({num(rootDelay, 2)} ÷ 2) + its root dispersion ({num(rootDisp, 2)}) + the ± ({num(bound)}). The first two are how far it is from its own clock source; the third is your path to it.{#if nAddr > 1} Each is a median over its addresses, so they can differ from the total by a rounding.{/if}
        {:else}
          The ± ({num(bound)}) plus {num(server)} for the server's own distance from its clock source. It didn't report the parts.
        {/if}
      </span>
    </li>
    <li>
      <b>Jitter {r.jitter_ms == null ? '—' : `${num(r.jitter_ms, 2)} ms`}</b>
      <span>How much the offset moved from query to query. It isn't in the error; it says how noisy this path is, and so how far to trust one reading.</span>
    </li>
    <li>
      <b>vs others {sgn(r.deviation_ms)} ms</b>
      <span>Its offset less the median of the other servers' offsets. Your clock cancels, so this shows only whether the server is in step with the rest.</span>
    </li>
  </ol>
</div>

<style>
  .work {
    display: grid;
    grid-template-columns: minmax(280px, 380px) minmax(280px, 1fr);
    gap: 18px 32px;
    padding: 14px 6px 18px;
    max-width: 1100px;
  }
  .fig h3 {
    font-size: var(--t-xs);
    font-weight: 560;
    color: var(--ink-3);
    margin: 0 0 6px;
  }
  .cap {
    margin: 6px 0 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
    max-width: 62ch;
    white-space: normal;
  }
  svg {
    width: 100%;
    max-width: 380px;
    height: auto;
    color: var(--ink-2);
    overflow: visible;
  }
  .lane {
    stroke: var(--rule-2);
    stroke-width: 1;
  }
  .leg {
    stroke: currentColor;
    stroke-width: 1.4;
  }
  .brace {
    stroke: var(--ink-3);
    stroke-width: 1;
    fill: none;
  }
  circle {
    fill: var(--ink);
  }
  .t {
    font: 600 11px var(--font);
    font-variant-numeric: tabular-nums;
    fill: var(--ink);
  }
  .lab {
    font: 400 10.5px var(--font);
    fill: var(--ink-3);
  }
  .lab.c {
    fill: var(--ink-2);
  }

  .scale {
    position: relative;
    height: 112px;
    margin: 4px 8px 30px;
    border-bottom: 1px solid var(--rule-2);
  }
  .row {
    position: relative;
    height: 34px;
  }
  .cap2 {
    position: absolute;
    top: 0;
    left: 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
    white-space: nowrap;
  }
  .b {
    position: absolute;
    top: 16px;
    height: 10px;
    border-radius: 2px;
  }
  .b.wide {
    background: var(--surface-3);
    outline: 1px solid var(--rule-2);
    outline-offset: -1px;
  }
  .b.narrow {
    background: var(--rule-2);
    outline: 1px solid var(--ink-3);
    outline-offset: -1px;
  }
  .zero {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px dashed var(--ink-3);
    z-index: 1;
  }
  .at {
    position: absolute;
    top: 10px;
    height: 56px;
    border-left: 2px solid var(--ink);
    z-index: 2;
  }
  .at span {
    position: absolute;
    top: 100%;
    left: 0;
    transform: translateX(-50%);
    margin-top: 2px;
    font: 600 11px var(--font);
    font-variant-numeric: tabular-nums;
    color: var(--ink);
    white-space: nowrap;
  }
  .axis {
    position: absolute;
    left: 0;
    right: 0;
    bottom: -18px;
    height: 14px;
  }
  .axis span {
    position: absolute;
    transform: translateX(-50%);
    font-size: var(--t-2xs);
    font-variant-numeric: tabular-nums;
    color: var(--ink-3);
  }

  .steps {
    grid-column: 1 / -1;
    list-style: none;
    margin: 6px 0 0;
    padding: 0;
    display: grid;
    gap: 0 32px;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
  }
  .steps li {
    display: grid;
    gap: 2px;
    padding: 8px 0;
    border-top: 1px solid var(--rule);
    font-size: var(--t-xs);
    color: var(--ink-2);
    white-space: normal;
  }
  .steps b {
    font-size: var(--t-sm);
    font-weight: 600;
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }
  code {
    font-family: var(--font);
    font-variation-settings: 'MONO' 1;
    font-size: 0.95em;
    color: var(--ink);
  }
  @media (max-width: 820px) {
    .work {
      grid-template-columns: 1fr;
    }
  }
</style>
