<script lang="ts">
  import { isBlank, type Cell } from '../lib/cells';
  import Glyph from './Glyph.svelte';
  import Provenance from './Provenance.svelte';
  // `sheet`: in the capability sheet, where measured is the default and says so once in the legend, the value may wrap
  // and only a published value carries its mark.
  let { c, compact = false, sheet = false }: { c: Cell; compact?: boolean; sheet?: boolean } = $props();
  const blank = $derived(isBlank(c));
</script>

{#if blank}
  <span class="none" title="Unknown: not documented and not measured">—</span>
{:else}
  <span class="cell" class:compact class:sheet title={c.evidence ? `${c.text}: ${c.evidence}` : c.text}>
    <Glyph status={c.status} size={13} />
    {#if sheet}
      <span class="t">{c.text}{#if c.prov !== 'unknown' && c.prov !== 'verified'}&nbsp;<Provenance kind={c.prov} />{/if}</span>
    {:else}
      <span class="t">{c.text}</span>
      {#if c.prov !== 'unknown'}<Provenance kind={c.prov} />{/if}
    {/if}
  </span>
{/if}

<style>
  .cell {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    white-space: nowrap;
    font-size: var(--t-xs);
  }
  .t {
    color: var(--ink);
    max-width: 30ch;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .compact .t {
    max-width: 14ch;
  }
  .sheet {
    align-items: start;
    white-space: normal;
  }
  .sheet .t {
    max-width: none;
    overflow: visible;
  }
  .sheet > :global(.glyph) {
    margin-top: 1px;
  }
  .none {
    color: var(--ink-3);
    font-size: var(--t-xs);
  }
</style>
