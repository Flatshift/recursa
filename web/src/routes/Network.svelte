<script lang="ts">
  // The Network page: which site each CDN serves this network from, in one quick check, and over time how the link
  // from your ISP to the next network behaves. Neither depends on a resolver, so it sits apart from a run.
  import { api } from '../lib/api';
  import type { RollCall } from '../lib/types';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import EdgeTable from '../components/EdgeTable.svelte';
  import BorderMonitor from '../components/BorderMonitor.svelte';

  let view = $state<'edges' | 'monitor'>('edges');
  let last = $state<RollCall | null>(null);
  let loaded = $state(false);
  let running = $state(false);
  let error = $state('');

  $effect(() => {
    api
      .get<{ last: RollCall | null }>('/api/network/edges')
      .then((r) => (last = r.last))
      .catch((e) => (error = (e as Error).message))
      .finally(() => (loaded = true));
  });

  async function check() {
    if (running) return;
    running = true;
    error = '';
    try {
      last = await api.post<RollCall>('/api/network/edges');
    } catch (e) {
      error = (e as Error).message;
    } finally {
      running = false;
    }
  }
</script>

<div class="view-root">
  <Toolbar title="Network" subtitle="Where the big CDNs serve this network from, and how its border links hold up">
    {#snippet center()}
      <span class="beta" title="New in this release: the readings are real, the wording and thresholds may still change.">Beta</span>
      <div class="segmented" role="tablist" aria-label="View">
        <button role="tab" aria-selected={view === 'edges'} onclick={() => (view = 'edges')}>Served from</button>
        <button role="tab" aria-selected={view === 'monitor'} onclick={() => (view = 'monitor')}>Over time</button>
      </div>
    {/snippet}
    {#snippet actions()}
      {#if view === 'edges'}
        <button class="btn primary" disabled={running || !loaded} title={running ? 'Asking each CDN' : 'Ask each CDN which site is answering you'} onclick={check}>
          <Icon name="play" size={12} />{running ? 'Checking' : last ? 'Check again' : 'Check'}
        </button>
      {/if}
    {/snippet}
  </Toolbar>

  <div class="view-body">
    {#if view === 'monitor'}
      <BorderMonitor />
    {/if}
    <div class="content scroll wrap" style:display={view === 'monitor' ? 'none' : null}>
      <EdgeTable {last} {loaded} {running} {error} {check} />
    </div>
  </div>
</div>

<style>
  .beta {
    margin-right: 10px;
    padding: 1px 7px;
    border: 1px solid var(--rule);
    border-radius: 8px;
    font-size: var(--t-xs);
    color: var(--ink-3);
    letter-spacing: 0.02em;
  }
  .wrap > :global(*) {
    max-width: 1280px;
  }
</style>
