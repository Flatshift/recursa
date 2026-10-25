<script lang="ts">
  // The CDN view while the run maps it. Each lookup drops its edge addresses into the resolver's cell as a dot; once
  // the edges are timed, every dot slides along a log scale to its fastest handshake, and edges that never answered
  // fall hollow to the far end. A resolver that stops answering is given up on and says so. Through the intel phase
  // the grid stays put while the edges' networks are named; the finished map waits for the end of the run.
  import { untrack } from 'svelte';
  import type { CdnCell, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { holder, ms } from '../lib/format';
  import CdnLogo from './CdnLogo.svelte';
  import PhaseStages, { type Stage } from './PhaseStages.svelte';

  let { run, rows }: { run: RunState; rows: Row[] } = $props();

  // The engine's catalog, in its order: hostnames asked per CDN, anycast controls last.
  const CDNS = [
    { id: 'akamai', name: 'Akamai', hosts: 5 },
    { id: 'cloudfront', name: 'CloudFront', hosts: 3 },
    { id: 'google', name: 'Google', hosts: 3 },
    { id: 'meta', name: 'Meta', hosts: 3 },
    { id: 'fastly', name: 'Fastly', hosts: 1, any: true },
    { id: 'cloudflare', name: 'Cloudflare', hosts: 1, any: true },
  ];
  const ASKED = CDNS.reduce((n, c) => n + c.hosts, 0);

  // Rows keep the order the ranking had when the mapping began: the live ranking moves too fast to follow.
  const first = untrack(() => rows.flatMap((r) => (r.resolver ? [r.target.resolver_id] : [])));
  const resolvers = $derived.by(() => {
    const all = [...new Map(rows.filter((r) => r.resolver).map((r) => [r.target.resolver_id, r.resolver!])).values()];
    const at = (id: string) => (first.includes(id) ? first.indexOf(id) : first.length);
    return all.sort((a, b) => at(a.id) - at(b.id));
  });
  const slot = $derived(new Map(run.resolvers.map((r, i) => [r.id, i])));
  const lookups = $derived(run.cdn_lookups ?? {});
  // The engine leaves out a resolver that answered none of the earlier tests.
  const skipped = $derived(
    new Set(
      resolvers
        .filter((r) => !run.targets.some((t) => t.resolver_id === r.id && Object.values(t.stats).some((x) => (x?.ok ?? 0) > 0)))
        .map((r) => r.id),
    ),
  );
  const timing = $derived(run.cdn_timing);
  const best = $derived(timing?.best ?? {});
  const silent = $derived(new Set((timing?.silent ?? []).filter((ip) => best[ip] == null)));
  const stage = $derived(run.phase === 'intel' && run.cdn ? 'name' : run.phase !== 'cdn' ? 'wait' : timing ? 'time' : 'ask');
  const timed = $derived(stage === 'time' || stage === 'name');

  const finished = $derived(Object.values(lookups).filter((l) => l.finished).length);
  const names = $derived(Object.values(lookups).reduce((n, l) => n + Object.values(l.cells ?? {}).reduce((m, c) => m + c.ok.length, 0), 0));
  const askShare = $derived(stage === 'ask' ? (run.total ? finished / run.total : 0) : 1);
  const timeShare = $derived(stage === 'name' ? 1 : timing?.total ? timing.timed / timing.total : 0);

  // The networks behind the edges, named one by one during intel, each a beat after the one before.
  const edges = $derived(Object.values(run.cdn?.edges ?? {}));
  const named = $derived(edges.filter((e) => e.asn != null).length);
  const enter = new Map<string, number>();
  const nets = $derived.by(() => {
    const m = new Map<string, { key: string; name: string; n: number; you: boolean }>();
    for (const e of edges) {
      if (e.asn == null) continue;
      const key = String(e.asn);
      const x = m.get(key) ?? { key, name: holder(e.holder) || `AS${e.asn}`, n: 0, you: false };
      x.n++;
      x.you ||= e.on_net;
      m.set(key, x);
    }
    let batch = 0;
    for (const k of m.keys()) if (!enter.has(k)) enter.set(k, batch++ * 90);
    return [...m.values()];
  });

  const stages = $derived.by((): Stage[] => [
    {
      label: 'Ask every resolver',
      value: `${finished} of ${run.total && stage === 'ask' ? run.total : resolvers.length} resolvers, ${names} names`,
      share: askShare,
      state: stage === 'ask' ? 'now' : 'done',
      tone: 'var(--w-amber)',
    },
    {
      label: 'Time each edge',
      value: timing ? `${timing.timed} of ${timing.total} handshakes, ${timing.edges} edges` : 'waiting for lookups',
      note: silent.size ? `${silent.size} silent` : undefined,
      share: timeShare,
      state: stage === 'time' ? 'now' : stage === 'name' ? 'done' : 'wait',
      tone: 'var(--w-ember)',
    },
    {
      label: 'Name their networks',
      value: stage === 'name' ? `${named} of ${edges.length} edges, ${nets.length} networks` : 'after the resolver checks',
      share: stage === 'name' && edges.length ? named / edges.length : 0,
      state: stage === 'name' ? (named >= edges.length ? 'done' : 'now') : 'wait',
      tone: 'var(--w-rose)',
    },
  ]);

  // The fastest edge any resolver got for each CDN, as the handshakes come in.
  const colBest = $derived.by(() => {
    const out: Record<string, number> = {};
    for (const l of Object.values(lookups))
      for (const [id, c] of Object.entries(l.cells ?? {}))
        for (const ip of c.ips) if (best[ip] != null) out[id] = Math.min(best[ip], out[id] ?? Infinity);
    return out;
  });

  // Where a dot sits on its cell's track: clustered at the start until timed, then on a 1–300 ms log scale.
  const LO = Math.log(1);
  const HI = Math.log(300);
  const at = (ip: string, i: number) => {
    if (best[ip] != null) return Math.min(1, Math.max(0, (Math.log(Math.max(1, best[ip])) - LO) / (HI - LO)));
    if (silent.has(ip)) return 1;
    return Math.min(0.5, 0.03 + i * 0.07);
  };
  // A timed edge's colour: green within 2 ms of the column's best, then warmer the slower it is.
  const heat = (ip: string, id: string) => {
    const v = best[ip];
    if (v == null) return undefined;
    if (colBest[id] != null && v <= colBest[id] + 2) return 'var(--good)';
    return v < 20 ? 'var(--w-amber)' : v < 80 ? 'var(--w-ember)' : 'var(--w-rose)';
  };
  const median = (v: number[]) => {
    const s = [...v].sort((a, b) => a - b);
    return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2;
  };
  const cellMs = (c: CdnCell | undefined) => {
    const t = (c?.ips ?? []).flatMap((ip) => (best[ip] != null ? [best[ip]] : []));
    return t.length ? median(t) : null;
  };
  const cellTitle = (name: string, c: CdnCell | undefined) => {
    if (!c) return `${name}: not looked up yet`;
    const lines = [`${name}: ${c.ok.length} hostname${c.ok.length === 1 ? '' : 's'} answered from the CDN, ${c.ips.length} edge${c.ips.length === 1 ? '' : 's'}`];
    if (c.missed.length) lines.push(`No CDN answer: ${c.missed.join(', ')}`);
    for (const ip of c.ips) lines.push(`${ip}  ${best[ip] != null ? ms(best[ip]) : silent.has(ip) ? 'silent' : 'not timed yet'}`);
    return lines.join('\n');
  };
</script>

<section class="cdn" aria-labelledby="cdnl-h" aria-busy="true">
  <header class="head">
    <div>
      <h2 id="cdnl-h">CDN mapping</h2>
      <p class="lede">
        {#if stage === 'wait'}CDN mapping runs after the latency tests, so it never disturbs them.{:else}Each resolver is
          asked for the same CDN hostnames. Every edge it hands back is timed with a handshake and named by its network.{/if}
      </p>
    </div>
    {#if stage !== 'wait'}
      <PhaseStages {stages} label="CDN mapping progress" />
      {#if stage === 'name' && nets.length}
        <div class="nets" aria-live="polite">
          <span class="nets-h">Edge networks</span>
          {#each nets as x (x.key)}
            <span class="net" class:you={x.you} style:animation-delay="{enter.get(x.key) ?? 0}ms"
              >{x.name} <span class="num nn">{x.n}</span>{#if x.you}<span class="nyou">your ISP</span>{/if}</span
            >
          {/each}
          {#if named < edges.length}<span class="naming">naming {edges.length - named} more</span>{/if}
        </div>
      {/if}
    {/if}
  </header>

  <div class="wrap panel">
    <table>
      <thead>
        <tr>
          <th class="first" scope="col">Resolver</th>
          <th scope="col" class="names-h">Lookups</th>
          {#each CDNS as c}
            <th scope="col" class="cdn-h">
              <CdnLogo id={c.id} name={c.name} />
              <span class="cn">{c.name}</span>
              <span class="ch num">{c.any ? 'anycast' : colBest[c.id] != null ? `best ${ms(colBest[c.id], false)}` : ' '}</span>
            </th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each resolvers as r (r.id)}
          {@const l = lookups[r.id]}
          {@const ok = l ? Object.values(l.cells ?? {}).reduce((n, c) => n + c.ok.length, 0) : 0}
          {@const off = skipped.has(r.id)}
          {@const dead = off || (!!l?.finished && l.answered === 0)}
          <tr class:dead class:wait={stage === 'ask' && !l && !off} data-vtn style:--vtn="cdnr-{slot.get(r.id)}">
            <th class="first" scope="row"><b>{r.provider}</b> <span class="muted">{r.variant}</span></th>
            <td class="names">
              {#if dead}
                <span class="gone">{off ? 'never answered' : 'not answering'}</span>
              {:else if l?.finished}
                <span class="got num">{l.answered} of {l.asked} names</span>
              {:else if l}
                <span class="mini" aria-hidden="true"><span style:scale="{ok / ASKED} 1"></span></span>
                <span class="num">{ok} of {ASKED}</span>
              {:else if stage === 'ask'}
                <span class="asking">asking</span>
              {:else}
                <span class="muted">—</span>
              {/if}
            </td>
            {#if dead}
              <td class="deadband" colspan={CDNS.length}>
                <span>
                  {off
                    ? 'No answers in the earlier tests, so it was not asked.'
                    : 'Stopped answering after three lookups, so it was left out of the map.'}
                </span>
              </td>
            {:else}
              {#each CDNS as c}
                {@const cell = l?.cells?.[c.id]}
                {@const v = cellMs(cell)}
                <td class="edge" title={cellTitle(c.name, cell)}>
                  <span class="cl" aria-hidden="true"><CdnLogo id={c.id} name={c.name} />{c.name}</span>
                  <span class="track" class:timing={timed}>
                    {#each cell?.ips ?? [] as ip, i (ip)}
                      <i
                        class="dot"
                        class:timed={best[ip] != null}
                        class:silent={silent.has(ip)}
                        style:--c={heat(ip, c.id)}
                        style:--x={at(ip, i)}
                        style:--d="{i * 45}ms"
                      ></i>
                    {/each}
                    {#if cell && !cell.ips.length && cell.missed.length}<span class="none">no CDN answer</span>{/if}
                  </span>
                  <span class="v num">
                    {#if v != null}
                      <span class="arrive">{ms(v, false)}</span>
                    {:else if cell?.ips.length && cell.ips.every((ip) => silent.has(ip))}
                      <span class="sil">silent</span>
                    {/if}
                  </span>
                </td>
              {/each}
            {/if}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if timed}
    <p class="legend">
      <span><i class="dot timed" style:--c="var(--good)"></i> within 2 ms of the best edge</span>
      <span><i class="dot timed" style:--c="var(--w-amber)"></i> under 20 ms</span>
      <span><i class="dot timed" style:--c="var(--w-ember)"></i> 20 to 80 ms</span>
      <span><i class="dot timed" style:--c="var(--w-rose)"></i> over 80 ms</span>
      <span><i class="dot silent"></i> no handshake answer</span>
      <span>Track runs 1 to 300 ms, log scale.</span>
    </p>
  {/if}
</section>

<style>
  .cdn {
    --track: 58px;
    container-type: inline-size;
    display: flex;
    flex-direction: column;
    gap: 12px;
    height: 100%;
  }
  .head {
    display: grid;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: var(--t-lg);
  }
  .lede {
    margin: 4px 0 0;
    max-width: 68ch;
    color: var(--ink-2);
    font-size: var(--t-sm);
  }

  .mini {
    display: block;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .mini > span {
    display: block;
    height: 100%;
    background: var(--ink-3);
    transform-origin: left;
    transition: scale 500ms var(--ease-out);
  }
  .sil {
    color: var(--ink-3);
  }

  .wrap {
    flex: 0 1 auto;
    overflow: auto;
    min-height: 220px;
  }
  table {
    width: 100%;
    border-collapse: separate;
    border-spacing: 0;
    font-size: var(--t-xs);
  }
  th,
  td {
    padding: 8px 10px;
    border-bottom: 1px solid var(--rule);
    text-align: left;
    vertical-align: middle;
    white-space: nowrap;
  }
  thead th {
    position: sticky;
    top: 0;
    z-index: 1;
    padding-block: 6px;
    background: var(--surface-2);
    color: var(--ink-3);
    font-weight: 600;
  }
  tbody tr:last-child > * {
    border-bottom: 0;
  }
  .first {
    position: sticky;
    left: 0;
    min-width: 140px;
    max-width: 200px;
    background: var(--surface);
    border-right: 1px solid var(--rule);
    font-weight: 400;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  thead .first {
    z-index: 2;
    background: var(--surface-2);
  }
  .first,
  .first .muted {
    transition: color 240ms var(--ease-out);
  }
  .wait .first,
  .wait .first .muted {
    color: var(--ink-3);
  }
  .cdn-h {
    min-width: 64px;
    padding-inline: 8px;
    text-align: right;
  }
  .cdn-h :global(.logo) {
    display: inline-block;
    vertical-align: -3px;
    margin-right: 4px;
  }
  .cn {
    color: var(--ink-2);
  }
  .ch {
    display: block;
    font-weight: 400;
    font-size: var(--t-2xs);
    min-height: 1.3em;
  }
  .cdn-h:last-child,
  .edge:last-child {
    padding-right: 22px;
  }

  /* Lookups column. */
  .names {
    min-width: 116px;
    color: var(--ink-2);
  }
  .names .mini {
    display: inline-block;
    width: 40px;
    margin-right: 6px;
    vertical-align: middle;
    background: color-mix(in srgb, var(--w-amber) 14%, var(--surface-3));
  }
  .names .mini > span {
    background: var(--w-amber);
  }
  .got {
    animation: arrive 360ms var(--ease-out) both;
  }
  .asking {
    color: var(--ink-3);
    animation: breathe 1.4s ease-in-out infinite;
  }
  .gone {
    color: var(--warning);
    font-weight: 600;
  }

  /* An edge cell: the track of dots, then the median of its fastest handshakes. */
  .edge {
    text-align: right;
    padding-inline: 8px;
  }
  .track {
    position: relative;
    display: inline-block;
    width: var(--track);
    height: 14px;
    vertical-align: middle;
  }
  .track::before {
    content: '';
    position: absolute;
    inset: 50% 0 auto;
    height: 1px;
    background: var(--rule);
  }
  /* Once timing starts the track warms from green at the fast end to rose at the slow end. */
  .track.timing::before {
    height: 2px;
    margin-top: -0.5px;
    background: linear-gradient(90deg, var(--good) 10%, var(--w-amber) 45%, var(--w-ember) 70%, var(--w-rose) 95%);
    opacity: 0.35;
    animation: arrive 400ms var(--ease-out) both;
  }
  /* Ticks at 10 and 100 ms once the edges are being timed. */
  .track.timing::after {
    content: '';
    position: absolute;
    inset: 4px 0;
    background:
      linear-gradient(var(--rule-2), var(--rule-2)) calc(40.4% - 0.5px) 0 / 1px 100% no-repeat,
      linear-gradient(var(--rule-2), var(--rule-2)) calc(80.7% - 0.5px) 0 / 1px 100% no-repeat;
    animation: arrive 400ms var(--ease-out) both;
  }
  .dot {
    position: absolute;
    z-index: 1;
    left: 0;
    top: 50%;
    width: 6px;
    height: 6px;
    margin-top: -3px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--w-amber) 55%, var(--surface-3));
    translate: calc(var(--x, 0) * (var(--track) - 6px)) 0;
    transition:
      translate 900ms var(--ease-out) var(--d, 0ms),
      background-color 400ms var(--ease-out) var(--d, 0ms),
      box-shadow 400ms var(--ease-out);
    animation: pop 380ms var(--ease-out) both;
  }
  /* Amber while the lookups land; once timing starts, an edge still waiting is grey so only timed edges carry colour. */
  .timing .dot {
    background: var(--rule-2);
  }
  .dot.timed {
    background: var(--c, var(--ink));
  }
  .dot.silent {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--ink-3);
  }
  .none {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center end;
    color: var(--ink-3);
    font-size: var(--t-2xs);
  }
  .v {
    display: inline-block;
    min-width: 3.2em;
    margin-left: 6px;
    color: var(--ink);
    text-align: right;
    vertical-align: middle;
  }
  .arrive {
    display: inline-block;
    animation: arrive 360ms var(--ease-out) both;
  }

  /* A resolver that stopped answering: one band across its CDN cells. */
  .dead .first,
  .dead .first .muted {
    color: var(--ink-3);
  }
  .deadband {
    background: repeating-linear-gradient(-45deg, transparent 0 6px, var(--surface-2) 6px 12px);
    color: var(--warning);
    animation: arrive 400ms var(--ease-out) both;
  }

  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    margin: 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .legend .dot {
    position: relative;
    display: inline-block;
    top: 0;
    margin: 0 4px 0 0;
    translate: none;
    vertical-align: 0;
    animation: none;
  }

  .nets {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .nets-h {
    margin-right: 4px;
    color: var(--ink-3);
    font-weight: 600;
  }
  .net {
    padding: 2px 8px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--w-rose) 9%, var(--surface));
    border: 1px solid color-mix(in srgb, var(--w-rose) 35%, var(--rule));
    white-space: nowrap;
    animation: pop 420ms var(--ease-out) both;
  }
  .nn {
    color: var(--ink-3);
    margin-left: 2px;
  }
  .net.you {
    border-color: var(--good);
    color: var(--ink);
  }
  .nyou {
    margin-left: 6px;
    color: var(--good);
    font-weight: 600;
  }
  .naming {
    color: var(--ink-3);
    animation: breathe 1.4s ease-in-out infinite;
  }

  /* The CDN name inside each cell, shown only once the header row is folded away. */
  .cl {
    display: none;
  }

  /* Narrow: each resolver becomes a block, its CDN cells a grid of three with their names inline. */
  @container (max-width: 820px) {
    table,
    tbody {
      display: block;
    }
    thead {
      display: none;
    }
    tbody tr {
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
      column-gap: 14px;
      padding: 10px 12px;
      border-bottom: 1px solid var(--rule);
    }
    tbody tr:last-child {
      border-bottom: 0;
    }
    th,
    td {
      padding: 3px 0;
      border-bottom: 0;
    }
    .first {
      position: static;
      grid-column: 1 / -1;
      max-width: none;
      border-right: 0;
      background: none;
      font-size: var(--t-sm);
    }
    .names {
      grid-column: 1 / -1;
      min-width: 0;
      padding-bottom: 6px;
    }
    .edge {
      display: flex;
      align-items: center;
      gap: 6px;
      min-width: 0;
      text-align: left;
    }
    .edge:last-child {
      padding-right: 0;
    }
    .cl {
      display: inline-flex;
      align-items: center;
      gap: 4px;
      flex: 1 1 auto;
      min-width: 0;
      overflow: hidden;
      text-overflow: ellipsis;
      color: var(--ink-2);
    }
    .v {
      min-width: 2.6em;
    }
    .deadband {
      grid-column: 1 / -1;
      white-space: normal;
      padding: 6px 8px;
    }
  }
  @container (max-width: 520px) {
    tbody tr {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  @keyframes pop {
    from {
      scale: 0.4;
      opacity: 0;
    }
  }
  @keyframes arrive {
    from {
      opacity: 0;
      transform: translateY(3px);
    }
  }
  @keyframes breathe {
    50% {
      opacity: 0.4;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .dot,
    .got,
    .asking,
    .arrive,
    .deadband,
    .net,
    .naming,
    .track.timing::before,
    .track.timing::after {
      animation: none;
    }
    .dot,
    .mini > span {
      transition: none;
    }
  }
</style>
