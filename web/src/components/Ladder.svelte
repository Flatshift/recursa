<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { Tween } from 'svelte/motion';
  import { fade } from 'svelte/transition';
  import { app } from '../lib/store.svelte';
  import type { Row } from '../lib/verdict';
  import { TESTS, worstP95 } from '../lib/verdict';
  import type { Sample, TestKind, WarmSample } from '../lib/types';
  import { TEST_LABEL, TRANSPORT_LABEL, label, ms, pct, plural } from '../lib/format';
  import Icon from './Icon.svelte';
  import TransportChip from './TransportChip.svelte';
  import BaselineChip from './BaselineChip.svelte';
  import SetupDial from './SetupDial.svelte';
  import { flip } from 'svelte/animate';
  import { RERANK, TICK_FADE, ZOOM, jitterOf, orderOf, rankTracker, reduceMotion, rise, roll } from '../lib/motion';
  import { pacedOrder } from '../lib/paced.svelte';
  import { logDomain, logInvert, logScale, logTicks, tickLabel } from '../lib/scale';
  import { warmByTarget, warmJitter } from '../lib/warm';

  let {
    rows,
    samples,
    test = 'all',
    tests,
    live = false,
    warm = [],
    multi = false,
    waiting = '',
    placeholders = 0,
    starting = false,
    onopen,
  }: {
    rows: Row[];
    samples: Sample[];
    test?: TestKind | 'all';
    tests: TestKind[];
    live?: boolean;
    /** Warm-up answers so far (live only): hollow rings in a target's lanes until it has measured answers. */
    warm?: WarmSample[];
    /** The run used several transports, so a resolver can have several rows. */
    multi?: boolean;
    /** The chart has a pane to itself: taller lanes and bigger dots, so the scatter reads. */
    /** What the run is doing before its first answer, shown in place of the axis. */
    waiting?: string;
    /** Ghost rows to draw while the run hasn't listed its targets yet. */
    placeholders?: number;
    /** The run is still setting up: the mark tunes itself in over the empty rows until warm-up begins. */
    starting?: boolean;
    onopen: (resolverId: string) => void;
  } = $props();

  const lanes = $derived<TestKind[]>(test === 'all' ? TESTS.filter((k) => tests.includes(k)) : [test]);
  const laneH = $derived(test === 'all' ? 18 : 30);
  const dotR = $derived(test === 'all' ? 3.2 : 4);
  const barH = 5;
  const rowH = $derived(lanes.length * laneH + 14);

  const measured = $derived(rows.filter((r) => r.measured));
  // While live, targets still waiting for their first answer hold a row below the ranking.
  const pending = $derived(live ? rows.filter((r) => !r.measured && (r.target.state === 'pending' || r.target.state === 'ready')) : []);
  const notMeasured = $derived(rows.filter((r) => !r.measured && !pending.includes(r)));
  // Ranked rows, then waiting ones, in one keyed list: a row that gets its first answers slides up into its rank
  // instead of leaving one list and appearing in another. The order changes only on a re-rank; each row reads its
  // gliding values by key.
  const keepOrder = orderOf();
  const byIdx = $derived(new Map([...measured, ...pending].map((r) => [r.target.idx, r])));
  const want = $derived(keepOrder([...measured, ...pending].map((r) => r.target.idx)));
  // While live, re-ranks are paced so each slide completes; rows that left are dropped at once.
  const paced = pacedOrder(() => want, () => live);
  const keepShown = orderOf();
  const order = $derived(keepShown(paced.current.filter((k) => byIdx.has(k))));
  const ghostCount = $derived(live && !rows.length ? placeholders : 0);
  // A stable, distribution-like band per ghost lane, so the placeholders don't all look the same.
  const band = (seed: number, lane: number) => {
    const h = Math.sin((seed + 1) * 12.9898 + (lane + 1) * 78.233) * 43758.5453;
    const f = h - Math.floor(h);
    return { left: 18 + f * 22, width: 14 + ((f * 7) % 1) * 20 };
  };

  // Warm-up answers per target and lane. They never count; they show the run is working.
  const warmBy = $derived(live ? warmByTarget(warm, lanes) : new Map<number, Partial<Record<TestKind, WarmSample[]>>>());
  const warmN = (idx: number) => lanes.reduce((n, k) => n + (warmBy.get(idx)?.[k]?.length ?? 0), 0);
  // Until a row has measured answers, the axis is drawn from the warm-up answers, if there are any.
  const warmAxis = $derived(!measured.length && warmBy.size > 0);

  // One log scale shared by every row. While live it also spans the warm-up answers, so the axis set by the warm-up
  // holds when measuring starts instead of snapping in and widening again as each test begins.
  const target = $derived.by(() => {
    let lo = Infinity;
    let hi = 0;
    for (const r of measured) {
      for (const k of lanes) {
        const d = r.dist[k];
        if (d.min != null) lo = Math.min(lo, d.min);
        if (d.max != null) hi = Math.max(hi, d.max);
      }
    }
    if (live) {
      for (const w of warm) {
        if (!lanes.includes(w.k)) continue;
        lo = Math.min(lo, w.us / 1000);
        hi = Math.max(hi, w.us / 1000);
      }
    }
    return logDomain(lo, hi);
  });
  // While live, the axis eases to new bounds (in log space) rather than jumping, so every dot, bar and tick glides.
  // At rest the displayed domain is the target tuple itself, so the scale stays memoized and nothing redraws.
  const toLog = (d: [number, number]): [number, number] => [Math.log10(d[0]), Math.log10(d[1])];
  const zoom = new Tween<[number, number]>(untrack(() => toLog(target)), ZOOM);
  $effect(() => {
    const to = toLog(target);
    untrack(() => zoom.set(to, live ? ZOOM : { duration: 0 }));
  });
  const domain = $derived.by(() => {
    const [a, b] = zoom.current;
    const [ta, tb] = toLog(target);
    if (!live || (Math.abs(a - ta) < 1e-4 && Math.abs(b - tb) < 1e-4)) return target;
    return [10 ** a, 10 ** b] as [number, number];
  });
  let plotW = $state(600);
  const pad = 18;
  // Ticks come from the target bounds but show once the zooming axis reaches them, so they enter and leave at its edge.
  const ticks = $derived(logTicks(target, plotW).filter((t) => t >= domain[0] && t <= domain[1]));
  // Fades and entrances run only for changes made after the first render, and only while live.
  let ready = $state(false);
  onMount(() => (ready = true));
  const tickFade = () => ({ duration: live && ready ? TICK_FADE : 0 });
  const riseOn = () => ({ on: live && ready });
  const x = $derived(logScale(domain, pad, plotW - pad));

  // Answers drawn in the first render (a view opened mid-run) are placed, not flown in.
  let settled = false;
  onMount(() => {
    const id = setTimeout(() => (settled = true));
    return () => clearTimeout(id);
  });
  const frac = (n: number) => n - Math.floor(n);
  const seedOf = (t: number, i: number, k: TestKind) => frac(Math.sin((t + 3) * 91.17 + (i + 5) * 17.31 + k.length * 7.7) * 24634.6345);
  /** A new answer travels in from the axis origin, so the distance it covers is the time it took, and settles to its
   *  resting opacity. Each 2 s copy's answers start spread across the copy's two seconds, so they stream in rather
   *  than landing as a block. A warm-up ring scales in where it lands instead: it was never timed for the record. */
  function land(el: SVGCircleElement, { dx, seed, ring = false }: { dx: number; seed: number; ring?: boolean }) {
    if (!live || !settled || reduceMotion) return;
    const delay = seed * 1700;
    if (ring) {
      el.animate([{ opacity: 0, transform: 'scale(2.4)' }, {}], { duration: 520, delay, easing: 'cubic-bezier(0.22, 1, 0.36, 1)', fill: 'backwards' });
      return;
    }
    const flight = 420 + Math.min(420, dx * 0.55);
    el.animate(
      [
        { transform: `translateX(${-dx}px)`, opacity: 0, easing: 'cubic-bezier(0.16, 1, 0.3, 1)' },
        { transform: 'translateX(0)', opacity: 1, offset: 0.62 },
        {},
      ],
      { duration: flight / 0.62, delay, fill: 'backwards' },
    );
  }

  // Rank changes during a live run: climbed (1) or dropped (-1) per target.
  const rankOf = rankTracker();
  const rankDir = $derived(rankOf(measured, live));

  // Answered samples per target and lane, in arrival order, so each lane draws only its
  // own dots (no per-sample test filter in the markup) and new dots append at the end.
  const dots = $derived.by(() => {
    const m = new Map<number, Partial<Record<TestKind, Sample[]>>>();
    for (const s of samples) {
      if (s.o !== 'ok' || !lanes.includes(s.k)) continue;
      let byLane = m.get(s.t);
      if (!byLane) m.set(s.t, (byLane = {}));
      (byLane[s.k] ??= []).push(s);
    }
    return m;
  });
  const laneY = (k: TestKind) => 7 + lanes.indexOf(k) * laneH + laneH / 2;
  const ringY = (w: WarmSample, i: number) => laneY(w.k) + warmJitter(w.t, w.k, i) * (laneH / 2 - dotR - 1);
  const dotY = (s: Sample) => laneY(s.k) + jitterOf(s) * (laneH / 2 - dotR - 0.5);

  // Density ridge per lane: a kernel estimate on log-ms bins (0.02 decades, Gaussian σ 0.04), scaled to the lane's own
  // peak and mirrored about the lane, so the shape of the spread reads before the dots do. Computed once per data copy
  // in log space; only the bins' x positions follow the zooming axis.
  const BIN = 0.02;
  const KR = 6;
  const KERNEL = Array.from({ length: 2 * KR + 1 }, (_, j) => Math.exp(-(((j - KR) / 2) ** 2) / 2));
  type Ridge = { b0: number; v: number[] };
  function density(list: Sample[] | undefined): Ridge | null {
    if (!list || list.length < 8) return null;
    const bins = list.map((s) => Math.round(Math.log10(Math.max(s.us, 1) / 1000) / BIN));
    const b0 = Math.min(...bins) - KR;
    const v = new Array<number>(Math.max(...bins) + KR + 1 - b0).fill(0);
    for (const b of bins) for (let j = 0; j < KERNEL.length; j++) v[b - b0 + j - KR] += KERNEL[j];
    const peak = Math.max(...v);
    // Thin tails would draw as needles out to every lone outlier; the dots already show those.
    return { b0, v: v.map((n) => (n / peak < 0.04 ? 0 : n / peak)) };
  }
  const ridges = $derived.by(() => {
    const m = new Map<number, Partial<Record<TestKind, Ridge>>>();
    for (const [t, byLane] of dots) {
      const out: Partial<Record<TestKind, Ridge>> = {};
      for (const k of lanes) {
        const r = density(byLane[k]);
        if (r) out[k] = r;
      }
      m.set(t, out);
    }
    return m;
  });
  function ridgePath(r: Ridge, k: TestKind): string {
    const y = laneY(k);
    const h = (laneH / 2 - 1) * 0.9;
    const xs = r.v.map((_, i) => x(10 ** ((r.b0 + i) * BIN)).toFixed(1));
    const top = r.v.map((n, i) => `${xs[i]},${(y - n * h).toFixed(1)}`);
    const bottom = r.v.map((n, i) => `${xs[i]},${(y + n * h).toFixed(1)}`).reverse();
    return `M${top.join('L')}L${bottom.join('L')}Z`;
  }

  // A finished run's dots never change, so each lane is one prebuilt string of circles: no
  // per-dot bookkeeping, which is most of the cost of drawing thousands of samples. Live
  // runs keep one element per dot so new samples can fade in as they land.
  const still = (list: Sample[] | undefined, k: TestKind) =>
    (list ?? []).map((s) => `<circle class="dot ${k}" cx="${x(s.us / 1000).toFixed(1)}" cy="${dotY(s).toFixed(1)}" r="${dotR}"/>`).join('');

  let tip = $state<{ x: number; y: number; s: Sample; row: Row } | null>(null);
  // A crosshair through every row at the pointer, with its value read off on the axis.
  let cross = $state<{ left: number; px: number } | null>(null);
  let el: HTMLDivElement;

  function hover(e: PointerEvent, row: Row) {
    const svg = e.currentTarget as SVGSVGElement;
    const rect = svg.getBoundingClientRect();
    const px = e.clientX - rect.left;
    const py = e.clientY - rect.top;
    cross = px >= pad && px <= plotW - pad ? { left: rect.left - el.getBoundingClientRect().left + px, px } : null;
    const byLane = dots.get(row.target.idx) ?? {};
    let best: Sample | null = null;
    let bd = 18 * 18;
    for (const s of lanes.flatMap((k) => byLane[k] ?? [])) {
      const dx = x(s.us / 1000) - px;
      const dy = dotY(s) - py;
      const d = dx * dx + dy * dy;
      if (d < bd) {
        bd = d;
        best = s;
      }
    }
    tip = best ? { x: e.clientX, y: rect.top + laneY(best.k) - laneH / 2, s: best, row } : null;
  }

  function onKey(e: KeyboardEvent, i: number, row: Row) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onopen(row.target.resolver_id);
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = document.querySelector<HTMLElement>(`[data-ladder-row="${i + (e.key === 'ArrowDown' ? 1 : -1)}"]`);
      next?.focus();
    }
  }

  function leave() {
    tip = null;
    cross = null;
  }

  const metric = (r: Row) => (test === 'all' ? r.composite : r.dist[test].p50);
  const p95 = (r: Row) => (test === 'all' ? worstP95(r) : r.dist[test].p95);
  const timeouts = (r: Row) => lanes.reduce((n, k) => n + r.dist[k].timeouts, 0);
  const errors = (r: Row) => lanes.reduce((n, k) => n + r.dist[k].errors, 0);
  const sent = (r: Row) => lanes.reduce((n, k) => n + r.dist[k].sent, 0);
  const answered = (r: Row) => {
    const n = sent(r);
    return n ? lanes.reduce((a, k) => a + r.dist[k].n, 0) / n : null;
  };

  function aria(r: Row): string {
    const name = label(r.target.provider, r.target.variant);
    const parts = lanes.map((k) => `${TEST_LABEL[k]} median ${ms(r.dist[k].p50)}`);
    return `${name} over ${TRANSPORT_LABEL[r.target.transport]}: ${parts.join(', ')}, ${pct(answered(r))} answered${r.intercepted ? ', answered by a device on your network' : ''}. Press Enter for details.`;
  }

  const outcomeLabel: Record<string, string> = {
    ok: 'answered',
    nxdomain: 'NXDOMAIN',
    nodata: 'no data',
    servfail: 'SERVFAIL',
    refused: 'REFUSED',
    rcode: 'error',
    hijack: 'rewritten NXDOMAIN',
    blocked: 'blocked by a filter',
    from_cache: 'from cache (left out)',
    timeout: 'no answer',
    unreachable: 'unreachable',
    neterr: 'network error',
  };
</script>

<div class="ladder" style:--row-h="{rowH}px" bind:this={el}>
  <div class="axis top" aria-hidden="true">
    <div class="lcol">
      {#if warmAxis && waiting}<p class="waiting wl">{waiting}</p>{/if}
    </div>
    <div class="pcol" bind:clientWidth={plotW}>
      {#if live && !measured.length && waiting && !warmAxis}
        <p class="waiting">{waiting}</p>
      {:else}
        <svg width={plotW} height="24">
          {#each ticks as t (t)}
            <text x={x(t)} y="16" text-anchor={x(t) > plotW - 24 ? 'end' : 'middle'} class:hid={cross && Math.abs(x(t) - cross.px) < 26} in:fade={tickFade()} out:fade={tickFade()}>{tickLabel(t)}</text>
          {/each}
          {#if cross}
            {@const v = logInvert(domain, pad, plotW - pad, cross.px)}
            {@const s = v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)}
            {@const w = s.length * 6.6 + 22}
            <g class="readout" transform="translate({Math.min(plotW - w / 2 - 1, Math.max(w / 2 + 1, cross.px))} 0)">
              <rect x={-w / 2} y="4" width={w} height="16" rx="8" />
              <text y="16" text-anchor="middle">{s} ms</text>
            </g>
          {/if}
        </svg>
      {/if}
    </div>
    <div class="tcol"><span class="unit">no answer</span></div>
    <div class="ncol"><span class="unit" title={test === 'all' ? 'Speed score, in ms: the three tests weighted together, each median pulled toward its p95. Rows are sorted by it.' : undefined}>{test === 'all' ? 'speed score' : 'median'}</span></div>
  </div>

  {#if cross}<div class="cross" style:left="{cross.left}px" aria-hidden="true"></div>{/if}

  <div class="rows" role="group" aria-label={test === 'all' ? 'Resolvers ranked by speed score' : `Resolvers ranked by ${TEST_LABEL[test]} median`}>
    {#each order as idx, i (idx)}
      {@const r = byIdx.get(idx)!}
      {@const m = r.measured}
      {@const byLane = dots.get(idx) ?? {}}
      {@const warmLane = warmBy.get(idx) ?? {}}
      {@const wn = warmN(idx)}
      {@const to = timeouts(r)}
      {@const er = errors(r)}
      <!-- A waiting row has no role or tab stop until it has answers; clicks on it do nothing. -->
      <!-- svelte-ignore a11y_no_static_element_interactions, a11y_no_noninteractive_tabindex -->
      <div
        animate:flip={RERANK}
        in:rise={riseOn()}
        class="row"
        class:waitrow={!m}
        class:sel={m && app.inspect === r.target.resolver_id}
        data-resolver={r.target.resolver_id}
        class:flag={r.intercepted}
        class:climbed={rankDir.get(idx) === 1}
        class:weak={m && !r.eligible && !r.intercepted}
        role={m ? 'button' : undefined}
        tabindex={m ? 0 : undefined}
        data-ladder-row={m ? i : undefined}
        aria-label={m
          ? aria(r)
          : wn
            ? `${label(r.target.provider, r.target.variant)}: warming up, ${plural(wn, 'warm-up answer')}, not recorded`
            : `${label(r.target.provider, r.target.variant)}: waiting for answers`}
        onclick={() => m && onopen(r.target.resolver_id)}
        onkeydown={(e) => m && onKey(e, i, r)}
      >
        <div class="lcol">
          <span class="rank num" aria-hidden="true">
            {#if m && r.eligible}{#key i + 1}<span class="rn" class:up={rankDir.get(idx) === 1} in:roll={{ dir: rankDir.get(idx) ?? 0 }}>{i + 1}</span>{/key}{:else if r.intercepted}<Icon name="warn" size={13} />{:else}·{/if}
          </span>
          <span class="who">
            <span class="name"
              ><b>{r.target.provider}</b> {r.target.variant}{#if multi}<TransportChip t={r.target.transport} v6={r.target.family === 'v6'} />{/if}</span
            >
            <span class="sub mono">
              <!-- The baseline chip rides on the address line: beside a long name such as "Pi Hole system resolver" it cut the name off. -->
              {#if r.baseline}<BaselineChip what={r.forwarder} lead />{/if}
              {#if !multi}<span class="tp">{TRANSPORT_LABEL[r.target.transport]}{r.target.family === 'v6' ? '·v6' : ''}</span>{/if}
              {r.target.transport === 'udp' || r.target.transport === 'tcp' ? r.target.ip : r.target.endpoint.replace('https://', '')}
            </span>
            {#if r.intercepted}<span class="flagtxt">Answered by a device on your network</span>{/if}
            {#if r.target.state === 'dropped'}<span class="flagtxt">{r.target.reason}</span>{/if}
          </span>
        </div>
        <div class="pcol stack">
          <!-- svelte-ignore a11y_no_static_element_interactions (pointer tooltip only; the row itself is the keyboard target) -->
          <svg width={plotW} height={rowH} onpointermove={(e) => m && hover(e, r)} onpointerleave={leave}>
            {#each ticks as t (t)}
              <line class="grid" x1={x(t)} x2={x(t)} y1="0" y2={rowH} in:fade={tickFade()} out:fade={tickFade()} />
            {/each}
            {#each lanes as k}
              {@const d = r.dist[k]}
              {@const y = laneY(k)}
              {@const ridge = ridges.get(idx)?.[k]}
              <line class="lane-rule" x1={pad} x2={plotW - pad} y1={y} y2={y} />
              {#if ridge}<path class="ridge {k}" d={ridgePath(ridge, k)} />{/if}
              <!-- A lane keeps its warm-up rings until its own answers arrive, then lets them go. -->
              {#if live && !byLane[k]?.length && warmLane[k]?.length}
                <g out:fade={{ duration: 500 }}>
                  {#each warmLane[k] ?? [] as w, j}
                    <circle class="ring {k}" cx={x(w.us / 1000)} cy={ringY(w, j)} r={dotR} use:land={{ dx: 0, seed: seedOf(w.t, j, k), ring: true }} />
                  {/each}
                </g>
              {/if}
              {#if live}
                <!-- Unkeyed: samples append (a cached Uncached answer judged afterwards drops out once, and the dots after it shift). -->
                {#each byLane[k] ?? [] as s}
                  <circle class="dot {k}" cx={x(s.us / 1000)} cy={dotY(s)} r={dotR} use:land={{ dx: x(s.us / 1000) - pad, seed: seedOf(s.t, s.i, k) }} />
                {/each}
              {:else}
                {@html still(byLane[k], k)}
              {/if}
              {#if d.p25 != null && d.p75 != null && d.n >= 3}
                {#if d.p95 != null}
                  <line class="whisker {k}" x1={x(d.p75)} x2={x(d.p95)} y1={y} y2={y} />
                  <line class="whisker {k}" x1={x(d.p95)} x2={x(d.p95)} y1={y - barH / 2 + 0.5} y2={y + barH / 2 - 0.5} />
                {/if}
                <rect class="iqr {k}" x={x(d.p25)} y={y - barH / 2} width={Math.max(3, x(d.p75) - x(d.p25))} height={barH} rx={barH / 2} />
              {/if}
              {#if d.p50 != null}
                <rect class="median" x={x(d.p50) - 1.25} y={y - laneH / 2 + 2} width="2.5" height={laneH - 4} rx="1.25" />
              {/if}
            {/each}
          </svg>
          {#if !m && !wn}
            <div class="ghost-lanes" style:--lane-h="{laneH}px" out:fade={{ duration: 300 }}>
              {#each lanes as _, j}
                {@const b = band(idx, j)}
                <span class="lane"><span class="ghost" style:margin-left="{b.left}%" style:width="{b.width}%"></span></span>
              {/each}
            </div>
          {/if}
        </div>
        <div class="tcol num" title={m ? `${to} timed out, ${er} errors` : undefined}>
          {#if m && to + er > 0}
            <span class="x">×</span>{to + er}
          {/if}
        </div>
        <div class="ncol stack">
          {#if m}
            <span class="nv" in:fade={{ duration: live && ready ? 400 : 0, delay: 150 }}>
              <span class="big num">{ms(metric(r), false)}<span class="u">ms</span></span>
              <span class="small num" title={test === 'all' ? 'The slowest of the three tests’ 95th percentiles' : undefined}><span>{test === 'all' ? 'worst p95' : 'p95'} {ms(p95(r), false)}</span><span>· {pct(answered(r), 0)}</span></span>
            </span>
          {:else}
            <span class="nv" out:fade={{ duration: 250 }}>
              <span class="ghost" style:width="54px" style:height="14px"></span>
              {#if wn}<span class="small num">{plural(wn, 'warm-up answer')}</span>{:else}<span class="ghost" style:width="72px" style:height="8px" style:margin-top="5px"></span>{/if}
            </span>
          {/if}
        </div>
      </div>
    {/each}
    {#each { length: ghostCount } as _, i}
      <div class="row waitrow" aria-hidden="true">
        <div class="lcol">
          <span class="rank num">·</span>
          <span class="who"><span class="ghost" style:width="{120 + ((i * 37) % 60)}px" style:height="11px"></span><span class="ghost" style:width="90px" style:height="8px" style:margin-top="5px"></span></span>
        </div>
        <div class="pcol ghost-lanes" style:height="{rowH}px" style:--lane-h="{laneH}px">
          {#each lanes as _, j}
            {@const b = band(i, j)}
            <span class="lane"><span class="ghost" style:margin-left="{b.left}%" style:width="{b.width}%"></span></span>
          {/each}
        </div>
        <div class="tcol"></div>
        <div class="ncol">
          <span class="ghost" style:width="54px" style:height="14px"></span>
          <span class="ghost" style:width="72px" style:height="8px" style:margin-top="5px"></span>
        </div>
      </div>
    {/each}
    {#if live && starting}
      <div class="starting"><SetupDial /></div>
    {/if}
  </div>

  {#if notMeasured.length}
    <div class="nm">
      <h4>Not measured</h4>
      <ul>
        {#each notMeasured as r (r.target.idx)}
          <li>
            <span class="who"><b>{r.target.provider}</b> {r.target.variant} <span class="mono muted">{TRANSPORT_LABEL[r.target.transport]}</span></span>
            <span class="why">{r.target.reason ?? r.target.state}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>

{#if tip}
  <div class="tip" style:left="{tip.x}px" style:top="{tip.y}px" role="tooltip">
    <div class="tt-h"><span class="key {tip.s.k}"></span>{TEST_LABEL[tip.s.k]} · round {tip.s.i + 1}</div>
    <div class="tt-v num">{ms(tip.s.us / 1000)}</div>
    <div class="tt-s">{label(tip.row.target.provider, tip.row.target.variant)} · {outcomeLabel[tip.s.o]}</div>
  </div>
{/if}

<style>
  .ladder {
    --lcol: clamp(180px, 26%, 250px);
    --tcol: 56px;
    --ncol: 156px;
    position: relative;
    /* Sized by its pane, not the window: an inspector or sidebar narrows it too. */
    container: ladder / inline-size;
  }
  .axis,
  .row {
    display: grid;
    grid-template-columns: var(--lcol) minmax(0, 1fr) var(--tcol) var(--ncol);
    align-items: center;
  }
  .axis {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--surface);
    border-bottom: 1px solid var(--rule);
  }
  .axis text {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variation-settings: 'MONO' 1;
  }
  .unit {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .axis .tcol,
  .axis .ncol {
    text-align: right;
    padding-right: 4px;
  }
  .pcol {
    min-width: 0;
    overflow: hidden;
  }
  .rows {
    display: block;
    position: relative;
  }
  /* Over the plot column, centred on the first rows so it stays in view however many there are. */
  .starting {
    position: absolute;
    inset: 0 0 auto;
    height: min(100%, 480px);
    z-index: 1;
    display: grid;
    grid-template-columns: var(--lcol) minmax(0, 1fr) var(--tcol) var(--ncol);
    align-items: center;
    pointer-events: none;
  }
  .starting > :global(.dial) {
    grid-column: 2;
    justify-self: center;
  }
  /* Opaque, so rows trading places slide over each other instead of overprinting; the one that climbed rides on top. */
  .row {
    position: relative;
    background: var(--surface);
    min-height: var(--row-h);
    border-bottom: 1px solid var(--rule);
    cursor: default;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .row.climbed {
    z-index: 1;
  }
  .row:hover {
    background: var(--surface-2);
  }
  .row:focus-visible {
    outline-offset: -2px;
    background: var(--surface-2);
  }
  .row.sel,
  .row.sel:hover {
    background: var(--sel);
  }
  .row.sel .rank {
    color: var(--ink);
  }
  .row.weak .who,
  .row.weak .ncol {
    opacity: 0.66;
  }
  .row.flag {
    background: color-mix(in oklch, var(--warn-soft) 55%, transparent);
  }
  .lcol {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 7px 10px 7px 4px;
    min-width: 0;
  }
  .rank {
    width: 22px;
    flex: none;
    text-align: right;
    color: var(--ink-3);
    font-size: var(--t-sm);
    padding-top: 1px;
    /* Clips the numeral as it rolls in, so the column never changes size. */
    overflow: hidden;
  }
  .rn {
    display: block;
  }
  /* A row that climbed: its numeral starts in full ink and heavier, and settles back. */
  .rn.up {
    animation: climb 1200ms var(--ease-out);
  }
  @keyframes climb {
    from {
      color: var(--ink);
      font-weight: 760;
    }
  }
  .rank :global(svg) {
    color: var(--warning);
  }
  .who {
    display: grid;
    min-width: 0;
    line-height: 1.3;
  }
  .name {
    font-size: var(--t-sm);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--ink-2);
  }
  .name b {
    color: var(--ink);
    font-weight: 620;
  }
  .sub {
    font-size: var(--t-xs);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tp {
    color: var(--ink-2);
    margin-right: 4px;
  }
  .flagtxt {
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }
  /* Global: a finished run's dots are inserted as markup, which carries no scoping class. */
  .pcol :global(.dot) {
    opacity: 0.55;
  }
  .lane-rule {
    stroke: var(--rule);
    stroke-dasharray: 1 3;
  }
  /* The spread's shape, under the dots: a tint of the lane colour, never a line. */
  .ridge {
    fill-opacity: var(--ridge-a);
  }
  .ridge.cached {
    fill: var(--s-cached);
  }
  .ridge.uncached {
    fill: var(--s-uncached);
  }
  .ridge.tld {
    fill: var(--s-tld);
  }
  .ring {
    fill: none;
    stroke-width: 1.25;
    opacity: 0.8;
    transform-box: fill-box;
    transform-origin: center;
  }
  @media (prefers-reduced-motion: reduce) {
    .rn.up {
      animation: none;
    }
  }
  .ring.cached {
    stroke: var(--s-cached);
  }
  .ring.uncached {
    stroke: var(--s-uncached);
  }
  .ring.tld {
    stroke: var(--s-tld);
  }
  .pcol :global(.dot.cached),
  .iqr.cached {
    fill: var(--s-cached);
  }
  .pcol :global(.dot.uncached),
  .iqr.uncached {
    fill: var(--s-uncached);
  }
  .pcol :global(.dot.tld),
  .iqr.tld {
    fill: var(--s-tld);
  }
  .iqr {
    stroke: var(--surface);
    stroke-width: 1.5;
    paint-order: stroke;
  }
  .row:hover .iqr,
  .row.sel .iqr {
    stroke: var(--surface-2);
  }
  .whisker {
    stroke-width: 1.25;
    stroke-linecap: round;
  }
  .whisker.cached {
    stroke: var(--s-cached);
  }
  .whisker.uncached {
    stroke: var(--s-uncached);
  }
  .whisker.tld {
    stroke: var(--s-tld);
  }
  .median {
    fill: var(--ink);
    stroke: var(--surface);
    stroke-width: 1.5;
    paint-order: stroke;
  }
  /* Stacked cells: what a waiting row shows and what replaces it share one place, so they cross-fade. */
  .stack {
    display: grid;
  }
  .stack > :global(*) {
    grid-area: 1 / 1;
  }
  .ncol .nv {
    display: grid;
    justify-items: end;
  }
  .cross {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    margin-left: -0.5px;
    background: color-mix(in oklch, var(--ink) 30%, transparent);
    pointer-events: none;
    z-index: 1;
  }
  .readout rect {
    fill: var(--ink);
  }
  .axis .readout text {
    fill: var(--surface);
    font-weight: 600;
  }
  .axis text.hid {
    opacity: 0;
  }
  .tcol {
    text-align: right;
    font-size: var(--t-xs);
    color: var(--ink-2);
    padding-right: 4px;
  }
  .x {
    color: var(--critical);
    margin-right: 2px;
    font-weight: 700;
  }
  .ncol {
    display: grid;
    justify-items: end;
    padding-right: 4px;
    line-height: 1.25;
  }
  .big {
    font-size: var(--t-md);
    font-weight: 600;
  }
  .u {
    font-size: var(--t-xs);
    color: var(--ink-3);
    margin-left: 2px;
    font-weight: 500;
  }
  .small {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .waiting {
    font-size: var(--t-xs);
    color: var(--ink-2);
    line-height: 22px;
    padding-left: var(--pad, 18px);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .axis .lcol {
    padding-block: 0;
  }
  /* The p95 and the answered share wrap onto two lines rather than spill past the row when the column is narrow. */
  .ncol .small {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    column-gap: 0.3em;
    white-space: nowrap;
  }
  .waiting.wl {
    padding-left: 0;
    line-height: 1.25;
    white-space: normal;
  }
  .waitrow,
  .waitrow:hover {
    background: var(--surface);
  }
  .waitrow .who {
    opacity: 0.7;
    transition: opacity 400ms var(--ease-out);
  }
  .ghost-lanes {
    display: grid;
    align-content: center;
    padding: 7px 0;
    box-sizing: border-box;
  }
  .lane {
    height: var(--lane-h);
    display: flex;
    align-items: center;
  }
  .lane .ghost {
    height: 6px;
    border-radius: 3px;
  }
  .ncol .ghost {
    justify-self: end;
  }
  .nm {
    margin-top: 18px;
  }
  .nm h4 {
    font-size: var(--t-xs);
    color: var(--ink-3);
    font-weight: 620;
    margin-bottom: 6px;
  }
  .nm ul {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .nm li {
    display: grid;
    grid-template-columns: 250px minmax(0, 1fr);
    gap: 12px;
    padding: 6px 4px;
    border-top: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .nm .why {
    color: var(--ink-2);
  }
  .nm .who {
    display: block;
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
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--ink-2);
  }
  .tt-v {
    font-size: var(--t-md);
    font-weight: 620;
  }
  .tt-s {
    color: var(--ink-3);
  }
  @container ladder (max-width: 680px) {
    /* Label on its own line; plot and numbers share the row below. Set on the rows: a container query can't style
       the container itself. */
    .axis,
    .row {
      --ncol: 112px;
      grid-template-columns: minmax(0, 1fr) var(--ncol);
    }
    .axis .lcol,
    .axis .tcol,
    .row .tcol {
      display: none;
    }
    .row .lcol {
      grid-column: 1 / -1;
      padding-bottom: 0;
    }
    .nm li {
      grid-template-columns: 1fr;
      gap: 2px;
    }
  }
</style>
