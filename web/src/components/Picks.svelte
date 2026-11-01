<script lang="ts">
  // The picks under a page's verdict: a goal, what takes it, and the evidence. In a narrow verdict they stack; `rows`
  // sets each goal beside its name instead. The verdict around them is the container that decides.
  let { picks, rows = false }: { picks: { goal: string; name: string; ev: string }[]; rows?: boolean } = $props();
</script>

<dl class="picks" class:rows>
  {#each picks as p}
    <div class="pick">
      <dt>{p.goal}</dt>
      <dd><span class="pn">{p.name}</span><span class="ev">{p.ev}</span></dd>
    </div>
  {/each}
</dl>

<style>
  .picks {
    margin: 0;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(0, 1fr);
    padding-top: 16px;
    border-top: 1px solid var(--rule);
  }
  .pick {
    display: grid;
    align-content: start;
    gap: 4px;
    padding: 0 18px;
    border-left: 1px solid var(--rule);
  }
  .pick:first-child {
    padding-left: 0;
    border-left: 0;
  }
  dt {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  dd {
    margin: 0;
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .pn {
    font-weight: 620;
    font-size: var(--t-sm);
  }
  .ev {
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  @container (max-width: 640px) {
    .picks {
      grid-auto-flow: row;
      gap: 12px;
    }
    .pick {
      padding: 12px 0 0;
      border-left: 0;
      border-top: 1px solid var(--rule);
    }
    .pick:first-child {
      padding-top: 0;
      border-top: 0;
    }
    .rows {
      gap: 0;
      padding-top: 0;
      border-top: 0;
    }
    .rows .pick,
    .rows .pick:first-child {
      grid-template-columns: 150px minmax(0, 1fr);
      gap: 12px;
      padding: 9px 0;
      border-top: 1px solid var(--rule);
    }
  }
  :global(html.mobile) .rows .pick,
  :global(html.mobile) .rows .pick:first-child {
    grid-template-columns: 112px minmax(0, 1fr);
  }
</style>
