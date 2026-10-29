<script lang="ts" module>
  export type CapView = 'all' | 'privacy' | 'blocking' | 'routing' | 'protocol';
  export const CAP_VIEWS: [CapView, string][] = [
    ['all', 'All'],
    ['privacy', 'Privacy & security'],
    ['blocking', 'Blocking'],
    ['routing', 'Routing'],
    ['protocol', 'Protocol'],
  ];
</script>

<script lang="ts">
  import { app } from '../lib/store.svelte';
  import { mobile } from '../lib/api';
  import type { Resolver, RunState } from '../lib/types';
  import type { Row, Verdict } from '../lib/verdict';
  import { capCell, cdnCell, egressCell, isBlank, liveCell, roaCell, rovCell, type Cell as C } from '../lib/cells';
  import { CAP, CAP_KEYS, LIVE_CAPS } from '../lib/caps';
  import { label } from '../lib/format';
  import Cell from './Cell.svelte';
  import Glyph from './Glyph.svelte';
  import Info from './Info.svelte';
  import Provenance from './Provenance.svelte';

  let {
    run,
    rows,
    v = null,
    view = $bindable('all'),
    onopen,
  }: { run: RunState; rows: Row[]; v?: Verdict | null; view?: CapView; onopen: (id: string) => void } = $props();

  // One column per resolver, in ranking order.
  const resolvers = $derived.by(() => {
    const seen = new Set<string>();
    const out: Resolver[] = [];
    for (const r of rows) {
      if (seen.has(r.target.resolver_id) || !r.resolver) continue;
      seen.add(r.target.resolver_id);
      out.push(r.resolver);
    }
    return out;
  });
  const nameOf = (r: Resolver) => label(r.provider, r.variant);
  // In lists, the provider alone, unless two resolvers share it.
  const short = (r: Resolver) => (resolvers.filter((x) => x.provider === r.provider).length > 1 ? nameOf(r) : r.provider);

  // The sheet scrolls with the page, under a pinned header, until there are more resolvers than fit across; then it
  // scrolls sideways by itself, with the capability names pinned instead.
  let wrapW = $state(0);
  let tableW = $state(0);
  const across = $derived(tableW > wrapW + 1);

  type Line = { key: string; label: string; title: string; ref?: string; cell: (r: Resolver) => C };
  const cap = (key: string): Line => ({ key, label: CAP[key].label, title: CAP[key].explain, ref: CAP[key].ref, cell: (r) => capCell(run, r, key) });
  // The live threat test's rows join Blocking once any resolver has a result.
  const hasLive = $derived(Object.values(run.live ?? {}).some((l) => Object.values(l.categories).some((c) => c.tested > 0)));
  const liveLines = $derived(
    hasLive ? LIVE_CAPS.map((c): Line => ({ key: c.key, label: c.label, title: c.explain, cell: (r) => liveCell(run, r, c.category) })) : [],
  );
  // Capabilities run down the page and resolvers across it, like a spec sheet: seventeen columns never fit, nine
  // resolvers do, and a capability every resolver shares collapses to one line.
  const groups: [Exclude<CapView, 'all'>, string, Line[]][] = $derived([
    ['privacy', 'Privacy & security', CAP_KEYS.privacy.map(cap)],
    ['blocking', 'Blocking', [...CAP_KEYS.blocking.map(cap), ...liveLines]],
    [
      'routing',
      'Routing',
      [
        { key: 'cdn', label: 'CDN edges', title: 'Median extra round trip to CDN edges compared with the best any resolver found.', cell: (r) => cdnCell(run, r.id) },
        { key: 'roa', label: 'RPKI route origin', title: 'Whether the resolver’s address block is covered by a valid RPKI route origin authorization.', cell: (r) => roaCell(run, r.id) },
        { key: 'rov', label: 'Invalid routes filtered', title: 'Whether a lookup from this resolver can be lured to an RPKI-invalid route, tested live over IPv4 and IPv6 with Check My DNS by DNS-OARC. Filtering anywhere on the resolver’s path counts.', cell: (r) => rovCell(run, r.id) },
        { key: 'egress', label: 'Egress network', title: 'The network the resolver’s queries leave from for the Internet, from its egress address.', cell: (r) => egressCell(run, r.id) },
      ],
    ],
    ['protocol', 'Protocol', CAP_KEYS.protocol.map(cap)],
  ]);

  // While the run works, a cell its check hasn't reached yet is a skeleton: the probes fill a resolver's column, the CDN
  // mapping and the network intel fill the routing rows. Lines stay apart until the end, so nothing collapses mid-run.
  const running = $derived(run.status === 'running');
  const INTEL = new Set(['roa', 'rov', 'egress']);
  const waits = (key: string, r: Resolver) =>
    running &&
    (key === 'cdn'
      ? !!run.config.cdn && !run.cdn
      : INTEL.has(key)
        ? !run.intel?.[r.id]
        : key.startsWith('live_')
          ? !!run.config.live && !run.live?.[r.id]
          : run.config.probes !== false && !run.probes[r.id]);
  const busy = (key: string) => running && (key === 'cdn' ? run.phase === 'cdn' : key.startsWith('live_') ? run.phase === 'live' : INTEL.has(key) && run.phase === 'intel');

  type Built = { line: Line; cells: C[]; wait: boolean[]; same: boolean; blank: boolean };
  const built = $derived(
    groups.map(([k, title, lines]) => ({
      k,
      title,
      lines: lines.map((line): Built => {
        const cells = resolvers.map(line.cell);
        const wait = resolvers.map((r) => waits(line.key, r));
        const blank = cells.every(isBlank);
        const same = !running && resolvers.length > 1 && !blank && cells.every((c) => c.text === cells[0].text && c.status === cells[0].status && c.prov === cells[0].prov);
        return { line, cells, wait, same, blank };
      }),
    })),
  );
  const shown = $derived(view === 'all' ? built : built.filter((g) => g.k === view));

  // Ad blocking reads as a share of the common ad hosts tried, so it gets a bar as well as the count, and so do the
  // live threat rows. The other categories use vendors' own test names, which only confirm that blocking happens.
  const share = (key: string, r: Resolver) => {
    if (key.startsWith('live_')) {
      const f = run.live?.[r.id]?.categories[key.slice(5)];
      return f && f.tested ? f : null;
    }
    if (key !== 'filter_ads') return null;
    const f = run.probes[r.id]?.filtering?.[key.slice(7)];
    return f && f.tested ? f : null;
  };

  // Phone: each capability lists its values, with the resolvers that share each one, in ranking order.
  const bucket = (l: { line: Line; cells: C[]; wait: boolean[] }) => {
    const out = new Map<string, { c: C; s: ReturnType<typeof share>; wait: boolean; names: string[] }>();
    resolvers.forEach((r, i) => {
      const c = l.cells[i];
      const s = share(l.line.key, r);
      const k = l.wait[i] ? 'wait' : `${c.status}|${c.text}|${c.prov}|${s ? `${s.blocked}/${s.tested}` : ''}`;
      const b = out.get(k);
      if (b) b.names.push(short(r));
      else out.set(k, { c, s, wait: l.wait[i], names: [short(r)] });
    });
    return [...out.values()];
  };
  const MOBILE_VIEW: Record<CapView, string> = { all: 'All', privacy: 'Privacy', blocking: 'Blocking', routing: 'Routing', protocol: 'Protocol' };

  function withProbes() {
    if (app.draft) {
      app.draft.probes = true;
      app.saveDraft();
    }
    app.navigate('/');
  }

  const probed = $derived(run.config.probes !== false && Object.keys(run.probes).length > 0);

  // The verdict: the two picks that are about capabilities, then what this run found to differ.
  const head = $derived.by(() => {
    if (running && run.config.probes !== false) {
      const p = run.phase;
      if (p === 'cdn' || p === 'intel')
        return { sentence: 'Probes are done.', detail: 'The routing rows fill in after the CDN mapping and network intel.', picks: [] };
      if (p === 'live') return { sentence: 'Probes are done.', detail: 'The live threat rows fill in as each resolver is tested.', picks: [] };
      return {
        sentence: 'Capabilities are probed after the latency tests.',
        detail: 'Probing waits so it never disturbs the timing. Each resolver’s column fills in as it answers.',
        picks: [],
      };
    }
    if (!v) return null;
    const all = built.flatMap((g) => g.lines).filter((l) => !l.blank);
    if (!probed && hasLive) {
      return {
        sentence: 'Probes were off; the live threat test ran.',
        detail: 'The live threat rows were measured from your network; the others show only what operators publish (○). Turn on Capability probes in the next run’s setup to measure those too.',
        picks: [],
      };
    }
    if (!probed) {
      return {
        sentence: 'Probes were off for this run.',
        detail: mobile
          ? 'Probes test DNSSEC validation, ECS, QNAME minimisation and the rest from your network. Start a new run with them on to see what each resolver does.'
          : 'So this shows only what operators publish (○), not what the resolvers did from your network. Turn on Capability probes in the next run’s setup to measure them.',
        picks: [],
      };
    }
    const pick = (goal: string) => v.picks.find((p) => p.goal === goal);
    const protect = pick('Best protection');
    const priv = pick('Most private');
    // A run can leave malware blocking off, and then it can't say no resolver blocks.
    const malwareTested = hasLive || resolvers.some((r) => run.probes[r.id]?.filtering?.malware);
    // With the live threat test, protection is what a resolver blocked of live hosts, not test names.
    const lacks = hasLive ? 'blocked a live threat' : 'both blocks malware and validates DNSSEC';
    const sentence = !malwareTested
      ? priv
        ? `${priv.name} is the most private.`
        : 'No resolver here passes every privacy check.'
      : protect && priv
        ? protect.resolverId === priv.resolverId
          ? `${protect.name} protects best and is the most private.`
          : `${protect.name} protects best; ${priv.name} is the most private.`
        : protect
          ? `${protect.name} protects best; no resolver here passes every privacy check.`
          : priv
            ? `${priv.name} is the most private; no resolver here ${lacks}.`
            : `No resolver here ${lacks}, and none passes every privacy check.`;

    const differ = all.filter((l) => !l.same).length;
    let detail = `${all.length} capabilities probed from this network with fresh names no cache can answer; the resolvers differ on ${differ}.`;
    // Measured failures, grouped by capability, so the sentence stays short however many resolvers there are. A far CDN
    // edge is a cost, not a missing capability, so routing stays out of it.
    const gaps = built
      .filter((g) => g.k !== 'routing')
      .flatMap((g) => g.lines)
      .filter((l) => !l.blank)
      .map((l) => ({ l, who: resolvers.filter((_, i) => l.cells[i].status === 'fail' && l.cells[i].prov === 'verified').map(short) }))
      .filter((g) => g.who.length);
    // Name at most three resolvers anywhere in the head; a 26-resolver run would otherwise list them all.
    const few = (xs: string[]) => (xs.length > 3 ? `${xs.slice(0, 3).join(', ')} and ${xs.length - 3} more` : xs.length > 1 ? `${xs.slice(0, -1).join(', ')} and ${xs[xs.length - 1]}` : xs[0]);
    if (gaps.length) {
      const listed = gaps.slice(0, 3).map((g) => `${g.l.line.label} (${few(g.who)})`);
      detail += ` Gaps, marked ✕: ${listed.join(', ')}${gaps.length > 3 ? `, and ${gaps.length - 3} more` : ''}.`;
    }

    const both = resolvers.filter((r) => capCell(run, r, 'dot').status === 'pass' && capCell(run, r, 'dot').prov === 'verified' && capCell(run, r, 'doh').status === 'pass' && capCell(run, r, 'doh').prov === 'verified');
    const without = resolvers.filter((r) => !both.includes(r)).map(short);
    const ecs = resolvers.filter((r) => run.probes[r.id]?.checks.find((c) => c.key === 'ecs')?.present === true);
    const of = (k: number) => `${k} of ${resolvers.length}`;
    return {
      sentence,
      detail,
      picks: [
        {
          goal: 'Best protection',
          name: protect?.name ?? (malwareTested ? 'None' : 'Not tested'),
          ev: protect?.evidence ?? (hasLive ? 'None blocked a live threat' : malwareTested ? 'None both blocks malware and validates DNSSEC' : 'Malware blocking was off for this run'),
        },
        { goal: 'Most private', name: priv?.name ?? 'None', ev: priv?.evidence ?? 'None sends no subnet, minimises names and validates DNSSEC' },
        { goal: 'Encrypted DNS', name: of(both.length), ev: without.length ? `answer over DoT and DoH; not ${few(without)}` : 'All answer over DoT and DoH from here' },
        { goal: 'Sends your subnet', name: ecs.length ? of(ecs.length) : 'None', ev: ecs.length ? `${few(ecs.map(short))} pass part of your address on (ECS)` : 'None passes part of your address on' },
      ],
    };
  });
</script>

{#if head}
  <section class="verdict" aria-labelledby="cv-h">
    <div class="lead">
      <h2 id="cv-h">{head.sentence}</h2>
      <p class="detail">{head.detail}</p>
    </div>
    {#if head.picks.length}
      <dl class="picks">
        {#each head.picks as p}
          <div class="pick">
            <dt>{p.goal}</dt>
            <dd><span class="pn">{p.name}</span><span class="ev">{p.ev}</span></dd>
          </div>
        {/each}
      </dl>
    {/if}
  </section>
{/if}

{#if mobile && !probed && !hasLive && !running}
  <!-- Phone: without probes the matrix only repeats what operators publish, which the resolver sheet shows too. -->
  <button class="btn" onclick={withProbes}>New run with probes</button>
{:else}
<div class="bar chrome">
  <div class="segmented" role="group" aria-label="Capabilities shown">
    {#each CAP_VIEWS as [k, l]}<button aria-pressed={view === k} onclick={() => (view = k)}>{mobile ? MOBILE_VIEW[k] : l}</button>{/each}
  </div>
  <Info label="About the probes">
    <p>What each resolver actually does, probed from this network with fresh, uncacheable names.</p>
    <p>{mobile ? 'Open a resolver from Overview for its evidence.' : 'Hover a cell for its evidence.'} Select a resolver’s name for the full record, with the raw responses.</p>
    <p>A capability every resolver shares collapses to one line. Blocking shows the share of test names blocked.</p>
  </Info>
</div>

{#if mobile}
  <div class="mcaps">
    {#each shown as g (g.k)}
      {#if view === 'all'}<h3 class="mgrp">{g.title}</h3>{/if}
      {#each g.lines as l (l.line.key)}
        <div class="mcap">
          <p class="mcap-h"><span class="cap">{l.line.label}</span>{#if l.line.ref}<span class="ref">{l.line.ref}</span>{/if}</p>
          {#if l.same}
            <p class="mval"><Cell c={l.cells[0]} sheet /><span class="who">every resolver</span></p>
          {:else}
            {#each bucket(l) as b}
              <p class="mval" class:arrive={running && !b.wait}>
                {#if b.wait}
                  <span class="skel" class:now={busy(l.line.key)}></span>
                {:else if b.s}
                  <span class="blk">
                    <Glyph status={b.c.status} size={13} />
                    <span class="track" aria-hidden="true"><span style:width="{(b.s.blocked / b.s.tested) * 100}%"></span></span>
                    <span class="t">{b.s.blocked}/{b.s.tested}</span>
                  </span>
                {:else}
                  <Cell c={b.c} sheet />
                {/if}
                <span class="who">{b.names.join(', ')}</span>
              </p>
            {/each}
          {/if}
        </div>
      {/each}
    {/each}
  </div>
{:else}
<!-- svelte-ignore a11y_no_noninteractive_tabindex (scrollable region must be keyboard-reachable) -->
<div class="wrap" class:across role="region" aria-label="Capabilities by resolver" tabindex="0" style:--n={resolvers.length} bind:clientWidth={wrapW}>
  <table bind:clientWidth={tableW}>
    <thead>
      <tr>
        <th class="first" scope="col"><span class="vh">Capability</span></th>
        {#each resolvers as r (r.id)}
          <th scope="col" class:sel={app.inspect === r.id}>
            <button onclick={() => onopen(r.id)}><b>{r.provider}</b><span>{r.variant}</span></button>
          </th>
        {/each}
      </tr>
    </thead>
    {#each shown as g (g.k)}
      <tbody>
        {#if view === 'all'}
          <tr class="grp"><th colspan={resolvers.length + 1} scope="colgroup"><span>{g.title}</span></th></tr>
        {/if}
        {#each g.lines as l (l.line.key)}
          <tr>
            <th class="first" scope="row" title={l.line.title}>
              <span class="cap">{l.line.label}</span>
              {#if l.line.ref}<span class="ref">{l.line.ref}</span>{/if}
            </th>
            {#if l.same}
              <td class="same" colspan={resolvers.length}>
                <Cell c={l.cells[0]} sheet />
                <span class="every">every resolver</span>
              </td>
            {:else}
              {#each resolvers as r, i (r.id)}
                {@const s = share(l.line.key, r)}
                <td class:sel={app.inspect === r.id} class:arrive={running && !l.wait[i]}>
                  {#if l.wait[i]}
                    <span class="skel" class:now={busy(l.line.key)}></span>
                  {:else if s}
                    {@const c = l.cells[i]}
                    <span class="blk" title={c.evidence ? `${c.text}: ${c.evidence}` : c.text}>
                      <Glyph status={c.status} size={13} />
                      <span class="track" aria-hidden="true"><span style:width="{(s.blocked / s.tested) * 100}%"></span></span>
                      <span class="t">{s.blocked}/{s.tested}</span>
                    </span>
                  {:else}
                    <Cell c={l.cells[i]} sheet />
                  {/if}
                </td>
              {/each}
            {/if}
          </tr>
        {/each}
      </tbody>
    {/each}
  </table>
</div>
{/if}
<p class="legend">
  <span>Measured from your network in this run, unless marked:</span>
  <span><Provenance kind="published" /> published by the operator, not verified</span>
  <span>— unknown</span>
</p>
{/if}

<style>
  /* The verdict sits on the page plane, as on Overview and Reliability; the sheet below is the one raised surface. */
  .verdict {
    container: cverdict / inline-size;
    padding: 4px 4px 22px;
    display: grid;
    gap: 20px;
    max-width: 1180px;
  }
  h2#cv-h {
    font-size: var(--t-2xl);
    font-weight: 600;
    letter-spacing: -0.02em;
    line-height: 1.14;
    max-width: 24ch;
    text-wrap: balance;
  }
  .detail {
    margin-top: 10px;
    color: var(--ink-2);
    max-width: 64ch;
  }
  .picks {
    margin: 0;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(0, 1fr);
    padding-top: 16px;
    border-top: 1px solid var(--rule);
  }
  .pick {
    display: grid;
    align-content: start;
    gap: 4px;
    padding: 0 18px;
    border-left: 1px solid var(--rule);
  }
  .pick:first-child {
    padding-left: 0;
    border-left: 0;
  }
  dt {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  dd {
    margin: 0;
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .pn {
    font-weight: 620;
    font-size: var(--t-sm);
  }
  .ev {
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  @container cverdict (min-width: 1060px) {
    .lead {
      display: grid;
      grid-template-columns: minmax(0, 0.9fr) minmax(0, 1.1fr);
      column-gap: 48px;
      align-items: start;
    }
    .detail {
      margin-top: 3px;
    }
  }
  @container cverdict (max-width: 640px) {
    .picks {
      grid-auto-flow: row;
      gap: 12px;
    }
    .pick {
      padding: 12px 0 0;
      border-left: 0;
      border-top: 1px solid var(--rule);
    }
    .pick:first-child {
      padding-top: 0;
      border-top: 0;
    }
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }

  .wrap {
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--surface);
  }
  .wrap.across {
    overflow-x: auto;
  }
  table {
    border-collapse: separate;
    border-spacing: 0;
    width: 100%;
    font-size: var(--t-xs);
  }
  th,
  td {
    padding: 9px 10px;
    border-bottom: 1px solid var(--rule);
    text-align: left;
    vertical-align: top;
  }
  td {
    min-width: 92px;
    width: calc((100% - 180px) / var(--n));
  }
  tbody:last-child tr:last-child > * {
    border-bottom: 0;
  }
  tbody tr:not(.grp):hover > * {
    background: var(--surface-2);
  }
  .sel,
  tbody tr:not(.grp):hover > .sel {
    background: var(--sel);
  }
  /* Pinned to the pane's top edge, above its 22px padding, so no row shows through the gap. */
  thead th {
    position: sticky;
    top: -22px;
    z-index: 2;
    background: var(--surface-2);
    vertical-align: bottom;
  }
  thead th.sel {
    background: var(--sel);
  }
  thead button {
    display: grid;
    padding: 0;
    border: 0;
    background: none;
    text-align: left;
    cursor: default;
    font-size: var(--t-sm);
    line-height: 1.25;
  }
  thead button span {
    color: var(--ink-3);
    font-size: var(--t-xs);
    font-weight: 400;
  }
  thead button:hover b {
    text-decoration: underline;
    text-decoration-color: var(--rule-2);
    text-underline-offset: 3px;
  }
  thead th:first-child {
    border-top-left-radius: var(--r-lg);
  }
  thead th:last-child {
    border-top-right-radius: var(--r-lg);
  }
  .first {
    position: sticky;
    left: 0;
    z-index: 1;
    width: 180px;
    min-width: 180px;
    background: var(--surface);
    border-right: 1px solid var(--rule);
    cursor: help;
  }
  thead .first {
    z-index: 3;
    background: var(--surface-2);
    cursor: default;
  }
  .cap {
    display: block;
    color: var(--ink);
    font-weight: 560;
    font-size: var(--t-sm);
  }
  .ref {
    display: block;
    margin-top: 1px;
    color: var(--ink-3);
    font-weight: 400;
  }
  /* Group heads are hairline captions, not bands. */
  .grp th {
    padding: 18px 10px 6px;
    background: var(--surface);
    color: var(--ink-2);
    font-weight: 620;
    font-size: var(--t-xs);
  }
  .grp span {
    position: sticky;
    left: 10px;
  }
  tbody:first-of-type .grp th {
    padding-top: 10px;
  }
  .same {
    color: var(--ink-3);
  }
  .same :global(.cell) {
    margin-right: 8px;
  }
  .every {
    font-size: var(--t-xs);
  }
  .blk {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    white-space: nowrap;
  }
  .track {
    position: relative;
    width: 28px;
    height: 4px;
    border-radius: 2px;
    background: var(--rule);
    overflow: hidden;
  }
  .track span {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--ink-2);
  }
  .t {
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }
  .vh {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
  .legend {
    display: flex;
    gap: 6px 18px;
    flex-wrap: wrap;
    margin-top: 10px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .legend span {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  /* Phone: a smaller headline, and capabilities as a list rather than a sheet. */
  :global(html.mobile) h2#cv-h {
    font-size: var(--t-lg);
    letter-spacing: -0.01em;
  }
  :global(html.mobile) .verdict {
    padding: 0 0 16px;
    gap: 14px;
  }
  .mcaps {
    display: grid;
    border-top: 1px solid var(--rule);
  }
  .mgrp {
    padding: 18px 0 6px;
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-2);
    border-bottom: 1px solid var(--rule);
  }
  .mcaps > .mgrp:first-child {
    padding-top: 10px;
  }
  .mcap {
    display: grid;
    gap: 6px;
    padding: 10px 0;
    border-bottom: 1px solid var(--rule);
  }
  .mcap-h .ref {
    font-size: var(--t-xs);
  }
  .mval {
    display: grid;
    grid-template-columns: minmax(0, auto) 1fr;
    align-items: baseline;
    gap: 10px;
    font-size: var(--t-xs);
  }
  .mval :global(.cell) {
    white-space: normal;
  }
  .who {
    color: var(--ink-3);
    text-align: right;
  }
</style>
