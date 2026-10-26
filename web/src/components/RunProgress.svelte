<script lang="ts">
  import { untrack } from 'svelte';
  import type { RunState } from '../lib/types';
  import { duration, label } from '../lib/format';
  import { isCheck, phaseName, phasesOf } from '../lib/phases';
  import { silentEdges } from '../lib/cdn';
  import { rise } from '../lib/motion';

  let { run }: { run: RunState } = $props();

  const phases = $derived(phasesOf(run.config));
  const idx = $derived(run.phase === 'done' ? phases.length : phases.indexOf(run.phase));
  const running = $derived(run.status === 'running');
  // CDN mapping is a short round of lookups (about a fifth of it) and then the edge timing, each with its own count.
  const frac = $derived.by(() => {
    const t = run.cdn_timing;
    if (run.phase === 'cdn' && t) return 0.2 + 0.8 * Math.min(1, t.timed / t.total);
    const f = run.total > 0 ? Math.min(1, run.done / run.total) : 0;
    return run.phase === 'cdn' ? 0.2 * f : f;
  });
  // The checks after the latency rounds report per handshake (the path check) or per resolver. Until a check has reported, its track sweeps; from the first report it fills.
  const busy = $derived(isCheck(run.phase) && run.done === 0);

  // What the phase is doing, in a line above the rail, so the run says what it is busy with as well as how far along.
  const WORK: Record<string, string> = {
    setup: 'Checking your network and reaching each resolver',
    warmup: 'A few unrecorded queries prime each resolver’s cache and path',
    cached: 'Timing names each resolver already holds',
    uncached: 'Timing names no resolver can have cached',
    tld: 'Timing names that send each resolver to the TLD servers',
    path: 'Five rounds of TCP handshakes find each resolver’s network distance and catch port 53 interception',
    probes: 'Asking each resolver what it does: DNSSEC, ECS, QNAME minimisation, blocking',
    cdn: 'Looking up CDN hostnames through every resolver, then timing each edge it sent you to',
    intel: 'Looking up each resolver’s network: RPKI, ROV, where its queries leave',
    live: 'Looking up live malware and phishing hosts through every resolver',
  };
  // Seconds in the phase tick for the check that reports only at the end, so a silent phase still visibly moves.
  let since = $state(Date.now());
  let now = $state(Date.now());
  $effect(() => {
    run.phase;
    untrack(() => (since = now = Date.now()));
  });
  $effect(() => {
    if (!running) return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });
  // The resolver that reported last in this check, newest key last.
  const latest = $derived.by(() => {
    const got = run.phase === 'probes' ? run.probes : run.phase === 'intel' ? run.intel : run.phase === 'live' ? (run.live ?? {}) : run.phase === 'cdn' && !run.cdn_timing ? Object.fromEntries(Object.entries(run.cdn_lookups ?? {}).filter(([, l]) => l.finished)) : null;
    const id = got && run.done > 0 ? Object.keys(got).at(-1) : undefined;
    const r = id ? run.resolvers.find((x) => x.id === id) : undefined;
    return r ? label(r.provider, r.variant) : '';
  });
  const count = $derived.by(() => {
    if (run.phase === 'path') return run.done > 0 ? `${run.done} of ${run.total} handshakes` : duration(now - since);
    if (run.phase === 'probes' || run.phase === 'intel' || run.phase === 'live') return run.total > 0 ? `${run.done} of ${run.total} resolvers` : '';
    // The CDN lookups report per resolver; timing the edges they found comes after the last one.
    if (run.phase === 'cdn') {
      if (run.cdn_timing) {
        const t = run.cdn_timing;
        const quiet = silentEdges(t).length;
        return `${t.edges} edges, ${t.timed} of ${t.total} handshakes${quiet ? `, ${quiet} silent` : ''}`;
      }
      return run.total > 0 && run.done >= run.total ? 'Timing edges' : `${run.done} of ${run.total} resolvers`;
    }
    if (busy) return duration(now - since);
    return run.total > 0 ? `${run.done.toLocaleString()} of ${run.total.toLocaleString()} queries` : '';
  });
</script>

{#if running}
  <div class="prog chrome">
    <p class="head">
      {#key run.phase}
        <span class="what" in:rise={{ on: true }}>
          <b>{phaseName(run.phase)}</b>
          <span class="desc">{WORK[run.phase] ?? ''}</span>
        </span>
      {/key}
      {#if latest}
        {#key latest}<span class="latest" in:rise={{ on: true }}>{latest} done</span>{/key}
      {/if}
      <span class="count">{count}</span>
    </p>
    <ol class="rail" aria-label="Run progress">
      {#each phases as p, i}
        <li class:done={i < idx} class:now={i === idx} class:busy={i === idx && busy} aria-current={i === idx ? 'step' : undefined}>
          <span class="lbl">{phaseName(p)}</span>
          <span class="track"><span class="fill" style:transform="scaleX({i < idx ? 1 : i === idx ? frac : 0})"></span></span>
        </li>
      {/each}
    </ol>
    <p class="visually-hidden" aria-live="polite">{phaseName(run.phase)} phase</p>
  </div>
{/if}

<style>
  .prog {
    padding: 10px 28px 12px;
    border-bottom: 1px solid var(--rule);
    background: var(--bg);
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin: 0 0 9px;
    font-size: var(--t-sm);
    min-width: 0;
  }
  .what {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .latest {
    flex: none;
    color: var(--ink-2);
  }
  .head b {
    flex: none;
    font-weight: 620;
    color: var(--ink);
  }
  .desc {
    flex: 1 1 auto;
    min-width: 0;
    color: var(--ink-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .count {
    flex: none;
    color: var(--ink-2);
    font-variant-numeric: tabular-nums;
  }
  .rail {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(0, 1fr);
    gap: 6px;
  }
  .rail li {
    display: grid;
    gap: 5px;
    min-width: 0;
  }
  .lbl {
    font-size: var(--t-xs);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rail li.now .lbl {
    color: var(--ink);
    font-weight: 600;
  }
  .rail li.done .lbl {
    color: var(--ink-2);
  }
  .track {
    position: relative;
    height: 3px;
    border-radius: 2px;
    background: var(--rule);
    overflow: hidden;
  }
  /* A query going out along the path: one short segment glides the length of the track until the check reports. */
  .busy .track::before {
    content: '';
    position: absolute;
    inset: 0 auto 0 0;
    width: 24%;
    border-radius: 2px;
    background: var(--accent);
    opacity: 0.55;
    animation: sweep 1800ms var(--ease-out) infinite;
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(417%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    /* Still: a dashed track says in progress. */
    .busy .track::before {
      animation: none;
      width: 100%;
      background: repeating-linear-gradient(90deg, var(--accent) 0 4px, transparent 4px 8px);
      opacity: 0.45;
    }
  }
  .fill {
    position: relative;
    display: block;
    height: 100%;
    background: var(--accent);
    transform-origin: left;
    transition: transform 300ms var(--ease-out);
  }
  @media (max-width: 640px) {
    .head {
      flex-wrap: wrap;
      row-gap: 2px;
    }
    .desc {
      order: 3;
      flex-basis: 100%;
      white-space: normal;
    }
    .count {
      margin-left: auto;
    }
    .rail {
      grid-auto-flow: row;
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
  }
</style>
