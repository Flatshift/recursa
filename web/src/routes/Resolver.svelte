<script lang="ts">
  // One resolver on its own page: the same sections as the Inspector, with the room to read them.
  import { app } from '../lib/store.svelte';
  import { api } from '../lib/api';
  import type { RunState } from '../lib/types';
  import { HIT_SHARE, buildRows } from '../lib/verdict';
  import { label } from '../lib/format';
  import { status } from '../lib/status.svelte';
  import { RUN_ALL_TITLE, allKinds, blocker, runAll, runningAll } from '../lib/checks.svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import ResolverDetail from '../components/ResolverDetail.svelte';

  let { id }: { id: string } = $props();

  const resolver = $derived(app.resolvers.find((r) => r.id === id || r.aliases?.includes(id) || r.custom_id === id));

  // The latest finished run, for what it measured about this resolver.
  let last = $state<RunState | null>(null);
  $effect(() => {
    status.load();
    const done = app.boot?.runs.find((r) => r.status === 'done');
    if (done && last?.id !== done.id) api.get<RunState>(`/api/runs/${done.id}`).then((r) => (last = r)).catch(() => {});
  });
  const inLast = $derived(!!(resolver && last?.probes[resolver.id]));
  const rows = $derived(last && inLast ? buildRows(last, app.boot?.resolvers ?? [], HIT_SHARE[app.mix]) : []);
  const stop = $derived(blocker());
  const busy = $derived(!!resolver && runningAll(resolver.id));
</script>

<div class="view-root">
  {#if resolver}
    <Toolbar title={label(resolver.provider, resolver.variant)} subtitle={[...resolver.ipv4, ...resolver.ipv6].join(' · ')}>
      {#snippet actions()}
        <button
          class="btn sm"
          disabled={!!stop}
          title={stop ?? RUN_ALL_TITLE}
          onclick={() => runAll(resolver.id, allKinds(resolver))}
        >
          <Icon name="play" size={12} />{busy ? 'Running…' : 'Run all checks'}
        </button>
        <button class="btn sm" onclick={() => app.navigate('/resolvers')}>Resolvers</button>
      {/snippet}
    </Toolbar>
    <div class="page scroll">
      <ResolverDetail {resolver} run={inLast ? last : null} {rows} wide />
    </div>
  {:else}
    <Toolbar title="Resolver">
      {#snippet actions()}
        <button class="btn sm" onclick={() => app.navigate('/resolvers')}>Resolvers</button>
      {/snippet}
    </Toolbar>
    <div class="page scroll">
      <p class="muted">No resolver with the id <span class="mono">{id}</span>.</p>
    </div>
  {/if}
</div>

<style>
  .page {
    flex: 1;
    min-height: 0;
    padding: 4px 28px 32px;
  }
  .page > :global(.detail) {
    max-width: 860px;
  }
  @media (max-width: 560px) {
    .page {
      padding: 4px 16px 32px;
    }
  }
</style>
