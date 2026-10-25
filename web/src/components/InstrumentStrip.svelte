<script lang="ts" module>
  const CONDITIONS = 'conditions';
  /** Open the measurement conditions from elsewhere: on a phone the strip only shows when something is wrong. */
  export function showConditions() {
    document.getElementById(CONDITIONS)?.showPopover();
  }
</script>

<script lang="ts">
  // The instrument describing itself: where it measures from and how precisely. The bar says only
  // what changes the reading of a result and grows louder when something is wrong; a click opens the
  // full conditions.
  import type { Env, LinkCause, LinkStats, LinkUnavailable } from '../lib/types';
  import { holder, score, us } from '../lib/format';
  import { isMac } from '../lib/keys';
  import { mobile } from '../lib/api';
  import Icon from './Icon.svelte';

  let {
    env,
    now = null,
    link,
    frozen = false,
  }: { env: Env | null; now?: Env | null; link?: LinkStats | LinkUnavailable; frozen?: boolean } = $props();
  const v = $derived(env?.vantage ?? null);
  const cal = $derived(env?.calibration ?? null);
  const icpt = $derived(env?.interception ?? null);
  const iface = $derived(icpt?.link ?? null);
  const gw = $derived(link && !('state' in link) ? link : null);
  const unchecked = $derived(link && 'state' in link ? link : null);
  // Noisy when stalls pass 2% of the echoes or the p95 passes 20 ms. Mirrors LinkSummary::noisy in crates/engine/src/link.rs.
  const noisy = $derived(!!gw && (gw.stalls > 0.02 * gw.sent || gw.p95 > 20));
  const caught = $derived(icpt?.verdict === 'intercepted' || icpt?.verdict === 'partial');
  // What this machine can say about the stalls (crates/engine/src/link.rs classifies them).
  const machine = mobile ? 'iPhone' : isMac ? 'Mac' : 'computer';
  const causes: Record<LinkCause, string> = {
    busy: `Likely cause: this ${machine} was busy.`,
    traffic: `Likely cause: other traffic on this ${machine}.`,
    awdl: 'Likely cause: AirDrop or another Apple device feature using the Wi-Fi radio.',
    driver: 'Likely cause: the network driver reported errors or dropped packets.',
    unknown: `Nothing else on this ${machine} explains the stalls: the Wi-Fi link itself.`
  };
  const cause = $derived(gw?.host?.cause ? causes[gw.host.cause] : null);
  // A full-tunnel VPN carries the route to 1.1.1.1, so the link is a tunnel interface: the vantage and
  // the latencies are the VPN's, not this network's.
  const vpn = $derived(!!iface && /^(utun|ipsec|ppp|tun|tap|wg)\d/.test(iface.interface));
  // Cellular interfaces are pdp_ip* on iOS; the engine only knows Wi-Fi from not-Wi-Fi.
  const kind = $derived(iface ? (iface.wifi ? 'Wi-Fi' : iface.interface.startsWith('pdp_ip') ? 'Cellular' : 'Wired') : '');
  const medium = $derived(vpn ? 'Link' : iface ? kind : 'Router');
  // A recorded run from another network than this one: comparing it with today's runs compares networks.
  const nv = $derived(now?.vantage ?? null);
  const elsewhere = $derived.by(() => {
    if (!frozen || !v || !nv) return false;
    const [a, b] = [v.network?.asn, nv.network?.asn];
    return a && b ? a !== b : (v.ipv4 ?? v.ipv6) !== (nv.ipv4 ?? nv.ipv6);
  });
  const stallPct = $derived(gw && gw.sent ? Math.max(1, Math.round((100 * gw.stalls) / gw.sent)) : 0);

  // Phone: the strip takes screen only when it has something to warn about; the conditions open from a run's menu.
  const loud = $derived(caught || noisy || vpn || elsewhere);
  const shown = $derived(!mobile || loud);
  $effect(() => {
    document.documentElement.classList.toggle('strip-on', shown);
  });

  let bar: HTMLButtonElement | undefined = $state();
  let pop: HTMLDivElement | undefined = $state();
  let open = $state(false);
  const id = CONDITIONS;

  // Above the chevron that opens it, flush with the bar's right edge, so it reads as that control's sheet.
  // Set before the popover shows, so it never paints at the corner first; its width is fixed by CSS.
  function placePop() {
    if (!pop) return;
    const w = Math.min(420, innerWidth - 16);
    if (!bar) {
      pop.style.left = `${(innerWidth - w) / 2}px`;
      pop.style.bottom = 'calc(8px + env(safe-area-inset-bottom))';
      return;
    }
    const b = bar.getBoundingClientRect();
    pop.style.left = `${Math.max(8, Math.min(b.right - 8 - w, innerWidth - w - 8))}px`;
    pop.style.bottom = `${innerHeight - b.top + 6}px`;
  }
</script>

{#if shown}
<footer class="strip chrome">
  <button
    bind:this={bar}
    class="bar mono"
    type="button"
    popovertarget={id}
    aria-expanded={open}
    aria-label="Measurement conditions{frozen ? ', recorded with this run' : ''}"
  >
    {#if frozen}<span class="mark">Recorded</span>{/if}
    <span class="where">
      {#if v}
        <span>{v.ipv4 ?? v.ipv6 ?? 'unknown'}</span>
        {#if v.network?.asn}<span class="dim net">AS{v.network.asn} <span class="name">{holder(v.network.holder)}</span></span>{/if}
      {:else}
        <span class="dim">Finding vantage…</span>
      {/if}
    </span>
    <span class="checks">
      {#if elsewhere}<span class="check flag"><span class="wide">Not your current network</span><span class="tight">Other network</span></span>{/if}
      {#if vpn}<span class="check flag">VPN <span class="dim wide">{iface?.interface}</span></span>{/if}
      <span class="check" class:alert={caught} class:calm={!caught}>
        {#if !icpt}<span class="dim">Port 53 …</span>
        {:else if icpt.verdict === 'direct'}<span class="dim">Port 53</span> direct
        {:else if icpt.verdict === 'intercepted'}<Icon name="warn" size={12} /> Port 53 intercepted
        {:else if icpt.verdict === 'partial'}<Icon name="warn" size={12} /> Port 53 partly intercepted
        {:else}<span class="dim">Port 53 unknown</span>{/if}
      </span>
      {#if noisy}
        <span class="check alert"><Icon name="warn" size={12} /> {medium} stalls {stallPct}%</span>
      {/if}
      {#if cal && !env?.kernel_rx}
        <span class="check minor"><span class="dim">Userspace timing · floor</span> {us(cal.floor_p50_us)}</span>
      {/if}
    </span>
    <span class="chev" class:up={!open}><Icon name="chevron-down" size={12} /></span>
  </button>
</footer>
{/if}

<div
  bind:this={pop}
  {id}
  class="pop"
  popover="auto"
  role="dialog"
  aria-label="Measurement conditions"
  onbeforetoggle={(e) => {
    open = (e as ToggleEvent).newState === 'open';
    if (open) placePop();
  }}
>
  <header>
    <b>Measurement conditions</b>
    <span class="dim">{frozen ? 'Recorded with this run' : 'This network, now'}</span>
  </header>
  <dl>
    <dt>Vantage</dt>
    <dd>
      {#if v}
        <span class="mono">{v.ipv4 ?? v.ipv6 ?? 'unknown'}</span>
        {#if v.network?.asn}<span class="mono">AS{v.network.asn}</span> {holder(v.network.holder)}{/if}
        {#if elsewhere && nv}
          <small class="note">
            Now on {nv.network?.asn ? `AS${nv.network.asn} ${holder(nv.network.holder)}` : (nv.ipv4 ?? nv.ipv6)}: comparing
            this run with today's compares two networks.
          </small>
        {/if}
        <small>Your public address as Cloudflare sees it (OpenDNS if Cloudflare is unreachable). Network from Team Cymru.</small>
      {:else}Finding your public address…{/if}
    </dd>

    <dt>IPv6</dt>
    <dd>{v ? (v.ipv6_reachable ? 'Reaches resolvers directly' : 'Not reachable from this network') : '…'}</dd>

    <dt>System</dt>
    <dd>
      <span class="mono">{env?.system?.length ? env.system.join(', ') : 'none found'}</span>
      <small>The resolvers this computer is set to use.</small>
    </dd>

    <dt>Port 53</dt>
    <dd class:warn={caught}>
      {#if !icpt}Checking…
      {:else if icpt.verdict === 'direct'}Direct
      {:else if icpt.verdict === 'intercepted'}Intercepted
      {:else if icpt.verdict === 'partial'}Partly intercepted
      {:else}Unknown{/if}
      {#if caught && icpt?.fingerprint}<span class="dim"> by {icpt.fingerprint.name}</span>{/if}
      <small>
        {caught
          ? 'A device on this network answers plain DNS meant for other resolvers. UDP rows are measured but not ranked.'
          : 'Plain DNS reaches the resolvers you address; no device on this network answers for them.'}
      </small>
    </dd>

    <dt>Link</dt>
    <dd class:warn={noisy}>
      {#if iface}{vpn ? 'VPN' : kind} <span class="mono dim">{iface.interface}</span>{/if}
      {#if vpn}<small class="note">Traffic leaves through a VPN tunnel: the vantage and the latencies are the VPN's, not this network's.</small>{/if}
      {#if gw}
        {iface ? ' · ' : ''}router <span class="mono">{score(gw.p50)} ms</span>, p95 <span class="mono">{score(gw.p95)} ms</span>
        <small>
          Pinged {gw.gateway} every 100 ms during the run: {gw.stalls} of {gw.sent} stalled.{noisy
            ? ' Stalls on this link inflate the speed scores.'
            : ''}{cause ? ' ' + cause : ''}
        </small>
      {:else if unchecked}
        <small>Not checked: {unchecked.reason}</small>
      {:else}
        <small>The router is timed during each run.</small>
      {/if}
    </dd>

    <dt>Timing</dt>
    <dd>
      {env?.kernel_rx ? 'Kernel receive stamps' : 'Userspace stamps'}
      {#if cal}
        · floor <span class="mono">{us(cal.floor_p50_us)}</span>
        <small>
          The loopback round trip through the same code: the most the tool itself can add.{env?.kernel_rx &&
          cal.stamped > 0
            ? ` Kernel stamps remove the ${us(cal.wake_p50_us)} wake-up delay.`
            : ''}
        </small>
      {/if}
    </dd>
  </dl>
</div>

<style>
  /* The window's status bar: the instrument describing itself. */
  .strip {
    height: var(--status-h);
    box-sizing: content-box;
    padding-bottom: env(safe-area-inset-bottom);
    border-top: 1px solid var(--rule);
    background: var(--side);
    container-type: inline-size;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 16px;
    width: 100%;
    height: 100%;
    padding: 0 10px 0 14px;
    border: 0;
    border-radius: 0;
    background: transparent;
    font-size: var(--t-xs);
    color: var(--ink-2);
    text-align: left;
    overflow: hidden;
    transition: background-color 150ms ease-out;
  }
  .bar:hover,
  .bar[aria-expanded='true'] {
    background: var(--surface-2);
  }
  .mark {
    flex-shrink: 0;
    color: var(--ink);
    font-weight: 600;
  }
  /* Where: the address holds, the network's name gives way first (the popover has it whole). */
  .where {
    display: flex;
    gap: 8px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .where > * {
    flex-shrink: 0;
  }
  .where > .net {
    flex-shrink: 1;
    min-width: 7ch;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Trust: pushed to the right edge, and never clipped. */
  .checks {
    display: flex;
    gap: 14px;
    margin-left: auto;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .chev {
    display: inline-grid;
    flex-shrink: 0;
    color: var(--ink-3);
    transition: transform 150ms ease-out;
  }
  .chev.up {
    transform: rotate(180deg);
  }
  .dim {
    color: var(--ink-3);
  }
  /* Something that changes how to read the results: the only loud thing in the bar. */
  .alert {
    padding: 1px 6px;
    margin: 0 -2px;
    border-radius: var(--r-sm);
    background: var(--warn-soft);
    color: var(--ink);
    font-weight: 600;
  }
  /* Context that changes what the numbers describe, without anything being wrong. */
  .flag {
    padding: 0 5px;
    margin: 0 -2px;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    color: var(--ink);
    font-weight: 600;
  }
  /* Narrow windows drop the least useful details first; alerts and flags always stay. */
  @container (width < 760px) {
    .net .name,
    .minor {
      display: none;
    }
  }
  .tight {
    display: none;
  }
  @container (width < 520px) {
    .net,
    .calm,
    .wide {
      display: none;
    }
    .tight {
      display: inline;
    }
  }

  .pop {
    position: fixed;
    inset: auto;
    margin: 0;
    width: min(420px, calc(100vw - 16px));
    max-height: calc(100vh - var(--status-h) - 20px);
    overflow-y: auto;
    padding: 12px 14px 14px;
    border: 0;
    border-radius: var(--r-lg);
    background: var(--surface);
    color: var(--ink-2);
    box-shadow: var(--shadow-pop);
    font-size: var(--t-xs);
    line-height: 1.45;
  }
  .pop header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 12px;
    padding-bottom: 8px;
    margin-bottom: 4px;
    border-bottom: 1px solid var(--rule);
  }
  .pop header b {
    color: var(--ink);
    font-size: var(--t-sm);
  }
  .pop dl {
    display: grid;
    grid-template-columns: 64px 1fr;
    column-gap: 12px;
    margin: 0;
  }
  .pop dt,
  .pop dd {
    padding: 6px 0;
  }
  .pop dt + dd ~ dt,
  .pop dt + dd ~ dt + dd {
    border-top: 1px solid var(--rule);
  }
  .pop dt {
    color: var(--ink-3);
  }
  .pop dd {
    margin: 0;
    color: var(--ink);
  }
  .pop dd.warn {
    font-weight: 600;
  }
  .pop small {
    display: block;
    margin-top: 2px;
    font-size: inherit;
    font-weight: 400;
    color: var(--ink-3);
  }
  .pop small.note {
    color: var(--ink-2);
  }
  @media (prefers-reduced-motion: reduce) {
    .bar,
    .chev {
      transition: none;
    }
  }
</style>
