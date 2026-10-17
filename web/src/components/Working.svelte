<script lang="ts">
  // After the latency rounds the run keeps working with no new samples to show: the path check, the capability
  // probes, the CDN mapping and the network intel. This line names the check under way, and beside it a sample
  // streams in and lands, the way answers did while measuring, so the quiet reads as work, not a stall. Under reduced
  // motion the sample has landed.
  import { fade } from 'svelte/transition';
  import { reduceMotion } from '../lib/motion';
  import type { RunState } from '../lib/types';

  let { run }: { run: RunState } = $props();
  const TEXT: Record<string, string> = {
    path: 'Timing the network path to each resolver and checking for port 53 interception',
    probes: 'Asking each resolver what it does: DNSSEC, ECS, QNAME minimisation, blocking',
    cdn: 'Mapping CDN edges through every resolver and timing them',
    intel: 'Looking up each resolver’s network: RPKI, ROV, where its queries leave',
  };
  const text = $derived(run.status === 'running' ? TEXT[run.phase] : undefined);
  // The probes and intel report per resolver; the path check and CDN mapping report only at the end.
  const count = $derived((run.phase === 'probes' || run.phase === 'intel') && run.done > 0 && run.total > 0 ? ` · ${run.done} of ${run.total} resolvers` : '');
</script>

{#if text}
  <p class="working" transition:fade={{ duration: reduceMotion ? 0 : 200 }}>
    <span class="trace" aria-hidden="true"><i class="sample"></i></span>
    <span class="text">Latency measured. {text}{count}</span>
  </p>
{/if}

<style>
  .working {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    margin: 0;
    padding: 6px 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
    min-width: 0;
  }
  .text {
    text-wrap: pretty;
  }
  /* A short plot edge: the sample enters from the left, decelerates onto its value, rests, and the next one follows. */
  .trace {
    --run: 38px;
    flex: none;
    position: relative;
    margin-top: 0.7em;
    width: 44px;
    height: 1px;
    background: var(--rule-2);
  }
  .sample {
    position: absolute;
    left: 0;
    top: -2.5px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ink);
    opacity: 0;
    animation: land 2000ms var(--ease-out) infinite;
  }
  @keyframes land {
    0% {
      transform: translateX(0);
      opacity: 0;
    }
    10% {
      opacity: 1;
    }
    62% {
      transform: translateX(var(--run));
      opacity: 1;
    }
    86% {
      transform: translateX(var(--run));
      opacity: 1;
    }
    100% {
      transform: translateX(var(--run));
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .sample {
      animation: none;
      transform: translateX(var(--run));
      opacity: 1;
    }
  }
</style>
