<script lang="ts" module>
  /** The part of a monitor's config the panel edits; a page's own config may hold more. */
  interface Config {
    enabled: boolean;
    interval_min: number;
    retention_days: number;
  }
</script>

<script lang="ts" generics="C extends Config">
  // The frame of an "Over time" view for a background monitor: its on/off switch, how often it samples and how long it
  // keeps, a line on what it does, the window to show with Clear history, and the empty state. The page brings its own
  // words and extra controls and draws the record. It renders into the page's scroll area with no wrapper of its own.
  import type { Snippet } from 'svelte';
  import { RETENTION_DAYS } from '../lib/monitor.svelte';
  import Icon from './Icon.svelte';

  let {
    mon,
    days = $bindable(),
    windows,
    intervals,
    toggleLabel,
    toggleName,
    locked = false,
    extra,
    hint,
    nothing,
    children,
  }: {
    /** The page's monitor (lib/monitor.svelte.ts). */
    mon: {
      readonly data: { history: { cycles: number } } | null;
      readonly cfg: C | null;
      readonly saving: boolean;
      save: (next: C) => Promise<void>;
      clearHistory: () => Promise<void>;
    };
    /** The window shown, in days. The page's query reads it. */
    days: number;
    /** The windows to choose from, in days. */
    windows: number[];
    /** How often it may sample, in minutes. */
    intervals: number[];
    /** The switch as a screen reader says it, and its words in bold. */
    toggleLabel: string;
    toggleName: string;
    /** The switch and both selects are disabled. */
    locked?: boolean;
    /** More controls, last in the panel. */
    extra: Snippet;
    /** The line under the panel on what is sampled. */
    hint: Snippet;
    /** What to do about an empty record, under the empty state's heading. */
    nothing: Snippet;
    /** The record, shown when there is one. */
    children: Snippet;
  } = $props();

  const h = $derived(mon.data?.history);
  const empty = $derived(!!h && h.cycles === 0);
</script>

{#if mon.cfg}
  <div class="mcfg panel">
    <label class="toggle">
      <input
        type="checkbox"
        aria-label={toggleLabel}
        checked={mon.cfg.enabled}
        disabled={mon.saving || locked}
        onchange={(e) => mon.save({ ...mon.cfg!, enabled: (e.currentTarget as HTMLInputElement).checked })}
      />
      <span><b>{toggleName}</b> while Recursa is running</span>
    </label>
    <label class="inline">
      Every
      <select value={mon.cfg.interval_min} disabled={locked} onchange={(e) => mon.save({ ...mon.cfg!, interval_min: Number((e.currentTarget as HTMLSelectElement).value) })}>
        {#each intervals as m}<option value={m}>{m < 60 ? `${m} min` : `${m / 60} h`}</option>{/each}
      </select>
    </label>
    <label class="inline">
      Keep
      <select value={mon.cfg.retention_days} disabled={locked} onchange={(e) => mon.save({ ...mon.cfg!, retention_days: Number((e.currentTarget as HTMLSelectElement).value) })}>
        {#each RETENTION_DAYS as d}<option value={d}>{d} days</option>{/each}
      </select>
    </label>
    {@render extra()}
  </div>
  <p class="hint">{@render hint()}</p>
{/if}

<div class="bar">
  <div class="segmented" role="tablist" aria-label="Window">
    {#each windows as d}
      <button role="tab" aria-selected={days === d} onclick={() => (days = d)}>{d === 1 ? '24 hours' : `${d} days`}</button>
    {/each}
  </div>
  {#if h && h.cycles}
    <button class="btn ghost sm danger" onclick={mon.clearHistory}><Icon name="trash" size={13} />Clear history</button>
  {/if}
</div>

{#if empty}
  <div class="empty">
    <p><b>Nothing recorded {days === 1 ? 'in the last 24 hours' : `in the last ${days} days`}.</b></p>
    <p class="ink-2">{@render nothing()}</p>
  </div>
{:else}
  {@render children()}
{/if}

<style>
  .mcfg {
    display: flex;
    flex-wrap: wrap;
    gap: 14px 22px;
    align-items: center;
    padding: 12px 16px;
  }
  .toggle {
    display: flex;
    gap: 9px;
    align-items: center;
    font-size: var(--t-sm);
  }
  .inline {
    display: inline-flex;
    gap: 8px;
    align-items: center;
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  /* p.hint, not .hint: it must outrank the page's `.wrap > *` width cap, which loads later and would win a tie. */
  p.hint {
    color: var(--ink-3);
    font-size: var(--t-xs);
    line-height: 1.5;
    max-width: 76ch;
  }
  .bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin: 14px 0 12px;
  }
  .empty {
    padding: 28px 4px;
  }
</style>
