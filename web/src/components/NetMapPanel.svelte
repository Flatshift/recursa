<script lang="ts">
  // The map half of the Network page: who you are, the headline, the graph or its table, and the switches for the two
  // optional data sources. Before the first run it says what leaves this machine, because that is the question to answer
  // before a button that sends traffic.
  import { app } from '../lib/store.svelte';
  import type { netmap } from '../lib/netmap.svelte';
  import { ago, duration, holder } from '../lib/format';
  import { asrelMonth as monthOf } from '../lib/netview';
  import Icon from './Icon.svelte';
  import NetMap from './NetMap.svelte';
  import NetMapTable from './NetMapTable.svelte';

  let { nm }: { nm: ReturnType<typeof netmap> } = $props();

  const WITHHELD = "Not enough evidence for a headline yet: your ISP's last routers don't answer.";
  const d = $derived(nm.data);
  const running = $derived(nm.running);
  // While a run is going the picture is the one filling in; the last complete one comes back when it ends.
  const run = $derived(running ? (d?.progress?.partial ?? null) : (d?.last ?? null));
  const v6Traces = $derived((run?.traces ?? []).filter((t) => t.target.family === 6));
  const v6Total = $derived(v6Traces.length);
  const v6Same = $derived(v6Traces.filter((t) => t.same_route === true).length);
  const sets = $derived(d?.datasets ?? null);
  const asrelMonth = $derived(sets?.asrel?.source ? monthOf(sets.asrel.source) : null);
  const cap = $derived(app.env?.trace);
  const needsAdmin = $derived(cap === 'needs_admin');
  const unsupported = $derived(cap === 'unsupported');

  const you = $derived.by(() => {
    const y = run?.you ?? d?.last?.you;
    if (!y) return '';
    const net = y.holder ? `${holder(y.holder)}${y.asn ? ` (AS${y.asn})` : ''}` : y.asn ? `AS${y.asn}` : 'an unknown network';
    return `Your connection: ${net}${y.owner ? `, owned by ${y.owner}` : ''}${y.city ? `, near ${y.city}` : ''}, per Cloudflare's geo-IP.`;
  });
  const headline = $derived(run?.headline?.text ?? (run && !running ? WITHHELD : ''));
  const label = $derived(running ? 'Mapping the way to each destination' : (headline || 'No map yet'));

  // The table is the default below 900 px, where the graph would be too small to read; a pick overrides it.
  let narrow = $state(typeof matchMedia !== 'undefined' && matchMedia('(max-width: 900px)').matches);
  $effect(() => {
    const q = matchMedia('(max-width: 900px)');
    const f = () => (narrow = q.matches);
    q.addEventListener('change', f);
    return () => q.removeEventListener('change', f);
  });
  let pick = $state<'map' | 'table' | null>(null);
  const view = $derived(pick ?? (narrow ? 'table' : 'map'));
  let reopen = $state(false);

  const isoDay = (ms: number) => new Date(ms).toISOString().slice(0, 10);
  const first = $derived(nm.loaded && !d?.last && !running);
</script>

{#snippet privacy(withMap: boolean)}
  <div class="empty panel">
    {#if withMap}
      <p><b>Nothing mapped yet.</b></p>
      <p class="ink-2">Recursa traces the way to twenty sites and draws where your ISP hands them off.</p>
    {/if}
    <p><b>What leaves this machine.</b> About 60 small probe packets to each of about 30 named sites (nine CDNs, two public DNS services and an NTP server), again over IPv6 when this computer has a route, and nothing else.</p>
    <p>
      <b>What is fetched.</b> Three public datasets, with no information about you in the request: prefix-to-network tables from CAIDA (about 3 MB,
      daily), the CAIDA AS relationships table (about 2 MB, monthly) and, if you allow it, PeeringDB's exchange and facility lists (about 14 MB once, then small updates).
    </p>
    {#if sets?.peeringdb.accepted}
      <p class="ink-2"><Icon name="check" size={13} />PeeringDB is allowed.</p>
    {:else}
      <button class="btn" onclick={() => nm.allowPeeringDb(true)}>Allow PeeringDB</button>
    {/if}
    <p>
      <b>What may be asked.</b> If you allow it, a router past your ISP that no network announces is looked up in the address registry (one small
      request per block, to the regional registry via rdap.org). The request carries that router's address, never one of yours, and the answer is
      kept for a month.
    </p>
    {#if sets?.rdap.accepted}
      <p class="ink-2"><Icon name="check" size={13} />Registry lookups are allowed.</p>
    {:else}
      <button class="btn" onclick={() => nm.allowRegistry(true)}>Allow registry lookups</button>
    {/if}
    <p><b>What stays here.</b> Every trace and the map, in Recursa's own database. Router names are only looked up if you turn that on, and then your DNS server sees them.</p>
    {#if withMap}
      <button class="btn primary" disabled={needsAdmin || unsupported} onclick={nm.start}><Icon name="play" size={12} />Map</button>
    {/if}
  </div>
{/snippet}

{#if !nm.loaded}
  <div class="panel skeleton" aria-busy="true" aria-label="Loading the last map"><span class="skel now w40"></span></div>
{:else if unsupported || needsAdmin}
  <div class="empty panel">
    <p><b>{unsupported ? 'The map is not available in this build.' : 'Mapping needs administrator rights here.'}</b></p>
    <p class="ink-2">
      {#if unsupported}
        It needs the path tracer, which this platform does not offer.
      {:else}
        <Icon name="lock" size={13} />Needs administrator rights on {app.env?.platform === 'windows' ? 'Windows' : 'this computer'}. Start Recursa as an administrator to map.
      {/if}
    </p>
  </div>
{:else if first}
  {#if nm.error}<p class="err"><Icon name="warn" size={14} />{nm.error}</p>{/if}
  {@render privacy(true)}
{:else}
  {#if nm.error}
    <p class="err"><Icon name="warn" size={14} />{nm.error}{run ? ' Showing the last map.' : ''}</p>
  {/if}
  {#if headline}<p class="headline" class:withheld={!run?.headline}>{headline}</p>{/if}
  {#if you}
    <p class="you">
      <span>{you}</span>
      {#if run && !running}<span class="ink-3">Mapped {ago(run.ts)} in {duration(run.elapsed_ms)}.</span>{/if}
    </p>
  {/if}

  <div class="bar">
    <div class="segmented" role="tablist" aria-label="Map or table">
      <button role="tab" aria-selected={view === 'map'} onclick={() => (pick = 'map')}>Map</button>
      <button role="tab" aria-selected={view === 'table'} onclick={() => (pick = 'table')}>Table</button>
    </div>
    <div class="sources">
      <label class="toggle" title="Router names are looked up through your system DNS server, one per address, only while this is on.">
        <input type="checkbox" checked={sets?.rdns ?? false} onchange={(e) => nm.decodeNames(e.currentTarget.checked)} />
        <span>Decode router names<span class="ink-3">, your DNS server sees their names</span></span>
      </label>
      <label class="toggle" title="Exchange and facility lists, about 14 MB once and then small updates, with nothing about you in the request.">
        <input type="checkbox" checked={sets?.peeringdb.accepted ?? false} onchange={(e) => nm.allowPeeringDb(e.currentTarget.checked)} />
        <span>
          PeeringDB<span class="ink-3">
            {#if !sets?.peeringdb.accepted}, names exchanges and facilities{:else if sets.peeringdb.synced_at}, synced {isoDay(sets.peeringdb.synced_at)}{:else}, first sync running{/if}
          </span>
        </span>
      </label>
      <label class="toggle" title="Who holds an address block no network announces, from the regional registry. Only routers past your ISP are asked about.">
        <input type="checkbox" checked={sets?.rdap.accepted ?? false} onchange={(e) => nm.allowRegistry(e.currentTarget.checked)} />
        <span>
          Registry lookups<span class="ink-3">
            {#if !sets?.rdap.accepted}, names blocks no network announces{:else if sets.rdap.blocks}, {sets.rdap.blocks} block{sets.rdap.blocks === 1 ? '' : 's'} kept{/if}
          </span>
        </span>
      </label>
      {#if sets && sets.pfx2as.fetched_at == null}
        <span class="chip">Network names need the CAIDA table, fetching</span>
      {/if}
    </div>
  </div>

  {#if view === 'table' && run}
    <NetMapTable {run} />
  {:else}
    <NetMap {run} {label} />
  {/if}
  {#if run?.skipped.length}
    <p class="fine">Not traced: {run.skipped.map((s) => `${s.target} (${s.reason})`).join(', ')}.</p>
  {/if}

  {#if reopen}{@render privacy(false)}{/if}
  <details class="foot">
    <summary><Icon name="chevron-right" size={12} />How to read this map</summary>
    <p>
      Each box is a network the traffic crosses, named from the announced prefix that holds the router's address. The line between two boxes is
      where one hands the traffic to the next; the line at the edge of your ISP is the hand-off. A city under a box is only ever "consistent with":
      the round trip to that router leaves room for it, which is not proof that the router is there. The figure under a box is the round trip
      from you to its first router; the figure on a line is how much that grows from one box to the next. "Owned by" names who the
      network's number is registered to in the address registry when that is another organisation than the name it goes by, which
      is how a network reads after it is bought. "Hops hidden" on a line counts routers that did not answer: they are not another network, only unseen, often an MPLS
      core that hides its routers, and the hand-off is at one of them or at the next box's first router.
    </p>
    <p>
      Every sentence names its evidence. ● was seen in the trace, ○ was read from a list someone else keeps, – was worked out here from the two.
      A word like "peering" appears only when a list says so.
    </p>
  </details>
  <p class="fine">
    <button class="link" onclick={() => (reopen = !reopen)}>What leaves this machine</button>
    {#if sets?.pfx2as.source}<span> · Prefix data: {sets.pfx2as.source}{#if sets.pfx2as.source6}, {sets.pfx2as.source6}{/if}</span>{/if}
    {#if asrelMonth}<span> · Relationships: {asrelMonth}</span>{/if}
  </p>
  {#if run?.ipv6 != null}
    <p class="fine">
      {#if !run.ipv6}
        IPv6: no route from this computer, so this map is IPv4 only.
      {:else}
        IPv6: {v6Same} of {v6Total} destinations take the same route as over IPv4.
      {/if}
    </p>
  {/if}
  {#if asrelMonth}
    <p class="fine">The CAIDA AS Relationships Dataset, {asrelMonth}, caida.org/catalog/datasets/as-relationships</p>
  {/if}
{/if}

<style>
  .headline {
    margin: 0 0 6px;
    font-size: var(--t-md);
    font-weight: 620;
    line-height: 1.35;
    letter-spacing: -0.01em;
    max-width: 68ch;
    text-wrap: balance;
  }
  .headline.withheld {
    font-size: var(--t-base);
    font-weight: 400;
    letter-spacing: 0;
    color: var(--ink-2);
  }
  .you {
    margin: 0 0 16px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
  }
  .ink-3 {
    color: var(--ink-3);
  }
  .bar {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 10px 24px;
    align-items: center;
    margin: 0 0 12px;
    font-size: var(--t-sm);
  }
  .sources {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 14px;
    align-items: center;
    min-width: 0;
  }
  .toggle {
    display: inline-flex;
    gap: 8px;
    align-items: center;
    color: var(--ink-2);
    min-width: 0;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 9px;
    border: 1px solid var(--rule-2);
    border-radius: 11px;
    font-size: var(--t-xs);
    color: var(--ink-2);
    white-space: nowrap;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    font-size: inherit;
    color: var(--ink);
    text-decoration: underline;
    text-decoration-color: var(--rule-2);
  }
  .skeleton {
    padding: 20px 16px;
  }
  .skel {
    display: inline-block;
    height: 12px;
    border-radius: 3px;
    background: var(--surface-3);
  }
  .w40 {
    width: 40%;
  }
  .empty {
    padding: 24px;
    display: grid;
    gap: 10px;
    justify-items: start;
    max-width: 66ch;
  }
  .empty p {
    margin: 0;
    font-size: var(--t-base);
    line-height: 1.55;
  }
  .err {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 10px;
    color: var(--critical);
    font-size: var(--t-sm);
  }
  .fine {
    color: var(--ink-3);
    font-size: var(--t-xs);
    margin: 10px 0 0;
  }
  .foot {
    display: grid;
    gap: 6px;
    margin-top: 14px;
    font-size: var(--t-sm);
    color: var(--ink-3);
    max-width: 80ch;
  }
  .foot[open] summary {
    color: var(--ink-2);
  }
  .foot p {
    color: var(--ink-2);
  }
  .foot summary {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    cursor: default;
    list-style: none;
  }
  .foot summary::-webkit-details-marker {
    display: none;
  }
  .foot[open] summary :global(svg) {
    transform: rotate(90deg);
  }
  .foot p {
    margin: 0;
    line-height: 1.5;
  }
  :global(html.mobile) .foot {
    font-size: var(--t-sm);
  }
</style>
