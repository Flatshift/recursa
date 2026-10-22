<script lang="ts" module>
  // Which sections are open is shared by the Inspector and the resolver page, so a section a user opened stays open
  // when they move between the two. Unset until the user or a result touches it.
  let opened = $state<Record<string, boolean>>({});
  export const openSection = (key: string) => (opened[key] = true);
</script>

<script lang="ts">
  // One section of a resolver's detail: a title, a one-line summary while it is folded, an optional action button
  // and the body. Problems open by default; healthy sections fold to their summary.
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  let {
    key,
    title,
    summary = '',
    problem = false,
    defaultOpen = false,
    action,
    children,
  }: { key: string; title: string; summary?: string; problem?: boolean; defaultOpen?: boolean; action?: Snippet; children: Snippet } = $props();

  const open = $derived(opened[key] ?? (problem || defaultOpen));
</script>

<section class="sec" class:problem data-section={key}>
  <div class="sec-head">
    <button class="sec-toggle" aria-expanded={open} aria-label="{title}: {open ? 'fold' : summary || 'unfold'}" onclick={() => (opened[key] = !open)}>
      <span class="chev"><Icon name={open ? 'chevron-down' : 'chevron-right'} size={13} /></span>
      <h3>{title}</h3>
      {#if !open && summary}<span class="sum">{summary}</span>{/if}
    </button>
    {#if action}<div class="sec-act">{@render action()}</div>{/if}
  </div>
  {#if open}
    <div class="sec-body">{@render children()}</div>
  {/if}
</section>

<style>
  .sec {
    border-top: 1px solid var(--rule);
  }
  .sec-head {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 40px;
    padding: 4px 0;
  }
  .sec-toggle {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 4px 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: default;
  }
  .sec-toggle:hover h3 {
    color: var(--ink);
  }
  .chev {
    flex: none;
    align-self: center;
    color: var(--ink-3);
  }
  h3 {
    flex: none;
    margin: 0;
    font-size: var(--t-sm);
    font-weight: 650;
  }
  .problem h3::after {
    content: '';
    display: inline-block;
    width: 6px;
    height: 6px;
    margin: 0 0 1px 7px;
    border-radius: 50%;
    background: var(--critical);
  }
  .sum {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .sec-act {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .sec-body {
    padding: 2px 0 16px;
  }
</style>
