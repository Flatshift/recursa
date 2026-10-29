<script lang="ts">
  // The map: your home on the left, the networks the traffic crosses in path order, the destinations on the right. A
  // hand-off is an edge, not a node, so it carries the label (exchange, direct or via transit), the traces crossing it
  // as width and the round trip it adds as a luminance; under each box is the round trip to its first router, so the
  // steps along a path add up to the traceroute figure. Colour is the kind of network and shape says it again; there is no
  // red or green. The picture fits the pane and zooms from there, scrolling sideways once it is wider. Below it, one
  // hairline row per finding says what was found and what it rests on.
  import type { MapRun, NetClaim, NetGroup, NetRole } from '../lib/types';
  import { DIAMOND, layout, TREE, type TreeNode } from '../lib/tree';
  import { clouds, destinations, evidenceLabel, evidenceText, linkLabel, linkWay, markOf, siteName, v6Only, type Cloud } from '../lib/netview';
  import { ms, signedMs } from '../lib/format';
  import { logScale } from '../lib/scale';
  import { reduceMotion } from '../lib/motion';
  import Provenance from './Provenance.svelte';

  let { run, label }: { run: MapRun | null; label: string } = $props();

  const groups = $derived(run?.groups ?? []);
  const cdnClouds = $derived(run ? clouds(run) : []);
  const tree = $derived(layout(groups, run?.edges ?? [], TREE, cdnClouds.map((c) => c.ids)));
  const byId = $derived(new Map(groups.map((g) => [g.id, g])));
  const at = $derived(new Map(tree.nodes.map((n) => [n.id, n])));
  const dests = $derived(run ? destinations(run) : []);
  const name = $derived(new Map(dests.map((d) => [d.target.id, d.target])));

  // The step along a link, 0.1 ms to 100 ms, as how solid its line is.
  const solid = logScale([0.1, 100], 0.3, 1);
  const SKEL_X = TREE.pad + TREE.colW - (TREE.nodeW - TREE.nodeH);
  const SKEL_W = SKEL_X + TREE.nodeW + TREE.pad;
  const SKEL_H = TREE.nodeH + TREE.pad * 2;

  const ROLE: Record<NetRole, string> = { home: 'unknown', isp: 'isp', carrier: 'carrier', exchange: 'ix', content: 'content', unknown: 'unknown', silent: 'unknown' };
  const hue = (r: NetRole) => `var(--net-${ROLE[r]})`;
  const ROLE_TEXT: Record<NetRole, string> = {
    home: 'your network',
    isp: 'your ISP',
    carrier: 'another network on the way',
    exchange: 'an internet exchange',
    content: 'the destination network',
    unknown: 'a network no announced prefix holds',
    silent: 'routers that did not answer',
  };

  /** Destinations behind a leaf, each with the roll call's site code in parentheses. */
  function leaves(g: NetGroup): string[] {
    const seen: string[] = [];
    for (const id of g.targets) {
      const t = name.get(id);
      if (t && !seen.includes(siteName(t))) seen.push(siteName(t));
    }
    return seen;
  }
  function second(g: NetGroup): string {
    if (g.role === 'content') {
      const l = leaves(g);
      return l.length ? l[0] + (l.length > 1 ? ` +${l.length - 1}` : '') + (g.members.length ? '' : ', not reached') + (onlyV6(g) ? ' · IPv6' : '') : '';
    }
    const bits: string[] = [];
    if (onlyV6(g)) bits.push('IPv6');
    if (g.city_hint) bits.push(`consistent with ${g.city_hint}`);
    if (g.owner) bits.push(`owned by ${g.owner}`);
    return bits.join(' · ');
  }
  /** Only IPv6 traces cross it: a route the IPv4 traces do not take. */
  const onlyV6 = (g: NetGroup) => !!run && v6Only(g.targets, run);
  function desc(g: NetGroup): string {
    const bits = [`${ROLE_TEXT[g.role]}, hops ${g.ttl_from}${g.ttl_to > g.ttl_from ? ` to ${g.ttl_to}` : ''}`];
    if (g.members.length) bits.push(`${g.members.length} ${g.members.length === 1 ? 'router' : 'routers'}`);
    if (g.city_hint) bits.push(`consistent with ${g.city_hint}`);
    if (g.owner) bits.push(`owned by ${g.owner}`);
    if (g.rtt_ms != null) bits.push(`${ms(g.rtt_ms, true)} round trip`);
    if (g.role === 'content') bits.push(g.members.length ? `reached as ${leaves(g).join(', ')}` : `${leaves(g).join(', ')} not reached`);
    if (onlyV6(g)) bits.push('over IPv6');
    return `${bits.join('; ')}.`;
  }
  const dashed = (i: number) => {
    const l = run!.edges[i];
    return (
      l.kind === 'unknown' ||
      l.silent_hops > 0 ||
      ['silent', 'unknown'].includes(byId.get(l.from)?.role ?? '') ||
      ['silent', 'unknown'].includes(byId.get(l.to)?.role ?? '') ||
      run!.claims.some((c) => c.link === i && c.tier === 'low')
    );
  };
  const delay = (col: number) => (reduceMotion ? undefined : `${col * 110}ms`);
  // The map fits the pane by default, never larger than life; the zoom buttons scale it from there and it scrolls
  // sideways when it no longer fits, with a fade at the right edge to say there is more.
  let viewW = $state(0);
  let scrolled = $state(0);
  let zoom = $state<number | null>(null);
  const fit = $derived(viewW > 0 ? Math.min(1, (viewW - 24) / tree.width) : 1);
  const scale = $derived(zoom ?? fit);
  const svgW = $derived(Math.round(tree.width * scale));
  const svgH = $derived(Math.round(tree.height * scale));
  const zoomBy = (f: number) => (zoom = Math.min(2, Math.max(0.4, scale * f)));
  const more = $derived(groups.length > 0 && svgW + 24 - scrolled > viewW + 1);

  // One row per distinct finding: destinations that were found to leave the same way share a row.
  interface Finding {
    key: string;
    claim: NetClaim;
    names: string[];
    ids: string[];
  }
  const findings = $derived.by(() => {
    const out: Finding[] = [];
    for (const c of run?.claims ?? []) {
      const key = JSON.stringify([c.link, c.text, c.tier, c.evidence]);
      const t = name.get(c.target);
      const f = out.find((o) => o.key === key);
      if (f) {
        f.ids.push(c.target);
        if (t) f.names.push(siteName(t));
      } else out.push({ key, claim: c, names: t ? [siteName(t)] : [c.target], ids: [c.target] });
    }
    return out;
  });
  let open = $state<Record<string, boolean>>({});
  const toggle = (k: string) => (open[k] = !open[k]);
  /** The groups a finding's destinations pass through, nearest first, that have routers to list. */
  // A cloud is drawn as one shaded block per column its boxes sit in, each wrapping that column's boxes and their
  // round-trip line.
  const blobs = (c: Cloud) => {
    const cols = new Map<number, TreeNode[]>();
    for (const id of c.ids) {
      const n = at.get(id);
      if (n) cols.set(n.col, [...(cols.get(n.col) ?? []), n]);
    }
    return [...cols]
      .map(([col, ns]) => {
        const top = Math.min(...ns.map((n) => n.y));
        const bottom = Math.max(...ns.map((n) => n.y + n.h));
        return { col, x: ns[0].x - 8, y: top - 6, w: TREE.nodeW + 16, h: bottom - top + 24 };
      })
      .sort((a, b) => a.col - b.col);
  };
  const path = (ids: string[]) => groups.filter((g) => g.members.length && g.targets.some((t) => ids.includes(t))).sort((a, b) => a.ttl_from - b.ttl_from);
</script>

<div class="map panel" class:more>
  <div class="scroller" bind:clientWidth={viewW} onscroll={(e) => (scrolled = e.currentTarget.scrollLeft)}>
    {#if groups.length}
      <svg width={svgW} height={svgH} viewBox="0 0 {tree.width} {tree.height}" role="img" aria-label={label}>
        <title>{label}</title>
        {#each cdnClouds as c (c.cdn)}
          {#each blobs(c) as b (b.col)}
            <g class="cloud" class:rev={!reduceMotion} style:animation-delay={delay(b.col)}>
              <rect x={b.x} y={b.y} width={b.w} height={b.h} rx="14" />
              <text class="clabel" x={b.x + 10} y={b.y + b.h - 7}>{c.name}</text>
            </g>
          {/each}
        {/each}
        {#each tree.edges as e (e.index)}
          {@const l = run!.edges[e.index]}
          {@const to = at.get(l.to)}
          {@const hid = l.silent_hops > 0}
          {@const lines = (l.handoff ? 1 : 0) + (hid ? 1 : 0) + (l.rtt_step_ms != null ? 1 : 0)}
          {@const way = linkWay(l, groups)}
          <g class="edge" class:peer={way === 'peer'} class:transit={way === 'transit'} class:v6={v6Only(l.targets, run!)} class:rev={!reduceMotion} style:animation-delay={delay(to?.col ?? 0)}>
            <path
              d={e.d}
              fill="none"
              stroke="var(--ink-2)"
              stroke-width={Math.min(4, Math.max(1, l.count))}
              stroke-opacity={l.rtt_step_ms == null ? 0.3 : solid(Math.max(l.rtt_step_ms, 0.1))}
              stroke-dasharray={dashed(e.index) ? '5 4' : undefined}
            />
            {#if lines}
              <text class="elabel" x={e.lx} y={e.above ? e.ly - 7 - (lines - 1) * 14 : e.ly + 16} text-anchor="middle">
                {#if l.handoff}<tspan class="hand">{linkLabel(l, groups)}</tspan>{/if}
                {#if hid}<tspan x={e.lx} dy={l.handoff ? 14 : 0} class="hid">{l.silent_hops} {l.silent_hops === 1 ? 'hop' : 'hops'} hidden</tspan>{/if}
                {#if l.rtt_step_ms != null}<tspan x={e.lx} dy={l.handoff || hid ? 14 : 0} class="step">{signedMs(l.rtt_step_ms, true)}</tspan>{/if}
              </text>
            {/if}
          </g>
        {/each}

        {#each groups as g (g.id)}
          {@const n = at.get(g.id)!}
          {@const cy = n.y + n.h / 2}
          {@const sub = second(g)}
          {@const hollow = g.role === 'silent' || !g.members.length}
          <g class="node" class:v6={onlyV6(g)} class:rev={!reduceMotion} style:animation-delay={delay(n.col)}>
            <title>{g.label}</title>
            <desc>{desc(g)}</desc>
            {#if g.role === 'exchange'}
              <polygon
                points="{n.x + n.w / 2},{cy - DIAMOND} {n.x + n.w / 2 + DIAMOND},{cy} {n.x + n.w / 2},{cy + DIAMOND} {n.x + n.w / 2 - DIAMOND},{cy}"
                fill="color-mix(in oklch, {hue(g.role)} 22%, var(--surface))"
                stroke={hue(g.role)}
                stroke-width="2"
              />
              <text class="xlabel" x={n.x + n.w / 2} y={cy + 29} text-anchor="middle">{g.label}</text>
            {:else if g.role === 'home'}
              <rect x={n.x} y={n.y} width={n.w} height={n.h} rx="6" fill="color-mix(in oklch, {hue(g.role)} 12%, var(--surface))" stroke={hue(g.role)} stroke-width="1.5" />
              <path
                class="house"
                d="M{n.x + n.w / 2 - 11} {cy + 1}L{n.x + n.w / 2} {cy - 9}l11 10M{n.x + n.w / 2 - 7.5} {cy - 1.5}V{cy + 10}h15V{cy - 1.5}M{n.x + n.w / 2 - 2.5} {cy + 10}v-6h5v6"
              />
            {:else}
              <rect
                x={n.x}
                y={n.y}
                width={n.w}
                height={n.h}
                rx={g.role === 'content' ? n.h / 2 : 6}
                fill={hollow ? 'none' : `color-mix(in oklch, ${hue(g.role)} ${g.role === 'isp' ? 22 : 12}%, var(--surface))`}
                stroke={hue(g.role)}
                stroke-width={g.role === 'isp' ? 2 : 1.5}
                stroke-dasharray={hollow ? '4 3' : undefined}
              />
              <foreignObject x={n.x} y={n.y} width={n.w} height={n.h}>
                <div class="nbox" class:pill={g.role === 'content'} class:lone={!sub}>
                  <span class="nlabel">{g.label}</span>
                  {#if sub}<span class="nsub">{sub}</span>{/if}
                </div>
              </foreignObject>
            {/if}
            {#if g.rtt_ms != null}
              <text class="rtt" x={n.x + n.w} y={n.y + n.h + 13} text-anchor="end">{ms(g.rtt_ms, true)}</text>
            {/if}
          </g>
        {/each}
      </svg>
    {:else}
      <!-- Mapping, before the first trace finishes: your home and the ISP column, ready to fill. -->
      <svg width={SKEL_W} height={SKEL_H} viewBox="0 0 {SKEL_W} {SKEL_H}" role="img" aria-label={label} aria-busy="true">
        <title>{label}</title>
        <rect class="skel-node" x={TREE.pad} y={TREE.pad} width={TREE.nodeH} height={TREE.nodeH} rx="6" />
        <path class="skel-edge" d="M{TREE.pad + TREE.nodeH} {TREE.pad + TREE.nodeH / 2}H{SKEL_X}" />
        <rect class="skel-node" x={SKEL_X} y={TREE.pad} width={TREE.nodeW} height={TREE.nodeH} rx="6" />
      </svg>
    {/if}
  </div>
  {#if groups.length}
    <div class="zoom" role="group" aria-label="Zoom">
      <button type="button" aria-label="Zoom out" disabled={scale <= 0.4} onclick={() => zoomBy(1 / 1.25)}>&minus;</button>
      <button type="button" aria-label="Zoom in" disabled={scale >= 2} onclick={() => zoomBy(1.25)}>+</button>
      <button type="button" disabled={zoom == null} onclick={() => (zoom = null)}>Fit</button>
    </div>
    <p class="legend" aria-hidden="true">
      <span class="kitem"><svg width="36" height="8" viewBox="0 0 36 8"><path d="M0 4H36" stroke="var(--ink-2)" stroke-width="1" /><path d="M0 4H36" stroke="var(--ink-2)" stroke-width="4" stroke-dasharray="0 18 18" /></svg>Line width: traces crossing</span>
      <span class="kitem"><span>0.1 ms</span><svg width="72" height="8" viewBox="0 0 72 8"><defs><linearGradient id="step-strip"><stop offset="0" stop-color="var(--ink-2)" stop-opacity="0.3" /><stop offset="1" stop-color="var(--ink-2)" stop-opacity="1" /></linearGradient></defs><rect width="72" height="8" rx="2" fill="url(#step-strip)" /></svg><span>100 ms added from one box's first router to the next's</span></span>
      <span class="kitem"><svg width="36" height="8" viewBox="0 0 36 8"><path d="M0 4H36" stroke="var(--ink-2)" stroke-width="1.5" stroke-dasharray="5 4" /></svg>Dashed: inferred, or past hidden routers</span>
      <span class="kitem"><svg width="36" height="8" viewBox="0 0 36 8"><path d="M0 4H36" stroke="var(--link-peer)" stroke-width="2" /></svg>Peering, at an exchange or listed as peers</span>
      <span class="kitem"><svg width="36" height="8" viewBox="0 0 36 8"><path d="M0 4H36" stroke="var(--link-transit)" stroke-width="2" /></svg>Transit, onto a carrier or a listed transit pair</span>
      {#if cdnClouds.length}<span class="kitem"><svg width="36" height="12" viewBox="0 0 36 12"><rect class="cloud-key" x="0.5" y="0.5" width="35" height="11" rx="4" /></svg>Shaded: one CDN, reached at an exchange and past it</span>{/if}
      {#if run?.ipv6}<span class="kitem"><svg width="36" height="8" viewBox="0 0 36 8"><path d="M0 4H36" stroke="var(--v6)" stroke-width="2" /></svg>Highlighted: where IPv6 takes a different route</span>{/if}
    </p>
  {/if}
</div>

{#if findings.length}
  <section class="findings panel" aria-label="Findings">
    {#each findings as f (f.key)}
      {@const routers = path(f.ids)}
      <article class="finding">
        <h3>{f.names.join(', ')}</h3>
        <div class="body">
          <p class="said">{f.claim.text}</p>
          <div class="chips">
            {#each f.claim.evidence as ev, i}
              {@const k = `${f.key}:${i}`}
              <button
                class="chip"
                aria-expanded={!!open[k]}
                title={ev.kind === 'listed' ? `${ev.body.source}${ev.body.updated ? `, edited ${ev.body.updated}` : ''}` : ev.kind === 'observed' ? 'Seen in the trace' : 'Worked out from what was seen and listed'}
                onclick={() => toggle(k)}
              >
                <Provenance kind={markOf(ev)} titled={false} />{evidenceLabel(ev)}
              </button>
            {/each}
            {#if routers.length}
              <button class="more" aria-expanded={!!open[`${f.key}:routers`]} onclick={() => toggle(`${f.key}:routers`)}>
                {open[`${f.key}:routers`] ? 'Hide routers' : 'Show routers'}
              </button>
            {/if}
          </div>
          {#each f.claim.evidence as ev, i}
            {#if open[`${f.key}:${i}`]}<p class="raw"><span class="ink-3">{evidenceLabel(ev)}:</span> {evidenceText(ev)}</p>{/if}
          {/each}
          {#if routers.length && open[`${f.key}:routers`]}
            <div class="routers">
              {#each routers as g (g.id)}
                <div class="rg">{g.label}{g.parallel ? ', parallel routers' : ''}</div>
                {#each g.members as m}
                  <div class="rr">
                    <span class="addr">{m.addr}</span>
                    <span class="rn">{m.name ?? ''}{m.prefix ? `${m.name ? ', ' : ''}${m.prefix}${m.moas ? ' (several origins)' : ''}` : ''}</span>
                    <span class="num">{m.best_ms == null ? '' : ms(m.best_ms)}</span>
                    <span class="num">{m.recv} of {m.sent}</span>
                  </div>
                {/each}
              {/each}
            </div>
          {/if}
        </div>
      </article>
    {/each}
  </section>
{/if}

<style>
  .map {
    position: relative;
    overflow: hidden;
  }
  .scroller {
    overflow-x: auto;
    padding: 12px;
  }
  .map.more::after {
    content: '';
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 56px;
    background: linear-gradient(to right, transparent, var(--surface));
    pointer-events: none;
  }
  svg {
    display: block;
  }
  /* The label inside a box: one line each, the name alone may take two. */
  .nbox {
    box-sizing: border-box;
    height: 100%;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 1px;
    padding: 0 12px;
    line-height: 1.25;
    overflow: hidden;
  }
  .nbox.pill {
    padding: 0 18px;
  }
  .nlabel,
  .nsub {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .nlabel {
    font-size: var(--t-sm);
    font-weight: 560;
    color: var(--ink);
  }
  .lone .nlabel {
    white-space: normal;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow-wrap: anywhere;
  }
  .nsub {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .rtt {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
  .xlabel {
    font-size: var(--t-sm);
    font-weight: 560;
    fill: var(--ink);
    paint-order: stroke;
    stroke: var(--surface);
    stroke-width: 4px;
    stroke-linejoin: round;
  }
  .elabel {
    font-size: var(--t-xs);
    fill: var(--ink-2);
    paint-order: stroke;
    stroke: var(--surface);
    stroke-width: 4px;
    stroke-linejoin: round;
  }
  .hand {
    font-weight: 560;
    fill: var(--ink);
  }
  .step {
    font-variation-settings: 'MONO' 1;
    font-variant-numeric: tabular-nums;
  }
  .hid {
    fill: var(--ink-2);
  }
  .edge.peer path {
    stroke: var(--link-peer);
  }
  .edge.transit path {
    stroke: var(--link-transit);
  }
  /* A route only IPv6 takes. */
  .edge.v6 path {
    stroke: var(--v6);
  }
  .cloud rect,
  .cloud-key {
    fill: color-mix(in oklch, var(--net-content) 7%, var(--surface));
    stroke: var(--net-content);
    stroke-width: 1;
    stroke-dasharray: 2 4;
    stroke-linecap: round;
  }
  .clabel {
    font-size: var(--t-2xs);
    font-weight: 560;
    fill: var(--net-content);
    letter-spacing: 0.02em;
  }
  .house {
    fill: none;
    stroke: var(--ink-2);
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .node.v6 rect,
  .node.v6 polygon {
    stroke: var(--v6);
    stroke-width: 2px;
  }
  .skel-node {
    fill: var(--surface-3);
  }
  .skel-edge {
    fill: none;
    stroke: var(--rule-2);
    stroke-width: 2;
    stroke-dasharray: 5 4;
  }
  /* Columns appear left to right as the trace results arrive. */
  .rev {
    animation: reveal var(--dur) var(--ease-out) both;
  }
  @keyframes reveal {
    from {
      opacity: 0;
    }
  }
  .zoom {
    position: absolute;
    top: 8px;
    right: 8px;
    z-index: 1;
    display: flex;
    border: 1px solid var(--rule-2);
    border-radius: var(--r-md);
    background: var(--surface);
    overflow: hidden;
  }
  .zoom button {
    height: 26px;
    min-width: 28px;
    padding: 0 8px;
    border: 0;
    background: none;
    color: var(--ink-2);
    font-size: var(--t-sm);
    font-weight: 560;
  }
  .zoom button + button {
    border-left: 1px solid var(--rule);
  }
  .zoom button:hover:not(:disabled) {
    background: var(--surface-2);
    color: var(--ink);
  }
  .zoom button:disabled {
    color: var(--ink-3);
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 24px;
    margin: 0;
    padding: 8px 16px;
    border-top: 1px solid var(--rule);
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .kitem {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .kitem svg {
    flex: none;
  }

  .findings {
    margin-top: 12px;
  }
  .finding {
    display: grid;
    grid-template-columns: minmax(0, 220px) minmax(0, 1fr);
    gap: 6px 24px;
    padding: 14px 16px;
    border-top: 1px solid var(--rule);
  }
  .finding:first-child {
    border-top: 0;
  }
  h3 {
    margin: 0;
    font-size: var(--t-base);
    font-weight: 600;
    line-height: 1.4;
    text-wrap: balance;
  }
  .body {
    display: grid;
    gap: 8px;
    align-content: start;
    min-width: 0;
    max-width: 72ch;
  }
  .said {
    margin: 0;
    font-size: var(--t-base);
    line-height: 1.5;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 9px;
    border: 1px solid var(--rule-2);
    border-radius: 11px;
    background: transparent;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .chip:hover {
    color: var(--ink);
    border-color: var(--ink-3);
  }
  .chip[aria-expanded='true'] {
    background: var(--sel);
    color: var(--ink);
  }
  .raw {
    margin: 0;
    font-size: var(--t-sm);
    line-height: 1.5;
    color: var(--ink-2);
    overflow-wrap: anywhere;
  }
  .ink-3 {
    color: var(--ink-3);
  }
  .more {
    padding: 0;
    border: 0;
    background: none;
    font-size: var(--t-xs);
    color: var(--ink-2);
    text-decoration: underline;
    text-decoration-color: var(--rule-2);
  }
  .more:hover {
    color: var(--ink);
    text-decoration-color: var(--ink-3);
  }
  .routers {
    display: grid;
    gap: 3px;
    font-size: var(--t-sm);
  }
  .rg {
    margin-top: 6px;
    color: var(--ink-3);
  }
  .rr {
    display: grid;
    grid-template-columns: minmax(0, 10em) minmax(0, 1fr) 5em 5em;
    gap: 10px;
  }
  .rr > * {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .addr {
    font-variation-settings: 'MONO' 1;
  }
  .rn {
    color: var(--ink-3);
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  @media (max-width: 720px) {
    .finding {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .rev {
      animation: none;
    }
  }
  :global(html.mobile) .scroller {
    padding: 8px;
  }
</style>
