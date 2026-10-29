<script lang="ts">
  // The Peering page: a map of where your ISP hands traffic to the big content networks, and over time how the link
  // from your ISP to the next network behaves. Neither depends on a resolver, so it sits apart from a run.
  import { app } from '../lib/store.svelte';
  import { netmap } from '../lib/netmap.svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import NetMapPanel from '../components/NetMapPanel.svelte';
  import BorderMonitor from '../components/BorderMonitor.svelte';

  // The view is in the path, so the sidebar's monitor link and a reload land on the same half.
  const view = $derived(app.route.name === 'network' && app.route.view === 'monitor' ? 'monitor' : 'edges');
  const show = (v: 'edges' | 'monitor') => app.navigate(v === 'monitor' ? '/network/monitor' : '/network', true);
  const nm = netmap();
  const blocked = $derived(app.env?.trace === 'needs_admin' || app.env?.trace === 'unsupported');
  const prog = $derived(nm.data?.progress);
</script>

<div class="view-root">
  <Toolbar title="Peering" subtitle="Where your ISP hands traffic to the big content networks, and how its border links hold up">
    {#snippet center()}
      <span
        class="beta"
        title="New in this release: the readings are real, the wording and thresholds may still change. Border inference has only been validated on one connection."
        >Beta</span
      >
      <div class="segmented" role="tablist" aria-label="View">
        <button role="tab" aria-selected={view === 'edges'} onclick={() => show('edges')}>Map</button>
        <button role="tab" aria-selected={view === 'monitor'} onclick={() => show('monitor')}>Over time</button>
      </div>
    {/snippet}
    {#snippet actions()}
      {#if view === 'edges'}
        {#if nm.running}
          <span class="progress" role="status">Mapping{prog ? `, ${prog.done} of ${prog.total}` : ''}</span>
          <button class="btn" onclick={nm.cancel}>Cancel</button>
        {:else}
          <button class="btn primary" disabled={!nm.loaded || blocked} title="Trace the way to each destination and draw where your ISP hands it off" onclick={nm.start}>
            <Icon name="play" size={12} />Map
          </button>
        {/if}
      {/if}
    {/snippet}
  </Toolbar>

  <div class="view-body">
    {#if view === 'monitor'}
      <BorderMonitor />
    {/if}
    <div class="content scroll wrap" style:display={view === 'monitor' ? 'none' : null}>
      <NetMapPanel {nm} />
    </div>
  </div>
</div>

<style>
  .beta {
    display: inline-flex;
    align-items: center;
    align-self: center;
    margin-right: 10px;
    padding: 0 7px;
    line-height: 18px;
    border: 1px solid var(--rule);
    border-radius: 8px;
    font-size: var(--t-xs);
    color: var(--ink-3);
    letter-spacing: 0.02em;
  }
  .progress {
    margin-right: 8px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .wrap > :global(*) {
    max-width: 1280px;
  }
</style>
