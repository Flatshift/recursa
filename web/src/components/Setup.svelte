<script lang="ts">
  import { tick } from 'svelte';
  import { app } from '../lib/store.svelte';
  import { api, mobile } from '../lib/api';
  import { shortcut, shortcutParts } from '../lib/keys';
  import type { Check, Resolver, TestKind, Transport } from '../lib/types';
  import CachedNames from './CachedNames.svelte';
  import { TEST_HINT, TRANSPORT_LABEL, documented, duration, label, ms, runEstimateMs } from '../lib/format';
  import { TEST_LABEL } from '../lib/phases';
  import { onResolverContext } from '../lib/resolverMenu';
  import { latestChecks } from '../lib/measured';
  import Icon from './Icon.svelte';
  import Toolbar from './Toolbar.svelte';

  const draft = $derived(app.draft!);
  const resolvers = $derived(app.resolvers);
  let filter = $state('');
  let filterEl: HTMLInputElement | undefined = $state();
  let adding = $state(false);
  let newIp = $state('');
  let newName = $state('');
  let newDot = $state('');
  let newDoh = $state('');
  let addError = $state<string | null>(null);

  $effect(() => {
    // Persist the plan as it changes (per-viewer convenience only).
    JSON.stringify(app.draft);
    app.saveDraft();
  });

  const matches = (r: Resolver) => {
    const f = filter.trim().toLowerCase();
    if (!f) return true;
    return [r.provider, r.variant, r.name, ...r.ipv4, ...r.ipv6, r.published.filtering.join(' ')].join(' ').toLowerCase().includes(f);
  };

  const system = $derived(resolvers.filter((r) => r.group === 'system'));
  const custom = $derived(resolvers.filter((r) => r.group === 'custom'));
  const providers = $derived.by(() => {
    const m = new Map<string, Resolver[]>();
    for (const r of resolvers.filter((r) => documented(r))) {
      if (!m.has(r.provider)) m.set(r.provider, []);
      m.get(r.provider)!.push(r);
    }
    return [...m.entries()];
  });

  const selected = $derived(new Set(draft.resolvers));
  function toggle(id: string) {
    draft.resolvers = selected.has(id) ? draft.resolvers.filter((x) => x !== id) : [...draft.resolvers, id];
  }
  function setAll(ids: string[]) {
    draft.resolvers = ids;
  }

  // Quick picks that set up a comparison: the resolvers plus the checks that make it meaningful.
  const filtering = $derived(resolvers.filter((r) => r.group === 'public' && r.published.filtering.length > 0));
  function pickFiltering() {
    setAll(filtering.map((r) => r.id));
    draft.probes = true;
    draft.filters = { ...draft.filters, threats: true, ads: true };
    app.notify(`Selected the ${filtering.length} public resolvers that block. Malware, phishing and ad checks are on.`);
  }

  // Resolvers that send ECS: what your recent runs measured beats the operator's claim.
  // Looked up on demand (runs are cached once read), not every time this page opens.
  const sendsEcs = (r: Resolver, measured: Map<string, Check>) => {
    const m = measured.get(r.id);
    if (m) return m.present === true;
    return documented(r) && r.published.ecs === true;
  };
  async function pickEcs() {
    const measured = await latestChecks(app.boot?.runs ?? [], 'ecs');
    const list = resolvers.filter((r) => sendsEcs(r, measured));
    setAll(list.map((r) => r.id));
    draft.probes = true;
    draft.cdn = true;
    const seen = list.filter((r) => measured.has(r.id)).length;
    const how =
      seen === list.length ? 'as measured in your recent runs' : seen ? `${seen} measured, ${list.length - seen} by the operator's word` : "by the operator's word";
    app.notify(`Selected ${list.length} resolvers that send ECS, ${how}. CDN mapping is on.`);
  }

  function toggleIn<T>(list: T[], v: T): T[] {
    return list.includes(v) ? list.filter((x) => x !== v) : [...list, v];
  }

  const chosen = $derived(resolvers.filter((r) => selected.has(r.id)));
  // Phone: the page is a summary; Edit reveals the picker and the plan sheet. With nothing chosen the picker
  // opens by itself and stays open through the first picks, until Done.
  let editing = $state(false);
  $effect(() => {
    if (draft.resolvers.length === 0) editing = true;
  });
  const pickerOpen = $derived(!mobile || editing);
  let planOpen = $state(false);
  // The sheet takes focus when it opens and gives it back to Edit when it closes, so VoiceOver stays in step.
  let planSheet = $state<HTMLElement>();
  let planEdit = $state<HTMLElement>();
  function openPlan() {
    planOpen = true;
    tick().then(() => planSheet?.focus());
  }
  function closePlan() {
    planOpen = false;
    planEdit?.focus({ preventScroll: true });
  }
  const chosenNames = $derived(chosen.map((r) => label(r.provider, r.variant)).join(' · '));
  const planLine = $derived(
    [
      draft.tests.map((k) => TEST_LABEL[k]).join(', ') || 'No tests',
      ...(draft.cdn ? ['CDN mapping'] : []),
      ...(draft.probes ? ['probes'] : []),
      ...(draft.live ? ['live threats'] : []),
    ].join(' · '),
  );
  const filterOn = $derived(filter.trim() !== '');
  // Phone: a filter opens every matching group; clearing it puts back the ones the user had open.
  function openWhile(on: boolean) {
    return (node: HTMLDetailsElement) => {
      if (!on) return;
      const before = node.open;
      node.open = true;
      return () => {
        node.open = before;
      };
    };
  }
  // One line per provider that publishes limits, among the resolvers picked.
  const limited = $derived([...new Map(chosen.filter((r) => r.limits).map((r) => [r.provider, r])).values()]);
  const estimate = $derived.by(() => {
    let targets = 0;
    for (const r of chosen) {
      const fam = draft.family === 'both' ? 2 : 1;
      let t = draft.transports.filter((x) => (x === 'udp' || x === 'tcp' ? r.udp : x === 'dot' ? !!r.dot : !!r.doh)).length;
      if (!r.udp && t === 0) t = 1;
      targets += t * fam;
    }
    const queries = targets * (draft.tests.length * draft.samples + 11);
    return { targets, queries, ms: runEstimateMs(targets, chosen.length, draft) };
  });

  const v6ok = $derived(app.env?.vantage?.ipv6_reachable ?? false);
  const noNames = $derived(draft.tests.includes('cached') && !draft.cached_names?.length);
  const canStart = $derived(draft.resolvers.length > 0 && draft.tests.length > 0 && draft.transports.length > 0 && estimate.targets > 0 && !noNames);
  const blocker = $derived(
    !draft.resolvers.length
      ? 'Pick at least one resolver.'
      : !draft.tests.length
        ? 'Pick at least one test.'
        : !draft.transports.length
          ? 'Pick at least one transport.'
          : !estimate.targets
            ? 'None of these resolvers has a known endpoint for the chosen transports.'
            : noNames
              ? 'Add at least one name for the Cached test.'
              : null,
  );

  async function addResolver(e: SubmitEvent) {
    e.preventDefault();
    addError = null;
    try {
      const r = await api.post<Resolver>('/api/resolvers', { ip: newIp, name: newName, dot: newDot, doh: newDoh });
      await app.refreshBoot();
      draft.resolvers = [...new Set([...draft.resolvers, r.id])];
      newIp = newName = newDot = newDoh = '';
      adding = false;
      app.notify(r.group === 'system' ? `That's your system resolver: it's now called ${r.provider}.` : `Added ${r.provider} ${r.name}.`);
    } catch (err) {
      addError = (err as Error).message;
    }
  }

  async function removeResolver(id: string) {
    await api.del(`/api/resolvers/${id}`);
    draft.resolvers = draft.resolvers.filter((x) => x !== id);
    await app.refreshBoot();
  }

  $effect(() => {
    if (app.findRev) filterEl?.focus();
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && planOpen) {
      e.preventDefault();
      closePlan();
      return;
    }
    const tag = (e.target as HTMLElement)?.tagName;
    if (e.key === '/' && tag !== 'INPUT' && tag !== 'TEXTAREA') {
      e.preventDefault();
      filterEl?.focus();
    }
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && canStart) {
      e.preventDefault();
      app.startRun();
    }
  }

  const transports: { v: Transport; hint: string }[] = $derived([
    { v: 'udp', hint: app.env?.kernel_rx === false ? 'Plain DNS, port 53.' : 'Plain DNS, port 53. Kernel-stamped.' },
    { v: 'tcp', hint: 'Plain DNS over a persistent TCP connection.' },
    { v: 'dot', hint: 'DNS over TLS, port 853.' },
    { v: 'doh', hint: 'DNS over HTTPS/2, port 443.' },
  ]);
  const tests: TestKind[] = ['cached', 'uncached', 'tld'];
  // On the phone the quick picks are a segmented row; on the desktop, a row of ghost buttons.
  const pickCls = mobile ? 'segbtn' : 'btn ghost sm';
  const icpt = $derived(app.env?.interception ?? null);
  let checking = $state(false);
  async function recheck() {
    checking = true;
    await app.recheckInterception();
    checking = false;
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="view-root">
  <Toolbar
    title="Benchmark"
    subtitle={blocker ?? `≈ ${duration(estimate.ms)} · ${estimate.queries.toLocaleString()} queries · ${estimate.targets} targets`}
  >
    {#snippet actions()}
      {#if !mobile}
        <button class="btn primary" disabled={!canStart} title={blocker ?? `Start (${shortcut('cmd+Enter')})`} onclick={() => app.startRun()}>
          <Icon name="play" size={12} />Start
        </button>
      {/if}
    {/snippet}
  </Toolbar>

  <div class="view-body">
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <section class="pick content scroll" aria-labelledby="pick-h" oncontextmenu={onResolverContext}>
      <header class="pick-head">
        <h2 id="pick-h" class="section-title">Resolvers <span class="count num">{draft.resolvers.length}</span></h2>
        {#if mobile && draft.resolvers.length > 0}
          <button class="textbtn" aria-expanded={editing} onclick={() => (editing = !editing)}>{editing ? 'Done' : 'Edit'}</button>
        {/if}
        {#if pickerOpen}
        <div class="quick" class:seg={mobile} class:five={mobile}>
          <button class={pickCls} onclick={() => setAll(resolvers.filter((r) => r.default).map((r) => r.id))}>Defaults</button>
          <button class={pickCls} onclick={() => setAll(resolvers.filter((r) => r.group !== 'legacy').map((r) => r.id))}
            >{mobile ? 'All' : 'Everything'}</button
          >
          <button
            class={pickCls}
            title="Public resolvers whose operators say they block malware, phishing, ads or adult content. Also turns on the blocking checks."
            onclick={pickFiltering}>Blocking</button
          >
          <button
            class={pickCls}
            title="Resolvers that send EDNS Client Subnet (RFC 7871) to authoritative servers: as measured in your recent runs, or the operator's claim where not yet measured. Also turns on CDN mapping."
            onclick={pickEcs}>ECS</button
          >
          <button class={pickCls} onclick={() => setAll([])}>Clear</button>
        </div>
        {/if}
      </header>
      {#if mobile && !pickerOpen}
        <p class="names">{chosenNames}</p>
      {/if}
      {#if icpt && icpt.verdict !== 'direct' && icpt.verdict !== 'unknown'}
        <div class="intercept" role="status">
          <Icon name="warn" size={16} />
          <div>
            <p>
              <b>Plain DNS (UDP 53) is {icpt.verdict === 'partial' ? 'partly ' : ''}intercepted on this network.</b>
              {#each icpt.results.filter((r) => r.intercepted) as r, i}{i ? '; ' : ''}{r.name} answered in {ms(r.udp_ms)}{#if r.on_lan}{' '}from {r.hops === 0 ? 'zero router hops' : 'one router hop'} away{/if}{#if r.path_ms != null}, though the path to it
                  takes {ms(r.path_ms)}{/if}{/each}.
              {#if icpt.link?.wifi}You're on Wi-Fi ({icpt.link.interface}): access-point features such as UniFi DNS Assistance answer
                DNS per SSID.{/if}
              UDP rows will be measured and flagged, but not ranked. Turn off DNS interception (AP DNS features, DNAT rules) for a
              true UDP comparison, then re-check.
            </p>
            {#if icpt.fingerprint?.summary}
              <p class="who"><b>What answered.</b> {icpt.fingerprint.summary}</p>
            {/if}
          </div>
          <button class="btn sm" onclick={recheck} disabled={checking}><Icon name="refresh" size={13} />{checking ? 'Checking…' : 'Re-check'}</button>
        </div>
      {/if}
      {#if pickerOpen}
      <label class="search">
        <span class="visually-hidden">Filter resolvers</span>
        <input bind:this={filterEl} type="text" bind:value={filter} placeholder="Filter by name, address or what it blocks…" />
        <kbd aria-hidden="true">/</kbd>
      </label>

      {#if system.filter(matches).length}
        <div class="group">
          <h3>Your network</h3>
          {#each system.filter(matches) as r (r.id)}
            {@render row(r)}
          {/each}
          <p class="hint">{system[0]?.notes}</p>
        </div>
      {/if}

      <div class="group">
        <h3>Public resolvers</h3>
        {#each providers as [provider, list] (provider)}
          {@const visible = list.filter(matches)}
          {#if visible.length}
            {#if mobile}
              <details class="provider" {@attach openWhile(filterOn)}>
                <summary class="pname">
                  <span>{provider}</span>
                  <span class="pcount num">{list.filter((r) => selected.has(r.id)).length} of {list.length}</span>
                  <Icon name="chevron-down" size={14} />
                </summary>
                <div>
                  {#each visible as r (r.id)}
                    {@render row(r)}
                  {/each}
                </div>
              </details>
            {:else}
              <div class="provider">
                <div class="pname">{provider}</div>
                <div>
                  {#each visible as r (r.id)}
                    {@render row(r)}
                  {/each}
                </div>
              </div>
            {/if}
          {/if}
        {/each}
      </div>

      <div class="group">
        <h3>Custom</h3>
        {#each custom.filter(matches) as r (r.id)}
          {@render row(r, true)}
        {/each}
        {#if adding}
          <form class="add" onsubmit={addResolver}>
            <label><span>Address</span><input type="text" bind:value={newIp} placeholder="e.g. 75.75.75.75" required /></label>
            <label><span>Name <em>optional</em></span><input type="text" bind:value={newName} placeholder="My ISP" /></label>
            <label
              ><span>DoT host <em>optional</em></span><input type="text" bind:value={newDot} placeholder="dns.example.net" /></label
            >
            <label
              ><span>DoH URL <em>optional</em></span><input
                type="url"
                bind:value={newDoh}
                placeholder="https://dns.example.net/dns-query"
              /></label
            >
            {#if addError}<p class="err" role="alert">{addError}</p>{/if}
            <div class="add-actions">
              <button class="btn primary sm" type="submit">Add resolver</button>
              <button class="btn ghost sm" type="button" onclick={() => (adding = false)}>Cancel</button>
            </div>
          </form>
        {:else}
          <button class="btn ghost sm addbtn" onclick={() => (adding = true)}><Icon name="plus" size={14} />Add an ISP or custom resolver</button>
        {/if}
      </div>
      {/if}
    </section>

    {#if mobile}
      <section class="plan-sum" aria-labelledby="plan-h">
        <div class="sumrow">
          <h2 id="plan-h" class="section-title">Test plan</h2>
          <button class="textbtn" bind:this={planEdit} onclick={openPlan}>Edit</button>
        </div>
        <p class="names">{planLine}</p>
      </section>
    {:else}
    <aside class="plan scroll" aria-labelledby="plan-h">
        <h2 id="plan-h" class="section-title">Test plan</h2>
        {@render planBody()}
    </aside>
    {/if}

    {#if mobile && planOpen}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="sheet-scrim" onclick={closePlan}></div>
      <div class="plan-sheet" role="dialog" aria-modal="true" aria-labelledby="plan-sheet-h" tabindex="-1" bind:this={planSheet}>
        <header class="sheet-head">
          <span class="grabber" aria-hidden="true"></span>
          <h2 id="plan-sheet-h">Test plan</h2>
          <button class="textbtn" onclick={closePlan}>Done</button>
        </header>
        <div class="sheet-body scroll">{@render planBody()}</div>
      </div>
    {/if}

    {#if mobile}
      <!-- The phone's Start lives at the foot of the page so it stays in reach while scrolled. -->
      <div class="startbar">
        <p class="est" class:blocked={!!blocker}>
          {blocker ?? `≈ ${duration(estimate.ms)} · ${estimate.queries.toLocaleString()} queries · ${estimate.targets} targets`}
        </p>
        <button class="btn primary" disabled={!canStart} onclick={() => app.startRun()}><Icon name="play" size={14} />Start</button>
      </div>
    {/if}
  </div>
</div>

{#snippet planBody()}
        <fieldset>
          <legend>Transports</legend>
          <div class="seg">
            {#each transports as t}
              <label class="segbtn" title={t.hint}>
                <input
                  type="checkbox"
                  aria-label="{TRANSPORT_LABEL[t.v]}: {t.hint}"
                  checked={draft.transports.includes(t.v)}
                  onchange={() => (draft.transports = toggleIn(draft.transports, t.v))}
                />
                <span>{TRANSPORT_LABEL[t.v]}</span>
              </label>
            {/each}
          </div>
          <p class="hint">
            {draft.transports.length > 1
              ? 'Each transport becomes its own row, so you can compare them directly.'
              : 'Add TCP, DoT or DoH to see what encryption costs from here.'}
          </p>
        </fieldset>

        <fieldset>
          <legend>Tests</legend>
          {#each tests as k}
            <label class="testrow">
              <input type="checkbox" aria-label="{TEST_LABEL[k]} test" checked={draft.tests.includes(k)} onchange={() => (draft.tests = toggleIn(draft.tests, k))} />
              <span class="key {k}" aria-hidden="true"></span>
              <span class="tl">{TEST_LABEL[k]}</span>
              <span class="th">{TEST_HINT[k]}</span>
            </label>
            {#if k === 'cached' && draft.tests.includes('cached')}<CachedNames bind:names={draft.cached_names} />{/if}
          {/each}
        </fieldset>

        <fieldset class="two">
          <label>
            <span class="lbl">Queries per test</span>
            <select bind:value={draft.samples}>
              {#each [10, 20, 30, 50, 100] as n}<option value={n}>{n}</option>{/each}
            </select>
          </label>
          <label>
            <span class="lbl">Addresses</span>
            <select bind:value={draft.family}>
              <option value="v4">IPv4</option>
              <option value="v6" disabled={!v6ok}>IPv6{v6ok ? '' : ' (no IPv6 here)'}</option>
              <option value="both" disabled={!v6ok}>Both{v6ok ? '' : ' (no IPv6 here)'}</option>
            </select>
          </label>
        </fieldset>

        <fieldset>
          <legend>Pacing</legend>
          <div class="seg wide" role="radiogroup">
            <label class="segbtn">
              <input type="radio" name="mode" value="accurate" aria-label="Accurate pacing" bind:group={draft.mode} />
              <span>Accurate</span>
            </label>
            <label class="segbtn">
              <input type="radio" name="mode" value="quick" aria-label="Quick pacing" bind:group={draft.mode} />
              <span>Quick</span>
            </label>
          </div>
          <p class="hint">
            {draft.mode === 'accurate'
              ? 'One query in flight per resolver, sends 4 ms apart, so queries never queue on your link.'
              : 'Up to 8 queries in flight to different resolvers. Faster, slightly noisier.'}
          </p>
        </fieldset>

        {#if limited.length}
          <div class="limits" role="note">
            <p>
              <Icon name="info" size={14} />
              <span>
                <b>Rate limits.</b>
                {#each limited as r, i}{i ? ' ' : ''}{r.limits?.text}{/each}
                Recursa asks each resolver one question at a time, so a run sends tens of queries per second at most. It records the peak it
                sent, and says so if a resolver stops answering partway.
                <a href="/method#politeness" onclick={(e) => (e.preventDefault(), app.navigate('/method#politeness'))}>How this works</a>
              </span>
            </p>
          </div>
        {/if}

        <fieldset>
          <legend>After measuring</legend>
          <label class="toggle"><input type="checkbox" bind:checked={draft.probes} /><span>Capability probes</span></label>
          {#if draft.probes}
            <div class="sub">
              <label class="toggle"
                ><input type="checkbox" bind:checked={draft.filters.threats} /><span>Malware &amp; phishing blocking</span></label
              >
              <label class="toggle"><input type="checkbox" bind:checked={draft.filters.ads} /><span>Ad &amp; tracker blocking</span></label>
              <label class="toggle"><input type="checkbox" bind:checked={draft.filters.adult} /><span>Adult-content blocking</span></label>
              <p class="hint">Looks up the adult-content test domains Cloudflare and Cisco OpenDNS publish. The network can see these lookups.</p>
            </div>
          {/if}
          <label class="toggle"><input type="checkbox" bind:checked={draft.cdn} /><span>CDN mapping quality</span></label>
          <label class="toggle"><input type="checkbox" bind:checked={draft.live} /><span>Live threat test</span></label>
          <div class="sub">
            <p class="hint">Advanced. Looks up real malware and phishing hosts through every selected resolver.</p>
            {#if draft.live}
              <p class="live-warn" role="note">
                <Icon name="warn" size={14} />
                <span>
                  These are real hosts from today's URLhaus, ThreatView and OpenPhish feeds, about 40 per resolver. Recursa only looks up the names, over
                  DoT or DoH (plain DNS only to your own resolver on this network), and never connects to them. Security tools on your
                  network, like a gateway's threat management or a corporate firewall, may still flag the lookups or block this computer, and
                  resolvers with a dashboard (NextDNS, Control D) will list them as blocked threats.
                </span>
              </p>
            {/if}
          </div>
        </fieldset>
        <p class="hint start-hint">{#each shortcutParts('cmd+Enter') as k, i}{i ? ' ' : ''}<kbd>{k}</kbd>{/each} starts the benchmark.</p>
{/snippet}

{#snippet row(r: Resolver, removable = false)}
  <div class="rrow" class:on={selected.has(r.id)} data-resolver={r.id}>
    <label>
      <input type="checkbox" aria-label="{label(r.provider, r.variant)}, {r.ipv4[0] ?? r.ipv6[0] ?? r.name}" checked={selected.has(r.id)} onchange={() => toggle(r.id)} />
      <span class="rname">{documented(r) ? r.variant || r.provider : label(r.provider, r.variant)}</span>
      <span class="rip mono">{r.ipv4[0] ?? r.ipv6[0] ?? r.name}</span>
      <span class="chips">
        {#if !r.udp}<span class="chip enc" title={r.notes ?? ''}>encrypted only</span>{/if}
        {#if r.group === 'legacy'}<span class="chip">legacy</span>{/if}
        {#if r.limits}<span class="chip" title={r.limits.text}>rate limits</span>{/if}
        {#each r.published.filtering.slice(0, 3) as f}<span class="chip">{f}</span>{/each}
      </span>
    </label>
    {#if removable}
      <button class="btn ghost sm icon" aria-label="Remove {r.provider} {r.name}" onclick={() => removeResolver(r.id)}
        ><Icon name="trash" size={14} /></button
      >
    {:else if r.custom_id}
      <button
        class="btn ghost sm icon"
        aria-label="Forget the name {r.provider}"
        title="Forget the name “{r.provider}”"
        onclick={() => removeResolver(r.custom_id!)}><Icon name="close" size={13} /></button
      >
    {/if}
  </div>
{/snippet}

<style>
  .intercept {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 12px 14px;
    margin-bottom: 12px;
    border-radius: var(--r-lg);
    background: var(--warn-soft);
    border: 1px solid color-mix(in oklch, var(--warning) 45%, transparent);
    font-size: var(--t-sm);
  }
  .intercept > :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--warning);
  }
  .intercept div {
    flex: 1;
    max-width: 100ch;
  }
  .limits p {
    display: flex;
    gap: 8px;
    font-size: var(--t-xs);
    color: var(--ink-2);
    line-height: 1.5;
  }
  .limits :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .limits b {
    color: var(--ink);
    font-weight: 620;
  }
  .intercept .who {
    margin-top: 8px;
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .intercept .who b {
    color: var(--ink);
    font-weight: 620;
  }
  .intercept .btn {
    flex: none;
  }
  .live-warn {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin-top: 8px;
    padding: 10px 12px;
    max-width: 62ch;
    border-radius: var(--r-md);
    background: var(--warn-soft);
    border: 1px solid color-mix(in oklch, var(--warning) 45%, transparent);
    font-size: var(--t-xs);
    line-height: 1.5;
    color: var(--ink-2);
  }
  .live-warn :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--warning);
  }
  .pick {
    flex: 1;
  }
  .pick > :global(*) {
    max-width: 920px;
  }
  .pick-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 4px 12px;
    margin-bottom: 12px;
  }
  .pick-head h2 {
    white-space: nowrap;
  }
  .count {
    font-size: var(--t-sm);
    color: var(--ink-3);
    font-weight: 500;
    margin-left: 6px;
  }
  .quick {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 2px;
  }
  .search {
    position: relative;
    display: block;
    margin-bottom: 8px;
  }
  .search input {
    width: 100%;
    padding-right: 34px;
  }
  .search kbd {
    position: absolute;
    right: 9px;
    top: 50%;
    transform: translateY(-50%);
  }
  .group {
    margin-top: 18px;
  }
  .group h3 {
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-3);
    margin-bottom: 6px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--rule);
  }
  .provider {
    display: grid;
    grid-template-columns: 150px minmax(0, 1fr);
    gap: 12px;
    padding: 5px 0;
    border-bottom: 1px solid var(--rule);
  }
  .provider:last-child {
    border-bottom: 0;
  }
  .pname {
    font-weight: 600;
    font-size: var(--t-sm);
    padding-top: 6px;
  }
  .rrow {
    display: flex;
    align-items: center;
    border-radius: var(--r-md);
  }
  .rrow:hover {
    background: var(--surface-2);
  }
  .rrow label {
    flex: 1;
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr) auto;
    grid-template-areas: 'box name chips' 'box ip chips';
    column-gap: 10px;
    align-items: center;
    padding: 5px 8px;
    cursor: default;
    min-width: 0;
  }
  .rrow input {
    grid-area: box;
  }
  .rname {
    grid-area: name;
    font-size: var(--t-sm);
    font-weight: 540;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rip {
    grid-area: ip;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .chips {
    grid-area: chips;
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .chip {
    font-size: var(--t-2xs);
    line-height: 18px;
    padding: 0 6px;
    border-radius: 9px;
    border: 1px solid var(--rule);
    color: var(--ink-2);
    white-space: nowrap;
  }
  .chip.enc {
    border-color: var(--rule-2);
    color: var(--ink);
  }
  .icon {
    width: 28px;
    padding: 0;
    justify-content: center;
  }
  .hint {
    font-size: var(--t-xs);
    color: var(--ink-3);
    margin-top: 6px;
    max-width: 62ch;
  }
  .addbtn {
    margin-top: 6px;
  }
  .add {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 12px;
    margin-top: 8px;
    padding: 14px;
    border: 1px dashed var(--rule-2);
    border-radius: var(--r-md);
  }
  .add label {
    display: grid;
    gap: 4px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .add em {
    font-style: normal;
    color: var(--ink-3);
  }
  .add-actions {
    grid-column: 1 / -1;
    display: flex;
    gap: 6px;
  }
  .err {
    grid-column: 1 / -1;
    color: var(--critical);
    font-size: var(--t-xs);
  }

  .plan {
    flex: none;
    width: 340px;
    padding: 22px 22px 28px;
    border-left: 1px solid var(--rule);
    background: var(--surface);
  }
  .start-hint {
    margin-top: 22px;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 18px 0 0;
    min-width: 0;
  }
  legend,
  .lbl {
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-3);
    margin-bottom: 8px;
    padding: 0;
    display: block;
  }
  .testrow {
    display: grid;
    grid-template-columns: 16px 10px auto;
    grid-template-areas: 'box key title' '. . hint';
    column-gap: 8px;
    align-items: center;
    padding: 4px 0;
    cursor: default;
  }
  .testrow .key {
    grid-area: key;
    width: 10px;
    height: 10px;
  }
  .tl {
    grid-area: title;
    font-size: var(--t-sm);
    font-weight: 560;
  }
  .th {
    grid-area: hint;
    font-size: var(--t-xs);
    color: var(--ink-3);
    line-height: 1.4;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  .two select {
    width: 100%;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 4px 0;
    font-size: var(--t-sm);
    cursor: default;
  }
  .sub {
    padding-left: 24px;
  }
  @media (max-width: 980px) {
    .plan {
      width: 290px;
      padding: 18px 16px 24px;
    }
  }
  @media (max-width: 560px) {
    .provider {
      grid-template-columns: 1fr;
      gap: 0;
    }
    .add {
      grid-template-columns: 1fr;
    }
    .chips {
      display: none;
    }
  }

  /* iPhone: one column. Resolvers, then the plan, with Start pinned at the foot of the scroll. */
  :global(html.mobile) .view-body {
    flex-direction: column;
    overflow: auto;
    overscroll-behavior: contain;
  }
  :global(html.mobile) .pick,
  :global(html.mobile) .plan {
    flex: none;
    width: auto;
    overflow: visible;
  }
  :global(html.mobile) .pick {
    padding: 10px 16px 4px;
  }
  :global(html.mobile) .pick-head {
    display: grid;
    justify-content: stretch;
    gap: 10px;
    margin-bottom: 10px;
  }
  :global(html.mobile) .pick-head h2 {
    font-size: var(--t-md);
  }
  :global(html.mobile) .count {
    font-size: var(--t-base);
  }
  :global(html.mobile) .quick {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 3px;
  }
  :global(html.mobile) .intercept {
    flex-wrap: wrap;
    font-size: var(--t-base);
  }
  :global(html.mobile) .intercept .btn {
    width: 100%;
  }
  :global(html.mobile) .search kbd {
    display: none;
  }
  :global(html.mobile) .search input {
    padding-right: 12px;
  }
  :global(html.mobile) .group {
    margin-top: 20px;
  }
  :global(html.mobile) .group h3 {
    font-size: var(--t-sm);
    padding-bottom: 8px;
    margin-bottom: 4px;
  }
  :global(html.mobile) .provider {
    grid-template-columns: 1fr;
    gap: 0;
    padding: 8px 0;
  }
  :global(html.mobile) .pname {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 44px;
    padding: 0 12px;
    font-size: var(--t-base);
    cursor: pointer;
    list-style: none;
  }
  :global(html.mobile) .pname::-webkit-details-marker {
    display: none;
  }
  :global(html.mobile) .pname span:first-child {
    flex: 1;
  }
  :global(html.mobile) .pcount {
    font-size: var(--t-sm);
    font-weight: 500;
    color: var(--ink-3);
  }
  :global(html.mobile) .pname :global(svg) {
    flex: none;
    color: var(--ink-3);
    transition: transform 160ms var(--ease-out);
  }
  :global(html.mobile) details[open] > .pname :global(svg) {
    transform: rotate(180deg);
  }
  :global(html.mobile) .pick-head {
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
  }
  :global(html.mobile) .pick-head .quick {
    grid-column: 1 / -1;
  }
  :global(html.mobile) .textbtn {
    min-height: 44px;
    min-width: 44px;
    margin-right: -8px;
    padding: 0 8px;
    border: 0;
    background: none;
    color: var(--ink);
    font-size: var(--t-base);
    font-weight: 620;
  }
  :global(html.mobile) .names {
    margin: -4px 0 4px;
    font-size: var(--t-base);
    color: var(--ink-2);
    line-height: 1.45;
  }
  :global(html.mobile) .plan-sum {
    padding: 8px 16px 20px;
    margin-top: 20px;
    border-top: 1px solid var(--rule);
  }
  :global(html.mobile) .sumrow {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 8px;
  }
  :global(html.mobile) .sumrow h2 {
    font-size: var(--t-md);
  }
  :global(html.mobile) .plan-sheet {
    position: fixed;
    inset: calc(env(safe-area-inset-top) + 10px) 0 0 0;
    z-index: var(--z-sheet);
    display: flex;
    flex-direction: column;
    border-radius: 14px 14px 0 0;
    background: var(--surface);
    color: var(--ink);
    box-shadow: var(--shadow-sheet);
    animation: rise 320ms var(--ease-out);
  }
  :global(html.mobile) .sheet-head {
    position: relative;
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 16px 4px;
    border-bottom: 1px solid var(--rule);
  }
  :global(html.mobile) .sheet-head h2 {
    font-size: var(--t-md);
  }
  :global(html.mobile) .sheet-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    overscroll-behavior: contain;
    padding: 0 16px calc(32px + env(safe-area-inset-bottom));
  }
  @media (prefers-reduced-motion: reduce) {
    :global(html.mobile) .plan-sheet {
      animation: fadein 160ms linear;
    }
    :global(html.mobile) .pname :global(svg) {
      transition: none;
    }
  }
  :global(html.mobile) .rrow:hover {
    background: transparent;
  }
  :global(html.mobile) .rrow:active {
    background: var(--surface-2);
  }
  :global(html.mobile) .rrow label {
    grid-template-columns: 22px minmax(0, 1fr);
    grid-template-areas: 'box name' 'box ip';
    column-gap: 14px;
    min-height: 52px;
    padding: 6px 12px;
  }
  :global(html.mobile) .rname {
    font-size: var(--t-base);
  }
  :global(html.mobile) .rip {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .chips {
    display: none;
  }
  :global(html.mobile) .icon {
    width: 44px;
    min-height: 44px;
  }
  :global(html.mobile) .hint {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .addbtn {
    margin-top: 8px;
  }
  :global(html.mobile) .add {
    grid-template-columns: 1fr;
  }
  :global(html.mobile) .add label {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .plan {
    padding: 8px 16px 28px;
    margin-top: 20px;
    border-left: 0;
    border-top: 1px solid var(--rule);
    background: transparent;
  }
  :global(html.mobile) .plan h2 {
    font-size: var(--t-md);
    margin-top: 16px;
  }
  :global(html.mobile) fieldset {
    margin-top: 22px;
  }
  :global(html.mobile) legend,
  :global(html.mobile) .lbl {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .testrow {
    grid-template-columns: 22px 10px auto;
    column-gap: 12px;
    min-height: 48px;
    padding: 8px 0;
  }
  :global(html.mobile) .tl {
    font-size: var(--t-base);
  }
  :global(html.mobile) .th {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .toggle {
    gap: 12px;
    min-height: 44px;
    padding: 6px 0;
    font-size: var(--t-base);
  }
  :global(html.mobile) .sub {
    padding-left: 34px;
  }
  :global(html.mobile) .limits p,
  :global(html.mobile) .live-warn {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .start-hint {
    display: none;
  }
  :global(html.mobile) .startbar {
    margin-top: auto;
    padding-bottom: calc(10px + var(--inset-b));
  }
  .startbar {
    position: sticky;
    bottom: 0;
    z-index: var(--z-sticky);
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-top: 1px solid var(--rule);
    background: var(--surface);
  }
  .startbar .est {
    font-size: var(--t-sm);
    color: var(--ink-2);
    line-height: 1.35;
  }
  .startbar .est.blocked {
    color: var(--ink-3);
  }
  .startbar .btn {
    min-width: 112px;
    min-height: 44px;
    font-size: var(--t-base);
  }
</style>
