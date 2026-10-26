<script lang="ts">
  // The path check, while it runs and after. Every resolver gets a lane: its route traced hop by hop and drawn as a line, each
  // hop a node as high as its round trip on one square-root scale shared by every lane, with a whisker from its fastest to its
  // slowest reply, coloured by whose network it is in. Silent hops sit as hollow rings on the baseline and the line
  // crosses them dashed. Beside the route, the handshakes that measure the wire to the resolver tick in.
  import { untrack } from 'svelte';
  import type { Resolver, RunState, TraceHop } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { holder, ms } from '../lib/format';
  import PhaseStages, { type Stage } from './PhaseStages.svelte';

  let { run, rows }: { run: RunState; rows: Row[] } = $props();

  const HANDSHAKES = 5;
  const ROUNDS = 4;

  const isPrivate = (ip: string) =>
    /^(10\.|127\.|169\.254\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.|100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.)/.test(ip) ||
    /^(fe80|fc|fd|::1$)/i.test(ip);
  const traced = (r: Resolver) => r.group !== 'system' && [...r.ipv4, ...r.ipv6].slice(0, 1).some((ip) => !isPrivate(ip));

  // The lanes keep the order the ranking had when the check began, so nothing jumps while hops land. One lane per
  // resolver, however many transports it was tested on.
  const first = untrack(() => [...new Set(rows.flatMap((r) => (r.resolver ? [r.resolver.id] : [])))]);
  const lanes = $derived.by(() => {
    const by = new Map(run.resolvers.filter(traced).map((r) => [r.id, r]));
    const ids = [...first.filter((id) => by.has(id)), ...[...by.keys()].filter((id) => !first.includes(id))];
    return ids.map((id) => by.get(id)!);
  });

  // Live while the run is on the path check; afterwards the view shows what it found, without the progress.
  const live = $derived(run.status === 'running' && run.phase === 'path');
  const routes = $derived(run.routes ?? {});
  const canTrace = $derived(run.env?.trace === 'ok' || Object.keys(routes).length > 0);
  const ours = $derived(run.env?.vantage?.network?.asn ?? null);
  const cols = $derived(Math.max(10, ...Object.values(routes).map((t) => t.hops.length)));

  // One square-root scale for every lane, zero at the floor and the slowest hop's average (at least 20 ms) at the top:
  // it spreads the few milliseconds near home apart without letting one far hop flatten the rest. Whiskers past the
  // top stop at it.
  const H = 56;
  const PAD = 6;
  const top = $derived(Math.max(20, ...Object.values(routes).flatMap((t) => t.hops.map((h) => h.avg_ms ?? 0))));
  const y = (v: number) => PAD + (1 - Math.min(1, Math.sqrt(Math.max(0, v) / top))) * (H - 2 * PAD);
  const x = (i: number) => `${((i + 0.5) / cols) * 100}%`;
  const grid = $derived([10, 25, 50, 100, 250, 500].filter((v) => v < top * 0.9).slice(-2));

  type Kind = 'home' | 'isp' | 'transit' | 'dest';
  const kindOf = (h: TraceHop, destAsn: number | null): Kind =>
    h.private ? 'home' : h.asn != null && h.asn === ours ? 'isp' : h.asn != null && h.asn === destAsn ? 'dest' : 'transit';
  const bandLabel = (h: TraceHop, k: Kind) => {
    if (k === 'home') return 'your network';
    if (h.asn == null) return '';
    const name = holder(h.holder);
    return name ? `AS${h.asn} ${name}` : `AS${h.asn}`;
  };

  // Consecutive hops in one network become one band under the route, labelled once.
  function bands(hops: TraceHop[], destAsn: number | null) {
    const out: { start: number; span: number; kind: Kind; label: string }[] = [];
    hops.forEach((h, i) => {
      if (!h.addrs.length) {
        if (out.length) out[out.length - 1].span++;
        return;
      }
      const k = kindOf(h, destAsn);
      const label = bandLabel(h, k);
      const last = out[out.length - 1];
      if (last && last.kind === k && last.label === label) last.span = i + 2 - last.start;
      else out.push({ start: i + 1, span: 1, kind: k, label });
    });
    return out;
  }
  const hopTitle = (h: TraceHop, k: Kind) => {
    if (!h.addrs.length) return `Hop ${h.ttl}: no reply (${h.sent} sent)`;
    const lines = [`Hop ${h.ttl}: ${h.addrs.join(', ')}`];
    const net = bandLabel(h, k);
    if (net) lines.push(net + (k === 'isp' ? ' (your ISP)' : ''));
    lines.push(`best ${ms(h.best_ms)}, average ${ms(h.avg_ms)}, ${h.recv} of ${h.sent} replies`);
    return lines.join('\n');
  };

  const view = $derived(
    lanes.map((r) => {
      const t = routes[r.id];
      const hops = t?.hops ?? [];
      const end = [...hops].reverse().find((h) => h.addrs.length && !h.private);
      const destAsn = t?.reached ? (end?.asn ?? null) : null;
      const p = run.paths[r.id];
      const pts = hops.map((h, i) => {
        const got = h.samples_ms.filter((v): v is number => v != null);
        const v = h.avg_ms ?? h.best_ms;
        return { h, i, k: kindOf(h, destAsn), y: h.addrs.length && v != null ? y(v) : null, lo: got.length ? y(Math.min(...got)) : 0, hi: got.length ? y(Math.max(...got)) : 0 };
      });
      // The line joins each answering hop to the next; across silent hops it runs dashed.
      const answering = pts.filter((q) => q.y != null);
      const segs = answering.slice(1).map((b, n) => ({ a: answering[n], b, gap: b.i - answering[n].i > 1 }));
      return {
        r,
        t,
        hops: pts,
        segs,
        last: t?.reached ? answering[answering.length - 1] : undefined,
        bands: bands(hops, destAsn),
        shakes: Math.min(HANDSHAKES, p?.samples.length ?? 0),
        rtt: p?.rtt_ms ?? null,
      };
    }),
  );

  const rounds = $derived(Object.values(routes).reduce((n, t) => n + t.rounds, 0));
  const shook = $derived(Math.max(0, run.done - rounds));
  const hopsSeen = $derived(Object.values(routes).reduce((n, t) => n + t.hops.filter((h) => h.addrs.length).length, 0));
  const nets = $derived(new Set(Object.values(routes).flatMap((t) => t.hops.flatMap((h) => (h.asn != null ? [h.asn] : [])))).size);
  const traceTotal = $derived(lanes.length * ROUNDS);
  const stages = $derived.by((): Stage[] => {
    const hsTotal = lanes.length * HANDSHAKES;
    const hsShare = hsTotal ? Math.min(1, shook / hsTotal) : 1;
    const out: Stage[] = [
      { label: 'Handshake each resolver', value: `${Math.min(shook, hsTotal)} of ${hsTotal}`, share: hsShare, state: hsShare >= 1 ? 'done' : 'now', tone: 'var(--s-cached)' },
    ];
    if (!canTrace) return out;
    const trShare = traceTotal ? Math.min(1, rounds / traceTotal) : 1;
    out.push(
      { label: 'Trace each route', value: hopsSeen ? `${hopsSeen} hops answered` : 'sending probes', share: trShare, state: trShare >= 1 ? 'done' : 'now', tone: 'var(--s-cached)' },
      { label: 'Name the networks', value: nets ? `${nets} networks` : 'after the traces', share: nets ? 1 : 0, state: nets ? 'done' : trShare >= 1 ? 'now' : 'wait', tone: 'var(--s-cached)' },
    );
    return out;
  });
</script>

<section class="path" aria-labelledby="path-h" aria-busy={live}>
  <header>
    <h2 id="path-h">The path to each resolver</h2>
    <p class="lede">
      {#if canTrace && !live}
        Every route as traced hop by hop, with the handshake time to the resolver at the end. Each hop sits as high as its
        round trip, with a whisker from its fastest to its slowest reply; colour shows whose network it is in.
      {:else if canTrace}
        Every route is traced hop by hop while handshakes time the wire. Each hop sits as high as its round trip, with a
        whisker from its fastest to its slowest reply; colour shows whose network it is in.
      {:else}
        Handshakes time the wire to each resolver. Route traces need Administrator on this computer.
      {/if}
    </p>
  </header>

  {#if live}<PhaseStages {stages} label="Path check steps" />{/if}

  {#if canTrace}
    <ul class="legend" aria-label="Hop colours">
      <li><i class="k home"></i>Your network</li>
      <li><i class="k isp"></i>Your ISP</li>
      <li><i class="k transit"></i>In between</li>
      <li><i class="k dest"></i>The resolver's network</li>
      <li><i class="k ring"></i>No reply</li>
    </ul>
  {/if}

  <ol class="lanes" style:--cols={cols}>
    {#each view as l, n (l.r.id)}
      <li class="lane cascade" class:waiting={live && canTrace && !l.t} style:--i={n}>
        <div class="who">
          <span class="name">{l.r.name}</span>
          {#if l.r.variant}<span class="var">{l.r.variant}</span>{/if}
        </div>
        {#if canTrace}
          <div class="strip" title={l.t || !live ? '' : 'Waiting for the first round'}>
            <svg width="100%" height={H} role="img" aria-label="Route to {l.r.name}, {l.hops.length} hops">
              {#each grid as v (v)}
                <line class="grid" x1="0" x2="100%" y1={y(v)} y2={y(v)} />
                {#if n === 0}<text class="tick" x="-6" y={y(v) + 3}>{v} ms</text>{/if}
              {/each}
              {#each l.segs as s (`${s.a.i}-${s.b.i}`)}
                <line class="seg {s.b.k}" class:gap={s.gap} x1={x(s.a.i)} y1={s.a.y} x2={x(s.b.i)} y2={s.b.y} pathLength="1" />
              {/each}
              {#each l.hops as q (q.h.ttl)}
                <g class="hop {q.k}">
                  <title>{hopTitle(q.h, q.k)}</title>
                  <rect class="hit" x="{(q.i / cols) * 100}%" y="0" width="{100 / cols}%" height={H} />
                  <line class="guide" x1={x(q.i)} x2={x(q.i)} y1="0" y2={H} />
                  {#if q.y == null}
                    <circle class="ring" cx={x(q.i)} cy={H - PAD} r="3" />
                  {:else}
                    {#if q.hi < q.lo - 1}<line class="range" x1={x(q.i)} x2={x(q.i)} y1={q.hi} y2={q.lo} />{/if}
                    <circle class="node" class:end={q === l.last} cx={x(q.i)} r={q === l.last ? 4.5 : 3.25} style:cy="{q.y}px" />
                  {/if}
                </g>
              {/each}
            </svg>
          </div>
          <div class="bands" aria-hidden="true">
            {#each l.bands as b (b.start)}
              <span class="band {b.kind}" style:grid-column="{b.start} / span {b.span}"><span>{b.label}</span></span>
            {/each}
          </div>
        {/if}
        <div class="end">
          {#if canTrace}
            <span class="hops num">
              {#if !l.t}{live ? 'tracing' : 'no route'}{:else if l.t.reached}{l.t.hops.length} hops{:else}{l.hops.filter((x) => x.h.addrs.length).length} answered{/if}
            </span>
          {/if}
          <span class="shakes" aria-label="{l.shakes} of {HANDSHAKES} handshakes">
            {#each { length: HANDSHAKES } as _, i}<i class:on={i < l.shakes}></i>{/each}
          </span>
          <span class="rtt num">{l.rtt != null ? ms(l.rtt) : '–'}</span>
        </div>
      </li>
    {:else}
      <li class="none">No resolver in this run has a public address to trace; the system resolver and ones on this network are skipped.</li>
    {/each}
  </ol>
</section>

<style>
  .path {
    display: grid;
    gap: 18px;
    container-type: inline-size;
  }
  h2 {
    margin: 0;
    font-size: var(--t-lg);
    text-wrap: balance;
  }
  .lede {
    margin: 4px 0 0;
    max-width: 68ch;
    color: var(--ink-2);
    font-size: var(--t-sm);
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .legend li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .k {
    width: 8px;
    height: 12px;
    border-radius: 2px;
    background: var(--c);
  }
  .k.ring {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: none;
    border: 1.5px solid var(--ink-3);
  }
  .home {
    --c: var(--rule-2);
  }
  .isp {
    --c: var(--good);
  }
  .transit {
    --c: var(--ink-3);
  }
  .dest {
    --c: var(--ink);
  }

  .lanes {
    display: grid;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .none {
    color: var(--ink-2);
    font-size: var(--t-sm);
  }
  .lane {
    display: grid;
    grid-template-columns: minmax(110px, 180px) minmax(0, 1fr) auto;
    grid-template-areas: 'who strip end' '. bands .';
    column-gap: 16px;
    align-items: end;
    padding: 10px 0 8px;
    border-top: 1px solid var(--rule);
  }
  .who {
    grid-area: who;
    display: grid;
    align-self: center;
    min-width: 0;
  }
  .name {
    font-weight: 600;
    font-size: var(--t-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .var {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }

  /* Both leave a gutter on the left for the scale's labels. */
  .strip,
  .bands {
    padding-left: 30px;
  }
  .bands {
    display: grid;
    grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
  }
  .strip {
    grid-area: strip;
    position: relative;
    border-bottom: 1px solid var(--rule-2);
  }
  svg {
    display: block;
    overflow: visible;
  }
  .grid {
    stroke: var(--rule);
    stroke-dasharray: 2 3;
  }
  .tick {
    fill: var(--ink-3);
    font-size: var(--t-2xs);
    text-anchor: end;
  }
  /* Each new stretch of line draws itself in toward the hop that just answered. */
  .seg {
    stroke: var(--c);
    stroke-width: 1.75;
    stroke-linecap: round;
    stroke-dasharray: 1;
    animation: draw 520ms var(--ease-out) backwards;
  }
  .seg.gap {
    stroke: var(--ink-3);
    stroke-width: 1.25;
    stroke-dasharray: 0.02 0.03;
    animation: hop-in 400ms var(--ease-out) backwards;
  }
  .hit {
    fill: transparent;
  }
  .guide {
    stroke: var(--rule-2);
    opacity: 0;
    transition: opacity 150ms var(--ease-out);
  }
  .hop:hover .guide {
    opacity: 1;
  }
  .range {
    stroke: var(--c);
    stroke-width: 3;
    stroke-linecap: round;
    opacity: 0.3;
    animation: hop-in 400ms var(--ease-out) backwards;
  }
  .node {
    fill: var(--c);
    stroke: var(--surface);
    stroke-width: 1.5;
    transform-box: fill-box;
    transform-origin: center;
    transition: cy 500ms var(--ease-out);
    animation: node-in 420ms var(--ease-out) 260ms backwards;
  }
  .node.end {
    stroke: var(--c);
    stroke-width: 6;
    stroke-opacity: 0.2;
    paint-order: stroke;
  }
  .ring {
    fill: var(--surface);
    stroke: var(--ink-3);
    stroke-width: 1.25;
    animation: hop-in 400ms var(--ease-out) backwards;
  }
  /* A lane whose first round hasn't landed: a pulse runs along the empty baseline. */
  .waiting .strip {
    border-bottom-style: dashed;
  }
  .waiting .strip::after {
    content: '';
    position: absolute;
    bottom: -2px;
    left: 0;
    width: 24px;
    height: 3px;
    border-radius: 2px;
    background: var(--ink-3);
    animation: probe 1.6s var(--ease-out) infinite;
  }

  .bands {
    grid-area: bands;
    margin-top: 4px;
    min-height: 1.3em;
  }
  .band {
    grid-row: 1;
    min-width: 0;
    margin-inline: 1px;
    padding-top: 3px;
    border-top: 2px solid var(--c);
    font-size: var(--t-2xs);
    color: var(--ink-2);
    animation: hop-in 400ms var(--ease-out);
  }
  .band > span {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .band.isp > span {
    color: var(--good);
  }

  .end {
    grid-area: end;
    display: flex;
    align-items: center;
    gap: 10px;
    align-self: center;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .hops {
    min-width: 7ch;
    text-align: right;
  }
  .shakes {
    display: inline-flex;
    gap: 3px;
  }
  .shakes i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--surface-3);
    transition: background-color 300ms var(--ease-out);
  }
  .shakes i.on {
    background: var(--ink);
  }
  .rtt {
    min-width: 6ch;
    text-align: right;
    color: var(--ink);
    font-weight: 600;
  }

  @keyframes probe {
    from {
      left: 30px;
      opacity: 0;
    }
    20% {
      opacity: 1;
    }
    to {
      left: calc(100% - 24px);
      opacity: 0;
    }
  }
  @keyframes draw {
    from {
      stroke-dashoffset: 1;
    }
  }
  @keyframes node-in {
    from {
      opacity: 0;
      scale: 0.2;
    }
  }
  @keyframes hop-in {
    from {
      opacity: 0;
    }
  }

  @container (max-width: 600px) {
    .lane {
      grid-template-columns: minmax(0, 1fr) auto;
      grid-template-areas: 'who end' 'strip strip' 'bands bands';
      row-gap: 8px;
    }
    .hops {
      min-width: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .node,
    .guide,
    .shakes i {
      transition: none;
    }
    .seg,
    .range,
    .node,
    .ring,
    .band,
    .waiting .strip::after {
      animation: none;
    }
  }
</style>
