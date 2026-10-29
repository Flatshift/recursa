<script lang="ts">
  // The map as a table: one row per destination, the same findings as the graph in the order of a reading. This is the
  // accessible view and the one a phone gets, so it keeps every figure the graph shows.
  import type { MapRun } from '../lib/types';
  import { destinations, endMs, evidenceLabel, evidenceText, isV6, leavesAt, linkLabel, markOf, siteName } from '../lib/netview';
  import { ms } from '../lib/format';
  import Provenance from './Provenance.svelte';

  let { run }: { run: MapRun } = $props();
  const rows = $derived(destinations(run));
  /** The hand-off cell: an IPv6 row says how its route compares, a destination with no IPv6 trace says so softly. */
  function handoff(d: (typeof rows)[number]): string {
    if (isV6(d.target)) return d.same_route ? 'same route as IPv4' : 'different route';
    if (d.link) return linkLabel(d.link, run.groups);
    if (!d.claim) return 'Not placed';
    return run.ipv6 && !rows.some((r) => r.target.id === `${d.target.id}-v6`) ? 'IPv4 only' : '';
  }
</script>

<div class="table panel">
  <div class="row head" aria-hidden="true">
    <span>Destination</span><span title="The site the CDN says answered, from its own headers">Served from</span><span>Leaves your ISP at</span><span>Hand-off</span>
    <span>Next network</span><span title="Best of three echo replies from the last router that answered">Round trip</span><span>Evidence</span>
  </div>
  {#each rows as d (d.target.id)}
    <div class="row" class:soft={!d.claim}>
      <span class="dest" data-label="Destination">{d.target.name}{#if isV6(d.target)} <span class="soft v6tag">IPv6</span>{/if}</span>
      <span data-label="Served from" class:soft={!d.target.site}>{d.target.site ?? ''}</span>
      <span data-label="Leaves your ISP at" class="soft">{leavesAt(d.near)}</span>
      <span data-label="Hand-off" class:soft={isV6(d.target) ? d.same_route : handoff(d) === 'IPv4 only'}>{handoff(d)}</span>
      <span data-label="Next network">{d.far?.label ?? ''}</span>
      <span data-label="Round trip" class="num">{endMs(d.trace) == null ? '' : ms(endMs(d.trace))}</span>
      <span data-label="Evidence" class="ev">
        {#each d.claim?.evidence ?? [] as e}
          <span class="mark" title="{evidenceLabel(e)}: {evidenceText(e)}"><Provenance kind={markOf(e)} titled={false} /><span class="sr">{evidenceLabel(e)}</span></span>
        {/each}
        {#if !d.trace.reached}<span class="soft" title={`${siteName(d.target)} did not answer at its last hop`}>did not answer</span>{/if}
      </span>
    </div>
  {/each}
</div>

<style>
  .table {
    padding: 2px 0 4px;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(120px, 1.1fr) 90px minmax(150px, 1.3fr) minmax(110px, 1fr) minmax(130px, 1.1fr) 84px 110px;
    gap: 16px;
    align-items: center;
    padding: 11px 16px;
    border-top: 1px solid var(--rule);
    font-size: var(--t-sm);
    line-height: 1.4;
  }
  .row:not(.head):hover {
    background: var(--surface-2);
  }
  .row > * {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .row.head {
    border-top: 0;
    padding: 10px 16px 8px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .dest {
    font-weight: 560;
  }
  .soft {
    color: var(--ink-3);
  }
  .v6tag {
    font-size: var(--t-xs);
    font-weight: 400;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .ev {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 8px;
    align-items: center;
  }
  .mark {
    display: inline-flex;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
  }
  /* Narrow: each destination is a small block with its labels beside the values. */
  @media (max-width: 900px) {
    .row {
      grid-template-columns: 1fr 1fr;
      gap: 6px 12px;
      padding: 12px 16px;
    }
    .row.head {
      display: none;
    }
    .row > :not(.dest)::before {
      content: attr(data-label);
      display: block;
      font-size: var(--t-xs);
      color: var(--ink-3);
    }
    .row > .dest {
      grid-column: 1 / -1;
    }
    .num {
      text-align: left;
    }
  }
  :global(html.mobile) .row {
    padding-inline: 12px;
  }
</style>
