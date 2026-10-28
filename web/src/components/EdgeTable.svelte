<script lang="ts">
  // The edge roll call: one row per CDN with the site it says served you, the round trip to that edge, how that
  // compares with the fibre floor, and whose network the edge sits in. The CDNs name their sites in their own
  // headers, so every "served from" is the CDN's word, not a guess from an address.
  import type { EdgeSite, RollCall } from '../lib/types';
  import { ago, holder, ms, plural } from '../lib/format';
  import CdnLogo from './CdnLogo.svelte';
  import Icon from './Icon.svelte';

  let {
    last,
    loaded,
    running,
    error,
    check,
  }: { last: RollCall | null; loaded: boolean; running: boolean; error: string; check: () => void } = $props();

  /** Rows to draw skeletons over before the first result lands, in the order the check asks. */
  const PLACEHOLDER = [
    { cdn: 'cloudflare', name: 'Cloudflare' },
    { cdn: 'fastly', name: 'Fastly' },
    { cdn: 'cloudfront', name: 'CloudFront' },
    { cdn: 'google', name: 'Google' },
    { cdn: 'meta', name: 'Meta' },
    { cdn: 'akamai', name: 'Akamai' },
  ];
  const skeletonRows = $derived(last?.edges.map((e) => ({ cdn: e.cdn, name: e.name })) ?? PLACEHOLDER);

  const you = $derived(last?.you ?? null);
  const youText = $derived.by(() => {
    if (!you) return '';
    const net = you.holder ? `${holder(you.holder)}${you.asn ? ` (AS${you.asn})` : ''}` : you.asn ? `AS${you.asn}` : 'an unknown network';
    return `Your connection: ${net}${you.city ? `, near ${you.city}` : ''}, per Cloudflare's geo-IP.`;
  });

  const named = $derived(last?.edges.filter((e) => e.site).length ?? 0);
  const inIsp = $derived(last && you?.asn ? last.edges.filter((e) => e.asn && e.asn === you.asn) : []);

  function site(e: EdgeSite): string {
    if (e.error) return 'No answer';
    if (e.method === 'none') return 'Not disclosed';
    if (!e.site) return 'Not named';
    return e.site_city ? `${e.site_city} (${e.site})` : e.site;
  }
  function siteTitle(e: EdgeSite): string {
    if (e.error) return e.error;
    if (e.method === 'none') return `${e.name} does not name its sites in anything a client can read, so only the round trip is shown.`;
    if (!e.site) return 'The CDN answered but named no site this time.';
    const where = e.site_city ? `${e.site_city}${e.site_country ? `, ${e.site_country}` : ''}` : 'a site Recursa has no city for';
    return `${e.name} names its site ${e.site_raw ?? e.site}: ${where}. Read from ${methodName(e.method)}.`;
  }
  const methodName = (m: EdgeSite['method']) =>
    ({
      'cf-headers': "Cloudflare's own response headers",
      'x-served-by': 'the x-served-by header, the last cache named',
      'x-amz-cf-pop': 'the x-amz-cf-pop header',
      report_mapping: "Google's report_mapping page",
      'fb-rdns': "the edge's reverse DNS name",
      none: 'nothing: the site is not disclosed',
    })[m];

  /** The round trip against the fibre floor: "2.9×" with the floor in the tooltip, blank unless both ends are known. */
  function vsFloor(e: EdgeSite): { text: string; title: string; tone: '' | 'far' } {
    if (e.rtt_ms == null || e.floor_ms == null) return { text: '', title: '', tone: '' };
    if (e.floor_ms < 0.5) return { text: 'local', title: 'The site is within a few tens of kilometres, so the floor says nothing useful.', tone: '' };
    const x = e.rtt_ms / e.floor_ms;
    return {
      text: `${x.toFixed(1)}×`,
      title: `Fibre floor ${ms(e.floor_ms)} for about ${Math.round(e.floor_ms * 100)} km, a rule of thumb: light in glass covers 100 km in a millisecond, there and back.`,
      tone: x > 4 ? 'far' : '',
    };
  }
  const network = (e: EdgeSite) => (e.holder ? holder(e.holder) : e.asn ? `AS${e.asn}` : e.ip ? 'Unknown network' : '');
  const quality = (e: EdgeSite) => {
    const q = e.quality;
    if (!q) return '';
    const bits = [q.label.toLowerCase()];
    if (q.rtt_ms != null) bits.push(`${q.rtt_ms} ms`);
    if (q.retransmits != null) bits.push(`${q.retransmits} retransmit${q.retransmits === 1 ? '' : 's'}`);
    if (q.bandwidth_kbps != null) bits.push(`${(q.bandwidth_kbps / 1000).toFixed(1)} Mb/s`);
    return bits.join(', ');
  };
</script>

{#if !loaded || (running && !last)}
  <div class="table panel" aria-busy="true" aria-label="Asking each CDN">
    <div class="row head" aria-hidden="true"><span>CDN</span><span>Served from</span><span>Round trip</span><span>vs floor</span><span>Next network</span></div>
    {#each skeletonRows as r (r.cdn)}
      <div class="row">
        <span class="cdn"><CdnLogo id={r.cdn} name={r.name} />{r.name}</span>
        <span><span class="skel now w60"></span></span>
        <span><span class="skel now w40"></span></span>
        <span><span class="skel now w24"></span></span>
        <span><span class="skel now w50"></span></span>
      </div>
    {/each}
  </div>
  <p class="fine">{loaded ? 'One request to each of six CDNs, about three seconds.' : 'Loading the last check.'}</p>
{:else if error && !last}
  <div class="empty panel">
    <p class="err"><Icon name="warn" size={14} />{error}</p>
    <p class="ink-2">The check asks six public CDN hosts over the system resolver. Nothing is sent but the request itself.</p>
    <button class="btn" onclick={check}>Try again</button>
  </div>
{:else if !last}
  <div class="empty panel">
    <p><b>Nothing checked yet.</b></p>
    <p class="ink-2">
      Ask the six biggest CDNs which of their sites is answering this network right now. Each names it in its own headers: a site
      near you, inside or next to your ISP, is what good connectivity looks like. The check takes about three seconds.
    </p>
    <button class="btn primary" onclick={check}><Icon name="play" size={12} />Check</button>
  </div>
{:else}
  {#if error}
    <p class="err top"><Icon name="warn" size={14} />{error} Showing the last check.</p>
  {/if}
  <p class="you">
    {youText}
    <span class="ink-3">Checked {ago(last.ts)}{running ? ', checking again' : ` in ${(last.elapsed_ms / 1000).toFixed(1)} s`}.</span>
  </p>

  <div class="table panel" class:dim={running} aria-busy={running}>
    <div class="row head" aria-hidden="true">
      <span>CDN</span><span>Served from</span><span title="Best of three TCP handshakes to the edge that answered">Round trip</span>
      <span title="Round trip over the great-circle fibre floor to the site. A rule of thumb, not a route.">vs floor</span>
      <span class="net" title="Whose network the edge address belongs to">Next network</span>
    </div>
    {#each last.edges as e (e.cdn)}
      {@const f = vsFloor(e)}
      <div class="row" class:failed={!!e.error}>
        <span class="cdn"><CdnLogo id={e.cdn} name={e.name} />{e.name}</span>
        <span class="site" title={siteTitle(e)} class:soft={!e.site}>{site(e)}</span>
        <span class="num">{e.rtt_ms == null ? '' : ms(e.rtt_ms)}</span>
        <span class="num" class:far={f.tone === 'far'} title={f.title}>{f.text}</span>
        <span class="net">
          {network(e)}
          {#if e.asn && you?.asn && e.asn === you.asn}<span class="tag" title="The edge address is inside your own ISP's network.">your ISP</span>{/if}
          {#if e.quality}
            <span class="q" title="Meta's own measurement of this connection, from an undocumented header. Fields: {e.quality.raw}">Meta sees it as {quality(e)}</span>
          {/if}
        </span>
      </div>
    {/each}
  </div>

  <details class="foot">
    <summary>
      <Icon name="chevron-right" size={12} />{named} of {last.edges.length} CDNs named their site{inIsp.length
        ? `; ${plural(inIsp.length, 'edge')} ${inIsp.length === 1 ? 'sits' : 'sit'} inside your ISP`
        : ''}. How to read this table
    </summary>
    <p>
      A site code is the airport nearest the data centre, the way CDNs name their own points of presence. Served from Ashburn means
      that site answered this request; it says nothing about which site a different request, or a different resolver, would get.
    </p>
    <p>
      These CDNs are anycast: the site you reach is the one your ISP's routes lead to, not the one closest on a map. A site far away
      with a short round trip is fine. A site nearby with a long round trip means the path there is long, or busy.
    </p>
    <p>
      The fibre floor is a rule of thumb, one millisecond per 100 km there and back on a straight line. Real fibre bends, so two to three
      times the floor is ordinary. Much more than that on a nearby site is worth a look over time.
    </p>
  </details>
{/if}

<style>
  .you {
    margin: 0 0 12px;
    font-size: var(--t-sm);
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
  }
  .ink-3 {
    color: var(--ink-3);
  }
  .table {
    padding: 4px 16px 6px;
    transition: opacity var(--dur) var(--ease-out);
  }
  .table.dim {
    opacity: 0.6;
  }
  .row {
    display: grid;
    grid-template-columns: 150px minmax(140px, 1.1fr) 92px 72px minmax(160px, 1.4fr);
    gap: 12px;
    align-items: center;
    padding: 10px 0;
    border-top: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .row.head {
    border-top: 0;
    padding: 8px 0 6px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .row.failed .site {
    color: var(--ink-3);
  }
  .cdn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-weight: 560;
  }
  .site.soft {
    color: var(--ink-3);
  }
  .far {
    color: var(--warning);
  }
  .net {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 8px;
    align-items: baseline;
    min-width: 0;
  }
  .q {
    flex-basis: 100%;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .tag {
    padding: 0 6px;
    border-radius: 8px;
    background: var(--sel);
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  /* Skeletons the width of the value that will replace them. */
  .skel {
    display: inline-block;
    height: 12px;
    border-radius: 3px;
    background: var(--surface-3);
  }
  .w60 {
    width: 60%;
  }
  .w50 {
    width: 50%;
  }
  .w40 {
    width: 40%;
  }
  .w24 {
    width: 24px;
  }
  .empty {
    padding: 28px 24px;
    display: grid;
    gap: 10px;
    justify-items: start;
    max-width: 62ch;
  }
  .empty p {
    margin: 0;
    font-size: var(--t-sm);
    line-height: 1.5;
  }
  .err {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--critical);
    font-size: var(--t-sm);
  }
  .err.top {
    margin: 0 0 10px;
  }
  .fine {
    color: var(--ink-3);
    font-size: var(--t-xs);
    margin: 10px 0 0;
  }
  .foot {
    display: grid;
    gap: 4px;
    margin-top: 12px;
    font-size: var(--t-xs);
    color: var(--ink-3);
    max-width: 110ch;
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
  @media (max-width: 900px) {
    .row {
      grid-template-columns: 120px minmax(0, 1fr) 72px 56px;
    }
    .row .net {
      grid-column: 1 / -1;
      padding-left: 0;
    }
    .row.head .net {
      display: none;
    }
  }
  :global(html.mobile) .table {
    padding: 4px 12px 6px;
  }
  :global(html.mobile) .foot {
    font-size: var(--t-sm);
  }
</style>
