<script lang="ts">
  import { untrack } from 'svelte';
  import { app } from '../lib/store.svelte';
  import { api, confirmAction } from '../lib/api';
  import { status, stateLabel, stateGlyph, localNote } from '../lib/status.svelte';
  import type { MonitorConfig, MonitorHist, MonitorHistory } from '../lib/types';
  import { ago, day, ms, pct, plural, when } from '../lib/format';
  import Glyph from '../components/Glyph.svelte';
  import Icon from '../components/Icon.svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import Info from '../components/Info.svelte';
  import Record, { type Mark } from '../components/Record.svelte';

  interface MonitorResp {
    config: MonitorConfig;
    last: number | null;
    paused: boolean;
    history: MonitorHistory;
  }

  const RANGES: [number, string, string][] = [
    [24, '24 h', 'the last 24 hours'],
    [72, '3 d', 'the last 3 days'],
    [168, '7 d', 'the last 7 days'],
    [720, '30 d', 'the last 30 days'],
  ];

  let hours = $state(72);
  let mon = $state<MonitorResp | null>(null);
  let cfg = $state<MonitorConfig | null>(null);
  let saving = $state(false);
  let refreshing = $state(false);
  let cursor = $state<number | null>(null);

  async function loadMonitor() {
    try {
      mon = await api.get<MonitorResp>(`/api/monitor?hours=${hours}`);
      if (!cfg) cfg = $state.snapshot(mon.config);
    } catch (e) {
      app.notify((e as Error).message, 'error');
    }
  }
  $effect(() => {
    hours;
    loadMonitor();
  });
  // Monitoring was switched from the menu bar: take its config as the new baseline.
  $effect(() => {
    if (!app.monitorRev) return;
    untrack(() => {
      cfg = null;
      loadMonitor();
    });
  });
  $effect(() => {
    status.load();
    // Refetch the history only when a monitoring cycle has been recorded since.
    const t = setInterval(async () => {
      if (document.hidden) return;
      try {
        const { last } = await api.get<{ last: number | null }>('/api/monitor/last');
        if (last !== mon?.last) loadMonitor();
      } catch {
        /* the next tick tries again */
      }
    }, 60_000);
    return () => clearInterval(t);
  });

  async function save(next: MonitorConfig) {
    saving = true;
    try {
      cfg = await api.put<MonitorConfig>('/api/monitor', next);
      if (app.boot) app.boot = { ...app.boot, monitor: cfg };
      app.notify(cfg.enabled ? `Monitoring every ${cfg.interval_min} min while Recursa runs.` : 'Monitoring paused.');
      loadMonitor();
    } catch (e) {
      app.notify((e as Error).message, 'error');
    } finally {
      saving = false;
    }
  }
  async function refresh() {
    refreshing = true;
    await status.load(true);
    refreshing = false;
  }
  async function clearHistory() {
    if (!(await confirmAction('Delete all monitoring history? This can’t be undone.', 'Delete History'))) return;
    try {
      await api.del('/api/monitor/history');
      loadMonitor();
    } catch (e) {
      app.notify((e as Error).message, 'error');
    }
  }

  const resolvers = $derived(app.resolvers);
  const nameOf = (id: string) => {
    const r = resolvers.find((x) => x.id === id);
    return r ? `${r.provider} ${r.variant}`.trim() : id;
  };
  const feeds = $derived(status.data?.live.feeds ?? []);
  const feedless = $derived(
    [...new Set(resolvers.filter((r) => (r.group === 'public' || r.group === 'legacy') && !r.status_feed).map((r) => r.provider))],
  );

  // What each operator published about a resolver: curated incidents, plus its live feed's.
  function marks(id: string): Mark[] {
    const out: Mark[] = status.incidentsFor(id).map((i) => ({ t: new Date(i.start ?? i.date).getTime(), title: i.title }));
    const feed = resolvers.find((r) => r.id === id)?.status_feed;
    for (const f of feeds)
      if (f.feed === feed) for (const i of f.incidents) if (i.started) out.push({ t: new Date(i.started).getTime(), title: i.title });
    return out;
  }

  // ---- The verdict: what the record says, in one sentence, then the evidence. ----
  const interval = $derived(cfg?.interval_min ?? mon?.config.interval_min ?? 5);
  const rangeText = $derived(RANGES.find((r) => r[0] === hours)?.[2] ?? `the last ${hours} hours`);
  const recorded = $derived((mon?.history.resolvers ?? []).filter((h) => h.cycles > 0));
  // A bound, so round up: "up to 32 min", not "31 m 37 s".
  const mins = (t: number) => (t < 90 * 60_000 ? `${Math.ceil(t / 60_000)} min` : `${Math.ceil(t / 3_600_000)} h`);
  const times = (n: number) => (n === 1 ? 'once' : n === 2 ? 'twice' : `${n} times`);
  const list = (xs: string[]) => (xs.length < 2 ? xs.join('') : `${xs.slice(0, -1).join(', ')} and ${xs[xs.length - 1]}`);

  const verdict = $derived.by(() => {
    if (!recorded.length) return null;
    const down = recorded.filter((h) => h.outages.length);
    const rest = recorded.length - down.length;
    let sentence: string;
    if (!down.length)
      sentence = recorded.length === 1 ? `${nameOf(recorded[0].resolver_id)} answered throughout ${rangeText}.` : `Every resolver you monitor answered throughout ${rangeText}.`;
    else if (down.length === 1)
      sentence = `${nameOf(down[0].resolver_id)} stopped answering ${times(down[0].outages.length)} in ${rangeText}${rest ? `; ${rest === 1 ? 'the other' : 'the others'} never did` : ''}.`;
    else
      sentence = `${down.length === recorded.length ? 'Every resolver you monitor' : list(down.map((h) => nameOf(h.resolver_id)))} stopped answering at some point in ${rangeText}${rest ? `; ${rest === 1 ? 'the other' : 'the others'} never did` : ''}.`;

    // Cells with no checks before now: Recursa wasn't running (or monitoring was off).
    const h = mon!.history;
    const elapsed = Math.min(Math.ceil(h.hours / (h.cell / 3_600_000)), Math.ceil((Date.now() - h.since) / h.cell));
    const seen = new Set(recorded.flatMap((r) => r.buckets.filter((c) => c[1] > 0).map((c) => c[0])));
    const blankH = Math.round(((elapsed - seen.size) * h.cell) / 3_600_000);
    const cycles = Math.max(...recorded.map((r) => r.cycles));
    const lossy = recorded.filter((r) => (r.query_success ?? 1) < 0.99 && !r.outages.length);
    const detail = [
      `${plural(recorded.length, 'resolver')} checked from this network in ${plural(cycles, 'cycle')}, now every ${interval} min. A resolver is down for a cycle only when none of its servers answers anything.`,
      lossy.length ? `${list(lossy.map((r) => nameOf(r.resolver_id)))} never went down but dropped more than 1% of queries.` : '',
      blankH > 0 ? `About ${plural(blankH, 'hour')} ${blankH === 1 ? 'is' : 'are'} blank: Recursa wasn't running then.` : '',
    ]
      .filter(Boolean)
      .join(' ');

    const best = <T,>(xs: T[], f: (x: T) => number | null) => xs.filter((x) => f(x) != null).sort((a, b) => f(a)! - f(b)!)[0];
    const picks: { goal: string; name: string; ev: string }[] = [];
    if (recorded.length > 1) {
      const a = best(recorded, (r) => (r.query_success == null ? null : -r.query_success));
      if (a) picks.push({ goal: 'Most answered', name: nameOf(a.resolver_id), ev: `${pct(a.query_success)} of queries` });
      const q = best(recorded, (r) => r.p50);
      if (q) picks.push({ goal: 'Quickest typical answer', name: nameOf(q.resolver_id), ev: `median ${ms(q.p50)}` });
      const s = best(recorded, (r) => (r.p50 && r.p95 ? r.p95 / r.p50 : null));
      if (s) picks.push({ goal: 'Steadiest', name: nameOf(s.resolver_id), ev: `95% within ${ms(s.p95)}, median ${ms(s.p50)}` });
      const longest = outages[0];
      picks.push(
        longest
          ? { goal: 'Longest outage', name: longest.name, ev: `${longest.ongoing ? 'ongoing, ' : ''}up to ${mins(longest.upTo)} from ${when(longest.start)}` }
          : { goal: 'Outages', name: 'None', ev: `in ${plural(cycles, 'cycle')}` },
      );
    }
    return { sentence, detail, picks };
  });

  // ---- Outages, longest first. A cycle only sees the moments it checks, so a span is bounded: from just after the
  // last answered check to the first one answered again. ----
  const outages = $derived.by(() => {
    if (!mon) return [];
    const now = Date.now();
    return mon.history.resolvers
      .flatMap((h: MonitorHist) =>
        h.outages.map((o) => ({
          id: h.resolver_id,
          name: nameOf(h.resolver_id),
          start: o.start,
          cycles: o.cycles,
          ongoing: o.recovered == null,
          upTo: (o.recovered ?? now) - o.start + interval * 60_000,
          cell: Math.floor((o.start - mon!.history.since) / mon!.history.cell),
        })),
      )
      .sort((a, b) => b.upTo - a.upTo);
  });

  // ---- Incident history: curated plus live feed incidents, newest first. ----
  let provFilter = $state<string>('all');
  const timeline = $derived.by(() => {
    const items: { date: string; provider: string; kind: string; title: string; summary: string; scope?: string; cause?: string; source: string | null; confidence?: string; live?: boolean }[] = [];
    for (const i of status.data?.history ?? []) {
      const p = i.providers[0] === '*' ? 'All validating resolvers' : resolvers.find((r) => r.id === i.providers[0])?.provider ?? i.providers[0];
      items.push({ date: i.start ?? i.date, provider: p, kind: i.kind, title: i.title, summary: i.summary, scope: i.scope, cause: i.cause, source: i.source, confidence: i.confidence });
    }
    for (const f of feeds) {
      for (const i of f.incidents) {
        if (!i.started) continue;
        items.push({
          date: i.started,
          provider: f.name,
          kind: i.status === 'resolved' ? 'resolved' : 'ongoing',
          title: i.title,
          summary: i.update ?? '',
          source: i.url,
          live: true,
        });
      }
    }
    return items.sort((a, b) => new Date(b.date).getTime() - new Date(a.date).getTime());
  });
  const providers = $derived(['all', ...new Set(timeline.map((t) => t.provider))]);
  const shown = $derived(provFilter === 'all' ? timeline : timeline.filter((t) => t.provider === provFilter));
</script>

<div class="view-root">
  <Toolbar title="Reliability" subtitle="What Recursa measured from your network, kept apart from what operators publish">
    {#snippet center()}
      <div class="segmented" role="group" aria-label="Time range">
        {#each RANGES as [h, label]}
          <button aria-pressed={hours === h} onclick={() => (hours = h)}>{label}</button>
        {/each}
      </div>
    {/snippet}
  </Toolbar>
  <div class="view-body">
    <div class="content scroll wrap">
      <section class="verdict" aria-labelledby="rv-h">
        <div class="lead">
          <h2 id="rv-h">{verdict?.sentence ?? (mon ? 'No record from this network yet.' : '')}</h2>
          {#if verdict}
            <p class="detail">{verdict.detail}</p>
          {:else if mon}
            <p class="detail">
              Turn on background monitoring and leave Recursa running. Every few minutes it checks each resolver from this network
              and records whether it answered and how fast. Over days that becomes an uptime record no status page can give you.
            </p>
          {/if}
        </div>
        {#if verdict?.picks.length}
          <dl class="picks">
            {#each verdict.picks as p}
              <div class="pick">
                <dt>{p.goal}</dt>
                <dd><span class="pn">{p.name}</span><span class="ev">{p.ev}</span></dd>
              </div>
            {/each}
          </dl>
        {/if}
      </section>

      {#if cfg}
        <div class="mcfg">
          <label class="toggle">
            <input type="checkbox" checked={cfg.enabled} disabled={saving} onchange={(e) => save({ ...cfg!, enabled: (e.currentTarget as HTMLInputElement).checked })} />
            <span>Monitor in the background while Recursa runs</span>
          </label>
          <label class="inline">
            every
            <select value={cfg.interval_min} onchange={(e) => save({ ...cfg!, interval_min: Number((e.currentTarget as HTMLSelectElement).value) })}>
              {#each [1, 5, 10, 15, 30, 60] as m}<option value={m}>{m} min</option>{/each}
            </select>
          </label>
          <label class="inline">
            keep
            <select value={cfg.retention_days} onchange={(e) => save({ ...cfg!, retention_days: Number((e.currentTarget as HTMLSelectElement).value) })}>
              {#each [7, 30, 90, 365] as d}<option value={d}>{d} days</option>{/each}
            </select>
          </label>
          <details class="pickers">
            <summary><Icon name="chevron-right" size={12} />{plural(cfg.resolvers.length, 'resolver')} {cfg.enabled ? 'monitored' : 'selected'}</summary>
            <div class="chips">
              {#each resolvers.filter((r) => r.group !== 'legacy') as r (r.id)}
                {@const on = cfg.resolvers.includes(r.id)}
                <button
                  class="chip"
                  class:on
                  aria-pressed={on}
                  onclick={() => save({ ...cfg!, resolvers: on ? cfg!.resolvers.filter((x) => x !== r.id) : [...cfg!.resolvers, r.id] })}
                  >{#if on}<Icon name="check" size={11} />{/if}{r.provider} {r.variant}</button
                >
              {/each}
            </div>
          </details>
          {#if mon?.last}<span class="last">Last check {ago(mon.last)}</span>{/if}
        </div>
      {/if}

      {#if mon && mon.history.resolvers.length}
        <section class="sheet" aria-labelledby="rec-h">
          <div class="bh">
            <div class="ht">
              <h2 id="rec-h" class="section-title">Your network's record</h2>
              <Info label="How monitoring works">
                <p>
                  Each cycle sends one cached, one uncached and one TLD query to both of a resolver's servers, primary and secondary,
                  and weighs them together: half of a cycle goes if one server stops answering. A cycle counts as down only when
                  neither server answers anything. Monitoring pauses during manual runs.
                </p>
                <p>
                  Each column is {mon.history.cell === 3_600_000 ? 'an hour' : `${mon.history.cell / 3_600_000} hours`}. Its shading is
                  where that time's answers landed on the log scale at the right, darkest where most did, so a resolver that answers
                  from cache and from the internet shows two bands. The line is the median; the bars beneath are the share left
                  unanswered.
                </p>
                {#if app.env?.interception?.verdict === 'intercepted'}<p>While port 53 is intercepted on this network, resolvers that offer DoT or DoH are monitored over encrypted DNS instead, which has one endpoint.</p>{/if}
              </Info>
            </div>
            <button class="btn ghost sm" onclick={clearHistory}><Icon name="trash" size={13} />Clear history</button>
          </div>
          <Record history={mon.history} intervalMin={interval} {nameOf} {marks} bind:cursor />
          <p class="legend" aria-hidden="true">
            <span><i class="sw-heat"></i>Where answers landed</span>
            <span><i class="sw-med"></i>Median</span>
            <span><i class="sw-loss"><b class="w"></b><b class="s"></b><b class="c"></b></i>Unanswered: some, most, all</span>
            <span><i class="sw-out"></i>Down</span>
            <span><i class="sw-gap"></i>No checks</span>
            <span><i class="sw-mark"></i>Operator reported an incident</span>
          </p>
        </section>

        {#if outages.length}
          <section class="block" aria-labelledby="out-h">
            <h2 id="out-h" class="section-title out-h">Outages</h2>
            <table class="outs">
              <thead>
                <tr><th>Resolver</th><th>Began</th><th class="r">Lasted up to</th><th class="r">Missed cycles</th></tr>
              </thead>
              <tbody>
                {#each outages as o}
                  <tr onpointerenter={() => (cursor = o.cell)} onpointerleave={() => (cursor = null)}>
                    <td><Glyph status="fail" size={12} />{o.name}</td>
                    <td>{when(o.start)}</td>
                    <td class="r">{o.ongoing ? 'ongoing' : mins(o.upTo)}</td>
                    <td class="r">{o.cycles}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </section>
        {/if}
      {/if}

      <section class="block" aria-labelledby="ops-h">
        <div class="bh">
          <div class="ht">
            <h2 id="ops-h" class="section-title">What operators publish</h2>
            <Info label="About operator status">
              <p>Each operator's own status page, read where it publishes a machine-readable feed. It describes their service as a whole, not what reaches your network.</p>
            </Info>
          </div>
          <button class="btn ghost sm" onclick={refresh} disabled={refreshing || status.data?.measuring}>
            <Icon name="refresh" size={13} />{refreshing ? 'Refreshing…' : 'Refresh'}
          </button>
        </div>
        {#if status.data?.measuring}<p class="hint">Paused while a benchmark is measuring, so status checks don't add network noise.</p>{/if}
        {#if status.error}<p class="hint err">{status.error}</p>{/if}
        <ul class="ops">
          {#each feeds as f (f.feed)}
            <li>
              <div class="op-name">
                <span class="nm">{f.name}</span>
                {#if f.locality || f.affected?.length || f.component}
                  <span class="sub">{f.locality ? localNote(f) : f.affected?.length ? `Affected: ${f.affected.join(', ')}` : `Component: ${f.component}`}</span>
                {/if}
              </div>
              <span class="op-state"><Glyph status={f.ok ? stateGlyph(f.state) : 'unknown'} size={13} />{f.ok ? stateLabel(f) : 'Feed unreachable'}</span>
              <span class="op-inc">
                {#if f.incidents[0]}<span class="muted">{day(f.incidents[0].started)}</span> {f.incidents[0].title}{:else if f.ok}<span class="muted">No recent DNS incidents</span>{/if}
              </span>
              <a class="op-link" href={f.page} target="_blank" rel="noreferrer">Status page <Icon name="external" size={11} /></a>
            </li>
          {/each}
          {#if !feeds.length && status.loading}<li class="muted">Checking status pages…</li>{/if}
        </ul>
        {#if feedless.length}
          <p class="feedless">
            <Icon name="info" size={13} />
            <span>
              No machine-readable status feed from <b>{feedless.join(', ')}</b>. That is no record, not a clean one: for those, your
              own monitoring above is the only reliability evidence.
            </span>
          </p>
        {/if}
        {#if status.data?.live.fetched_at}<p class="hint">Checked {ago(status.data.live.fetched_at)}. Status is cached for 10 minutes.</p>{/if}
      </section>

      <section class="block" aria-labelledby="tl-h">
        <div class="bh">
          <div class="ht">
            <h2 id="tl-h" class="section-title">Incident history</h2>
            <Info label="About incident history">
              <p>
                Curated, sourced incidents plus recent entries from live status feeds. Unconfirmed reports are labeled as such. A longer
                list means an operator publishes more, not that it fails more, and a resolver with no entries may simply publish nothing.
              </p>
            </Info>
          </div>
          <label>
            <span class="visually-hidden">Provider</span>
            <select bind:value={provFilter}>
              {#each providers as p}<option value={p}>{p === 'all' ? 'All providers' : p}</option>{/each}
            </select>
          </label>
        </div>
        <ol class="tl">
          {#each shown as i}
            <li>
              <span class="tdate">{day(i.date)}</span>
              <div class="tbody">
                <p class="tt">
                  <span class="tprov">{i.provider}</span>
                  <span class="kind {i.kind}">{i.kind.replace('-', ' ')}</span>
                  {#if i.confidence === 'unconfirmed'}<span class="kind unconfirmed">unconfirmed</span>{/if}
                  {#if i.live}<span class="kind live">live feed</span>{/if}
                </p>
                <p class="ttitle">{i.title}</p>
                {#if i.summary}<p class="tsum">{i.summary}</p>{/if}
                {#if i.scope || i.cause}<p class="tmeta">{[i.scope && `Scope: ${i.scope}`, i.cause && `Cause: ${i.cause}`].filter(Boolean).join(' · ')}</p>{/if}
                {#if i.source}<a href={i.source} target="_blank" rel="noreferrer">Source <Icon name="external" size={11} /></a>{/if}
              </div>
            </li>
          {/each}
        </ol>
      </section>
    </div>
  </div>
</div>

<style>
  .wrap > :global(*) {
    max-width: 1180px;
  }
  /* The verdict sits on the page plane, as on Latency; the record below is the one raised sheet. */
  .verdict {
    container: rverdict / inline-size;
    padding: 10px 4px 0;
    display: grid;
    gap: 22px;
  }
  h2#rv-h {
    font-size: var(--t-2xl);
    font-weight: 600;
    letter-spacing: -0.02em;
    line-height: 1.14;
    max-width: 24ch;
    text-wrap: balance;
    min-height: 1.14em;
  }
  .detail {
    margin-top: 10px;
    color: var(--ink-2);
    max-width: 62ch;
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
  @container rverdict (min-width: 1060px) {
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
  @container rverdict (max-width: 640px) {
    .picks {
      grid-auto-flow: row;
      padding-top: 0;
      border-top: 0;
    }
    .pick,
    .pick:first-child {
      grid-template-columns: 150px minmax(0, 1fr);
      gap: 12px;
      padding: 9px 0;
      border-left: 0;
      border-top: 1px solid var(--rule);
    }
  }

  .mcfg {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 22px;
    align-items: center;
    margin: 26px 4px 0;
    font-size: var(--t-sm);
  }
  .toggle {
    display: flex;
    gap: 8px;
    align-items: center;
    cursor: default;
  }
  .toggle input {
    accent-color: var(--ink);
    width: 15px;
    height: 15px;
    margin: 0;
  }
  .inline {
    display: inline-flex;
    gap: 7px;
    align-items: center;
    color: var(--ink-2);
  }
  .pickers {
    display: contents;
  }
  .pickers summary {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    cursor: default;
    color: var(--ink-2);
    list-style: none;
  }
  .pickers summary::-webkit-details-marker {
    display: none;
  }
  .pickers[open] summary :global(svg) {
    transform: rotate(90deg);
  }
  .last {
    margin-left: auto;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .chips {
    order: 10;
    flex-basis: 100%;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    border: 1px solid var(--rule);
    background: transparent;
    border-radius: 14px;
    padding: 3px 10px;
    font-size: var(--t-xs);
    cursor: default;
    color: var(--ink-2);
  }
  .chip.on {
    background: var(--sel);
    border-color: var(--rule-2);
    color: var(--ink);
  }

  .sheet {
    margin-top: 18px;
    padding: 14px 20px 14px;
    background: var(--surface);
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
  }
  .block {
    margin-top: 40px;
  }
  .ht {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .bh {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    margin-bottom: 10px;
  }
  .hint {
    font-size: var(--t-xs);
    color: var(--ink-3);
    margin-top: 8px;
    max-width: 80ch;
  }
  .err {
    color: var(--critical);
  }

  .legend {
    display: flex;
    gap: 6px 16px;
    flex-wrap: wrap;
    margin-top: 12px;
    font-size: var(--t-2xs);
    color: var(--ink-3);
  }
  .legend span {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .legend i {
    display: inline-block;
    width: 14px;
    height: 10px;
  }
  .sw-heat {
    background: linear-gradient(
      to top,
      color-mix(in oklch, var(--ink) 10%, transparent) 0 33%,
      color-mix(in oklch, var(--ink) 46%, transparent) 33% 66%,
      color-mix(in oklch, var(--ink) 80%, transparent) 66%
    );
  }
  .sw-med {
    height: 1.5px !important;
    background: var(--ink);
  }
  .sw-loss {
    display: inline-flex !important;
    gap: 1px;
    align-items: end;
  }
  .sw-loss b {
    flex: 1;
  }
  .sw-loss .w {
    height: 50%;
    background: var(--warning);
  }
  .sw-loss .s {
    height: 75%;
    background: var(--serious);
  }
  .sw-loss .c {
    height: 100%;
    background: var(--critical);
  }
  .sw-out {
    background: color-mix(in oklch, var(--critical) 16%, transparent);
    box-shadow: inset 1.5px 0 0 var(--critical);
  }
  .sw-gap {
    background: var(--surface-2);
    outline: 1px solid var(--rule);
    outline-offset: -1px;
  }
  .sw-mark {
    width: 7px !important;
    height: 7px !important;
    border: 1.5px solid var(--ink-2);
    border-radius: 50%;
  }

  .out-h {
    margin-bottom: 10px;
  }
  table.outs {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--t-sm);
  }
  .outs th {
    font-size: var(--t-xs);
    font-weight: 500;
    color: var(--ink-3);
    text-align: left;
    padding: 0 0 6px;
    border-bottom: 1px solid var(--rule);
  }
  .outs td {
    padding: 7px 0;
    border-bottom: 1px solid var(--rule);
    font-variant-numeric: tabular-nums;
  }
  .outs td:first-child {
    display: flex;
    gap: 7px;
    align-items: center;
    font-weight: 560;
  }
  .outs tbody tr:hover {
    background: var(--sel);
  }
  .outs .r {
    text-align: right;
  }

  .ops {
    list-style: none;
    margin: 0;
    padding: 0;
    border-top: 1px solid var(--rule);
  }
  .ops li {
    display: grid;
    grid-template-columns: minmax(150px, 0.8fr) 150px minmax(0, 2fr) auto;
    gap: 6px 18px;
    align-items: baseline;
    padding: 9px 0;
    border-bottom: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .op-name {
    display: grid;
    gap: 1px;
    min-width: 0;
  }
  .nm {
    font-weight: 600;
  }
  .sub {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .op-state {
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .op-inc {
    color: var(--ink-2);
    min-width: 0;
  }
  .op-link {
    font-size: var(--t-xs);
    display: inline-flex;
    gap: 3px;
    align-items: center;
    white-space: nowrap;
  }
  .feedless {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin-top: 12px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    max-width: 80ch;
  }
  .feedless :global(svg) {
    flex: none;
    margin-top: 3px;
  }

  .tl {
    list-style: none;
    margin: 16px 0 0;
    padding: 0;
    border-left: 1px solid var(--rule-2);
  }
  .tl li {
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr);
    gap: 16px;
    padding: 0 0 22px 18px;
    position: relative;
  }
  .tl li::before {
    content: '';
    position: absolute;
    left: -4.5px;
    top: 6px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--bg);
    border: 1.5px solid var(--ink-2);
  }
  .tdate {
    font-size: var(--t-xs);
    color: var(--ink-3);
    padding-top: 2px;
    font-variant-numeric: tabular-nums;
  }
  .tbody {
    display: grid;
    gap: 4px;
    max-width: 80ch;
  }
  .tt {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    font-size: var(--t-xs);
  }
  .tprov {
    font-weight: 620;
  }
  .kind {
    padding: 0 7px;
    border-radius: 9px;
    border: 1px solid var(--rule-2);
    color: var(--ink-2);
    font-size: var(--t-2xs);
    line-height: 17px;
  }
  .kind.outage,
  .kind.ongoing {
    border-color: color-mix(in oklch, var(--critical) 55%, var(--rule));
  }
  .kind.routing,
  .kind.degradation {
    border-color: color-mix(in oklch, var(--warning) 60%, var(--rule));
  }
  .kind.unconfirmed {
    border-style: dashed;
  }
  .ttitle {
    font-weight: 600;
  }
  .tsum {
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .tmeta {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .tbody a {
    font-size: var(--t-xs);
    display: inline-flex;
    gap: 3px;
    align-items: center;
  }
  @media (max-width: 900px) {
    .verdict {
      padding: 4px 0 0;
    }
    .sheet {
      padding: 12px 14px;
    }
  }
  @media (max-width: 720px) {
    .ops li {
      grid-template-columns: minmax(0, 1fr) auto;
    }
    .op-inc {
      grid-column: 1 / -1;
    }
    .tl li {
      grid-template-columns: 1fr;
      gap: 4px;
    }
    .last {
      margin-left: 0;
    }
  }
</style>
