<script lang="ts">
  // The networks the CDN edges sit in, one chip each with its edge count; your ISP's carries a mark. `live` is the run
  // naming them one by one: tinted chips that pop in, each after the one before (`enter` holds the delays in ms).
  import type { Snippet } from 'svelte';
  import type { EdgeNet } from '../lib/cdn';

  let { nets, live = false, enter, children }: { nets: EdgeNet[]; live?: boolean; enter?: Map<string, number>; children?: Snippet } = $props();
</script>

{#if nets.length}
  <div class="nets" aria-live={live ? 'polite' : undefined}>
    <span class="nets-h">Edge networks</span>
    {#each nets as x (x.key)}
      <span class="net" class:live class:you={x.you} style:animation-delay={enter ? `${enter.get(x.key) ?? 0}ms` : undefined}
        >{x.name} <span class="num nn">{x.n}</span>{#if x.you}<span class="nyou">your ISP</span>{/if}</span
      >
    {/each}
    {@render children?.()}
  </div>
{/if}

<style>
  .nets {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .nets-h {
    margin-right: 4px;
    color: var(--ink-3);
    font-weight: 600;
  }
  .net {
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--surface-2);
    border: 1px solid var(--rule);
    white-space: nowrap;
  }
  .net.live {
    background: color-mix(in srgb, var(--w-rose) 9%, var(--surface));
    border-color: color-mix(in srgb, var(--w-rose) 35%, var(--rule));
    animation: pop 420ms var(--ease-out) both;
  }
  .nn {
    color: var(--ink-3);
    margin-left: 2px;
  }
  .net.you {
    border-color: var(--good);
    color: var(--ink);
  }
  .nyou {
    margin-left: 6px;
    color: var(--good);
    font-weight: 600;
  }
  @media (prefers-reduced-motion: reduce) {
    .net.live {
      animation: none;
    }
  }
</style>
