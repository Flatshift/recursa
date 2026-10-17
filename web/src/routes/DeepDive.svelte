<script lang="ts">
  import { app } from '../lib/store.svelte';
  import { api } from '../lib/api';
  import type { DeepDive } from '../lib/types';
  import { label, ms, signedMs } from '../lib/format';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import SetupDial from '../components/SetupDial.svelte';
  import DiveTrace from '../components/DiveTrace.svelte';
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
      <button class="btn primary sm" disabled={busy || running || !resolver} onclick={start}>
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
              <button class="btn primary" disabled={running || !resolver} onclick={start}><Icon name="play" size={13} />Run deep dive</button>
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
</style>
