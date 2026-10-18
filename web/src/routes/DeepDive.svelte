<script lang="ts">
  import { app } from '../lib/store.svelte';
  import { api } from '../lib/api';
  import type { DeepDive, LiveBlocking, LiveCategory } from '../lib/types';
  import { ago, label, ms, signedMs } from '../lib/format';
  import { deeper, liveChecks, LIVE_SAMPLE_TTL_MS } from '../lib/live.svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import SetupDial from '../components/SetupDial.svelte';
  import DiveTrace from '../components/DiveTrace.svelte';
  import DeeperChecks from '../components/DeeperChecks.svelte';
  import { fly } from 'svelte/transition';
  import { quintOut } from 'svelte/easing';
  import { reduceMotion } from '../lib/motion';

  let { id }: { id: string } = $props();

  const resolver = $derived(app.resolvers.find((r) => r.id === id || r.aliases?.includes(id) || r.custom_id === id));
  const title = $derived(resolver ? label(resolver.provider, resolver.variant) : 'Unknown resolver');
  const running = $derived(app.run?.status === 'running');

  let busy = $state(false);
  let dive = $state<DeepDive | null>(null);
  let error = $state<string | null>(null);

  async function start() {
    busy = true;
    error = null;
    dive = null;
    try {
      dive = await api.post<DeepDive>(`/api/resolvers/${encodeURIComponent(resolver?.id ?? id)}/deep-dive`);
    } catch (e) {
      error = (e as Error).message;
      dive = null;
    } finally {
      busy = false;
    }
  }

  // The live-feed blocking check is separate from the dive and opt-in: those names can set off security tools on the network.
  // Results are kept for the session, by resolver, so checks against the same hosts can be compared.
  const rid = $derived(resolver?.id ?? id);
  const live = $derived<LiveBlocking | null>(liveChecks[rid] ?? null);
  let liveBusy = $state(false);
  /** The host square last tapped or clicked, shown under its category; `cat` is `resolver:category`. */
  let picked = $state<{ cat: string; text: string } | null>(null);
  let liveError = $state<string | null>(null);
  let allowPlain = $state(false);

  async function startLive() {
    const key = rid;
    liveBusy = true;
    liveError = null;
    try {
      liveChecks[key] = await api.post<LiveBlocking>(`/api/resolvers/${encodeURIComponent(key)}/live-blocking`, { plain: allowPlain });
    } catch (e) {
      liveError = (e as Error).message;
    } finally {
      liveBusy = false;
    }
  }

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

  // Allowing a host is only a miss for a resolver that blocks that category: one it claims, or one on this network
  // (or added by hand) that the user is checking.
  const expects = (rId: string, cat: string) => {
    const r = app.resolvers.find((x) => x.id === rId);
    return !r || r.group === 'system' || r.group === 'custom' || r.published.filtering.includes(cat);
  };

  const share = (b: number, t: number) => (t ? Math.round((100 * b) / t) : 0);

  // Every resolver checked against this same sample, most blocked first.
  const peers = $derived.by(() => {
    if (!live) return [];
    return Object.entries(liveChecks)
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

  const residency = $derived.by(() => {
    if (!dive) return '';
    const { first, last, elapsed_s, suggests } = dive.ttl;
    const s = elapsed_s.toFixed(1);
    switch (suggests) {
      case 'counting_down':
        return `The TTL dropped from ${first} to ${last} s over ${s} s, keeping pace with the clock. That's what a cached answer looks like.`;
      case 'unchanged':
        return `The TTL stayed at ${first} s even though ${s} s went by. Either the resolver fetched it fresh each time, it pins or rewrites TTLs, or a different server behind the same address answered.`;
      case 'refreshed':
        return `The TTL went up, from ${first} to ${last} s, over ${s} s. Most likely the answer was fetched again in between, or another server in the resolver's pool answered.`;
      case 'irregular':
        return `The TTL dropped from ${first} to ${last} s over ${s} s, but not in step with the clock. Probably the answer was refreshed partway through, or more than one server answered.`;
      default:
        return first == null ? "No answer had a TTL, so there's nothing to compare." : 'Not enough time passed between the first and last ask to read the TTL.';
    }
  });

  const FLAGS = ['qr', 'aa', 'tc', 'rd', 'ra', 'ad', 'cd'] as const;

  // The findings rise in one after another while the trace is still filling in.
  const reveal = (_: Element, { n }: { n: number }) =>
    fly(_, { y: 10, duration: reduceMotion ? 0 : 420, delay: reduceMotion ? 0 : 260 + n * 90, easing: quintOut });
</script>

<div class="view-root">
  <Toolbar title="Deep dive" subtitle={title}>
    {#snippet actions()}
      <button class="btn sm" onclick={() => app.navigate('/resolvers')}><Icon name="list" size={13} />Resolvers</button>
      <button class="btn primary sm" disabled={busy || liveBusy || deeper.busy || running || !resolver} onclick={start}>
        <Icon name="play" size={13} />{dive ? 'Run again' : 'Run deep dive'}
      </button>
    {/snippet}
  </Toolbar>

  <div class="view-body">
    <div class="content scroll">
      <article>
        <header class="hero">
          <div class="mark"><SetupDial still={!busy} /></div>
          <div class="intro">
            <p class="lede">
              Sends 18 ordinary queries to this resolver, spaced the way the benchmark spaces them: a popular name twice, three
              made-up names, the popular name 12 more times, then one last ask two seconds later. The count is fixed.
            </p>
            <p class="status" role="status">
              {#if busy}Asking {title}… this takes a few seconds.
              {:else if running}A benchmark is running. Try again when it finishes.
              {:else if dive}{dive.queries} queries over {dive.transport}, {(dive.duration_ms / 1000).toFixed(1)} s. Popular name:
                <span class="mono">{dive.name}</span>.
              {/if}
            </p>
            {#if !dive && !busy}
              <button class="btn primary" disabled={liveBusy || deeper.busy || running || !resolver} onclick={start}><Icon name="play" size={13} />Run deep dive</button>
            {/if}
          </div>
        </header>

        {#if error}<p class="note bad" role="alert">{error}</p>{/if}
        {#if dive?.aborted}<p class="note bad">{dive.aborted}</p>{/if}

        <div class="trace"><DiveTrace {dive} {busy} /></div>

        {#if dive}
          <section in:reveal={{ n: 0 }}>
            <h2>Round trips</h2>
            {#if dive.latency.samples_ms.length}
              <dl class="stats">
                <div><dt>Fastest</dt><dd class="num">{ms(dive.latency.stats.min)}</dd></div>
                <div><dt>Median</dt><dd class="num">{ms(dive.latency.stats.p50)}</dd></div>
                <div><dt>Slowest 5%</dt><dd class="num">{ms(dive.latency.stats.p95)}</dd></div>
                <div><dt>Slowest</dt><dd class="num">{ms(dive.latency.stats.max)}</dd></div>
                <div><dt title="Interquartile range, the same jitter measure the benchmark uses">Jitter</dt><dd class="num">{ms(dive.latency.stats.iqr)}</dd></div>
              </dl>
              <p class="why">
                The {dive.latency.stats.sent} repeats of {dive.name}; {dive.latency.stats.ok} got an answer.
                {#if dive.latency.user_space_timed}At least one reply had no kernel receive stamp, so these are user-space times and run a
                  little high from scheduler wake-up.{:else}Times come from the kernel's receive stamp.{/if}
              </p>
            {:else}
              <p class="muted">No repeat was answered.</p>
            {/if}
          </section>

          <section in:reveal={{ n: 1 }}>
            <h2>Cache <span class="tag">Inferred</span></h2>
            {#if dive.cache.cold_ms != null && dive.cache.warm_ms != null}
              <dl class="stats">
                <div><dt>Random name</dt><dd class="num">{ms(dive.cache.cold_ms)}</dd></div>
                <div><dt>Popular name, asked again</dt><dd class="num">{ms(dive.cache.warm_ms)}</dd></div>
                <div><dt>Difference</dt><dd class="num">{signedMs(dive.cache.recursion_cost_ms)}</dd></div>
              </dl>
              <p class="why">
                A random name can't be in anyone's cache, so the resolver has to go find it. The popular name, asked a second time,
                should come straight from cache. The gap between the two is a rough guess at what a full lookup costs. Treat it as a
                guess: it lumps the resolver's own work in with the trips to the authoritative servers.
                {#if dive.cache.cold_samples_ms.length > 1}The random-name time is the median of {dive.cache.cold_samples_ms.length} ({dive.cache.cold_samples_ms.map((x) => ms(x)).join(', ')}).{/if}
              </p>
            {:else}
              <p class="muted">Not enough answers to compare a random name with a repeated one.</p>
            {/if}
          </section>

          <section in:reveal={{ n: 2 }}>
            <h2>Is the answer coming from cache? <span class="tag">Inferred</span></h2>
            <p>{residency}</p>
            <p class="why">
              Compares the TTL on the first answer record for {dive.name}, first ask against last. Recursa can't see what the
              resolver does upstream, so it doesn't guess how many lookups it made.
            </p>
          </section>

          <section in:reveal={{ n: 3 }}>
            <h2>One response, in detail</h2>
            {#if dive.response}
              {@const r = dive.response}
              <dl class="facts">
                <div><dt>Result</dt><dd><span class="mono">{r.rcode_name}</span> <span class="muted">· code {r.rcode}</span></dd></div>
                <div>
                  <dt>Flags</dt>
                  <dd class="flags">
                    {#each FLAGS as f}<span class="flag" class:on={r.flags[f]} title={r.flags[f] ? 'Set' : 'Not set'}>{f.toUpperCase()}</span>{/each}
                  </dd>
                </div>
                <div><dt>Records</dt><dd>{r.answers} answer · {r.authority} authority · {r.additional} additional</dd></div>
                <div><dt>Message size</dt><dd class="num">{r.size_bytes} bytes</dd></div>
                <div>
                  <dt>EDNS</dt>
                  <dd>
                    {#if r.edns}
                      UDP size {r.edns.udp_size} · version {r.edns.version}
                    {:else}
                      <span class="muted">none in the response</span>
                    {/if}
                  </dd>
                </div>
                <div>
                  <dt title="Recursa asked with DNSSEC OK set; AD means the resolver says it validated the answer">DNSSEC</dt>
                  <dd>
                    DO {r.edns?.dnssec_ok ? 'echoed' : 'not echoed'} · AD {r.flags.ad ? 'set (validated)' : 'not set'}
                  </dd>
                </div>
                <div>
                  <dt>Extended errors</dt>
                  <dd>
                    {#each r.ede as e}<span class="ede">EDE {e.code} {e.name}{e.text ? ` "${e.text}"` : ''}</span>{:else}<span class="muted">none</span>{/each}
                  </dd>
                </div>
                <div><dt>NSID</dt><dd>{#if r.nsid}<span class="mono">{r.nsid}</span>{:else}<span class="muted">not sent</span>{/if}</dd></div>
              </dl>
              <p class="why">The first answer for {dive.name}. Record counts are from the message header.</p>
            {:else}
              <p class="muted">No query was answered.</p>
            {/if}
          </section>
        {/if}

        <DeeperChecks {rid} {title} disabled={busy || liveBusy || running || !resolver} />

        <section class="live">
          <h2>Blocking, against live threat feeds</h2>
          <p>
            A run checks blocking with the test names blocking services publish. This goes further: it takes hosts from today's
            URLhaus malware and OpenPhish phishing feeds, keeps up to 20 of each that still resolve through an unfiltered resolver
            (Google or Cloudflare, over DoT), and asks this resolver for them.
          </p>
          <p class="warn">
            <Icon name="warn" size={14} /><span
              >These are real malicious hosts. Security tools on the network (a UniFi gateway, Azure, a corporate firewall) may flag
              the lookups or block this device. Run this only when you mean to test a blocking resolver.</span
            >
          </p>
          <label class="toggle"
            ><input type="checkbox" bind:checked={allowPlain} /><span>Allow plain DNS if this resolver has no DoT or DoH</span></label
          >
          <p class="why">
            Otherwise the names go only over DoT or DoH, where nothing on the path can read them. Your own resolver on this network (a
            Pi-hole, your router) is tested over plain DNS either way.
          </p>
          <button class="btn" disabled={liveBusy || busy || deeper.busy || running || !resolver} onclick={startLive}
            ><Icon name="play" size={13} />{live ? 'Check again' : 'Check with live feeds'}</button
          >
          <p class="status" role="status">
            {#if liveBusy}Confirming live hosts, then asking {title}… this takes up to a minute.
            {:else if live}Over {live.transport}, {(live.duration_ms / 1000).toFixed(1)} s.
            {/if}
          </p>
          {#if liveError}<p class="note bad" role="alert">{liveError}</p>{/if}
          {#if live}
            {#each live.notes as n}<p class="note">{n}</p>{/each}
            {#each CATS as c}
              {@const fc = live.categories[c.key]}
              {@const miss = expects(rid, c.key)}
              <div class="cat" class:miss>
                <h3>{c.label}</h3>
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

            <h3>Same hosts, other resolvers</h3>
            {#if peers.length > 1}
              <table class="peers">
                <thead><tr><th>Resolver</th>{#each CATS as c}<th>{c.label}</th>{/each}</tr></thead>
                <tbody>
                  {#each peers as p (p.id)}
                    <tr class:me={p.id === rid}>
                      <td>
                        {#if p.id === rid}{p.name}{:else}<button class="name" onclick={() => app.navigate(`/deep-dive/${p.id}`)}>{p.name}</button>{/if}
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
          {/if}
        </section>
      </article>
    </div>
  </div>
</div>

<style>
  article {
    max-width: 74ch;
    padding-bottom: 32px;
  }
  .hero {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 20px;
    margin-left: -24px;
  }
  .mark {
    flex: none;
  }
  .intro {
    flex: 1 1 320px;
    display: grid;
    justify-items: start;
    gap: 12px;
  }
  .lede {
    font-size: var(--t-md);
    color: var(--ink-2);
  }
  .status {
    min-height: 1.5em;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .trace {
    margin-top: 8px;
  }
  .note {
    margin-top: 14px;
    padding: 10px 12px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    background: var(--surface-2);
    border-radius: var(--r-md);
  }
  .note.bad {
    color: var(--ink);
    border: 1px solid var(--rule);
  }
  section {
    margin-top: 30px;
  }
  h2 {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-bottom: 10px;
    font-size: var(--t-lg);
    font-weight: 650;
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
  .stats {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 8px 28px;
  }
  .stats dt {
    padding: 0;
  }
  .stats dd {
    margin: 2px 0 0;
    font-size: var(--t-lg);
    font-weight: 600;
  }
  .why {
    margin-top: 10px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  dt {
    color: var(--ink-3);
    font-size: var(--t-xs);
    padding-top: 2px;
  }
  dd {
    margin: 0;
  }
  .facts {
    margin: 0;
    display: grid;
  }
  .facts > div {
    display: grid;
    grid-template-columns: 150px minmax(0, 1fr);
    gap: 12px;
    padding: 7px 0;
    border-top: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .flags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .flag {
    padding: 0 6px;
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    color: var(--ink-3);
    font-size: var(--t-xs);
  }
  .flag.on {
    background: var(--accent-soft);
    color: var(--accent-text);
    border-color: transparent;
  }
  .ede {
    display: block;
  }
  .live {
    display: grid;
    justify-items: start;
    gap: 10px;
    padding-top: 20px;
    border-top: 1px solid var(--rule);
  }
  .live > p:first-of-type {
    color: var(--ink-2);
  }
  .live .note {
    margin-top: 0;
  }
  .warn {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 10px 12px;
    font-size: var(--t-sm);
    background: var(--warn-soft);
    border-radius: var(--r-md);
  }
  .warn :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--warning);
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: var(--t-sm);
  }
  .live .why {
    margin-top: 0;
  }
  h3 {
    margin: 8px 0 0;
    font-size: var(--t-base);
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
  .cat h3 {
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
