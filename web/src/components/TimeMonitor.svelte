<script lang="ts">
  // The Time page's "Over time" view: a light background check of a few time servers, kept for weeks, so what one run
  // can't show comes through: how steady each server is, whether it drops out, and whether this computer's clock wanders.
  import { app } from '../lib/store.svelte';
  import { RETENTION_DAYS, monitor } from '../lib/monitor.svelte';
  import type { NtpEntry, NtpMonitorConfig, NtpMonitorPoint, NtpMonitorResp, NtpMonitorServer } from '../lib/types';
  import { ago, ms, signedMs, when } from '../lib/format';
  import { MAX_CUSTOM, MAX_SERVERS, idMap, nameOf } from '../lib/ntp';
  import Icon from './Icon.svelte';

  let { catalog }: { catalog: NtpEntry[] } = $props();

  const W = 240;

  let days = $state(7);
  let customText = $state('');

  const mon = monitor<NtpMonitorConfig, NtpMonitorResp>({
    endpoint: '/api/ntp/monitor',
    query: () => `days=${days}`,
    saved: (c) => (c.enabled ? `Checking ${c.servers.length} time servers every ${c.interval_min} min while Recursa runs.` : 'Time monitoring paused.'),
    clearPrompt: 'Delete all time monitoring history? This can’t be undone.',
  });

  const choices = $derived(catalog.filter((e) => e.kind !== 'pool' && !e.nts_only));
  const byId = $derived(idMap(catalog));
  const typed = $derived((mon.cfg?.servers ?? []).filter((s) => !byId.has(s)));

  function toggle(id: string) {
    if (!mon.cfg) return;
    const on = mon.cfg.servers.includes(id);
    if (!on && mon.cfg.servers.length >= MAX_SERVERS) return app.notify(`At most ${MAX_SERVERS} servers are watched.`);
    mon.save({ ...mon.cfg, servers: on ? mon.cfg.servers.filter((x) => x !== id) : [...mon.cfg.servers, id] });
  }
  function addTyped() {
    const t = customText.trim();
    if (!mon.cfg || !t) return;
    if (typed.length >= MAX_CUSTOM) return app.notify(`At most ${MAX_CUSTOM} servers of your own.`);
    if (mon.cfg.servers.length >= MAX_SERVERS) return app.notify(`At most ${MAX_SERVERS} servers are watched.`);
    if (!mon.cfg.servers.includes(t)) mon.save({ ...mon.cfg, servers: [...mon.cfg.servers, t] });
    customText = '';
  }

  const upClass = (p: number) => (p >= 99 ? '' : p >= 90 ? 'some' : 'bad');
  const why = (k: string | null) =>
    ({ dns: 'its name didn’t resolve', no_address: 'no address', timeout: 'no answer', kiss: 'asked us to stop', unsynchronized: 'not synchronised', bad_reply: 'a reply that didn’t check out', network: 'the network refused' } as Record<string, string>)[k ?? ''] ??
    'no answer';

  /** A line over the window, broken wherever there is no value, scaled to fit. */
  function line(pts: NtpMonitorPoint[], pick: (p: NtpMonitorPoint) => number | null, since: number, until: number, lo: number, hi: number, h: number) {
    const span = Math.max(1, until - since);
    const y = (v: number) => (hi === lo ? h / 2 : h - 2 - ((v - lo) / (hi - lo)) * (h - 4));
    let d = '';
    let pen = false;
    for (const p of pts) {
      const v = pick(p);
      if (v == null) {
        pen = false;
        continue;
      }
      d += `${pen ? 'L' : 'M'}${(((p.ts - since) / span) * W).toFixed(1)} ${y(v).toFixed(1)}`;
      pen = true;
    }
    return d;
  }
  function band(pts: NtpMonitorPoint[], since: number, until: number, lo: number, hi: number, h: number) {
    const span = Math.max(1, until - since);
    const y = (v: number) => (hi === lo ? h / 2 : h - 2 - ((v - lo) / (hi - lo)) * (h - 4));
    return pts
      .filter((p) => p.lo_ms != null && p.hi_ms != null)
      .map((p) => {
        const x = ((p.ts - since) / span) * W;
        return `M${x.toFixed(1)} ${y(p.hi_ms!).toFixed(1)}V${y(p.lo_ms!).toFixed(1)}`;
      })
      .join('');
  }
  const fails = (pts: NtpMonitorPoint[], since: number, until: number) => {
    const span = Math.max(1, until - since);
    return pts.filter((p) => p.ok < p.sent).map((p) => ((p.ts - since) / span) * W);
  };
  const range = (vals: (number | null | undefined)[]) => {
    const v = vals.filter((x): x is number => x != null);
    return v.length ? ([Math.min(...v), Math.max(...v)] as const) : ([0, 0] as const);
  };
  function charts(s: NtpMonitorServer, since: number, until: number) {
    const [olo, ohi] = range(s.points.flatMap((p) => [p.lo_ms, p.hi_ms, p.offset_ms]));
    const [dlo, dhi] = range(s.points.map((p) => p.delay_ms));
    return {
      offLine: line(s.points, (p) => p.offset_ms, since, until, olo, ohi, 36),
      offBand: band(s.points, since, until, olo, ohi, 36),
      olo,
      ohi,
      delLine: line(s.points, (p) => p.delay_ms, since, until, Math.min(dlo, 0), dhi, 36),
      dhi,
      fails: fails(s.points, since, until),
    };
  }
  const hourMax = (s: NtpMonitorServer) => Math.max(1, ...s.stats.delay_by_hour.map((d) => d ?? 0));

  const h = $derived(mon.data?.history);
  const clock = $derived(h?.clock);
  const clockChart = $derived.by(() => {
    if (!h || !clock || clock.points.length < 2) return null;
    const lo = Math.min(...clock.points.map((p) => p.median_ms));
    const hi = Math.max(...clock.points.map((p) => p.median_ms));
    const span = Math.max(1, h.until - h.since);
    const y = (v: number) => (hi === lo ? 28 : 54 - ((v - lo) / (hi - lo)) * 52);
    return {
      d: clock.points.map((p, i) => `${i ? 'L' : 'M'}${(((p.ts - h.since) / span) * W).toFixed(1)} ${y(p.median_ms).toFixed(1)}`).join(''),
      lo,
      hi,
    };
  });
  const empty = $derived(!!h && h.cycles === 0);
</script>

<div class="content scroll wrap">
  {#if mon.cfg}
    <div class="mcfg panel">
      <label class="toggle">
        <input type="checkbox" aria-label="Watch these time servers in the background" checked={mon.cfg.enabled} disabled={mon.saving} onchange={(e) => mon.save({ ...mon.cfg!, enabled: (e.currentTarget as HTMLInputElement).checked })} />
        <span><b>Watch in the background</b> while Recursa is running</span>
      </label>
      <label class="inline">
        Every
        <select value={mon.cfg.interval_min} onchange={(e) => mon.save({ ...mon.cfg!, interval_min: Number((e.currentTarget as HTMLSelectElement).value) })}>
          {#each [15, 30, 60, 120, 360] as m}<option value={m}>{m < 60 ? `${m} min` : `${m / 60} h`}</option>{/each}
        </select>
      </label>
      <label class="inline">
        Keep
        <select value={mon.cfg.retention_days} onchange={(e) => mon.save({ ...mon.cfg!, retention_days: Number((e.currentTarget as HTMLSelectElement).value) })}>
          {#each RETENTION_DAYS as d}<option value={d}>{d} days</option>{/each}
        </select>
      </label>
      <details class="pickers">
        <summary>{mon.cfg.servers.length} server{mon.cfg.servers.length === 1 ? '' : 's'} watched</summary>
        <div class="chips">
          {#each choices as e (e.id)}
            {@const on = mon.cfg.servers.includes(e.id)}
            <button class="chip" class:on aria-pressed={on} disabled={mon.saving} onclick={() => toggle(e.id)}>{e.name}</button>
          {/each}
          {#each typed as t (t)}
            <button class="chip on" aria-pressed="true" disabled={mon.saving} title="A server you typed. Click to stop watching it." onclick={() => toggle(t)}>{t} ×</button>
          {/each}
        </div>
        <form class="custom" onsubmit={(ev) => (ev.preventDefault(), addTyped())}>
          <input bind:value={customText} placeholder="Your own server: host or host:port" aria-label="A time server of your own" spellcheck="false" autocapitalize="off" autocomplete="off" />
          <button class="btn sm" type="submit" disabled={mon.saving || !customText.trim()}>Add</button>
        </form>
      </details>
    </div>
    <p class="hint">
      Each check sends one packet to each address of each server, never more often than every 15 minutes, through the same
      record of who was asked as a run: a server that asked us to stop is left alone here too. It pauses during manual runs{mon.data?.last ? `; last check ${ago(mon.data.last)}` : ''}.
    </p>
  {/if}

  <div class="bar">
    <div class="segmented" role="tablist" aria-label="Window">
      {#each [1, 7, 30] as d}
        <button role="tab" aria-selected={days === d} onclick={() => (days = d)}>{d === 1 ? '24 hours' : `${d} days`}</button>
      {/each}
    </div>
    {#if h && h.cycles}
      <button class="btn ghost sm danger" onclick={mon.clearHistory}><Icon name="trash" size={13} />Clear history</button>
    {/if}
  </div>

  {#if empty}
    <div class="empty">
      <p><b>Nothing recorded {days === 1 ? 'in the last 24 hours' : `in the last ${days} days`}.</b></p>
      <p class="ink-2">
        {#if mon.cfg?.enabled}The first check runs within a few seconds of the interval; this page updates when it has.{:else}Switch on “Watch in the background” above. Recursa checks while it is open, so a long record needs it left running.{/if}
      </p>
    </div>
  {:else if h && clock}
    <section class="panel clock">
      <h3>This computer’s clock</h3>
      {#if clockChart}
        <div class="cc">
          <svg viewBox="0 0 {W} 56" preserveAspectRatio="none" role="img" aria-label="The median offset of the servers over the window, from {signedMs(clockChart.lo, true)} to {signedMs(clockChart.hi, true)}">
            <path d={clockChart.d} fill="none" stroke="var(--ink)" stroke-width="1.5" vector-effect="non-scaling-stroke" />
          </svg>
          <span class="axis num"><span>{signedMs(clockChart.hi, true)}</span><span>{signedMs(clockChart.lo, true)}</span></span>
        </div>
      {/if}
      <dl class="kv">
        <div><dt>Typical offset</dt><dd class="num">{signedMs(clock.median_ms, true)}</dd></div>
        <div><dt>Wanders by</dt><dd class="num" title="How far the middle half of the checks spread.">{ms(clock.spread_ms)}</dd></div>
        <div>
          <dt>Trend</dt>
          <dd class="num" title="Least-squares slope of the checks' medians; needs six hours and eight checks.">
            {clock.trend_ms_per_hour == null ? 'not enough record' : `${signedMs(clock.trend_ms_per_hour, true)} per hour`}
          </dd>
        </div>
        <div><dt>Checks</dt><dd class="num">{clock.cycles}</dd></div>
      </dl>
      <p class="fine">
        One line for every check: the median of the servers that could vote. A steady line near zero means the clock keeps time; a
        slope means it drifts between corrections. A check with fewer than three independent servers gives no point.
      </p>
    </section>

    <div class="hist panel">
      <div class="hrow hhead" aria-hidden="true">
        <span>Server</span><span title="Share of checks answered">Answered</span><span title="Median offset, and how far the middle half spread">Offset</span><span title="Median round trip, and the 95th percentile">Delay</span>
        <span class="charts">Offset · delay, oldest left</span>
      </div>
      {#each h.servers as s (s.id)}
        {@const c = charts(s, h.since, h.until)}
        <div class="hrow">
          <span class="hn">
            {nameOf(byId, s.id)}
            {#if s.stats.falseticks}<span class="tag" title="Checks in which its interval fell outside the majority's.">off {s.stats.falseticks} of {s.stats.voted}</span>{/if}
          </span>
          <span class="num {upClass(s.stats.up_pct)}">{s.stats.up_pct.toFixed(s.stats.up_pct === 100 ? 0 : 1)}%</span>
          <span class="num">{signedMs(s.stats.offset_median_ms, true)}{#if s.stats.offset_spread_ms != null}<small>±{ms(s.stats.offset_spread_ms / 2, false)}</small>{/if}</span>
          <span class="num">{ms(s.stats.delay_median_ms, false)}{#if s.stats.delay_p95_ms != null}<small>/ {ms(s.stats.delay_p95_ms, false)}</small>{/if}</span>
          <span class="charts">
            <svg viewBox="0 0 {W} 36" preserveAspectRatio="none" role="img" aria-label="{nameOf(byId, s.id)} offset over the window, {signedMs(c.olo, true)} to {signedMs(c.ohi, true)}">
              <path d={c.offBand} stroke="color-mix(in oklch, var(--ink) 14%, transparent)" stroke-width="3" fill="none" vector-effect="non-scaling-stroke" />
              <path d={c.offLine} fill="none" stroke="var(--ink)" stroke-width="1.25" vector-effect="non-scaling-stroke" />
              {#each c.fails as x}<path d="M{x.toFixed(1)} 30V36" stroke="var(--critical)" stroke-width="2" vector-effect="non-scaling-stroke" />{/each}
            </svg>
            <svg viewBox="0 0 {W} 36" preserveAspectRatio="none" role="img" aria-label="{nameOf(byId, s.id)} delay over the window, up to {ms(c.dhi)}">
              <path d={c.delLine} fill="none" stroke="var(--ink-2)" stroke-width="1.25" vector-effect="non-scaling-stroke" />
            </svg>
          </span>
        </div>
        {#if s.stats.last_ok === false}
          <p class="note"><Icon name="warn" size={12} /> Last check {when(s.stats.last_ts)}: {why(s.stats.last_kind)}.{s.stats.flaps > 1 ? ` It has gone quiet or come back ${s.stats.flaps} times in this window.` : ''}</p>
        {:else if s.stats.flaps > 1}
          <p class="note"><Icon name="warn" size={12} /> Went quiet or came back {s.stats.flaps} times in this window.</p>
        {/if}
        <details class="byhour">
          <summary>Delay by time of day</summary>
          <div class="hours" role="img" aria-label="Median delay for each hour of the day, UTC">
            {#each s.stats.delay_by_hour as d, i}
              <span class="hb" title="{String(i).padStart(2, '0')}:00 UTC · {d == null ? 'no answers' : ms(d)}"><i style:height="{d == null ? 0 : Math.max(4, (d / hourMax(s)) * 100)}%"></i></span>
            {/each}
          </div>
          <p class="fine">Hours are UTC, so the shape moves with your own busy hours. A delay that climbs every evening is the path getting busy, not the server.</p>
        </details>
      {/each}
    </div>
  {/if}
</div>

<style>
  .wrap > :global(*) {
    max-width: 1280px;
  }
  .mcfg {
    display: flex;
    flex-wrap: wrap;
    gap: 14px 22px;
    align-items: center;
    padding: 12px 16px;
  }
  .toggle {
    display: flex;
    gap: 9px;
    align-items: center;
    font-size: var(--t-sm);
  }
  .inline {
    display: inline-flex;
    gap: 8px;
    align-items: center;
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .pickers {
    flex-basis: 100%;
    font-size: var(--t-sm);
  }
  .pickers summary {
    cursor: default;
    color: var(--ink-2);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
  }
  .chip {
    border: 1px solid var(--rule-2);
    background: var(--surface);
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
  .custom {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    max-width: 420px;
  }
  .custom input {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--rule-2);
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: var(--t-sm);
  }
  .hint,
  .fine {
    color: var(--ink-3);
    font-size: var(--t-xs);
    line-height: 1.5;
    max-width: 76ch;
  }
  .bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin: 14px 0 12px;
  }
  .panel {
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--surface);
  }
  .clock {
    padding: 12px 16px 14px;
    margin-bottom: 14px;
  }
  .clock h3 {
    margin: 0 0 8px;
    font-size: var(--t-md);
  }
  .cc {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 72px;
    gap: 10px;
    height: 56px;
  }
  .cc svg {
    width: 100%;
    height: 56px;
    display: block;
    border-bottom: 1px solid var(--rule);
  }
  .axis {
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    font-size: var(--t-xs);
    color: var(--ink-3);
    text-align: right;
  }
  .kv {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 28px;
    margin: 10px 0 6px;
  }
  .kv dt {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .kv dd {
    margin: 0;
    font-size: var(--t-md);
  }
  .hist {
    padding: 8px 16px 12px;
  }
  .hrow {
    display: grid;
    grid-template-columns: 200px 72px 110px 100px minmax(0, 1fr);
    gap: 12px;
    align-items: center;
    padding: 8px 0;
    border-top: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .hhead {
    border-top: 0;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .hrow small {
    color: var(--ink-3);
    margin-left: 4px;
  }
  .hn {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tag {
    margin-left: 6px;
    padding: 0 6px;
    border-radius: 8px;
    background: var(--warn-soft);
    font-size: var(--t-xs);
  }
  .some {
    color: var(--warning);
  }
  .bad {
    color: var(--critical);
  }
  .charts {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    min-width: 0;
  }
  .hhead .charts {
    display: block;
  }
  .charts svg {
    width: 100%;
    height: 36px;
    background: var(--surface-2);
    border-radius: var(--r-sm);
  }
  .note {
    display: flex;
    gap: 6px;
    align-items: center;
    margin: 0 0 6px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .byhour {
    font-size: var(--t-xs);
    color: var(--ink-3);
    padding-bottom: 4px;
  }
  .byhour summary {
    cursor: default;
  }
  .hours {
    display: grid;
    grid-template-columns: repeat(24, 1fr);
    gap: 2px;
    height: 44px;
    margin: 8px 0 4px;
    align-items: end;
  }
  .hb {
    display: flex;
    align-items: flex-end;
    height: 100%;
    background: var(--surface-2);
  }
  .hb i {
    display: block;
    width: 100%;
    background: var(--ink-3);
  }
  .empty {
    padding: 28px 4px;
  }
  @media (max-width: 900px) {
    .hrow {
      grid-template-columns: 1fr 64px 96px 90px;
    }
    .hrow .charts,
    .hhead .charts {
      grid-column: 1 / -1;
    }
  }
</style>
