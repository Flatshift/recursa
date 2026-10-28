<script lang="ts">
  // The Network page's "Over time" view: a light background probe of the two routers either side of your ISP's border,
  // the last one inside it and the first one beyond, towards each CDN. When only the far one slows at the same hours
  // each day, the queue is on the link between them. Kept for weeks, so the daily shape shows.
  import { app } from '../lib/store.svelte';
  import { RETENTION_DAYS, monitor } from '../lib/monitor.svelte';
  import type { BorderPairHistory, BorderPoint, BorderMonitorConfig, BorderResp } from '../lib/types';
  import { ago, holder, ms, when } from '../lib/format';
  import CdnLogo from './CdnLogo.svelte';
  import Icon from './Icon.svelte';

  const W = 240;
  // Up to 30 minutes: a verdict wants at least two probes in an hour.
  const INTERVALS = [5, 10, 15, 30];

  let hours = $state(72);

  const mon = monitor<BorderMonitorConfig, BorderResp>({
    endpoint: '/api/network/border',
    query: () => `hours=${hours}`,
    saved: (c) => (c.enabled ? `Probing the border links every ${c.interval_min} min while Recursa runs.` : 'Border probing paused.'),
    onsaved: (c) => {
      if (app.boot) app.boot = { ...app.boot, border_monitor: c };
    },
    clearPrompt: 'Delete all border link history? This can’t be undone.',
  });

  const cap = $derived(mon.data?.cap ?? app.env?.trace ?? 'unsupported');
  const needsAdmin = $derived(cap === 'needs_admin');
  const unsupported = $derived(cap === 'unsupported');
  const h = $derived(mon.data?.history);
  const empty = $derived(!!h && h.cycles === 0);
  const home = $derived(h?.home.holder ? holder(h.home.holder) : h?.home.asn ? `AS${h.home.asn}` : 'your ISP');

  /** A line over the window, broken wherever there is no value, scaled to fit. */
  function line(pts: BorderPoint[], pick: (p: BorderPoint) => number | null, since: number, until: number, lo: number, hi: number, hgt: number) {
    const span = Math.max(1, until - since);
    const y = (v: number) => (hi === lo ? hgt / 2 : hgt - 2 - ((v - lo) / (hi - lo)) * (hgt - 4));
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
  function chart(ph: BorderPairHistory, since: number, until: number) {
    const vals = ph.points.flatMap((p) => [p.near_ms, p.far_ms]).filter((v): v is number => v != null);
    const lo = vals.length ? Math.min(0, ...vals) : 0;
    const hi = vals.length ? Math.max(...vals) : 0;
    return {
      near: line(ph.points, (p) => p.near_ms, since, until, lo, hi, 48),
      far: line(ph.points, (p) => p.far_ms, since, until, lo, hi, 48),
      hi,
      gaps: ph.points.filter((p) => p.far_ms == null && p.n > 0).map((p) => ((p.ts - since) / Math.max(1, until - since)) * W),
    };
  }
  const hour = (u: number) => `${String(u).padStart(2, '0')}:00`;
  const hourList = (hs: number[]) => {
    if (!hs.length) return '';
    const s = [...hs].sort((a, b) => a - b);
    // Runs of consecutive hours read as a range: 19:00 to 22:00 UTC.
    const runs: string[] = [];
    let a = s[0];
    let b = s[0];
    for (const x of s.slice(1)) {
      if (x === b + 1) b = x;
      else {
        runs.push(a === b ? hour(a) : `${hour(a)} to ${hour(b + 1)}`);
        a = b = x;
      }
    }
    runs.push(a === b ? hour(a) : `${hour(a)} to ${hour(b + 1)}`);
    return `${runs.join(', ')} UTC`;
  };
  function verdict(ph: BorderPairHistory): { text: string; tone: 'wait' | 'ok' | 'warn' } {
    const v = ph.verdict;
    const far = ph.pair.far_holder ? holder(ph.pair.far_holder) : ph.pair.far_asn ? `AS${ph.pair.far_asn}` : 'the next network';
    switch (v.kind) {
      case 'collecting':
        return { text: `Collecting, ${v.hours} of 24 h. A verdict needs a full day of hourly samples.`, tone: 'wait' };
      case 'steady':
        return { text: `Steady. Neither router slows at a regular hour, so this link carries the evening without a queue.`, tone: 'ok' };
      case 'interconnect':
        return { text: `The link from ${home} to ${far} adds ${ms(v.adds_ms)} at peak (${hourList(v.hours)}). The router inside your ISP stays flat, so the queue sits on the link between them.`, tone: 'warn' };
      case 'own_link':
        return { text: `Your own link adds ${ms(v.adds_ms)} at peak. Both routers rise together, so the queue is before the border: your line, or your ISP's own network.`, tone: 'warn' };
    }
  }
  const kindText = (ph: BorderPairHistory) => {
    const far = ph.pair.far_holder ? holder(ph.pair.far_holder) : ph.pair.far_asn ? `AS${ph.pair.far_asn}` : 'an unnamed network';
    return ph.pair.kind === 'direct' ? `${home} to ${far}, reached directly` : `${home} to ${far}, on the way to ${name(ph.pair.cdn)}`;
  };
  /** The CDN as the roll call names it; the benchmark's own labels ("anycast control") are about steering, not this. */
  const SHORT: Record<string, string> = { cloudflare: 'Cloudflare', fastly: 'Fastly', cloudfront: 'CloudFront', google: 'Google', meta: 'Meta', akamai: 'Akamai' };
  const name = (cdn: string) => SHORT[cdn] ?? app.boot?.method.cdns.find((c) => c.id === cdn)?.name ?? cdn;
</script>

<div class="content scroll wrap">
  {#if unsupported}
    <div class="empty panel">
      <p><b>Border probing is not available in this build.</b></p>
      <p class="ink-2">It needs the path tracer, which this platform does not offer. The "Served from" check works everywhere.</p>
    </div>
  {:else}
    {#if mon.cfg}
      <div class="mcfg panel">
        <label class="toggle">
          <input
            type="checkbox"
            aria-label="Probe the border links in the background"
            checked={mon.cfg.enabled}
            disabled={mon.saving || needsAdmin}
            onchange={(e) => mon.save({ ...mon.cfg!, enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          <span><b>Probe in the background</b> while Recursa is running</span>
        </label>
        <label class="inline">
          Every
          <select value={mon.cfg.interval_min} disabled={needsAdmin} onchange={(e) => mon.save({ ...mon.cfg!, interval_min: Number((e.currentTarget as HTMLSelectElement).value) })}>
            {#each INTERVALS as m}<option value={m}>{m} min</option>{/each}
          </select>
        </label>
        <label class="inline">
          Keep
          <select value={mon.cfg.retention_days} disabled={needsAdmin} onchange={(e) => mon.save({ ...mon.cfg!, retention_days: Number((e.currentTarget as HTMLSelectElement).value) })}>
            {#each RETENTION_DAYS as d}<option value={d}>{d} days</option>{/each}
          </select>
        </label>
        {#if needsAdmin}
          <p class="admin"><Icon name="lock" size={13} />Needs administrator rights on {app.env?.platform === 'windows' ? 'Windows' : 'this computer'}. Start Recursa as an administrator to switch it on.</p>
        {/if}
      </div>
      <p class="hint">
        Each cycle sends two short probes towards each CDN edge: one that stops at the last router inside your ISP, one at the first
        router beyond it. When only the second slows down at the same hours each day, the queue is on the link between them. The
        pair is found once a day with one path trace{mon.data?.last ? `; last probe ${ago(mon.data.last)}` : ''}.
      </p>
    {/if}

    <div class="bar">
      <div class="segmented" role="tablist" aria-label="Window">
        {#each [24, 72, 168] as hs}
          <button role="tab" aria-selected={hours === hs} onclick={() => (hours = hs)}>{hs === 24 ? '24 hours' : `${hs / 24} days`}</button>
        {/each}
      </div>
      {#if h && h.cycles}
        <button class="btn ghost sm danger" onclick={mon.clearHistory}><Icon name="trash" size={13} />Clear history</button>
      {/if}
    </div>

    {#if !mon.data}
      <div class="card panel" aria-busy="true">
        <div class="ch"><span class="skel now w40"></span></div>
        <div class="skel now chart"></div>
      </div>
    {:else if empty}
      <div class="empty">
        <p><b>Nothing recorded {hours === 24 ? 'in the last 24 hours' : `in the last ${hours / 24} days`}.</b></p>
        <p class="ink-2">
          {#if needsAdmin}Border probing needs administrator rights here, so there is nothing to show.{:else if mon.cfg?.enabled}The first cycle finds the border pairs with a path trace, then probes within the interval; this page updates when it has.{:else}Switch on "Probe in the background" above. Recursa probes while it is open, so a day's record needs it left running.{/if}
        </p>
      </div>
    {:else if h}
      {#each h.pairs as ph (ph.pair.id)}
        {@const c = chart(ph, h.since, h.until)}
        {@const v = verdict(ph)}
        <section class="card panel">
          <div class="ch">
            <span class="cdn"><CdnLogo id={ph.pair.cdn} name={name(ph.pair.cdn)} />{name(ph.pair.cdn)}</span>
            <span class="pair" title="Inside: {ph.pair.near}. Beyond: {ph.pair.far}. Found {when(ph.pair.found_at)}.">{kindText(ph)}</span>
          </div>
          <div class="cc">
            <svg viewBox="0 0 {W} 48" preserveAspectRatio="none" role="img" aria-label="Round trips to the router inside {home} and the first beyond it, over the window, up to {ms(c.hi)}">
              <path d={c.near} fill="none" stroke="var(--ink-3)" stroke-width="1.25" vector-effect="non-scaling-stroke" />
              <path d={c.far} fill="none" stroke="var(--accent)" stroke-width="1.5" vector-effect="non-scaling-stroke" />
              {#each c.gaps as x}<path d="M{x.toFixed(1)} 42V48" stroke="var(--critical)" stroke-width="2" vector-effect="non-scaling-stroke" />{/each}
            </svg>
            <span class="axis num"><span>{ms(c.hi, false)} ms</span><span>0</span></span>
          </div>
          <p class="verdict {v.tone}">{v.text}</p>
          <p class="fine">
            <span class="leg"><i class="near"></i>inside {home}</span>
            <span class="leg"><i class="far"></i>first router beyond</span>
            {#if ph.last}
              <span>Last probe {ago(ph.last.ts)}: {ph.last.near_ms == null ? 'inside silent' : `inside ${ms(ph.last.near_ms)}`}, {ph.last.far_ms == null ? 'beyond silent' : `beyond ${ms(ph.last.far_ms)}`}.</span>
            {/if}
          </p>
        </section>
      {/each}
      {#each h.missing as m (m.cdn)}
        <div class="card panel thin">
          <span class="cdn"><CdnLogo id={m.cdn} name={name(m.cdn)} />{name(m.cdn)}</span>
          <span class="ink-2">Border not found: {m.reason}. It is looked for again at the next discovery.</span>
        </div>
      {/each}
      <p class="fine foot">
        Hours are UTC, so a peak moves with your own evening. A rise that only the far router shows is evidence of a busy link
        between the two networks, not proof: the far router may also answer probes slowly on its own. A rise on both sides is before
        the border.
      </p>
    {/if}
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
  .admin {
    flex-basis: 100%;
    margin: 0;
    display: inline-flex;
    gap: 6px;
    align-items: center;
    font-size: var(--t-xs);
    color: var(--ink-2);
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
  .empty {
    padding: 28px 4px;
  }
  .empty.panel {
    padding: 28px 24px;
  }
  .empty p {
    margin: 0 0 6px;
    font-size: var(--t-sm);
  }
  .card {
    padding: 12px 16px 12px;
    margin-bottom: 12px;
  }
  .card.thin {
    display: flex;
    gap: 12px;
    align-items: center;
    font-size: var(--t-sm);
  }
  .ch {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    align-items: baseline;
    margin-bottom: 8px;
  }
  .cdn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-weight: 560;
    font-size: var(--t-sm);
  }
  .pair {
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .cc {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 72px;
    gap: 10px;
    height: 48px;
  }
  .cc svg {
    width: 100%;
    height: 48px;
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
  .verdict {
    margin: 10px 0 4px;
    font-size: var(--t-sm);
    line-height: 1.5;
    max-width: 90ch;
  }
  .verdict.wait {
    color: var(--ink-2);
  }
  .verdict.warn {
    color: var(--ink);
  }
  .verdict.warn::before,
  .verdict.ok::before {
    content: '';
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    margin-right: 7px;
    vertical-align: 1px;
    background: var(--warning);
  }
  .verdict.ok::before {
    background: var(--good);
  }
  .fine {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 2px 14px;
  }
  .fine.foot {
    display: block;
    max-width: 110ch;
  }
  .leg {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .leg i {
    display: inline-block;
    width: 14px;
    height: 2px;
    background: var(--ink-3);
  }
  .leg i.far {
    background: var(--accent);
  }
  .skel {
    display: inline-block;
    height: 12px;
    border-radius: 3px;
    background: var(--surface-3);
  }
  .skel.chart {
    display: block;
    height: 48px;
    width: 100%;
  }
  .w40 {
    width: 40%;
  }
</style>
