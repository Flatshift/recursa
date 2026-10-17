<script lang="ts">
  // After the latency rounds the run keeps working with no new samples to show: the path check, the capability
  // probes, the CDN mapping and the network intel. This line names the check under way; the progress rail above
  // carries the motion, so the two never disagree.
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
    <span class="text">Latency measured. {text}{count}</span>
  </p>
{/if}

<style>
  .working {
    margin: 0;
    padding: 6px 0;
    font-size: var(--t-xs);
    color: var(--ink-2);
    min-width: 0;
  }
  .text {
    text-wrap: pretty;
  }
</style>
