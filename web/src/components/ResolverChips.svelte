<script lang="ts">
  import Icon from './Icon.svelte';
  import type { MonitorConfig, Resolver } from '../lib/types';

  /** The resolvers the background monitor checks, a chip each; a click saves the config with that one toggled. */
  let {
    cfg,
    resolvers,
    disabled = false,
    save,
  }: { cfg: MonitorConfig; resolvers: Resolver[]; disabled?: boolean; save: (next: MonitorConfig) => void } = $props();
</script>

<div class="chips">
  {#each resolvers.filter((r) => r.group !== 'legacy') as r (r.id)}
    {@const on = cfg.resolvers.includes(r.id)}
    <button
      type="button"
      class="chip"
      class:on
      aria-pressed={on}
      {disabled}
      onclick={() => save({ ...cfg, resolvers: on ? cfg.resolvers.filter((x) => x !== r.id) : [...cfg.resolvers, r.id] })}
      >{#if on}<Icon name="check" size={11} />{/if}{r.provider} {r.variant}</button
    >
  {/each}
</div>

<style>
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    border: 1px solid var(--rule);
    background: transparent;
    border-radius: 14px;
    padding: 3px 10px;
    font-size: var(--t-xs);
    cursor: default;
    color: var(--ink-2);
  }
  .chip.on {
    background: var(--sel);
    border-color: var(--rule-2);
    color: var(--ink);
  }
  :global(html.mobile) .chip {
    min-height: 36px;
  }
</style>
