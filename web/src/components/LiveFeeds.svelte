<script lang="ts">
  import { app } from '../lib/store.svelte';
  import type { LiveBlocking, LiveCategory } from '../lib/types';
  import { ago, label } from '../lib/format';
  import { results, LIVE_SAMPLE_TTL_MS } from '../lib/checks.svelte';
  import { expectsBlock } from '../lib/verdict';

  let { rid, live }: { rid: string; live: LiveBlocking | null } = $props();

  /** The host square last tapped or clicked, shown under its category; `cat` is `resolver:category`. */
  let picked = $state<{ cat: string; text: string } | null>(null);

  const CATS = [
    { key: 'malware', label: 'Malware' },
    { key: 'phishing', label: 'Phishing' },
  ];

  const DAY = 86_400_000;
  const AGES = [
    { label: 'Listed under a day ago', max: DAY },
    { label: '1–7 days ago', max: 7 * DAY },
    { label: '7–30 days ago', max: 30 * DAY },
    { label: 'Over 30 days ago', max: Infinity },
  ];

  const tally = (hs: { blocked: boolean | null }[]) => ({
    blocked: hs.filter((h) => h.blocked === true).length,
    tested: hs.filter((h) => h.blocked != null).length,
  });

  /** A category's hosts, newest listing first, grouped by how long the feed had listed them when the sample was taken. */
  function byAge(fc: LiveCategory, at: number) {
    const hosts = fc.samples.map((s) => ({ ...s, listed: fc.listed_at[s.domain] ?? null }));
    hosts.sort((a, b) => (b.listed ?? -Infinity) - (a.listed ?? -Infinity));
    const groups = [...AGES.map((a) => ({ label: a.label, hosts: [] as typeof hosts })), { label: 'Not dated', hosts: [] as typeof hosts }];
    for (const h of hosts) groups[h.listed == null ? AGES.length : AGES.findIndex((a) => at - h.listed! < a.max)].hosts.push(h);
    return groups.filter((g) => g.hosts.length).map((g) => ({ ...g, ...tally(g.hosts) }));
  }

  // Allowing a host is only a miss for a resolver that blocks that category.
  const expects = (rId: string, cat: string) => {
    const r = app.resolvers.find((x) => x.id === rId);
    return !r || expectsBlock(r, cat);
  };

  const share = (b: number, t: number) => (t ? Math.round((100 * b) / t) : 0);

  // Every resolver checked against this same sample, most blocked first.
  const peers = $derived.by(() => {
    if (!live) return [];
    return Object.entries(results.live)
      .filter(([, c]) => c.sample_taken_at === live.sample_taken_at)
      .map(([pid, c]) => {
        const r = app.resolvers.find((x) => x.id === pid);
        const cats = CATS.map((k) => c.categories[k.key]);
        const blocked = cats.reduce((n, f) => n + (f?.blocked ?? 0), 0);
        const tested = cats.reduce((n, f) => n + (f?.tested ?? 0), 0);
        return { id: pid, name: r ? label(r.provider, r.variant) : pid, cats, rate: tested ? blocked / tested : -1 };
      })
      .sort((a, b) => b.rate - a.rate);
  });

  const clock = (t: number) => new Date(t).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
</script>

{#if live}
  <div class="feeds">
    <p class="meta">Over {live.transport}, {(live.duration_ms / 1000).toFixed(1)} s.</p>
    {#each live.notes as n}<p class="note">{n}</p>{/each}
    {#each CATS as c}
      {@const fc = live.categories[c.key]}
      {@const miss = expects(rid, c.key)}
      <div class="cat" class:miss>
        <h4>{c.label}</h4>
        {#if fc?.tested}
          <p class="rate">
            <span class="pct num">{share(fc.blocked, fc.tested)}%</span>
            <span>blocked, {fc.blocked} of {fc.tested} live hosts</span>
          </p>
          <div class="meter" aria-hidden="true"><span style:width="{share(fc.blocked, fc.tested)}%"></span></div>
        {:else}
          <p class="rate"><span class="pct">Not tested</span></p>
        {/if}
        {#if fc?.samples.length}
          <div class="ages">
            {#each byAge(fc, live.sample_taken_at) as g}
              <span class="age">{g.label}</span>
              <span class="cells">
                {#each g.hosts as h}
                  {@const detail = `${h.domain}: ${h.blocked === true ? 'blocked' : h.blocked === false ? 'resolved' : 'no answer'} · ${h.how}${h.listed != null ? ` · listed ${ago(h.listed)}` : ''}`}
                  <button
                    type="button"
                    class="cell"
                    class:b={h.blocked === true}
                    class:a={h.blocked === false}
                    class:on={picked?.cat === `${rid}:${c.key}` && picked.text === detail}
                    title={detail}
                    aria-label={detail}
                    onclick={() => (picked = { cat: `${rid}:${c.key}`, text: detail })}
                  ></button>
                {/each}
              </span>
              <span class="num muted">{g.tested ? `${g.blocked}/${g.tested}` : '—'}</span>
            {/each}
          </div>
          <p class="pick" aria-live="polite">{picked?.cat === `${rid}:${c.key}` ? picked.text : ''}</p>
        {/if}
        <p class="src">
          {fc?.source ?? (c.key === 'malware' ? 'URLhaus' : 'OpenPhish')}{#if fc?.fetched_at}, feed downloaded {ago(fc.fetched_at)}{/if}.
          {#if c.key === 'phishing' && fc}OpenPhish doesn't date its entries; it lists pages seen recently.{/if}
        </p>
      </div>
    {/each}
    <p class="legend">
      <span><span class="cell b"></span>Blocked</span>
      <span><span class="cell a"></span>Resolved</span>
      <span><span class="cell"></span>No answer</span>
      <span class="muted">One square per host. Tap or click one for the name and the answer.</span>
    </p>
    {#each CATS as c}
      {@const fc = live.categories[c.key]}
      {#if fc?.samples.length}
        <details>
          <summary>{c.label} hosts, one by one</summary>
          <ul class="samples">
            {#each fc.samples as s}
              <li>
                <span class="mono">{s.domain}</span>
                <span class:blocked={s.blocked === true} class="muted"
                  >{s.blocked === true ? 'Blocked' : s.blocked === false ? 'Resolved' : 'No answer'} · {s.how}{#if fc.listed_at[s.domain]}
                    · listed {ago(fc.listed_at[s.domain])}{/if}</span
                >
              </li>
            {/each}
          </ul>
        </details>
      {/if}
    {/each}

    <h4>Same hosts, other resolvers</h4>
    {#if peers.length > 1}
      <table class="peers">
        <thead><tr><th>Resolver</th>{#each CATS as c}<th>{c.label}</th>{/each}</tr></thead>
        <tbody>
          {#each peers as p (p.id)}
            <tr class:me={p.id === rid}>
              <td>
                {#if p.id === rid}{p.name}{:else}<button class="name" onclick={() => app.navigate(`/resolver/${p.id}`)}>{p.name}</button>{/if}
              </td>
              {#each p.cats as f, i}
                <td class:miss={expects(p.id, CATS[i].key)}>
                  {#if f?.tested}
                    <span class="meter sm" aria-hidden="true"><span style:width="{share(f.blocked, f.tested)}%"></span></span>
                    <span class="num">{f.blocked}/{f.tested}</span>
                  {:else}<span class="muted">Not tested</span>{/if}
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    <p class="why">
      Every resolver checked before {clock(live.sample_taken_at + LIVE_SAMPLE_TTL_MS)} is asked for these same hosts, so their
      results line up here. After that a fresh sample is drawn. A host counts as blocked on NXDOMAIN, REFUSED, an empty answer,
      a sinkhole address, a known block page or an EDE code 15–18.
    </p>
  </div>
{/if}

<style>
  .feeds {
    display: grid;
    justify-items: start;
    gap: 10px;
  }
  .meta {
    font-size: var(--t-xs);
    color: var(--ink-3);
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
  h4 {
    margin: 8px 0 0;
    font-size: var(--t-sm);
    font-weight: 640;
  }
  /* Blocked reads as good and resolved as a miss only where blocking is expected; otherwise both stay neutral. */
  .cat,
  .peers td {
    --hit: var(--s-neutral);
    --miss: var(--s-neutral);
  }
  .cat.miss,
  .peers td.miss {
    --hit: var(--good);
    --miss: var(--critical);
  }
  .cat {
    width: 100%;
    display: grid;
    gap: 8px;
  }
  .cat h4 {
    margin: 0;
  }
  .rate {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 4px 10px;
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .pct {
    font-size: var(--t-xl);
    font-weight: 640;
    letter-spacing: -0.02em;
    color: var(--ink);
  }
  .meter {
    display: block;
    width: min(100%, 360px);
    height: 6px;
    border-radius: 3px;
    background: var(--surface-2);
    box-shadow: inset 0 0 0 1px var(--rule);
    overflow: hidden;
  }
  .meter > span {
    display: block;
    height: 100%;
    background: var(--hit);
  }
  .meter.sm {
    display: inline-block;
    width: 28px;
    height: 4px;
    vertical-align: middle;
    margin-right: 6px;
  }
  .ages {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr) max-content;
    align-items: center;
    gap: 6px 14px;
    font-size: var(--t-xs);
  }
  .age {
    color: var(--ink-3);
  }
  .cells {
    display: flex;
    flex-wrap: wrap;
    gap: 3px;
  }
  /* Shape carries the answer as well as color: filled is blocked, outlined is resolved, dashed is no answer. */
  .cell {
    display: inline-block;
    width: 13px;
    height: 13px;
    border-radius: 3px;
    border: 1.5px dashed var(--ink-3);
  }
  .cell.b {
    border: 1.5px solid var(--hit);
    background: var(--hit);
  }
  .cell.a {
    border: 1.5px solid var(--miss);
  }
  button.cell {
    padding: 0;
    background: none;
    cursor: pointer;
  }
  button.cell.b {
    background: var(--hit);
  }
  .cell.on {
    outline: 2px solid var(--ink);
    outline-offset: 1px;
  }
  .pick {
    margin: 6px 0 0;
    font-size: var(--t-xs);
    overflow-wrap: anywhere;
  }
  .src {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 16px;
    font-size: var(--t-xs);
    --hit: var(--ink-2);
    --miss: var(--ink-2);
  }
  .legend > span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .legend > .muted {
    display: inline;
  }
  .peers {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--t-sm);
  }
  .peers th {
    text-align: left;
    font-weight: 560;
    font-size: var(--t-xs);
    color: var(--ink-3);
    padding: 0 12px 4px 0;
  }
  .peers td {
    padding: 6px 12px 6px 0;
    border-top: 1px solid var(--rule);
    white-space: nowrap;
  }
  .peers tr.me td:first-child {
    font-weight: 620;
  }
  .name {
    padding: 0;
    border: 0;
    background: none;
    color: var(--ink);
    font: inherit;
    text-decoration: underline;
    text-decoration-color: var(--rule);
    text-underline-offset: 3px;
    cursor: pointer;
  }
  .name:hover {
    text-decoration-color: currentColor;
  }
  details {
    width: 100%;
    font-size: var(--t-sm);
  }
  summary {
    cursor: pointer;
  }
  .samples {
    margin: 6px 0 0;
    padding: 0;
    list-style: none;
  }
  .samples li {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 10px;
    padding: 4px 0;
    border-top: 1px solid var(--rule);
  }
  .samples .blocked {
    color: var(--ink-2);
  }
</style>
