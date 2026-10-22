<script lang="ts" module>
  import type { DeepChecks } from '../lib/types';

  /** Short phrases for the real problems in a set of behaviour checks; empty when it found none. */
  export function faults(res: DeepChecks): string[] {
    const out: string[] = [];
    for (const s of res.truncation) {
      if (!s.answered) out.push(s.transport === 'TCP' ? 'No answer over TCP' : 'Dropped a large answer');
      else if (s.buffer != null && s.size_bytes != null && s.size_bytes > s.buffer) out.push('Ignored the EDNS buffer');
    }
    if (res.ttl.some((t) => t.kind !== 'long' && t.resolver != null && t.authoritative != null && t.resolver > t.authoritative)) out.push('Raises short TTLs');
    if (res.dnssec.some((c) => c.expect_valid === true && c.ad)) {
      if (res.dnssec.some((c) => c.expect_valid === false && c.rcode != null && (c.ad || c.rcode !== 'SERVFAIL'))) out.push('Served a bad DNSSEC answer');
    }
    return [...new Set(out)];
  }
</script>

<script lang="ts">
  import type { CacheAsk, DnssecCase, SizeStep } from '../lib/types';
  import { ms } from '../lib/format';
  import Icon from './Icon.svelte';

  let { rid, res }: { rid: string; res: DeepChecks | null } = $props();

  /** A TTL in seconds, in the largest unit that keeps it readable. */
  function ttl(s: number | null | undefined): string {
    if (s == null) return '—';
    const fmt = (v: number, u: string) => `${Number.isInteger(v) ? v : v.toFixed(1)} ${u}`;
    if (s < 120) return `${s} s`;
    if (s < 7200) return fmt(s / 60, 'min');
    if (s < 172_800) return fmt(s / 3600, 'h');
    return fmt(s / 86_400, 'd');
  }

  type Verdict = { ok: boolean | null; text: string };

  /** What each truncation step shows: true is the expected behaviour, false a fault, null neither. */
  function sizeVerdict(s: SizeStep): Verdict {
    if (!s.answered) {
      return { ok: false, text: s.transport === 'TCP' ? 'No answer over TCP, so large answers fail' : 'No answer: dropped rather than truncated' };
    }
    if (s.buffer != null && s.size_bytes! > s.buffer) return { ok: false, text: `Ignored the ${s.buffer}-byte buffer` };
    // Whether the answer should fit comes from its full size over TCP, since the root key set grows during a key rollover.
    const full = fullSize;
    if (s.tc) {
      if (full == null) return { ok: null, text: 'Truncated, TC set: retry over TCP' };
      return full > s.buffer! ? { ok: true, text: 'Truncated, TC set: retry over TCP' } : { ok: null, text: 'Truncated though it fits, so this needs TCP' };
    }
    return { ok: true, text: s.transport === 'TCP' ? 'Full answer over TCP' : 'Full answer over UDP' };
  }

  const fullSize = $derived(res?.truncation.find((s) => s.transport === 'TCP' && s.answered)?.size_bytes ?? null);

  const sizeScale = $derived(Math.max(1232, ...(res?.truncation.map((s) => s.size_bytes ?? 0) ?? [])));

  const validates = $derived(!!res?.dnssec.some((c) => c.expect_valid === true && c.ad));

  function dnssecVerdict(c: DnssecCase): Verdict & { result: string } {
    const result =
      c.rcode == null ? 'No answer' : c.ad ? 'Validated' : c.rcode === 'SERVFAIL' ? 'Rejected (SERVFAIL)' : `Not validated (${c.rcode})`;
    if (c.rcode == null || !validates) return { ok: null, result, text: '' };
    if (c.expect_valid === null) return { ok: null, result, text: c.ad ? 'Supported' : 'Treated as unsigned' };
    const right = c.expect_valid ? c.ad : c.rcode === 'SERVFAIL';
    return {
      ok: right,
      result,
      text: right ? (c.expect_valid ? 'Accepted' : 'Refused the bad answer') : c.expect_valid ? 'Should have validated' : 'Served a bad answer',
    };
  }

  const negReading = $derived.by(() => {
    const n = res?.negative;
    if (!n || n.first == null) return '';
    const gap = n.gap_s.toFixed(0);
    if (n.later == null) return `The second ask, ${gap} s later, got no answer.`;
    if (n.later < n.first) return `After ${gap} s the TTL had counted down to ${n.later} s, so the "doesn't exist" answer was kept in cache.`;
    const fast = n.first_ms != null && n.later_ms != null && n.later_ms < n.first_ms * 0.6;
    return fast
      ? `After ${gap} s the TTL was still ${ttl(n.first)}, but the answer came back much faster (${ms(n.later_ms)} against ${ms(n.first_ms)}), so it likely came from a cache that doesn't count negative TTLs down.`
      : `After ${gap} s the TTL was still ${ttl(n.first)} and the answer took as long (${ms(n.later_ms)} against ${ms(n.first_ms)}). The resolver likely asked again, or another server behind the address answered.`;
  });

  /** The cache square last tapped or clicked, as `resolver:detail`. */
  let picked = $state<string | null>(null);

  function askText(a: CacheAsk): string {
    return a.ttl == null ? 'no answer' : `${a.fetched ? 'fetched upstream' : 'from cache'}, TTL ${a.ttl} s, ${ms(a.ms)}`;
  }

  const udp = $derived(res?.transports.find((t) => t.transport === 'UDP'));
  const costScale = $derived(Math.max(1, ...(res?.transports.map((t) => (t.setup_ms ?? 0) + (t.first_ms ?? 0)) ?? [])));
</script>

{#if res}
  <div class="checks">
    {#each res.notes as n}<p class="note">{n}</p>{/each}

    {#if res.truncation.length}
      <h4>Large answers and TCP fallback</h4>
      <div class="sizes">
        {#each res.truncation as s}
          {@const v = sizeVerdict(s)}
          <span class="lab">{s.transport}{s.buffer != null ? `, ${s.buffer}-byte buffer` : ''}</span>
          <span class="bar" aria-hidden="true">
            {#if s.size_bytes}<span class="fill" style:width="{(100 * s.size_bytes) / sizeScale}%"></span>{/if}
            {#if s.buffer != null}<span class="limit" style:left="{(100 * s.buffer) / sizeScale}%"></span>{/if}
          </span>
          <span class="num">{s.size_bytes != null ? `${s.size_bytes} B` : '—'}</span>
          <span class="verdict"
            ><span class="g" class:good={v.ok === true} class:bad={v.ok === false}><Icon name={v.ok === true ? 'check' : v.ok === false ? 'cross' : 'dash'} size={13} /></span>{v.text}{#if s.ad}<span
                class="muted"> · AD</span
              >{/if}</span
          >
        {/each}
      </div>
      <p class="why">
        Asks for the root zone's signed key set{#if fullSize}, {fullSize} bytes over TCP{/if}. When the answer is larger than the buffer,
        the resolver should send a short reply with TC set, telling the client to retry over TCP; when it fits, it should come
        whole. A resolver that drops the answer instead, or sends more than the buffer allows, leaves clients waiting on lost or
        fragmented packets. The tick marks the buffer.
      </p>
    {/if}

    {#if res.ttl.length}
      <h4>TTL limits</h4>
      <table class="grid">
        <thead><tr><th>Record</th><th>Zone's TTL</th><th>Resolver's</th><th></th></tr></thead>
        <tbody>
          {#each res.ttl as t}
            <tr>
              <td>{t.kind === 'long' ? 'Long-lived' : 'Short-lived'} <span class="mono muted">{t.qtype}</span></td>
              <td class="num">{ttl(t.authoritative)}</td>
              <td class="num">{ttl(t.resolver)}</td>
              <td>
                {#if t.authoritative == null || t.resolver == null}<span class="muted">Can't compare</span>
                {:else if t.resolver < t.authoritative - 2}Capped at {ttl(t.resolver)}
                {:else if t.resolver > t.authoritative}Raised to {ttl(t.resolver)}
                {:else}Kept as set{/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="why">
        A made-up name under a zone that gives every name a three-day TTL, so the answer is fresh and shows the longest the resolver
        keeps anything, and a 20-second record, which shows whether it raises short TTLs. A cap is normal and means answers are
        refreshed sooner. Raising a short TTL keeps answers past the time their owner set, which delays changes like failovers.
      </p>
    {/if}

    {#if res.negative}
      {@const n = res.negative}
      <h4>Negative caching <span class="tag">Inferred</span></h4>
      <dl class="stats">
        <div><dt>Zone's negative TTL</dt><dd class="num">{ttl(n.authoritative)}</dd></div>
        <div><dt>Resolver's</dt><dd class="num">{ttl(n.first)}</dd></div>
        <div><dt>{n.gap_s.toFixed(0)} s later</dt><dd class="num">{ttl(n.later)}</dd></div>
      </dl>
      <p>
        {#if n.first == null}No "doesn't exist" answer came back{n.rcode ? ` (${n.rcode})` : ''}, so there's nothing to compare.
        {:else}
          {#if n.authoritative != null}{n.first < n.authoritative - 2
              ? `The resolver keeps "doesn't exist" answers for at most ${ttl(n.first)}, less than the zone's ${ttl(n.authoritative)}.`
              : n.first > n.authoritative
                ? `The resolver keeps "doesn't exist" answers longer than the zone asks, ${ttl(n.first)} against ${ttl(n.authoritative)}.`
                : `The resolver keeps "doesn't exist" answers as long as the zone asks.`}{/if}
          {negReading}
        {/if}
      </p>
      <p class="why">
        A made-up name in a zone with a two-hour negative TTL, asked at the start and again at the end. A short negative cache means a
        newly created name starts working sooner.
      </p>
    {/if}

    {#if res.dnssec.length}
      <h4>DNSSEC cases</h4>
      {#if !validates}<p class="note">This resolver didn't validate the correctly signed names, so it doesn't check DNSSEC. The results below are for reference.</p>{/if}
      <table class="grid">
        <thead><tr><th>Case</th><th>Answer</th><th></th></tr></thead>
        <tbody>
          {#each res.dnssec as c}
            {@const v = dnssecVerdict(c)}
            <tr>
              <td>{c.label}<span class="mono muted sub">{c.name}</span></td>
              <td
                >{v.result}{#each c.ede as e}<span class="muted sub" title={e.text}>EDE {e.code} {e.name}</span>{/each}</td
              >
              <td class="verdict"
                >{#if v.text}<span class="g" class:good={v.ok === true} class:bad={v.ok === false}><Icon name={v.ok === true ? 'check' : v.ok === false ? 'cross' : 'dash'} size={13} /></span>{v.text}{/if}</td
              >
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="why">
        Names kept broken on purpose: a validating resolver should answer the correctly signed ones with AD set and refuse the
        broken ones with SERVFAIL. Ed448 is new enough that a resolver may treat it as unsigned, which is allowed.
      </p>
    {/if}

    {#if res.caches}
      {@const c = res.caches}
      <h4>Caches behind the address <span class="tag">Inferred</span></h4>
      <p>
        {#if c.caches == null}The TTLs didn't count down, so a fresh fetch can't be told from a cached answer.
        {:else if c.caches >= c.rounds}Every ask of the same name was fetched again: at least {c.caches} separate caches answer here,
          likely many more. Expect cold lookups more often than with a single cache.
        {:else if c.caches > 1}At least {c.caches} separate caches answer here: one name was fetched upstream {c.caches} times in
          {c.rounds} asks.
        {:else}One cache: after the first ask, every answer came from cache.{/if}
      </p>
      <div class="rounds">
        {#each c.rows as row, i}
          <span class="lab">Name {i + 1}</span>
          <span class="cells">
            {#each row.asks as a, j}
              {@const detail = `Name ${i + 1}, ask ${j + 1}: ${askText(a)}`}
              <button
                type="button"
                class="cell"
                class:b={a.fetched}
                class:a={!a.fetched && a.ttl != null}
                class:on={picked === `${rid}:${detail}`}
                title={detail}
                aria-label={detail}
                onclick={() => (picked = `${rid}:${detail}`)}
              ></button>
            {/each}
          </span>
        {/each}
      </div>
      <p class="pick" aria-live="polite">{picked?.startsWith(`${rid}:`) ? picked.slice(rid.length + 1) : ''}</p>
      <p class="legend">
        <span><span class="cell b"></span>Fetched upstream</span>
        <span><span class="cell a"></span>From cache</span>
        <span class="muted">One square per ask, a second apart, left to right. Tap or click one for its TTL and time.</span>
      </p>
      <details>
        <summary>Each ask, one by one</summary>
        <ul class="asks">
          {#each c.rows as row, i}
            <li>
              <span>Name {i + 1} <span class="mono muted">{row.name}</span></span>
              <span class="muted">{row.asks.map((a, j) => `${j + 1}: ${askText(a)}`).join('; ')}</span>
            </li>
          {/each}
        </ul>
      </details>
      <p class="why">
        {c.rows.length} made-up names, each asked {c.rounds} times. An answer with the full TTL was just fetched; one a second or more
        lower came from a cache. A name fetched again after it was cached was answered by a different cache.
      </p>
    {/if}

    {#if res.transports.length}
      <h4>What each transport costs</h4>
      <table class="grid cost">
        <thead><tr><th>Transport</th><th>New connection</th><th>Setup</th><th>First query</th><th>Later queries</th></tr></thead>
        <tbody>
          {#each res.transports as t}
            <tr>
              <td>{t.transport}</td>
              <td>
                {#if t.error}<span class="muted">{t.error}</span>
                {:else}<span class="stack" aria-hidden="true"
                    ><span class="setup" style:width="{(100 * (t.setup_ms ?? 0)) / costScale}%"></span><span
                      class="query"
                      style:width="{(100 * (t.first_ms ?? 0)) / costScale}%"
                    ></span></span
                  >{/if}
              </td>
              <td class="num">{t.setup_ms != null ? ms(t.setup_ms) : t.error ? '—' : 'none'}</td>
              <td class="num">{ms(t.first_ms)}</td>
              <td class="num">{ms(t.reused_ms)}{#if t.answered < t.asked}<span class="muted"> · {t.answered}/{t.asked}</span>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="legend">
        <span><span class="key setup"></span>Connect and handshake</span>
        <span><span class="key query"></span>First query</span>
      </p>
      <p class="why">
        A new connection pays its setup once: TCP's connect, plus the TLS handshake for DoT and DoH. Later queries reuse it, so
        their median is the ongoing cost{#if udp?.reused_ms != null}; compare it with UDP's {ms(udp.reused_ms)}{/if}. Ten asks of a
        popular name per transport.
      </p>
    {/if}
  </div>
{/if}

<style>
  .checks {
    display: grid;
    justify-items: start;
    gap: 10px;
  }
  h4 {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 14px 0 0;
    font-size: var(--t-sm);
    font-weight: 640;
  }
  .checks > h4:first-child {
    margin-top: 0;
  }
  .tag {
    padding: 1px 7px;
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    background: var(--surface-2);
    color: var(--ink-3);
    font-size: var(--t-xs);
    font-weight: 560;
  }
  .note {
    padding: 10px 12px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    background: var(--surface-2);
    border-radius: var(--r-md);
  }
  .why {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .muted {
    color: var(--ink-3);
  }
  .stats {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 8px 28px;
  }
  .stats dt {
    color: var(--ink-3);
    font-size: var(--t-xs);
  }
  .stats dd {
    margin: 2px 0 0;
    font-size: var(--t-lg);
    font-weight: 600;
  }
  /* Glyph color carries the status; the text beside it stays in ink. */
  .verdict {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .g {
    display: inline-flex;
    flex: none;
    color: var(--ink-3);
  }
  .g.good {
    color: var(--good);
  }
  .g.bad {
    color: var(--critical);
  }
  .sizes {
    width: 100%;
    display: grid;
    grid-template-columns: max-content minmax(60px, 1fr) max-content;
    align-items: center;
    gap: 4px 14px;
    font-size: var(--t-sm);
  }
  .sizes .verdict {
    grid-column: 2 / -1;
    margin-bottom: 6px;
    font-size: var(--t-xs);
  }
  .lab {
    color: var(--ink-2);
    font-size: var(--t-xs);
  }
  .bar {
    position: relative;
    height: 8px;
    border-radius: 3px;
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--rule);
  }
  .fill {
    position: absolute;
    inset: 0 auto 0 0;
    max-width: 100%;
    border-radius: 3px;
    background: var(--ink-2);
  }
  .limit {
    position: absolute;
    top: -3px;
    bottom: -3px;
    width: 2px;
    margin-left: -1px;
    background: var(--ink);
  }
  .grid {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--t-sm);
  }
  .grid th {
    text-align: left;
    font-weight: 560;
    font-size: var(--t-xs);
    color: var(--ink-3);
    padding: 0 12px 4px 0;
  }
  .grid td {
    padding: 6px 12px 6px 0;
    border-top: 1px solid var(--rule);
    vertical-align: top;
  }
  .grid td.num {
    white-space: nowrap;
  }
  .sub {
    display: block;
    font-size: var(--t-xs);
    overflow-wrap: anywhere;
  }
  .cost td:nth-child(2) {
    width: 40%;
    vertical-align: middle;
  }
  .stack {
    display: flex;
    width: 100%;
    height: 8px;
  }
  .stack > span,
  .key {
    display: inline-block;
    height: 8px;
  }
  .setup {
    background: var(--rule-2);
    border-radius: 3px 0 0 3px;
  }
  .query {
    background: var(--ink-2);
    border-radius: 0 3px 3px 0;
  }
  .key {
    width: 14px;
    border-radius: 2px;
  }
  .rounds {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    align-items: center;
    gap: 6px 14px;
  }
  .cells {
    display: flex;
    flex-wrap: wrap;
    gap: 3px;
  }
  /* Shape carries the meaning: filled is a fetch, outlined is a cached answer, dashed is no answer. */
  .cell {
    display: inline-block;
    width: 13px;
    height: 13px;
    border-radius: 3px;
    border: 1.5px dashed var(--ink-3);
  }
  .cell.b {
    border: 1.5px solid var(--ink-2);
    background: var(--ink-2);
  }
  .cell.a {
    border: 1.5px solid var(--ink-2);
  }
  button.cell {
    padding: 0;
    background: none;
    cursor: pointer;
  }
  button.cell.b {
    background: var(--ink-2);
  }
  .cell.on {
    outline: 2px solid var(--ink);
    outline-offset: 1px;
  }
  .pick {
    margin: 6px 0 0;
    font-size: var(--t-xs);
  }
  details {
    margin-top: 8px;
    font-size: var(--t-sm);
  }
  summary {
    cursor: pointer;
  }
  .asks {
    margin: 6px 0 0;
    padding: 0;
    list-style: none;
    font-size: var(--t-xs);
  }
  .asks li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px 0;
    border-top: 1px solid var(--rule);
    overflow-wrap: anywhere;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 16px;
    font-size: var(--t-xs);
  }
  .legend > span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .legend > .muted {
    display: inline;
  }
</style>
