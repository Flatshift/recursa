<script lang="ts">
  // The list a page collapses to while a resolver is inspected beside it: one line per resolver with its score as a
  // thin bar, so the detail pane gets the width. The header takes the full view back. ↑↓ walk it (lib/select).
  import Icon from './Icon.svelte';

  export type Pick = { id: string; name: string; sub?: string; score?: number | null };

  let {
    groups,
    selected,
    back,
    collapsed = false,
    onselect,
    onback,
  }: {
    /** Sections in the order the full view draws them; a single unnamed section needs no heading. */
    groups: [string, Pick[]][];
    selected: string | null;
    /** What the header returns to, e.g. "All results". */
    back: string;
    /** Too narrow for both: the list keeps its place (so ↑↓ still walk it) but takes no room. */
    collapsed?: boolean;
    onselect: (id: string) => void;
    onback: () => void;
  } = $props();

  // Bars are relative to the slowest score on the list; a row without a score draws none.
  const max = $derived(Math.max(0, ...groups.flatMap(([, ps]) => ps.map((p) => p.score ?? 0))));
  const scoreText = (v: number) => (v < 10 ? v.toFixed(1) : Math.round(v).toString());
</script>

<nav class="shortlist" class:collapsed aria-label="Resolvers">
  <button class="back" onclick={onback} title="Back to the full view (Esc)"><Icon name="chevron-left" size={14} /><span>{back}</span></button>
  <div class="rows scroll">
    {#each groups as [g, ps] (g)}
      {#if groups.length > 1}<h3>{g}</h3>{/if}
      {#each ps as p (p.id)}
        <button class="row" class:sel={p.id === selected} data-resolver={p.id} aria-current={p.id === selected ? 'true' : undefined} onclick={() => onselect(p.id)}>
          <span class="top">
            <span class="nm">{p.name}</span>
            {#if p.score != null}<span class="num sc">{scoreText(p.score)}</span>{/if}
          </span>
          {#if p.score != null && max > 0}
            <span class="bar"><span class="fill" style:width="{Math.max(2, (100 * p.score) / max)}%"></span></span>
          {:else if p.sub}
            <span class="sub mono">{p.sub}</span>
          {/if}
        </button>
      {/each}
    {/each}
  </div>
</nav>

<style>
  .shortlist {
    flex: none;
    width: 280px;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--rule);
    background: var(--surface);
  }
  .shortlist.collapsed {
    visibility: hidden;
    width: 0;
    border-right: 0;
  }
  .back {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: none;
    margin: 10px 10px 4px;
    padding: 5px 8px 5px 4px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--accent);
    font-size: var(--t-sm);
    font-weight: 560;
    text-align: left;
  }
  .back:hover {
    background: var(--surface-2);
  }
  .rows {
    flex: 1;
    min-height: 0;
    padding: 0 10px 24px;
  }
  h3 {
    margin: 12px 8px 4px;
    font-size: var(--t-2xs);
    font-weight: 620;
    color: var(--ink-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .row {
    display: grid;
    gap: 4px;
    width: 100%;
    padding: 7px 8px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--ink);
    text-align: left;
  }
  .row:hover {
    background: var(--surface-2);
  }
  .row.sel {
    background: var(--side-sel);
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    font-size: var(--t-sm);
  }
  .nm {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sc {
    flex: none;
    color: var(--ink-2);
    font-variation-settings: 'MONO' 1;
  }
  .bar {
    display: block;
    height: 3px;
    border-radius: 2px;
    background: var(--rule);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    border-radius: 2px;
    background: var(--ink-3);
  }
  .row.sel .fill {
    background: var(--accent);
  }
  .sub {
    font-size: var(--t-2xs);
    color: var(--ink-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
