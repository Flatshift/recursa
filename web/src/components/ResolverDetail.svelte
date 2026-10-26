<script lang="ts" module>
  import type { ResolverView } from '../lib/store.svelte';
  import type { CheckKind } from '../lib/checks.svelte';

  export const RESOLVER_VIEW_LABEL: Record<ResolverView, string> = {
    overview: 'Overview',
    latency: 'Latency',
    path: 'Path',
    behaviour: 'Behaviour',
    blocking: 'Blocking',
    service: 'Service',
  };
  /** The check each of the page's views runs. */
  export const VIEW_CHECK: Partial<Record<ResolverView, CheckKind>> = { latency: 'dive', path: 'trace', behaviour: 'checks', blocking: 'live' };
</script>

<script lang="ts">
  // Everything known about one resolver, with the checks it can run on demand, one view at a time: an overview with a
  // line per view, or a view's sections under its check.
  import type { Resolver, RunState, TestKind } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { HIT_SHARE, RELIABLE, TESTS, expectsBlock, verdict } from '../lib/verdict';
  import { CAPS, PUBLISHED_ONLY, type CapGroup } from '../lib/caps';
  import { capCell, cdnCell, roaCell, rovCell } from '../lib/cells';
  import { TRANSPORT_LABEL, day, holder, label, list, ms, pct, when } from '../lib/format';
  import { TEST_LABEL } from '../lib/phases';
  import { status, stateLabel, stateGlyph, localNote } from '../lib/status.svelte';
  import { logDomain } from '../lib/scale';
  import { app } from '../lib/store.svelte';
  import { cellPenalty, ecsByAuthority, edgeIps, youOf } from '../lib/cdn';
  import type { HostStatus } from '../lib/types';
  import { blocker, errorOf, isRunning, phase, ranAt, results, run } from '../lib/checks.svelte';
  import Cell from './Cell.svelte';
  import Ecdf from './Ecdf.svelte';
  import Glyph from './Glyph.svelte';
  import Icon from './Icon.svelte';
  import Provenance from './Provenance.svelte';
  import Floor from './Floor.svelte';
  import Route from './Route.svelte';
  import QuerySample from './QuerySample.svelte';
  import BehaviourChecks, { faults } from './BehaviourChecks.svelte';
  import LiveFeeds from './LiveFeeds.svelte';

  const STATUS: Record<HostStatus, string> = {
    ok: 'ok',
    failed: 'no answer',
    blocked: 'blocked',
    unreachable: 'unreachable',
    off_cdn: 'not this CDN',
  };
  const asYouText = (a?: string | null) => (a === 'same' ? 'same edges' : a === 'near' ? 'near edges' : a === 'different' ? 'other edges' : '');

  let {
    resolver,
    run: runState = null,
    rows = [],
    wide = false,
    view = 'overview',
    onview,
  }: {
    resolver: Resolver;
    run?: RunState | null;
    rows?: Row[];
    wide?: boolean;
    view?: ResolverView;
    onview?: (v: ResolverView) => void;
  } = $props();

  const id = $derived(resolver.id);
  const name = $derived(label(resolver.provider, resolver.variant));
  const ip = $derived(resolver.ipv4[0] ?? resolver.ipv6[0] ?? '');
  const mine = $derived(rows.filter((r) => r.target.resolver_id === id && r.measured));
  const probe = $derived(runState?.probes[id] ?? null);
  const intel = $derived(runState?.intel[id] ?? null);
  const path = $derived(runState?.paths?.[id] ?? null);
  const cdn = $derived(runState?.cdn?.resolvers[id] ?? null);
  // The registry facts (egress, service network, RPKI, CDN mapping) get their own group under the wire facts and path.
  const routing = $derived(
    !!(intel?.egress || probe?.egress.length || (intel?.service && !intel.service.private) || intel?.service?.rpki || intel?.service?.rov || cdn),
  );
  const feed = $derived(status.feedFor(resolver.status_feed));
  const incidents = $derived(status.incidentsFor(id));

  // The client subnet each authority saw, grouped for the CDN mapping line.
  const ecsSeen = $derived.by(() => {
    if (!cdn) return '';
    const by = ecsByAuthority(cdn, probe);
    const groups = new Map<string, string[]>();
    for (const [who, s] of Object.entries(by)) groups.set(s ?? 'none', [...(groups.get(s ?? 'none') ?? []), who]);
    return [...groups].map(([s, whos]) => `${s} to ${list(whos)}`).join(', ');
  });

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
  let allowPlain = $state(false);

  // Results of the on-demand checks, kept for the session.
  const trace = $derived(results.trace[id] ?? null);
  const dive = $derived(results.dive[id] ?? null);
  const checks = $derived(results.checks[id] ?? null);
  // A run's live threat test stands in until this page has its own check.
  const live = $derived(results.live[id] ?? runState?.live?.[id] ?? null);
  const traceCap = $derived(app.env?.trace ?? 'unsupported');
  const traceable = $derived(resolver.ipv4.length > 0 && traceCap !== 'unsupported');
  const stop = $derived(blocker());

  // One line on what the benchmark made of this resolver.
  const row = $derived(mine[0] ?? rows.find((r) => r.target.resolver_id === id) ?? null);
  const picks = $derived(runState && rows.length ? verdict(runState, rows, HIT_SHARE[app.mix]).picks.filter((p) => p.resolverId === id) : []);
  // When a check last ran here, so the headline and the line under it say where the findings came from.
  const checkedAt = $derived(Math.max(0, ...(Object.keys(results) as CheckKind[]).filter((k) => results[k][id]).map((k) => ranAt[`${k}:${id}`] ?? 0)));
  const clock = (t: number) => new Date(t).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  const line = $derived.by(() => {
    if (!runState) return checkedAt ? 'Not benchmarked yet.' : 'Not measured yet: run a benchmark, or test it below.';
    if (!row) return 'Not in this benchmark.';
    if (row.intercepted) return 'Answered by a device on your network, not by this service.';
    if (picks.length) return `Pick for ${list(picks.map((p) => p.goal.toLowerCase()))}.`;
    if (!row.eligible) return 'Measured, but too few answers to rank.';
    return 'Measured; not a pick in this benchmark.';
  });

  // What each view found, and what counts as a problem.
  const whereSum = $derived.by(() => {
    const parts: string[] = [];
    if (probe?.pop || probe?.nsid) parts.push(probe.pop ?? probe.nsid!);
    if (path?.rtt_ms != null) parts.push(`${ms(path.rtt_ms)} round trip`);
    if (intel?.egress) parts.push(`egress AS${intel.egress.asn ?? '?'} ${holder(intel.egress.holder)}`);
    const last = trace?.hops[trace.hops.length - 1];
    if (trace) parts.push(trace.reached && last ? `${last.ttl} hops away` : `trace stopped after hop ${trace.hops.length}`);
    return parts.join(' · ') || (runState ? 'Not probed in this run' : 'Not probed yet');
  });
  const whereBad = $derived(
    mine.some((r) => r.intercepted) || (!!runState && (roaCell(runState, id).status === 'fail' || rovCell(runState, id).status === 'fail')),
  );
  const latSum = $derived.by(() => {
    const r = mine[0];
    const parts = r ? [`cached ${ms(r.dist.cached.p50)}`, `uncached ${ms(r.dist.uncached.p50)}`] : [];
    if (r?.sent) parts.push(`${pct(r.success)} answered`);
    if (dive?.latency.samples_ms.length) parts.push(`sample median ${ms(dive.latency.stats.p50)}`);
    return parts.join(' · ') || (runState ? 'Not measured in this run' : 'Not measured yet');
  });
  const latBad = $derived(mine.some((r) => r.intercepted || (r.sent > 0 && r.success < RELIABLE)));
  const capCells = $derived(CAPS.map((c) => capCell(runState, resolver, c.key)));
  const checkFaults = $derived(checks ? faults(checks) : []);
  const behSum = $derived.by(() => {
    const verified = capCells.filter((c) => c.prov === 'verified');
    const pass = verified.filter((c) => c.status === 'pass').length;
    const caps = verified.length ? `${pass} of ${verified.length} verified capabilities pass` : checks ? '' : 'Published claims only';
    // Behaviour checks run here lead with what they found.
    const found = checks ? (checkFaults.length ? checkFaults.join(', ') : 'No faults in the behaviour checks') : '';
    return [found, caps].filter(Boolean).join(' · ');
  });
  const behBad = $derived(checkFaults.length > 0);
  const filt = $derived(Object.entries(probe?.filtering ?? {}));
  const blockSum = $derived.by(() => {
    const cats = Object.values(live?.categories ?? {});
    const tested = cats.reduce((a, c) => a + c.tested, 0);
    const caught = tested ? ` · blocked ${cats.reduce((a, c) => a + c.blocked, 0)} of ${tested} live threats` : '';
    if (filt.length) {
      const seen = filt.filter(([, f]) => f.blocked > 0).map(([cat]) => cat);
      return (seen.length ? `Blocking confirmed for ${list(seen)}` : 'No blocking seen') + caught;
    }
    return (resolver.published.filtering.length ? `Publishes ${list(resolver.published.filtering)} blocking` : 'No blocking published') + caught;
  });
  const blockBad = $derived.by(() => {
    const claims = (cat: string) => expectsBlock(resolver, cat);
    if (filt.some(([cat, f]) => claims(cat) && f.tested && (cat === 'ads' ? f.blocked / f.tested < 0.5 : f.blocked === 0))) return true;
    return Object.entries(live?.categories ?? {}).some(([cat, f]) => claims(cat) && f.tested && f.blocked / f.tested < 0.5);
  });
  const relSum = $derived(feed ? stateLabel(feed) : resolver.group === 'public' ? 'No status feed' : '');
  const relBad = $derived(!!feed && ['fail', 'partial'].includes(stateGlyph(feed.state)));
  const polSum = $derived([resolver.published.logging ?? 'Logging not summarised', resolver.published.jurisdiction ?? ''].filter(Boolean).join(' · '));

  type Key = 'where' | 'latency' | 'behaviour' | 'blocking' | 'reliability' | 'limits' | 'policy';
  const TITLE: Record<Key, string> = {
    where: 'Where it answers from',
    latency: 'Latency',
    behaviour: 'Behaviour',
    blocking: 'Blocking',
    reliability: 'Reliability',
    limits: 'Usage limits',
    policy: 'Policy',
  };
  // The sections each of the page's views holds.
  const KEYS: Record<Exclude<ResolverView, 'overview'>, Key[]> = {
    latency: ['latency'],
    path: ['where'],
    behaviour: ['behaviour'],
    blocking: ['blocking'],
    service: ['reliability', 'limits', 'policy'],
  };

  // The page's overview: a line per view with what it found, problems first.
  const sheet = $derived.by(() => {
    const all: { view: Exclude<ResolverView, 'overview'>; text: string; bad: boolean }[] = [
      { view: 'latency', text: latSum, bad: latBad },
      { view: 'path', text: whereSum, bad: whereBad },
      { view: 'behaviour', text: behSum, bad: behBad },
      { view: 'blocking', text: blockSum, bad: blockBad },
      { view: 'service', text: [relSum, polSum].filter(Boolean).join(' · '), bad: relBad },
    ];
    return [...all.filter((s) => s.bad), ...all.filter((s) => !s.bad)];
  });
  const from = $derived.by(() => {
    const src = [runState ? `the benchmark of ${when(runState.started_at)}` : '', checkedAt ? `checks run at ${clock(checkedAt)}` : ''].filter(Boolean);
    return src.length ? `From ${src.join(' and ')}.` : '';
  });

  const verb: Record<CheckKind, [string, string, string]> = {
    trace: ['Trace path', 'Trace again', 'Tracing…'],
    dive: ['Sample 18 queries', 'Sample again', 'Sampling…'],
    checks: ['Check behaviour', 'Check again', 'Checking…'],
    live: ['Check live feeds', 'Check again', 'Checking…'],
  };
  const tips = $derived<Record<CheckKind, string>>({
    trace: `Sends ICMP probes with rising TTL to ${ip}, about 2 s`,
    dive: 'Sends 18 ordinary queries, spaced like the benchmark, about 3 s',
    checks: 'About 90 queries for test names: truncation, TTLs, negative caching, DNSSEC, caches, transports; about 15 s',
    live: 'Asks for live malware and phishing hosts from today’s feeds; up to a minute',
  });
  // What each check does, while it has no result.
  const hints = $derived<Record<CheckKind, string>>({
    trace: `Trace path shows each router between this computer and ${ip}, with its round trip.`,
    dive: 'Sample 18 queries sends a popular name twice, three made-up names, the popular name 12 more times and one last ask two seconds later: round trips, the cost of a full lookup and whether answers come from cache.',
    checks:
      'Check behaviour sends about 90 queries for test names: a large signed answer over UDP and TCP, records with known TTLs, signed names with known faults, made-up names asked in rounds, and ten asks over each transport it offers.',
    live: "Check live feeds takes hosts from today's URLhaus and ThreatView malware feeds and the OpenPhish phishing feed, keeps up to 20 of each that still resolve through an unfiltered resolver, and asks this resolver for them.",
  });
  const traceWhy = $derived(
    traceCap === 'needs_admin' ? `Needs administrator rights on ${app.env?.platform === 'windows' ? 'Windows' : 'this computer'}` : null,
  );
  const has = (k: CheckKind) => !!results[k][id];
  const text = (k: CheckKind) => (isRunning(k, id) ? verb[k][2] : errorOf(k, id) ? 'Try again' : has(k) ? verb[k][1] : verb[k][0]);
  // The line beside a view's check on the page: what it does, how it is going, or when it last ran.
  const barText = (k: CheckKind) => {
    if (isRunning(k, id)) return phase(k, name, ip);
    const at = ranAt[`${k}:${id}`];
    if (errorOf(k, id)) return errorOf(k, id)!;
    return at && has(k) ? `Last run at ${clock(at)}. ${tips[k]}.` : hints[k];
  };
</script>

{#snippet check(kind: CheckKind, body?: unknown, why: string | null = null)}
  <button class="btn sm" disabled={!!(stop ?? why)} title={stop ?? why ?? tips[kind]} onclick={() => run(kind, id, body)}>
    <Icon name="play" size={12} />{text(kind)}
  </button>
{/snippet}

{#snippet test(kind: CheckKind)}
  {#if kind === 'live'}
    {@render check('live', { plain: allowPlain })}
  {:else if kind !== 'trace' || traceable}
    {@render check(kind, undefined, kind === 'trace' ? traceWhy : null)}
  {/if}
{/snippet}

<!-- A check's result. The view's bar carries what it does, its progress and its error, so this shows only what came
     back. -->
{#snippet result(kind: CheckKind)}
  {@const busy = isRunning(kind, id)}
  {#if kind === 'live'}
    <div class="result">
      <p class="warn">
        <Icon name="warn" size={14} /><span
          >These are real malicious hosts. Security tools on the network (a UniFi gateway, Azure, a corporate firewall) may flag
          the lookups or block this device. Run this only when you mean to test a blocking resolver.</span
        >
      </p>
      <label class="toggle"><input type="checkbox" bind:checked={allowPlain} /><span>Allow plain DNS if this resolver has no DoT or DoH</span></label>
      <p class="why">
        Otherwise the names go only over DoT or DoH, where nothing on the path can read them. Your own resolver on this network (a
        Pi-hole, your router) is tested over plain DNS either way.
      </p>
      <LiveFeeds rid={id} {live} fromRun={runState?.live} />
    </div>
  {:else if has(kind) || (busy && kind !== 'checks')}
    <div class="result">
      {#if kind === 'trace'}
        <Route {trace} {busy} rtt={path?.rtt_ms ?? null} />
      {:else if kind === 'dive'}
        <QuerySample {dive} {busy} />
      {:else if checks}
        <BehaviourChecks rid={id} res={checks} />
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet body(key: Key)}
  {#if key === 'where'}
    {#if runState}
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
          {#if mine.length}
            <div class="full">
              <dt>Where the time goes</dt>
              <dd><Floor {path} rows={mine} name={resolver.provider} /></dd>
            </div>
          {/if}
        {/if}
      </dl>
    {:else}
      <p class="muted small">No benchmark has probed this resolver yet.</p>
    {/if}
    {#if runState && routing}
      <div class="result">
        <h4>Addresses and routing</h4>
        <dl class="facts">
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
                <Cell c={roaCell(runState, id)} />
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
            <div><dt>Operator drops invalid routes</dt><dd><Cell c={rovCell(runState, id)} /></dd></div>
          {/if}
          {#if cdn}
            <div>
              <dt>CDN mapping</dt>
              <dd>
                <Cell c={cdnCell(runState, id)} />
                <span class="muted">· ECS {ecsSeen || 'not tested'}</span>
              </dd>
            </div>
          {/if}
        </dl>
        {#if intel?.service?.rpki || intel?.service?.rov}
          <p class="why">
            A valid ROA means networks that check RPKI will reject anyone else announcing this resolver's addresses. Operator ROV
            protects the resolver's own outbound routes. Both limit hijacks like the one that cut off 1.1.1.1 for 300 networks in
            June 2024.
          </p>
        {/if}
      </div>
    {/if}
    {#if cdn && runState?.cdn}
      <div class="result">
        <h4>CDN edges it chose for you</h4>
        <table class="mini edges">
          <thead><tr><th>CDN</th><th>Edge</th><th>Round trip</th><th>Your address</th></tr></thead>
          <tbody>
            {#each runState.cdn.cdns as c}
              {@const m = cdn.mappings[c.id]}
              {@const ips = edgeIps(m)}
              {@const pen = cellPenalty(c, m)}
              {@const asYou = m?.as_you}
              {@const y = youOf(c)}
              <tr>
                <th>{c.name.replace(' (anycast control)', '')}{#if c.role === 'control'} <span class="muted">anycast</span>{/if}</th>
                <td class="mono small">{ips.join(', ') || m?.error || '—'}<br /><span class="muted">{holder(runState.cdn.edges[ips[0] ?? '']?.holder)}</span></td>
                <td class="num">
                  {ms(m?.rtt_ms)}{#if pen != null && pen >= 0.5}<br /><span class="muted">+{ms(pen, false)} vs best</span>{/if}
                </td>
                <td class="num">
                  {#if y?.rtt_ms != null}
                    {ms(y.rtt_ms)}<br /><span class="muted">{asYouText(asYou)}</span>
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
                  <td class="num"><span class="muted">{asYouText(h.as_you)}</span></td>
                </tr>
              {/each}
            {/each}
          </tbody>
        </table>
        <p class="why">“Your address” is the edge each CDN picks when asked directly from this computer, with no resolver in between.</p>
      </div>
    {/if}
  {:else if key === 'latency'}
    {#if runState && mine.length}
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
              {#each runState.config.tests as k}
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
    {:else}
      <p class="muted small">{runState ? 'Not measured in this run.' : 'No benchmark has measured this resolver yet.'}</p>
    {/if}
  {:else if key === 'behaviour'}
    {#if !probe}<p class="muted small">No probes in this view, so these are the operator's published claims.</p>{/if}
    {#each groups as g}
      {@const caps = CAPS.filter((c) => c.group === g)}
      <h4>{g}</h4>
      <ul class="caps">
        {#each caps as c}
          {@const cell = capCell(runState, resolver, c.key)}
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
  {:else if key === 'blocking'}
    {#if filt.length}
      <h4>Test names</h4>
      <p class="muted small">
        Filtering services publish harmless names for testing their own blocking, and each mostly blocks only its own. A
        blocked name confirms the filter works; how much it catches is what Check live feeds measures.
      </p>
      {#if probe?.block_style}<p class="small">Blocks by answering <b class="mono">{probe.block_style}</b>.</p>{/if}
      {#each filt as [cat, f]}
        <div class="fcat">
          <button class="fhead" aria-expanded={!!openFilter[cat]} onclick={() => (openFilter[cat] = !openFilter[cat])}>
            <Icon name={openFilter[cat] ? 'chevron-down' : 'chevron-right'} size={13} />
            <span class="cap-l">{cat[0].toUpperCase() + cat.slice(1)}</span>
            <span class="num">{cat === 'ads' ? `${f.blocked}/${f.tested} blocked` : f.blocked ? 'Confirmed' : 'Not seen'}</span>
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
    {:else}
      <p class="muted small">{resolver.published.filtering.length ? `Publishes ${list(resolver.published.filtering)} blocking; no run has tested it from here.` : 'Publishes no blocking.'}</p>
    {/if}
  {:else if key === 'reliability'}
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
    {:else}
      <p class="small muted">Turn on monitoring to build a record of this resolver from this network.</p>
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
  {:else if key === 'limits' && resolver.limits}
    <p class="small">{resolver.limits.text} <a href={resolver.limits.source} target="_blank" rel="noreferrer">Source <Icon name="external" size={11} /></a></p>
    {#if runState?.rates?.[id] != null}
      <p class="small">
        This run sent it at most <b>{runState.rates[id].toFixed(0)}</b> queries per second{resolver.limits.qps ? `, against the ${resolver.limits.qps.toLocaleString()} it publishes${resolver.limits.per ? ` for ${resolver.limits.per}` : ''}` : ''}.
      </p>
    {/if}
  {:else if key === 'policy'}
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
  {/if}
{/snippet}
<div class="detail" class:wide>
  {#if view === 'overview'}
    <h2 class="headline">{line}</h2>
    {#if from}<p class="from">{from}</p>{/if}
    {#if resolver.notes}<p class="note">{resolver.notes}</p>{/if}
    <!-- A line per view: what it found, with its check beside it. The whole line opens the view. -->
    <ul class="sheet">
      {#each sheet as s (s.view)}
        {@const kind = VIEW_CHECK[s.view]}
        {@const err = kind ? errorOf(kind, id) : null}
        <li class="srow" class:problem={s.bad}>
          <button class="sname" aria-describedby="rd-{id}-{s.view}" onclick={() => onview?.(s.view)}>
            {RESOLVER_VIEW_LABEL[s.view]}{#if s.bad}<span class="visually-hidden">, needs a look</span>{/if}
          </button>
          <span class="sfind" class:bad={!!err} id="rd-{id}-{s.view}">
            {#if kind && isRunning(kind, id)}{phase(kind, name, ip)}{:else if err}{err}{:else}{s.text || 'Nothing measured yet'}{/if}
          </span>
          <span class="stest">{#if kind}{@render test(kind)}{/if}</span>
          <span class="chev" aria-hidden="true"><Icon name="chevron-right" size={14} /></span>
        </li>
      {/each}
    </ul>
  {:else}
    {@const kind = VIEW_CHECK[view]}
    {#if kind && (kind !== 'trace' || traceable)}
      <div class="testbar">
        <p class="tb-text" class:bad={!!errorOf(kind, id)} role="status">{barText(kind)}</p>
        {@render test(kind)}
      </div>
      {@render result(kind)}
    {/if}
    {#each KEYS[view].filter((k) => k !== 'limits' || resolver.limits) as key (key)}
      {#if view === 'service'}<h3 class="vh">{TITLE[key]}</h3>{/if}
      {@render body(key)}
    {/each}
  {/if}
</div>

<style>
  .detail {
    padding-bottom: 24px;
  }
  /* Tables take the pane's width; running text stays within a readable measure. */
  p {
    max-width: 72ch;
  }
  .note {
    margin: 0 0 14px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    padding: 10px 12px;
    background: var(--surface-2);
    border-radius: var(--r-md);
  }
  .result {
    margin-top: 16px;
  }
  /* The overview: the verdict as its headline, then a line per view. */
  .headline {
    margin: 20px 0 4px;
    font-size: var(--t-xl);
    font-weight: 560;
    line-height: 1.25;
    text-wrap: balance;
    max-width: 40ch;
  }
  .from {
    margin: 0;
    font-size: var(--t-sm);
    color: var(--ink-3);
  }
  .sheet {
    list-style: none;
    margin: 20px 0 0;
    padding: 0;
    border-top: 1px solid var(--rule);
    container-type: inline-size;
  }
  .srow {
    position: relative;
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr) auto 20px;
    align-items: center;
    gap: 16px;
    min-height: 52px;
    padding: 8px 4px 8px 12px;
    border-bottom: 1px solid var(--rule);
    transition: background 140ms cubic-bezier(0.22, 1, 0.36, 1);
  }
  .srow:hover {
    background: var(--surface-2);
  }
  .sname {
    position: static;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    font-weight: 600;
    color: var(--ink);
    text-align: left;
  }
  /* The name's button covers the whole line; the check's button sits above it. */
  .sname::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: var(--r-sm);
  }
  .sname:focus-visible {
    outline: none;
  }
  .sname:focus-visible::after {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }
  .srow.problem .sname::before {
    content: '';
    flex: none;
    width: 6px;
    height: 6px;
    margin-left: -12px;
    margin-right: -2px;
    border-radius: 50%;
    background: var(--critical);
  }
  .sfind {
    font-size: var(--t-sm);
    color: var(--ink-2);
    overflow-wrap: anywhere;
  }
  .sfind.bad,
  .tb-text.bad {
    color: var(--critical);
  }
  .stest {
    position: relative;
    z-index: 1;
  }
  .chev {
    display: flex;
    color: var(--ink-3);
  }
  @container (max-width: 560px) {
    .srow {
      grid-template-columns: minmax(0, 1fr) auto 20px;
      grid-template-areas: 'name test chev' 'find test chev';
      row-gap: 2px;
      padding-block: 10px;
    }
    .sname {
      grid-area: name;
    }
    .sfind {
      grid-area: find;
    }
    .stest {
      grid-area: test;
    }
    .chev {
      grid-area: chev;
    }
  }
  /* A phone: the finding gets the full line, the check's button goes under it. */
  @container (max-width: 420px) {
    .srow {
      grid-template-columns: minmax(0, 1fr) 20px;
      grid-template-areas: 'name chev' 'find chev' 'test chev';
    }
    .stest {
      justify-self: start;
      margin-top: 6px;
    }
  }
  /* A view's check: what it does or did, and its button. */
  .testbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px 16px;
    padding: 14px 0;
    border-bottom: 1px solid var(--rule);
  }
  .tb-text {
    flex: 1 1 320px;
    margin: 0;
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .vh {
    margin: 24px 0 8px;
    font-size: var(--t-md);
    font-weight: 640;
  }
  .vh:first-child {
    margin-top: 16px;
  }
  @media (prefers-reduced-motion: reduce) {
    .srow {
      transition: none;
    }
  }
  h4 {
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-3);
    margin: 14px 0 4px;
  }
  .result > h4 {
    margin-top: 0;
    color: var(--ink-2);
    font-size: var(--t-sm);
    font-weight: 640;
  }
  .roa {
    display: grid;
    gap: 2px;
    margin-top: 4px;
    font-size: var(--t-xs);
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
  .facts > div:first-child {
    border-top: 0;
  }
  .facts > div.full {
    grid-template-columns: 1fr;
    gap: 2px;
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
  .mini.edges td:nth-child(2),
  .mini.edges th:nth-child(2) {
    text-align: left;
  }
  /* A number and its note each stay on one line; the edge column absorbs the width. */
  .mini .num,
  .mini.edges th:first-child {
    white-space: nowrap;
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
  .warn {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin: 6px 0;
    padding: 8px 10px;
    font-size: var(--t-xs);
    background: var(--warn-soft);
    border-radius: var(--r-md);
  }
  .warn :global(svg) {
    flex: none;
    margin-top: 1px;
    color: var(--warning);
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 6px 0 2px;
    font-size: var(--t-xs);
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
  .wide .facts > div {
    grid-template-columns: 220px minmax(0, 1fr);
  }
  @media (max-width: 560px) {
    .facts > div {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    .incs li {
      grid-template-columns: 1fr;
    }
  }
</style>
