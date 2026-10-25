<script lang="ts">
  import type { CdnInfo, CdnReport, Mapping, Reference, Resolver, RunState } from '../lib/types';
  import { app } from '../lib/store.svelte';
  import { mobile } from '../lib/api';
  import { holder, label, list, ms } from '../lib/format';
  import { cellPenalty, ecsByAuthority, edgeIps, level, youOf } from '../lib/cdn';
  import CdnLogo from './CdnLogo.svelte';
  import Info from './Info.svelte';

  let { run, cdn, onopen }: { run: RunState; cdn: CdnReport; onopen: (id: string) => void } = $props();

  const byId = $derived(new Map(run.resolvers.map((r) => [r.id, r])));
  const name = (id: string) => {
    const r = byId.get(id);
    return r ? label(r.provider, r.variant) : id;
  };

  // DNS-steered CDNs get a column each. Anycast controls come last, marked as such, and
  // stay out of the findings and the penalty.
  const steered = $derived(cdn.cdns.filter((c) => c.role === 'steered'));
  const uniform = $derived(cdn.cdns.filter((c) => c.role === 'control'));
  // Resolvers that answered nothing earlier in the run are not asked.
  const left = $derived(run.resolvers.filter((r) => !(r.id in cdn.resolvers)));
  const short = (c: CdnInfo) => c.name.replace(/\s*\(.*\)$/, '').replace('Amazon ', '');
  const yous = $derived(new Map(cdn.cdns.map((c) => [c.id, youOf(c)])));
  const you = (c: CdnInfo) => yous.get(c.id) ?? null;
  const hostCount = $derived(cdn.cdns.reduce((n, c) => n + c.hosts.length, 0));
  const tableCols = $derived([...steered, ...uniform]);

  const rows = $derived(
    Object.entries(cdn.resolvers)
      .map(([id, r]) => ({ id, r, res: byId.get(id) as Resolver | undefined }))
      .filter((x) => x.res)
      .sort((a, b) => (a.r.penalty_ms ?? Infinity) - (b.r.penalty_ms ?? Infinity)),
  );
  const maxPen = $derived(Math.max(10, ...rows.map((x) => x.r.penalty_ms ?? 0)));
  const shade = (p: number | null | undefined) => (p == null ? 0 : Math.min(1, p / Math.max(30, maxPen)) * 0.26);
  const hasRef = $derived(steered.some((c) => you(c)?.ips.length));

  const clientNet = $derived(run.env?.vantage?.network);
  const clientIp = $derived(run.env?.vantage?.ipv4 ?? null);
  const edges = $derived(Object.values(cdn.edges));
  const onNet = $derived(edges.filter((e) => e.on_net));

  // The networks the edges sit in, largest first, your ISP's ahead.
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
    return [...m.values()].sort((a, b) => Number(b.you) - Number(a.you) || b.n - a.n);
  });

  /** What a resolver tells CDNs about you, short enough for a column. */
  function ecs(id: string): { text: string; title: string; tone: 'you' | 'other' | 'none' } {
    const r = cdn.resolvers[id];
    const p = run.probes[id];
    const by = ecsByAuthority(r, p);
    const sent = Object.values(by).filter((s): s is string => !!s);
    const len = (s: string) => `/${s.split('/')[1] ?? '?'}`;
    const seen =
      Object.entries(by)
        .map(([who, s]) => `${who}'s nameservers saw ${s ?? 'no subnet'}`)
        .join('; ') + '.';
    if (r.basis === 'on_net') return { text: 'On-net', title: `Resolves from inside your own network${clientNet?.asn ? ` (AS${clientNet.asn})` : ''}. ${seen}`, tone: 'you' };
    if (r.ecs_is_client === false && sent[0]) return { text: `Other ${len(sent[0])}`, title: `Sends a subnet that isn't yours, so CDNs map that network instead. ${seen}`, tone: 'other' };
    if (sent.length === Object.keys(by).length && sent[0]) return { text: len(sent[0]), title: `Sends your subnet. ${seen}`, tone: 'you' };
    if (sent[0]) return { text: 'Selective', title: `Sends your subnet (${len(sent[0])}) to some authorities and not others. ${seen}`, tone: 'you' };
    if (!p) return { text: '?', title: 'Not probed in this run.', tone: 'none' };
    if (!Object.keys(by).length) return { text: '?', title: "Couldn't be tested: no echoing authority answered.", tone: 'none' };
    return { text: 'None', title: `Sends no client subnet: CDNs see only the resolver's own address. ${seen}`, tone: 'none' };
  }

  /** A cell's address, the penalty it shows and whether it is level with the best. */
  const cell = (c: CdnInfo, m: Mapping | undefined) => ({ pen: cellPenalty(c, m), level: level(m) });

  function cellTitle(c: CdnInfo, m: Mapping | undefined, who: string) {
    if (!m) return '';
    const lines = [`${c.name} via ${who}`, `${m.hosts_used} of ${m.hosts_total} hostnames used`];
    if (m.error) lines.push(m.error);
    if (m.worst_host && m.worst_host[1] >= 0.05) lines.push(`Worst: ${m.worst_host[0]}, +${ms(m.worst_host[1])}`);
    const mine = m?.as_you;
    if (mine === 'same') lines.push('Same mapping as your own address.');
    if (mine === 'near') lines.push('Edges in the same cluster as your own address, about as near.');
    if (mine === 'different') lines.push(`Not the edge ${c.name} picks for your own address.`);
    return lines.join('\n');
  }
  function headTitle(c: CdnInfo) {
    const unexpected = (h: CdnInfo['hosts'][number]) =>
      h.unexpected ? ` (unexpected: ${c.role === 'control' ? 'edges differ between resolvers' : 'same edges for every resolver'})` : '';
    const names = c.hosts.map((h) => `${h.name}${unexpected(h)}`);
    return [...names, `${c.role === 'control' ? 'anycast control' : 'best edge'} ${ms(c.best_ms)}`].join('\n');
  }
  function refTitle(c: CdnInfo, y: Reference | null | undefined) {
    if (!y) return 'Not asked.';
    const lines = c.hosts.flatMap((h) => (h.you ? [`${h.name}: ${h.you.error ?? `${ms(h.you.rtt_ms)} via ${h.you.server}`}`] : []));
    return [`${c.name}, asked directly from this computer`, ...lines].join('\n');
  }

  // The findings, in at most two sentences above the table.
  const best = $derived(rows.find((x) => x.r.penalty_ms != null));
  const worst = $derived([...rows].reverse().find((x) => x.r.penalty_ms != null));
  // CDNs whose own pick for your address is clearly slower than what some resolver got.
  const slowMaps = $derived.by(() => {
    const m = new Map<string, number[]>();
    for (const c of steered) {
      const p = you(c)?.penalty_ms ?? 0;
      if (p < 3) continue;
      m.set(c.name, [...(m.get(c.name) ?? []), p]);
    }
    return [...m.entries()].map(([cdn, ps]) => ({ cdn, lo: Math.min(...ps), hi: Math.max(...ps) }));
  });
  const span = (x: { lo: number; hi: number }) => (x.hi - x.lo < 0.5 ? ms(x.hi) : `${ms(x.lo, false)}–${ms(x.hi)}`);
  // Where the CDN's own pick for you was slow: what did the resolvers that send your
  // subnet get on those same hosts? Counted, not explained: maps shift between lookups.
  const ecsSenders = $derived(rows.filter(({ r }) => (r.ecs || r.ecs_akamai) && r.ecs_is_client !== false && r.basis !== 'on_net'));
  const slowCols = $derived(steered.filter((c) => (you(c)?.penalty_ms ?? 0) >= 3));
  const outcome = $derived.by(() => {
    let same = 0;
    let nearer = 0;
    let farther = 0;
    for (const { r } of ecsSenders) {
      for (const c of slowCols) {
        if (!r.ecs) continue;
        const m = r.mappings[c.id];
        const y = m?.as_you;
        const mine = you(c)?.rtt_ms;
        if (y == null || m?.rtt_ms == null || mine == null) continue;
        if (y !== 'different') same++;
        else if (m.rtt_ms < mine) nearer++;
        else farther++;
      }
    }
    return { same, nearer, farther, total: same + nearer + farther };
  });
  // Resolvers that send your subnet yet got farther edges than your own address: the CDN
  // most likely ignored their ECS.
  const ignored = $derived.by(() => {
    const out: { id: string; cdns: string[] }[] = [];
    for (const { id, r } of ecsSenders) {
      if (!r.ecs) continue;
      const miss = steered.filter((c) => {
        const m = r.mappings[c.id];
        const mine = you(c)?.rtt_ms;
        return m?.as_you === 'different' && m?.rtt_ms != null && mine != null && m.rtt_ms - mine >= 3;
      });
      if (miss.length) out.push({ id, cdns: [...new Set(miss.map((c) => c.name))] });
    }
    return out;
  });
</script>

<section class="cdn" aria-labelledby="cdn-h">
  <header class="head">
    <div class="titles">
      <h2 id="cdn-h">CDN mapping</h2>
      <Info label="How CDN mapping is measured">
        <p>
          Most of a web page comes from a CDN, and your resolver decides which CDN edge you’re sent to. A lookup is paid once, but
          every new connection then waits at least three round trips on that edge, so an edge 3 ms farther costs about 9 ms each time.
          That’s why nearest CDN edges is a pick of its own, beside the fastest answers.
        </p>
        <p>
          CDNs that steer by DNS pick an edge from where the query comes from: your subnet, if the resolver forwards EDNS Client
          Subnet and the CDN honors it, otherwise the resolver's own location.
        </p>
        <p>
          Each resolver looked up the same {hostCount} hostnames twice, several for each CDN. Recursa also asked
          each CDN's own nameserver directly from this computer: that is <b>your address</b> row, the edge the CDN picks for you when
          nothing stands in between.
        </p>
          <p>
            Times are the best of three TCP handshakes, one network round trip each. For each hostname, the penalty is the extra
            round trip compared with the best edge any resolver (or the CDN itself) found for that same name, so a name that is far
            from everyone costs no one. A CDN's penalty is the median over its hostnames, and the resolver's is the mean over the
            CDNs that every ranked resolver has. Anycast controls are shown, not scored. Resolvers within the run's noise share a tier.
          </p>
        <p>CDN maps shift over minutes, so a single run is a snapshot: the same resolver can get a different edge an hour later.</p>
      </Info>
    </div>
    <div class="findings">
      {#if best && worst && best !== worst}
        <p>
          <b>{name(best.id)}</b> gets you the nearest edges. <b>{name(worst.id)}</b> costs {ms(worst.r.penalty_ms)} per new
          connection at the median{worst.r.worst
            ? `, up to ${ms(worst.r.worst[1])} on ${cdn.cdns.find((c) => c.id === worst.r.worst![0])?.name}`
            : ''}.
        </p>
      {/if}
      {#if slowMaps.length}
        <p>
          Asked directly, {#each slowMaps as x, i}{i ? (i === slowMaps.length - 1 ? ' and ' : ', ') : ''}<b>{x.cdn}</b> picked edges for
            your own address {span(x)}{/each} slower than the best a resolver got.{#if outcome.total}{' '}Of {outcome.total} lookups on
            those hosts by resolvers that send your subnet, {outcome.same} got that same pick (<span class="mine">●</span>){#if outcome.nearer},
              {outcome.nearer} a nearer edge{/if}{#if outcome.farther}, {outcome.farther} a farther one{/if}.{/if}
        </p>
      {/if}
      {#each ignored.slice(0, 3) as x}
        <p><b>{name(x.id)}</b> sends your subnet, yet got farther {list(x.cdns)} edges than your own address did.</p>
      {/each}
    </div>
    {#if nets.length}
      <div class="nets">
        <span class="nets-h">Edge networks</span>
        {#each nets as x (x.key)}
          <span class="net" class:you={x.you}>{x.name} <span class="num nn">{x.n}</span>{#if x.you}<span class="nyou">your ISP</span>{/if}</span>
        {/each}
      </div>
    {/if}
  </header>

  {#if mobile}
    <!-- Phone: one row per resolver, tapped open for its edge per CDN. The page scrolls; nothing scrolls sideways. -->
    <p class="mlegend muted">
      <span><span class="mine">●</span> same edge as your address</span>
      <span><span class="mine">○</span> about as near</span>
      <span><span class="onnet">ISP</span> edge inside your ISP</span>
      <span>+ms: extra time over the best edge</span>
    </p>
    <div class="mlist">
      {#if hasRef}
        <details class="mrow ref">
          <summary>
            <span class="mname"><b>Your address</b> <span class="mono">{clientIp ?? 'this device'}</span></span>
            <span class="mpen muted">asked directly: each CDN's own pick</span>
          </summary>
          <ul class="medges">
            {#each tableCols as c}
              {@const y = you(c)}
              <li style:--shade={shade(y?.penalty_ms)}>
                <span class="mcdn"><CdnLogo id={c.id} name={short(c)} /><span>{short(c)}</span></span>
                {#if y?.rtt_ms != null}
                  <span class="rt num">{ms(y.rtt_ms)}</span>
                  <span class="dp num">{(y.penalty_ms ?? 0) >= 0.5 ? `+${ms(y.penalty_ms)}` : ''}</span>
                {:else}
                  <span class="muted wide">{y?.error ? 'no answer' : '—'}</span>
                {/if}
              </li>
            {/each}
          </ul>
        </details>
      {/if}
      {#each rows as { id, r, res } (id)}
        {@const e = ecs(id)}
        <details class="mrow" data-resolver={id} class:sel={app.inspect === id}>
          <summary>
            <span class="mname"><b>{res?.provider}</b> <span>{res?.variant}</span></span>
            <span class="chip {e.tone}">{e.text}</span>
            <span class="mpen">
              {#if r.penalty_ms != null}
                <span class="bar" aria-hidden="true"><span style:width="{Math.max(3, (r.penalty_ms / maxPen) * 100)}%"></span></span>
                <span class="num pv">{r.penalty_ms < 0.05 ? 'best' : `+${ms(r.penalty_ms)}`}</span>
                {#if r.tier != null}<span class="muted">tier {r.tier}</span>{/if}
              {:else}
                <span class="muted">insufficient data</span>
              {/if}
            </span>
            {#if r.coverage && r.coverage.cdns_used < r.coverage.cdns_total}
              <span class="mcov muted">{r.coverage.cdns_used}/{r.coverage.cdns_total} CDNs · {r.coverage.hosts_used}/{r.coverage.hosts_total} hosts</span>
            {/if}
          </summary>
          <p class="mecs muted">{e.title}</p>
          <ul class="medges">
            {#each tableCols as c}
              {@const m = r.mappings[c.id]}
              {@const k = cell(c, m)}
              {@const mine = m?.as_you}
              <li style:--shade={shade(k.level ? 0 : k.pen)}>
                <span class="mcdn"
                  ><CdnLogo id={c.id} name={short(c)} /><span>{short(c)}</span>{#if c.role === 'control'}<span class="muted any">anycast</span>{/if}</span
                >
                {#if m?.rtt_ms != null}
                  <span class="rt num"
                    >{#if edgeIps(m).some((ip) => cdn.edges[ip]?.on_net)}<span class="onnet">ISP</span>{/if}{#if mine === 'same'}<span
                        class="mine"
                        aria-label="same as your address">●</span
                      >{:else if mine === 'near'}<span class="mine" aria-label="near your address's edges">○</span>{/if}{ms(m.rtt_ms)}</span
                  >
                  <span class="dp num">{k.pen != null && k.pen >= 0.5 && !k.level ? `+${ms(k.pen)}` : ''}</span>
                {:else}
                  <span class="muted wide">{m?.error ? 'error' : '—'}</span>
                {/if}
              </li>
            {/each}
          </ul>
          <button class="btn mopen" onclick={() => onopen(id)}>Resolver details</button>
        </details>
      {/each}
    </div>
  {:else}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex (scrollable region must be keyboard-reachable) -->
  <div class="wrap panel" role="region" aria-label="CDN edge round trips by resolver" tabindex="0">
    <table class="sticky-head">
      <thead>
        <tr>
          <th class="first" scope="col">Resolver</th>
          <th scope="col" class="ecs-h" title="The client subnet the resolver forwards (EDNS Client Subnet)">ECS</th>
          <th scope="col" class="pen-h">Penalty</th>
          {#each tableCols as c}
            {@const any = c.role === 'control'}
            <th scope="col" class="cdn-h" title={headTitle(c)}>
              <CdnLogo id={c.id} name={short(c)} />
              <span class="cn">{short(c)}</span>
              <span class="ch num">{any ? 'anycast' : `best ${ms(c.best_ms, false)}`}</span>
            </th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#if hasRef}
          <tr class="ref">
            <th class="first" scope="row">
              <span class="refname">Your address</span>
              <span class="refsub mono">{clientIp ?? 'this computer'}</span>
            </th>
            <td class="ecs"><span class="muted">—</span></td>
            <td class="pen"><span class="muted small">asked directly: the CDN's own pick</span></td>
            {#each tableCols as c}
              {@const y = you(c)}
              <td class="edge num" style:--shade={shade(y?.penalty_ms)} title={refTitle(c, y)}>
                {#if y?.rtt_ms != null}
                  <span class="rt">{ms(y.rtt_ms, false)}</span>
                  {#if (y.penalty_ms ?? 0) >= 0.5}<span class="dp">+{ms(y.penalty_ms, false)}</span>{/if}
                {:else}
                  <span class="muted">{y?.error ? 'no answer' : '—'}</span>
                {/if}
              </td>
            {/each}
          </tr>
        {/if}
        {#each rows as { id, r, res } (id)}
          {@const e = ecs(id)}
          <tr data-resolver={id} class:sel={app.inspect === id} data-vtn style:--vtn="cdnr-{run.resolvers.findIndex((x) => x.id === id)}">
            <th class="first" scope="row">
              <button onclick={() => onopen(id)}><b>{res?.provider}</b> <span>{res?.variant}</span></button>
            </th>
            <td class="ecs"><span class="chip {e.tone}" title={e.title}>{e.text}</span></td>
            <td class="pen">
              {#if r.penalty_ms != null}
                <span class="bar" aria-hidden="true"><span style:width="{Math.max(3, (r.penalty_ms / maxPen) * 100)}%"></span></span>
                <span class="num pv">{r.penalty_ms < 0.05 ? 'best' : `+${ms(r.penalty_ms, false)}`}</span>
                {#if r.tier != null}<span class="muted small">tier {r.tier}</span>{/if}
              {:else}
                <span class="muted">insufficient data</span>
              {/if}
              {#if r.coverage}
                <span class="cov muted"
                  >{r.coverage.cdns_used}/{r.coverage.cdns_total} CDNs · {r.coverage.hosts_used}/{r.coverage.hosts_total} hosts</span
                >
              {/if}
            </td>
            {#each tableCols as c}
              {@const m = r.mappings[c.id]}
              {@const k = cell(c, m)}
              {@const mine = m?.as_you}
              <td class="edge num" style:--shade={shade(k.level ? 0 : k.pen)} title={cellTitle(c, m, name(id))}>
                {#if m?.rtt_ms != null}
                  <span class="rt"
                    >{#if mine === 'same'}<span class="mine" aria-label="same as your address">●</span
                      >{:else if mine === 'near'}<span class="mine" aria-label="near your address's edges">○</span>{/if}{ms(m.rtt_ms, false)}</span
                  >
                  {#if k.pen != null && k.pen >= 0.5 && !k.level}<span class="dp">+{ms(k.pen, false)}</span>{/if}
                  {#if edgeIps(m).some((ip) => cdn.edges[ip]?.on_net)}<span class="onnet" title="Edge inside your ISP">ISP</span>{/if}
                {:else}
                  <span class="muted">{m?.error ? 'error' : '—'}</span>
                {/if}
              </td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {/if}

  <div class="foot">
    {#if hasRef}
      <p>
        <span class="mine">●</span> The resolver got the same edge (or one as near) that the CDN picks for your own address.
        <span class="mine">○</span> It got edges in the same cluster, about as near.
      </p>
    {/if}
    {#if uniform.length}
      <p>
          {list(uniform.map(short))} {uniform.length === 1 ? 'is' : 'are'} anycast, so resolver choice should not matter there. {uniform.length ===
          1
            ? 'It is'
            : 'They are'} shown as a check and {uniform.length === 1 ? 'isn’t' : 'aren’t'} counted in the penalty.
      </p>
    {/if}
    {#if left.length}
      <p>
        {list(left.map((r) => label(r.provider, r.variant)))} answered nothing earlier in the run, so {left.length === 1 ? 'it was' : 'they were'} left out of the
        map.
      </p>
    {/if}
    <p>Differences under {ms(2 * cdn.noise_ms, false)} ms are noise in this run.</p>
    <p>
      {#if onNet.length}
        {onNet.length} of {edges.length} edges sit inside your ISP{clientNet?.holder ? ` (${holder(clientNet.holder)})` : ''}.
      {:else}
        None of the {edges.length} edges sit inside your ISP's network.
      {/if}
      {mobile ? 'Tap a resolver for its edge on each CDN, in milliseconds.' : 'Hover a cell for its edges, their networks and round trips.'}
    </p>
  </div>
</section>

<style>
  /* The view fills the pane and the table scrolls inside it, under a fixed header. */
  .cdn {
    display: flex;
    flex-direction: column;
    gap: 12px;
    height: 100%;
  }
  .head {
    display: grid;
    gap: 6px;
  }
  .titles {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  h2 {
    font-size: var(--t-lg);
  }
  .findings {
    display: grid;
    gap: 4px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    max-width: 110ch;
  }
  .findings b {
    color: var(--ink);
    font-weight: 600;
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
    padding: 6px 10px;
    border-bottom: 1px solid var(--rule);
    text-align: left;
    vertical-align: middle;
  }
  /* Body rows breathe a little more than the header. */
  tbody th,
  tbody td {
    padding-block: 8px;
  }
  thead th {
    background: var(--surface-2);
    color: var(--ink-3);
    font-weight: 600;
    vertical-align: bottom;
    user-select: none;
    -webkit-user-select: none;
  }
  tbody tr:last-child > * {
    border-bottom: 0;
  }
  tbody tr[data-resolver]:hover > * {
    background-color: var(--surface-2);
  }
  tbody tr.sel > * {
    background: var(--sel);
  }
  .first {
    position: sticky;
    left: 0;
    z-index: 1;
    background: var(--surface);
    border-right: 1px solid var(--rule);
    min-width: 140px;
    max-width: 200px;
  }
  thead .first {
    background: var(--surface-2);
    z-index: 3;
  }
  .first button {
    display: grid;
    padding: 0;
    border: 0;
    background: none;
    text-align: left;
    font-size: var(--t-sm);
    line-height: 1.25;
  }
  .first button span {
    color: var(--ink-3);
    font-size: var(--t-xs);
  }
  .ref > * {
    background: var(--surface-2);
    border-bottom: 1px solid var(--rule-2);
  }
  .ref .first {
    background: var(--surface-2);
  }
  .refname {
    display: block;
    font-size: var(--t-sm);
    font-weight: 600;
  }
  .refsub {
    display: block;
    color: var(--ink-3);
    font-size: var(--t-xs);
    white-space: nowrap;
  }
  .small {
    font-size: var(--t-xs);
  }
  .cov {
    display: block;
    font-size: var(--t-xs);
  }
  .ecs-h,
  .pen-h {
    white-space: nowrap;
  }
  .ecs {
    white-space: nowrap;
  }
  .chip {
    display: inline-block;
    padding: 0 7px;
    line-height: 18px;
    border-radius: 9px;
    font-weight: 600;
    font-size: var(--t-2xs);
    border: 1px solid var(--rule-2);
    font-variation-settings: 'MONO' 1, 'CASL' 0;
  }
  .chip.you {
    border-color: color-mix(in oklab, var(--good) 55%, var(--rule));
  }
  .chip.other {
    border-color: color-mix(in oklab, var(--warning) 60%, var(--rule));
  }
  .chip.none {
    border-style: dashed;
    color: var(--ink-2);
    font-weight: 500;
  }
  .pen {
    white-space: nowrap;
  }
  .bar {
    display: inline-block;
    width: 48px;
    height: 6px;
    border-radius: 3px;
    background: var(--surface-3);
    vertical-align: middle;
    margin-right: 6px;
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: 3px;
    background: var(--ink-2);
  }
  .pv {
    font-weight: 620;
  }
  /* Nine CDN columns: slightly tighter sides keep the table inside a 1280 px window. */
  .cdn-h,
  .edge {
    padding-inline: 8px;
  }
  .cdn-h {
    text-align: right;
    min-width: 64px;
  }
  /* The table scrolls inside the panel, and macOS overlay scrollbars draw over the
     last column's right edge; the extra padding keeps the numbers clear of the bar. */
  .cdn-h:last-child,
  .edge:last-child {
    padding-right: 22px;
  }
  /* The mark sits above the name, on the same right edge. */
  .cdn-h :global(.logo) {
    margin: 0 0 3px auto;
  }
  .cn {
    display: block;
    color: var(--ink-2);
    white-space: nowrap;
  }
  .ch {
    display: block;
    font-weight: 400;
    font-size: var(--t-xs);
    white-space: nowrap;
  }
  .edge {
    text-align: right;
    white-space: nowrap;
    background-image: linear-gradient(color-mix(in oklch, var(--ink) calc(var(--shade) * 100%), transparent), color-mix(in oklch, var(--ink) calc(var(--shade) * 100%), transparent));
  }
  .rt {
    color: var(--ink);
  }
  .mine {
    margin-right: 4px;
    font-size: 8px;
    vertical-align: 1px;
    color: var(--accent-text);
  }
  .dp {
    display: block;
    color: var(--ink-2);
    font-size: var(--t-xs);
  }
  .onnet {
    margin-left: 4px;
    font-size: var(--t-xs);
    padding: 0 4px;
    border-radius: 4px;
    border: 1px solid var(--good);
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
    background: var(--surface-2);
    border: 1px solid var(--rule);
    white-space: nowrap;
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
  .foot {
    display: grid;
    gap: 4px;
    font-size: var(--t-xs);
    color: var(--ink-3);
    max-width: 110ch;
  }
  /* Phone: the page scrolls, and each resolver opens in place. */
  :global(html.mobile) .cdn {
    height: auto;
    gap: 16px;
  }
  :global(html.mobile) .foot {
    font-size: var(--t-sm);
  }
  .mlist {
    display: grid;
    border-top: 1px solid var(--rule);
  }
  .mrow {
    border-bottom: 1px solid var(--rule);
  }
  .mrow.sel {
    background: var(--sel);
  }
  .mrow.ref {
    background: var(--surface-2);
  }
  .mrow summary {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 6px 10px;
    min-height: 44px;
    padding: 12px 2px;
    list-style: none;
    cursor: pointer;
  }
  .mrow summary::-webkit-details-marker {
    display: none;
  }
  .mrow summary:active {
    background: var(--surface-2);
  }
  .mname {
    min-width: 0;
    font-size: var(--t-base);
    line-height: 1.25;
  }
  .mname b {
    font-weight: 620;
  }
  .mname span {
    color: var(--ink-3);
    font-size: var(--t-sm);
  }
  .mpen,
  .mcov {
    grid-column: 1 / -1;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--t-sm);
  }
  .mpen .bar {
    flex: 1;
    max-width: 160px;
    margin: 0;
  }
  .mcov {
    font-size: var(--t-xs);
  }
  .mecs {
    padding: 0 2px 8px;
    font-size: var(--t-xs);
  }
  .medges {
    display: grid;
    margin: 0 0 12px;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--rule);
  }
  .medges li {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: 10px;
    min-height: 40px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--rule);
    font-size: var(--t-sm);
    background-image: linear-gradient(color-mix(in oklch, var(--ink) calc(var(--shade) * 100%), transparent), color-mix(in oklch, var(--ink) calc(var(--shade) * 100%), transparent));
  }
  .mcdn {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    color: var(--ink-2);
  }
  .mcdn .any {
    font-size: var(--t-xs);
  }
  .medges .rt {
    text-align: right;
    font-weight: 560;
  }
  .medges .dp {
    display: inline;
    text-align: right;
    font-size: var(--t-xs);
  }
  .medges .wide {
    grid-column: 2 / -1;
    text-align: right;
  }
  .medges .onnet {
    margin: 0 6px 0 0;
  }
  .mlegend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    margin: 0 0 8px;
    font-size: var(--t-sm);
  }
  .mopen {
    width: 100%;
    min-height: 44px;
    margin-bottom: 12px;
  }
</style>
