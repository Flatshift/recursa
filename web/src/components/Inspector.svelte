<script lang="ts">
  import type { Resolver, RunState, TestKind } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { TESTS } from '../lib/verdict';
  import { shortcut } from '../lib/keys';
  import { CAPS, PUBLISHED_ONLY, type CapGroup } from '../lib/caps';
  import { capCell, cdnCell, roaCell, rovCell } from '../lib/cells';
  import { TEST_LABEL, TRANSPORT_LABEL, day, holder, ms, pct, when } from '../lib/format';
  import { status, stateLabel, stateGlyph, localNote } from '../lib/status.svelte';
  import Cell from './Cell.svelte';
  import Ecdf from './Ecdf.svelte';
  import Glyph from './Glyph.svelte';
  import Icon from './Icon.svelte';
  import Provenance from './Provenance.svelte';
  import { logDomain } from '../lib/scale';
  import { app } from '../lib/store.svelte';
  import { cellPenalty, edgeIps, youOf } from '../lib/cdn';
  import type { HostStatus } from '../lib/types';

  const STATUS: Record<HostStatus, string> = {
    ok: 'ok',
    failed: 'no answer',
    blocked: 'blocked',
    unreachable: 'unreachable',
    off_cdn: 'not this CDN',
  };

  let {
    resolver,
    run = null,
    rows = [],
    overlay = false,
    onclose,
  }: { resolver: Resolver; run?: RunState | null; rows?: Row[]; overlay?: boolean; onclose: () => void } = $props();

  // The run's row in history, for its saved mark.
  const row = $derived(run ? (app.boot?.runs.find((r) => r.id === run.id) ?? null) : null);

  // The inspector follows the selection; it loads the status feed it shows.
  $effect(() => {
    status.load();
  });

  // Drag the left edge to resize; the width is remembered.
  function startResize(e: PointerEvent) {
    e.preventDefault();
    const x0 = e.clientX;
    const w0 = app.inspectorWidth;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => app.setInspectorWidth(w0 + (x0 - ev.clientX));
    // Capture ends on pointer up and also when the system cancels the drag.
    const done = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('lostpointercapture', done);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('lostpointercapture', done);
  }
  function resizeKey(e: KeyboardEvent) {
    if (e.key === 'ArrowLeft') app.setInspectorWidth(app.inspectorWidth + 20);
    else if (e.key === 'ArrowRight') app.setInspectorWidth(app.inspectorWidth - 20);
  }

  const id = $derived(resolver.id);
  const mine = $derived(rows.filter((r) => r.target.resolver_id === id && r.measured));
  const probe = $derived(run?.probes[id] ?? null);
  const intel = $derived(run?.intel[id] ?? null);
  const path = $derived(run?.paths?.[id] ?? null);
  const cdn = $derived(run?.cdn?.resolvers[id] ?? null);
  const feed = $derived(status.feedFor(resolver.status_feed));
  const incidents = $derived(status.incidentsFor(id));

  const domain = $derived.by((): [number, number] => {
    let lo = Infinity;
    let hi = 0;
    for (const r of mine)
      for (const k of TESTS) {
        const d = r.dist[k];
        if (d.min != null) lo = Math.min(lo, d.min);
        if (d.max != null) hi = Math.max(hi, d.max);
      }
    return logDomain(lo, hi);
  });

  const groups: CapGroup[] = ['Security', 'Privacy', 'Blocking', 'Protocol', 'Transport', 'Identity'];
  let openEvidence = $state<Record<string, boolean>>({});
  let openFilter = $state<Record<string, boolean>>({});
</script>

<aside class="inspector" class:overlay style:width="{app.inspectorWidth}px" aria-labelledby="sheet-title">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (a separator is focusable to resize with the arrow keys) -->
  <div
    class="grip"
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize inspector"
    aria-valuenow={app.inspectorWidth}
    tabindex="0"
    onpointerdown={startResize}
    onkeydown={resizeKey}
  ></div>
  <div class="inner scroll">
    <header class="head">
      <div>
        <p class="kicker">
          {resolver.group === 'system' ? 'Your network' : resolver.group === 'custom' ? 'Custom resolver' : resolver.group === 'legacy' ? 'Legacy resolver' : 'Public resolver'}{resolver.published.jurisdiction ? ` · ${resolver.published.jurisdiction}` : ''}
        </p>
        <h2 id="sheet-title">{resolver.provider} <span>{resolver.variant}</span></h2>
        <p class="addrs mono">
          {[...resolver.ipv4, ...resolver.ipv6].join(' · ')}
        </p>
        {#if resolver.dot || resolver.doh}
          <p class="addrs mono">
            {#if resolver.dot}DoT {resolver.dot}{/if}{#if resolver.dot && resolver.doh}&nbsp;·&nbsp;{/if}{#if resolver.doh}DoH {resolver.doh.replace('https://', '')}{/if}
          </p>
        {/if}
      </div>
      <button class="tb-icon close" aria-label="Close inspector" title="Close inspector ({shortcut('alt+cmd+I')})" onclick={onclose}><Icon name="close" size={14} /></button>
    </header>

    {#if resolver.notes}<p class="note">{resolver.notes}</p>{/if}

    <p class="dive">
      <button class="btn sm" onclick={() => app.navigate(`/deep-dive/${id}`)} title="Sends 18 ordinary queries to this resolver"
        ><Icon name="pulse" size={13} />Deep dive…</button
      >
      {#if run && run.status !== 'running' && row}
        <button
          class="btn sm"
          class:on={row.saved}
          aria-pressed={row.saved}
          title={row.saved ? 'Saved: kept when runs are cleared. Click to unsave.' : 'Keep this run when runs are cleared or cleaned up'}
          onclick={() => app.setSaved(run!.id, !row!.saved)}><Icon name="bookmark" size={13} />{row.saved ? 'Saved' : 'Save run'}</button
        >
      {/if}
    </p>

    {#if run}
      <section>
        <h3>Where it answers from</h3>
        <dl class="facts">
          {#if probe?.pop || probe?.nsid}
            <div>
              <!-- A local forwarder relays the identity query, so the site is its upstream's. -->
              <dt>{intel?.service?.private ? 'Upstream site' : 'Anycast site'}</dt>
              <dd class="mono">{probe.pop ?? probe.nsid}{#if probe.pop && probe.nsid && probe.nsid !== probe.pop}<span class="muted"> · NSID {probe.nsid}</span>{/if}</dd>
            </div>
          {/if}
          {#if path?.rtt_ms != null}
            <div>
              <dt>Network round trip</dt>
              <dd><span class="num">{ms(path.rtt_ms)}</span> <span class="muted">TCP handshake to port {path.port}, best of {path.samples.length}</span></dd>
            </div>
          {/if}
          {#if intel?.egress}
            <div>
              <dt title="The address Google's authoritative servers saw this resolver's lookups come from">Queries authoritative servers from</dt>
              <dd>
                <span class="mono">{intel.egress.ip}</span>
                <span class="muted">· AS{intel.egress.asn ?? '?'} {holder(intel.egress.holder)}</span>
              </dd>
            </div>
          {:else if probe?.egress.length}
            <div><dt>Queries authoritative servers from</dt><dd class="mono">{probe.egress.join(', ')}</dd></div>
          {/if}
          {#if intel?.service && !intel.service.private}
            <div>
              <dt>Service network</dt>
              <dd>
                <span class="mono">{intel.service.prefix ?? intel.service.ip}</span>
                <span class="muted">· AS{intel.service.asn ?? '?'} {holder(intel.service.holder)}</span>
              </dd>
            </div>
            <div>
              <dt>RPKI route origin</dt>
              <dd>
                <Cell c={roaCell(run, id)} />
                {#if intel.service.rpki?.status === 'not_checked'}
                  <span class="muted">{intel.service.rpki.reason}</span>
                {:else if intel.service.rpki && intel.service.prefix}
                  <!-- Show the object itself, so the verdict can be checked rather than trusted. -->
                  <span class="roa">
                    {#each intel.service.rpki.roas ?? [] as roa}
                      <span class="mono">ROA AS{roa.origin} · {roa.prefix} · max /{roa.max_length}</span>
                    {:else}
                      <span>No ROA covers {intel.service.prefix} from AS{intel.service.asn ?? '?'}</span>
                    {/each}
                    <span class="muted"
                      >{intel.service.prefix} as announced by AS{intel.service.asn ?? '?'}{#if intel.service.rpki.vrp_time != null}, checked on this computer ({intel.service.rpki
                          .validator ?? 'RPKI validator'}), data from {when(intel.service.rpki.vrp_time)}{:else}; route-origin result from an earlier version{/if}</span
                    >
                  </span>
                {/if}
              </dd>
            </div>
            <div><dt>Operator drops invalid routes</dt><dd><Cell c={rovCell(run, id)} /></dd></div>
          {/if}
          {#if cdn}
            <div>
              <dt>CDN mapping</dt>
              <dd>
                <Cell c={cdnCell(run, id)} />
                <span class="muted"
                  >· ECS to Google {cdn.ecs ?? 'none'}{#if (cdn.ecs_akamai ?? probe?.ecs_akamai) !== undefined}, to Akamai {cdn.ecs_akamai ??
                      probe?.ecs_akamai ??
                      'none'}{/if}</span
                >
              </dd>
            </div>
          {/if}
        </dl>
        {#if run.intel[id]?.service?.rpki || run.intel[id]?.service?.rov}
          <p class="why">
            A valid ROA means networks that check RPKI will reject anyone else announcing this resolver's addresses. Operator ROV
            protects the resolver's own outbound routes. Both limit hijacks like the one that cut off 1.1.1.1 for 300 networks in
            June 2024.
          </p>
        {/if}
      </section>

      {#if mine.length}
        <section>
          <h3>Latency</h3>
          {#each mine as r (r.target.idx)}
            <div class="lat">
              <div class="lat-head">
                <span class="tp">{TRANSPORT_LABEL[r.target.transport]}{r.target.family === 'v6' ? ' · IPv6' : ''}</span>
                <span class="mono muted">{r.target.endpoint}</span>
                {#if r.intercepted}<span class="warnchip">answered by a device on your network</span>{/if}
              </div>
              {#if r.target.bootstrap}<p class="muted small">Address found without the system resolver: {r.target.bootstrap}</p>{/if}
              {#if r.target.hops != null}
                <p class="muted small">
                  Answers crossed {r.target.hops} router {r.target.hops === 1 ? 'hop' : 'hops'} (from their IP TTL){r.target.hops <= 1 && r.intercepted
                    ? ': too close to be the real service.'
                    : '.'}
                </p>
              {/if}
              <Ecdf dists={r.dist} {domain} />
              <table class="mini">
                <thead><tr><th></th><th>p50</th><th>p95</th><th>min</th><th>answered</th></tr></thead>
                <tbody>
                  {#each run.config.tests as k}
                    {@const d = r.dist[k as TestKind]}
                    <tr>
                      <th><i class="key {k}"></i>{TEST_LABEL[k as TestKind]}</th>
                      <td class="num">{ms(d.p50)}</td>
                      <td class="num">{ms(d.p95)}</td>
                      <td class="num">{ms(d.min)}</td>
                      <td class="num">{d.sent ? pct(d.n / d.sent) : '—'}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
              {#if r.target.setup_ms != null}<p class="muted small">Connection setup {ms(r.target.setup_ms)}, not counted in query times.</p>{/if}
            </div>
          {/each}
        </section>
      {/if}
    {/if}

    <section>
      <h3>Capabilities</h3>
      {#if !probe}<p class="muted small">No probes in this view, so these are the operator's published claims.</p>{/if}
      {#each groups as g}
        {@const caps = CAPS.filter((c) => c.group === g)}
        <h4>{g}</h4>
        <ul class="caps">
          {#each caps as c}
            {@const cell = capCell(run, resolver, c.key)}
            {#if !(c.group === 'Identity' && cell.prov === 'unknown')}
              <li>
                <div class="cap-top">
                  <span class="cap-l">{c.label}{#if c.ref}<span class="ref">{c.ref}</span>{/if}</span>
                  <Cell c={cell} />
                </div>
                <p class="cap-x">{c.explain}</p>
                {#if cell.evidence && cell.prov === 'verified'}
                  <button class="ev-toggle" aria-expanded={!!openEvidence[c.key]} onclick={() => (openEvidence[c.key] = !openEvidence[c.key])}>
                    {openEvidence[c.key] ? 'Hide evidence' : 'Evidence'}
                  </button>
                  {#if openEvidence[c.key]}<pre class="ev mono">{cell.evidence}</pre>{/if}
                {/if}
              </li>
            {/if}
          {/each}
        </ul>
      {/each}
      <h4>Published only</h4>
      <ul class="caps">
        {#each PUBLISHED_ONLY as p}
          {@const v = p.get(resolver.published)}
          <li>
            <div class="cap-top">
              <span class="cap-l">{p.label}{#if p.ref}<span class="ref">{p.ref}</span>{/if}</span>
              <span class="pub">
                <Glyph status={v == null ? 'unknown' : v ? 'pass' : 'info'} size={13} />
                {v == null ? 'Not documented' : v ? 'Offered' : 'Not offered'}
                <Provenance kind={v == null ? 'unknown' : 'published'} />
              </span>
            </div>
            <p class="cap-x">{p.explain}</p>
          </li>
        {/each}
      </ul>
    </section>

    {#if probe && Object.keys(probe.filtering).length}
      <section>
        <h3>Blocking evidence</h3>
        {#if probe.block_style}<p class="small">Blocks by answering <b class="mono">{probe.block_style}</b>.</p>{/if}
        {#each Object.entries(probe.filtering) as [cat, f]}
          <div class="fcat">
            <button class="fhead" aria-expanded={!!openFilter[cat]} onclick={() => (openFilter[cat] = !openFilter[cat])}>
              <Icon name={openFilter[cat] ? 'chevron-down' : 'chevron-right'} size={13} />
              <span class="cap-l">{cat[0].toUpperCase() + cat.slice(1)}</span>
              <span class="num">{f.blocked}/{f.tested} blocked</span>
              <span class="muted small">{f.source}</span>
            </button>
            {#if openFilter[cat]}
              <ul class="fs">
                {#each f.samples as s}
                  <li>
                    <Glyph status={s.blocked == null ? 'unknown' : s.blocked ? 'pass' : 'info'} size={12} />
                    <span class="mono dom">{s.domain}</span>
                    <span class="mono muted how">{s.how}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/each}
      </section>
    {/if}

    {#if cdn && run?.cdn}
      <section>
        <h3>CDN edges it chose for you</h3>
        <table class="mini wide">
          <thead><tr><th>CDN</th><th>Edge</th><th>Round trip</th><th>Your address</th></tr></thead>
          <tbody>
            {#each run.cdn.cdns as c}
              {@const m = cdn.mappings[c.id]}
              {@const ips = edgeIps(m)}
              {@const pen = cellPenalty(c, m)}
              {@const mine = m?.as_you}
              {@const y = youOf(c)}
              <tr>
                <th>{c.name.replace(' (anycast control)', '')}{#if c.role === 'control'} <span class="muted">anycast</span>{/if}</th>
                <td class="mono small">{ips.join(', ') || m?.error || '—'}<br /><span class="muted">{holder(run.cdn.edges[ips[0] ?? '']?.holder)}</span></td>
                <td class="num">
                  {ms(m?.rtt_ms)}{#if pen != null && pen >= 0.5}<br /><span class="muted">+{ms(pen, false)} vs best</span>{/if}
                </td>
                <td class="num">
                  {#if y?.rtt_ms != null}
                    {ms(y.rtt_ms)}<br /><span class="muted">{mine === 'same' ? 'same edges' : mine === 'near' ? 'near edges' : mine === 'different' ? 'other edges' : ''}</span>
                  {:else}<span class="muted">—</span>{/if}
                </td>
              </tr>
              {#each Object.entries(m?.hosts ?? {}) as [host, h]}
                <tr class="host">
                  <th class="small">{host}</th>
                  <td class="mono small">{h.status === 'ok' ? h.ips.join(', ') : `${STATUS[h.status]}${h.error ? `: ${h.error}` : ''}`}</td>
                  <td class="num">
                    {ms(h.rtt_ms)}{#if h.penalty_ms != null}<br /><span class="muted">{h.same ? 'within noise' : `+${ms(h.penalty_ms, false)} vs best`}</span>{/if}
                  </td>
                  <td class="num"><span class="muted">{h.as_you === 'same' ? 'same edges' : h.as_you === 'near' ? 'near edges' : h.as_you === 'different' ? 'other edges' : ''}</span></td>
                </tr>
              {/each}
            {/each}
          </tbody>
        </table>
        <p class="muted small">
          “Your address” is the edge each CDN picks when asked directly from this computer, with no resolver in between.
        </p>
      </section>
    {/if}

    <section>
      <h3>Reliability</h3>
      {#if feed}
        <p class="feed">
          <Glyph status={stateGlyph(feed.state)} />
          <b>{stateLabel(feed)}</b>
          <span class="muted">· {feed.name} status page{feed.component ? `, ${feed.component}` : ''}</span>
          <a href={feed.page} target="_blank" rel="noreferrer">Open <Icon name="external" size={12} /></a>
        </p>
        {#if feed.locality}<p class="small">{localNote(feed)}</p>
        {:else if feed.affected?.length}<p class="small">Affected: {feed.affected.join(', ')}</p>{/if}
        {#each feed.incidents.slice(0, 3) as i}
          <p class="inc small"><span class="muted">{day(i.started)}</span> {i.title} <span class="muted">· {i.status}</span></p>
        {/each}
      {:else if resolver.group === 'public'}
        <p class="small muted">{resolver.provider} publishes no machine-readable status feed. Turn on monitoring to build your own record from this network.</p>
      {/if}
      {#if incidents.length}
        <h4>Notable incidents</h4>
        <ul class="incs">
          {#each incidents as i}
            <li>
              <span class="mono muted">{i.date}</span>
              <span>
                <b>{i.title}</b>
                {#if i.confidence === 'unconfirmed'}<span class="warnchip">unconfirmed</span>{/if}
                <span class="small ink-2">{i.summary}</span>
                <a class="small" href={i.source} target="_blank" rel="noreferrer">Source <Icon name="external" size={11} /></a>
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    {#if resolver.limits}
      <section>
        <h3>Usage limits</h3>
        <p class="small">{resolver.limits.text} <a href={resolver.limits.source} target="_blank" rel="noreferrer">Source <Icon name="external" size={11} /></a></p>
        {#if run?.rates?.[id] != null}
          <p class="small">
            This run sent it at most <b>{run.rates[id].toFixed(0)}</b> queries per second{resolver.limits.qps ? `, against the ${resolver.limits.qps.toLocaleString()} it publishes${resolver.limits.per ? ` for ${resolver.limits.per}` : ''}` : ''}.
          </p>
        {/if}
      </section>
    {/if}

    <section>
      <h3>Policy</h3>
      <dl class="facts">
        <div><dt>Logging</dt><dd>{resolver.published.logging ?? 'Not summarised; see the operator’s policy.'} <Provenance kind={resolver.published.logging ? 'published' : 'unknown'} /></dd></div>
        <div><dt>Jurisdiction</dt><dd>{resolver.published.jurisdiction ?? '—'}</dd></div>
        <div>
          <dt>Links</dt>
          <dd class="links">
            {#if resolver.links.home}<a href={resolver.links.home} target="_blank" rel="noreferrer">Service page <Icon name="external" size={11} /></a>{/if}
            {#if resolver.links.privacy}<a href={resolver.links.privacy} target="_blank" rel="noreferrer">Privacy policy <Icon name="external" size={11} /></a>{/if}
            {#if !resolver.links.home && !resolver.links.privacy}—{/if}
          </dd>
        </div>
      </dl>
    </section>
  </div>
</aside>

<style>
  .roa {
    display: grid;
    gap: 2px;
    margin-top: 4px;
    font-size: var(--t-xs);
  }
  .inspector {
    position: relative;
    flex: none;
    max-width: 70%;
    min-height: 0;
    height: 100%;
    background: var(--surface);
    border-left: 1px solid var(--rule);
    color: var(--ink);
    display: grid;
  }
  /* A narrow window floats the inspector over the content instead of squeezing it. */
  .inspector.overlay {
    position: absolute;
    inset: 0 0 0 auto;
    z-index: var(--z-sheet);
    max-width: calc(100% - 24px);
    box-shadow: var(--shadow-sheet);
    animation: slide var(--dur) var(--ease-out);
  }
  @keyframes slide {
    from {
      transform: translateX(16px);
      opacity: 0;
    }
  }
  .grip {
    position: absolute;
    inset: 0 auto 0 -3px;
    width: 6px;
    cursor: col-resize;
    z-index: 2;
  }
  .grip:hover,
  .grip:focus-visible {
    background: color-mix(in oklch, var(--accent) 35%, transparent);
    outline: none;
  }
  .inner {
    padding: 0 22px 40px;
    height: 100%;
  }
  .head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    position: sticky;
    top: 0;
    padding: 16px 0 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--rule);
    z-index: 1;
  }
  .kicker {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  h2 {
    font-size: var(--t-lg);
    font-weight: 680;
  }
  h2 span {
    color: var(--ink-2);
    font-weight: 500;
  }
  .addrs {
    font-size: var(--t-xs);
    color: var(--ink-3);
    margin-top: 2px;
    overflow-wrap: anywhere;
  }
  .close {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 26px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--ink-2);
  }
  .close:hover {
    background: var(--surface-2);
    color: var(--ink);
  }
  .note {
    margin-top: 14px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    padding: 10px 12px;
    background: var(--surface-2);
    border-radius: var(--r-md);
  }
  section {
    margin-top: 26px;
  }
  h3 {
    font-size: var(--t-md);
    font-weight: 650;
    margin-bottom: 10px;
  }
  h4 {
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-3);
    margin: 14px 0 4px;
  }
  .dive {
    margin: 10px 0 0;
    display: flex;
    gap: 8px;
  }
  .dive .btn.on {
    color: var(--accent);
  }
  .facts {
    margin: 0;
    display: grid;
  }
  .facts > div {
    display: grid;
    grid-template-columns: 170px minmax(0, 1fr);
    gap: 12px;
    padding: 7px 0;
    border-top: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  dt {
    color: var(--ink-3);
    font-size: var(--t-xs);
    padding-top: 2px;
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .why {
    margin-top: 8px;
    font-size: var(--t-xs);
    color: var(--ink-3);
    max-width: 70ch;
  }
  .lat {
    margin-bottom: 18px;
  }
  .lat-head {
    display: flex;
    gap: 8px;
    align-items: baseline;
    flex-wrap: wrap;
    margin-bottom: 4px;
    font-size: var(--t-xs);
  }
  .tp {
    font-weight: 650;
    font-size: var(--t-sm);
  }
  .warnchip {
    padding: 0 6px;
    border-radius: 8px;
    background: var(--warn-soft);
    font-size: var(--t-2xs);
  }
  .mini {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--t-xs);
    margin-top: 6px;
  }
  .mini th,
  .mini td {
    padding: 5px 6px;
    border-top: 1px solid var(--rule);
    text-align: right;
  }
  .mini th:first-child {
    text-align: left;
    font-weight: 500;
  }
  .mini thead th {
    color: var(--ink-3);
    font-weight: 600;
    border-top: 0;
  }
  .mini .host th {
    padding-left: 14px;
    font-weight: 400;
    color: var(--ink-2);
  }
  .mini.wide td:nth-child(2),
  .mini.wide th:nth-child(2) {
    text-align: left;
  }
  .mini .key {
    margin-right: 6px;
  }
  .small {
    font-size: var(--t-xs);
  }
  .caps {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .caps li {
    padding: 8px 0;
    border-top: 1px solid var(--rule);
  }
  .cap-top {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    align-items: center;
  }
  .cap-l {
    font-size: var(--t-sm);
    font-weight: 560;
  }
  .ref {
    margin-left: 8px;
    font-size: var(--t-xs);
    color: var(--ink-3);
    font-weight: 400;
  }
  .cap-x {
    font-size: var(--t-xs);
    color: var(--ink-3);
    margin-top: 2px;
    max-width: 64ch;
  }
  .ev-toggle {
    margin-top: 4px;
    padding: 0;
    border: 0;
    background: none;
    font-size: var(--t-xs);
    color: var(--accent-text);
    cursor: default;
  }
  .ev {
    margin: 6px 0 0;
    padding: 8px 10px;
    background: var(--surface-2);
    border-radius: var(--r-md);
    font-size: var(--t-xs);
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--ink-2);
  }
  .pub {
    display: inline-flex;
    gap: 5px;
    align-items: center;
    font-size: var(--t-xs);
  }
  .fcat {
    border-top: 1px solid var(--rule);
  }
  .fhead {
    display: flex;
    gap: 10px;
    align-items: baseline;
    width: 100%;
    padding: 8px 0;
    border: 0;
    background: none;
    cursor: default;
    text-align: left;
    flex-wrap: wrap;
  }
  .fs {
    list-style: none;
    padding: 0 0 8px 22px;
    margin: 0;
  }
  .fs li {
    display: grid;
    grid-template-columns: 14px minmax(0, 1fr) minmax(0, 1fr);
    gap: 8px;
    align-items: center;
    font-size: var(--t-xs);
    padding: 2px 0;
  }
  .dom,
  .how {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .feed {
    display: flex;
    gap: 7px;
    align-items: center;
    flex-wrap: wrap;
    font-size: var(--t-sm);
  }
  .feed a,
  .links a,
  .incs a {
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }
  .inc {
    margin-top: 4px;
  }
  .incs {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .incs li {
    display: grid;
    grid-template-columns: 92px minmax(0, 1fr);
    gap: 10px;
    padding: 8px 0;
    border-top: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .incs li > span:last-child {
    display: grid;
    gap: 2px;
  }
  .links {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
  }
  @media (max-width: 560px) {
    .inner {
      padding: 18px 16px 40px;
    }
    .facts > div {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    .incs li {
      grid-template-columns: 1fr;
    }
  }
</style>
