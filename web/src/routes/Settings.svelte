<script lang="ts">
  // App settings. In the desktop app this is its own window (⌘, or Ctrl+,); in the browser, a page.
  import { app } from '../lib/store.svelte';
  import { api, mobile, loginItem, shell } from '../lib/api';
  import type { ComponentProps } from 'svelte';
  import type { Housekeeping, MonitorConfig, RpkiStatus } from '../lib/types';
  import { RESOLVER_INTERVAL_MIN, RETENTION_DAYS, resolverMonitor } from '../lib/monitor.svelte';
  import { ago, pct, plural } from '../lib/format';
  import { HIT_SHARE, MIX_LABEL, type Mix } from '../lib/verdict';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';
  import Mark from '../components/Mark.svelte';

  const mixes = Object.keys(MIX_LABEL) as Mix[];
  const themes = [
    { v: 'system', icon: 'monitor', label: 'Match system' },
    { v: 'light', icon: 'sun', label: 'Light' },
    { v: 'dark', icon: 'moon', label: 'Dark' },
  ] as const;

  const cfg = $derived(app.boot?.monitor ?? null);
  let login = $state<boolean | null>(null);
  let saving = $state(false);
  let error = $state<string | null>(null);

  // The Settings window can open before the bootstrap arrives.
  const canLogin = $derived(app.boot?.caps.login_item ?? false);
  $effect(() => {
    if (canLogin) loginItem.get().then((v) => (login = v), () => (login = null));
  });

  const monitor = resolverMonitor({ onerror: (m) => (error = m) });
  function saveMonitor(next: MonitorConfig) {
    error = null;
    return monitor.save(next);
  }

  // Automatic cleanup of unsaved runs. The engine applies it at start and after each run.
  let keep = $state<Housekeeping | null>(null);
  async function loadKeep() {
    try {
      keep = await api.get<Housekeeping>('/api/housekeeping');
    } catch (e) {
      error = (e as Error).message;
    }
  }
  $effect(() => {
    loadKeep();
  });
  // Only the changed rule goes over; the engine keeps the other as it is.
  async function saveKeep(next: Partial<Housekeeping>) {
    saving = true;
    error = null;
    try {
      keep = await api.put<Housekeeping>('/api/housekeeping', next);
    } catch (e) {
      error = (e as Error).message;
      await loadKeep();
    } finally {
      saving = false;
    }
  }
  // Keep a stored value the presets lack (the engine accepts any count) so the select never shows blank.
  const presetWith = (list: number[], v: number | null) => (v == null || list.includes(v) ? list : [v, ...list]);
  const offOr = (v: string) => (v === 'off' ? null : Number(v));

  async function setLogin(on: boolean) {
    try {
      login = await loginItem.set(on);
    } catch (e) {
      error = String(e);
    }
  }

  // Route-origin (RPKI) check on this computer. Only builds with the feature report `rpki_local`.
  let rpki = $state<RpkiStatus | null>(null);
  async function loadRpki() {
    try {
      rpki = await api.get<RpkiStatus>('/api/rpki');
      error = null;
    } catch (e) {
      error = (e as Error).message;
    }
  }
  async function setRpki(enabled: boolean) {
    error = null;
    try {
      rpki = await api.put<RpkiStatus>('/api/rpki', { enabled });
    } catch (e) {
      error = (e as Error).message;
    }
  }
  async function syncRpki() {
    error = null;
    try {
      rpki = await api.post<RpkiStatus>('/api/rpki/sync');
    } catch (e) {
      error = (e as Error).message;
    }
  }
  $effect(() => {
    if (app.env?.rpki_local) loadRpki();
  });
  // A first sync takes minutes: keep the status line current while one runs.
  const syncing = $derived(rpki?.state === 'syncing');
  $effect(() => {
    if (!syncing) return;
    const t = setInterval(loadRpki, 3000);
    return () => clearInterval(t);
  });
  const size = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);

  // One pane at a time, picked from the list on the left; the URL hash names the pane, so
  // other parts of the app can open Settings on one (the sidebar's run history menu, say).
  type Pane = { id: string; label: string; icon: ComponentProps<typeof Icon>['name'] };
  const panes: Pane[] = $derived([
    { id: 'general', label: 'General', icon: 'gear' },
    { id: 'benchmark', label: 'Benchmark', icon: 'play' },
    { id: 'monitoring', label: 'Monitoring', icon: 'pulse' },
    { id: 'history', label: 'Run history', icon: 'clock' },
    ...(app.env?.rpki_local ? [{ id: 'routes', label: 'Route origins', icon: 'lock' as const }] : []),
    { id: 'about', label: 'About', icon: 'info' },
  ]);
  let pane = $state(location.hash.slice(1));
  const current = $derived(panes.some((p) => p.id === pane) ? pane : 'general');
  $effect(() => {
    void app.route; // In-app navigation pushes the hash with the path, which fires no hashchange.
    const onHash = () => {
      pane = location.hash.slice(1);
      error = null;
    };
    onHash();
    addEventListener('hashchange', onHash);
    return () => removeEventListener('hashchange', onHash);
  });
  // Phone: a grouped list of sections. A row pushes its section, so the toolbar's Back returns to the list.
  const showRoot = $derived(mobile && !panes.some((p) => p.id === pane));
  function pick(id: string) {
    pane = id;
    error = null;
    if (mobile) history.pushState(null, '', `#${id}`);
    else history.replaceState(null, '', `#${id}`);
  }
  // Up, Down, Home and End move through the list, as in any tab list.
  function paneKey(e: KeyboardEvent) {
    const i = panes.findIndex((p) => p.id === current);
    const to = e.key === 'ArrowDown' || e.key === 'ArrowRight' ? i + 1 : e.key === 'ArrowUp' || e.key === 'ArrowLeft' ? i - 1 : e.key === 'Home' ? 0 : e.key === 'End' ? panes.length - 1 : null;
    if (to === null) return;
    e.preventDefault();
    const next = panes[(to + panes.length) % panes.length];
    pick(next.id);
    (document.getElementById(`pane-tab-${next.id}`) as HTMLElement | null)?.focus();
  }

  // Third-party notices: their licenses require them to ship with the app. Loaded only
  // when asked for (about 200 KB of text).
  let noticesDialog = $state<HTMLDialogElement>();
  let notices = $state('');
  async function showNotices() {
    notices = (await import('../legal/third-party-notices.txt?raw')).default;
    noticesDialog?.showModal();
  }
</script>

{#snippet body()}
  <div class="panes">
    {#if mobile}
      {#if showRoot}
        <nav class="root scroll" aria-label="Settings sections">
          <ul>
            {#each panes as p (p.id)}
              <li>
                <button type="button" class="rootrow" onclick={() => pick(p.id)}>
                  <Icon name={p.icon} size={16} />
                  <span>{p.label}</span>
                  <Icon name="chevron-right" size={14} />
                </button>
              </li>
            {/each}
          </ul>
        </nav>
      {/if}
    {:else}
    <nav class="nav" aria-label="Settings sections">
      <div role="tablist" aria-orientation="vertical">
        {#each panes as p (p.id)}
          {@const on = p.id === current}
          <button
            id="pane-tab-{p.id}"
            role="tab"
            class="tab"
            class:on
            aria-selected={on}
            aria-controls="pane-{p.id}"
            tabindex={on ? 0 : -1}
            onclick={() => pick(p.id)}
            onkeydown={paneKey}
          >
            <Icon name={p.icon} size={15} />
            <span>{p.label}</span>
          </button>
        {/each}
      </div>
    </nav>
    {/if}

    {#if !showRoot}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (the desktop tab panel is keyboard-reachable) -->
    <div class="pane scroll" role={mobile ? undefined : 'tabpanel'} id="pane-{current}" aria-labelledby={mobile ? undefined : `pane-tab-${current}`} tabindex={mobile ? undefined : 0}>
      {#if current === 'general'}
        <h2>General</h2>
        <div class="row">
          <span class="lbl" id="theme-l">Theme</span>
          <div class="segmented" role="radiogroup" aria-labelledby="theme-l">
            {#each themes as t}
              <button role="radio" aria-checked={app.theme === t.v} class:on={app.theme === t.v} onclick={() => app.setTheme(t.v)}>
                <Icon name={t.icon} size={13} />{t.label}
              </button>
            {/each}
          </div>
        </div>
        {#if canLogin}
          <label class="row check">
            <input type="checkbox" checked={!!login} disabled={login == null} onchange={(e) => setLogin((e.currentTarget as HTMLInputElement).checked)} />
            <span>Open at login, in the menu bar only</span>
          </label>
          <p class="note">Recursa keeps monitoring from the menu bar when its window is closed.</p>
        {/if}
      {:else if current === 'benchmark'}
        <h2>Benchmark</h2>
        <div class="row">
          <span class="lbl" id="mix-l">Traffic mix</span>
          <div class="segmented" role="radiogroup" aria-labelledby="mix-l">
            {#each mixes as m}
              <button role="radio" aria-checked={app.mix === m} class:on={app.mix === m} onclick={() => app.setMix(m)}>{MIX_LABEL[m]}</button>
            {/each}
          </div>
        </div>
        <p class="note">
          How many lookups a resolver answers from its cache: {pct(HIT_SHARE.home, 0)} at home, {pct(HIT_SHARE.office, 0)} in an office, {pct(HIT_SHARE.lab, 0)} in a lab with many unique names.
          Only the per-lookup figure and the Best from here pick use it.
        </p>
      {:else if current === 'monitoring'}
        <h2>Monitoring</h2>
        {#if cfg}
          <label class="row check">
            <input
              type="checkbox"
              checked={cfg.enabled}
              disabled={monitor.saving}
              onchange={(e) => saveMonitor({ ...cfg!, enabled: (e.currentTarget as HTMLInputElement).checked })}
            />
            <span>Check resolvers in the background while Recursa is running</span>
          </label>
          <div class="row">
            <label class="lbl" for="interval">Every</label>
            <select
              id="interval"
              value={cfg.interval_min}
              disabled={monitor.saving}
              onchange={(e) => saveMonitor({ ...cfg!, interval_min: Number((e.currentTarget as HTMLSelectElement).value) })}
            >
              {#each RESOLVER_INTERVAL_MIN as m}<option value={m}>{m} min</option>{/each}
            </select>
          </div>
          <div class="row">
            <label class="lbl" for="keep">Keep history</label>
            <select
              id="keep"
              value={cfg.retention_days}
              disabled={monitor.saving}
              onchange={(e) => saveMonitor({ ...cfg!, retention_days: Number((e.currentTarget as HTMLSelectElement).value) })}
            >
              {#each RETENTION_DAYS as d}<option value={d}>{d} days</option>{/each}
            </select>
          </div>
          <p class="note">
            {cfg.resolvers.length} resolvers are monitored. Choose them, and see the record, in Reliability.
          </p>
        {:else if !error}
          <p class="note">Loading…</p>
        {/if}
      {:else if current === 'history'}
        <h2>Run history</h2>
        {#if keep}
          <div class="row">
            <label class="lbl" for="keep-runs">Keep the last</label>
            <select id="keep-runs" value={keep.keep_runs ?? 'off'} disabled={saving} onchange={(e) => saveKeep({ keep_runs: offOr((e.currentTarget as HTMLSelectElement).value) })}>
              {#each presetWith([10, 25, 50, 100, 250, 500], keep.keep_runs) as n}<option value={n}>{plural(n, 'run')}</option>{/each}
              <option value="off">every run</option>
            </select>
          </div>
          <div class="row">
            <label class="lbl" for="keep-days">Keep runs from</label>
            <select id="keep-days" value={keep.keep_days ?? 'off'} disabled={saving} onchange={(e) => saveKeep({ keep_days: offOr((e.currentTarget as HTMLSelectElement).value) })}>
              {#each presetWith([7, 30, 90, 180, 365], keep.keep_days) as d}<option value={d}>the last {plural(d, 'day')}</option>{/each}
              <option value="off">any time</option>
            </select>
          </div>
          <p class="note">
            Older runs are deleted when Recursa starts and after each run finishes. A run is a few hundred kilobytes; 50 runs is a season of
            weekly checks, and 90 days is about as long as a comparison stays current.
          </p>
          <p class="note">
            Saved runs are never counted or deleted. Save a run with the bookmark on its row in the sidebar, or in the toolbar of its results.
            To delete every unsaved run now, use the menu by the sidebar's Runs heading{#if app.boot?.caps.app_menu}, or File › Clear All Runs{/if}.
          </p>
        {:else if !error}
          <p class="note">Loading…</p>
        {/if}
      {:else if current === 'routes'}
        <h2>Route origins</h2>
        <label class="row check">
          <input type="checkbox" checked={!!rpki?.enabled} disabled={!rpki} onchange={(e) => setRpki((e.currentTarget as HTMLInputElement).checked)} />
          <span>Check route origins (RPKI) on this computer</span>
        </label>
        <p class="note">
          Checks each resolver's network against the routing registries' signed route data (RPKI) here, instead of asking a website.
          Uses about 2.5 GB of disk. The first download is about 725 MB and takes 5 to 15 minutes, then it refreshes every 12 hours,
          never during a measurement. Turning it off deletes the data. It uses ARIN's trust anchor, which is covered by ARIN's
          <a href="https://www.arin.net/resources/manage/rpki/rpa.pdf" target="_blank" rel="noreferrer">Relying Party Agreement</a>.
        </p>
        {#if rpki?.enabled}
          <div class="row">
            <span class="note" role="status">
              {#if rpki.state === 'syncing'}
                {rpki.waiting ? 'Waiting for the measurement to end…' : 'Syncing route-origin data…'}
                {#if rpki.points}{plural(rpki.points, 'publication point')} read.{/if}
              {:else if rpki.state === 'ready'}
                {plural(rpki.vrps, 'route origin')}, synced {ago(rpki.last_sync)}.
              {:else}
                {rpki.error ?? 'Not synced yet.'}
              {/if}
              {#if rpki.disk_bytes}{size(rpki.disk_bytes)} on disk.{/if}
            </span>
            <button class="btn sm" disabled={rpki.state === 'syncing'} onclick={syncRpki}>Sync now</button>
          </div>
          {#if rpki.state === 'ready' && rpki.error}<p class="note">The last refresh failed: {rpki.error}</p>{/if}
        {/if}
      {:else}
        <h2>About</h2>
        <p class="about">
          <Mark size={36} />
          <span><b>Recursa</b>{app.env ? ` ${app.env.version}` : ''} <span class="by">by Brian Morisette</span></span>
        </p>
        <p>Find the fastest, best-placed DNS resolver for your network.</p>
        {#if app.env?.build}<p class="note">Build {app.env.build}</p>{/if}
        <p class="note">
          Copyright © 2026 Brian Morisette. Free software under the GNU General Public License, version 3 or later, with
          no warranty.
        </p>
        <div class="row">
          <a class="btn sm" href="https://www.gnu.org/licenses/gpl-3.0.html" target="_blank" rel="noreferrer">
            License <Icon name="external" size={12} />
          </a>
          <button class="btn sm" onclick={showNotices}>Third-party notices</button>
        </div>
      {/if}

      {#if error}<p class="err" role="alert">{error}</p>{/if}
    </div>
    {/if}
  </div>

  <dialog class="notices" bind:this={noticesDialog} aria-labelledby="notices-title" onclose={() => (notices = '')}>
    <header>
      <h2 id="notices-title">Third-party notices</h2>
      <button class="btn sm" onclick={() => noticesDialog?.close()}>Done</button>
    </header>
    <pre class="mono">{notices}</pre>
  </dialog>
{/snippet}

{#if shell}
  <div class="window chrome">{@render body()}</div>
{:else}
  <div class="view-root">
    <Toolbar title={!mobile || showRoot ? 'Settings' : (panes.find((p) => p.id === current)?.label ?? 'Settings')} />
    <div class="view-body">{@render body()}</div>
  </div>
{/if}

<style>
  .window {
    height: 100%;
    background: var(--bg);
  }
  .panes {
    display: grid;
    grid-template-columns: 172px minmax(0, 1fr);
    height: 100%;
    min-height: 0;
    flex: 1;
  }
  .nav {
    padding: 10px 8px;
    background: var(--side);
    border-right: 1px solid var(--rule);
    font-size: var(--t-sm);
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    min-height: 28px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--ink);
    text-align: left;
    cursor: default;
  }
  .tab :global(svg) {
    flex: none;
    color: var(--ink-3);
  }
  .tab:hover {
    background: color-mix(in oklch, var(--side-sel) 45%, transparent);
  }
  .tab.on {
    background: var(--side-sel);
  }
  .tab.on :global(svg) {
    color: var(--accent-text);
  }
  .pane {
    display: grid;
    align-content: start;
    gap: 14px;
    padding: 22px 28px 28px;
    max-width: 640px;
  }
  :global(html.mobile) .panes {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: minmax(0, 1fr);
  }
  /* Phone: an iOS-style grouped list. Rows are 44pt and divided by hairlines; the chevron says it opens. */
  .root ul {
    list-style: none;
    margin: 0;
    padding: 0 0 calc(24px + var(--inset-b));
    border-top: 1px solid var(--rule);
  }
  .rootrow {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    min-height: 48px;
    padding: 0 16px;
    border: 0;
    border-bottom: 1px solid var(--rule);
    background: transparent;
    color: var(--ink);
    font-size: var(--t-base);
    text-align: left;
  }
  .rootrow:active {
    background: var(--surface-2);
  }
  .rootrow span {
    flex: 1;
  }
  .rootrow :global(svg) {
    flex: none;
    color: var(--ink-3);
  }
  /* A segmented control takes its own line under its label. */
  :global(html.mobile) .row:has(.segmented) {
    flex-wrap: wrap;
    gap: 8px;
  }
  :global(html.mobile) .row:has(.segmented) .lbl {
    flex-basis: 100%;
  }
  :global(html.mobile) .row select {
    flex: 1 1 auto;
    min-height: 44px;
  }
  :global(html.mobile) .pane {
    padding: 18px 16px 28px;
  }
  /* The toolbar already names the section. */
  :global(html.mobile) .pane > h2 {
    display: none;
  }
  h2 {
    margin-bottom: 4px;
    font-size: var(--t-md);
    font-weight: 660;
    letter-spacing: -0.01em;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: var(--t-sm);
  }
  .lbl {
    min-width: 96px;
    color: var(--ink-2);
  }
  .check {
    gap: 9px;
  }
  .note {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .err {
    font-size: var(--t-xs);
    color: var(--critical);
  }
  .about {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 0;
    font-size: var(--t-sm);
    color: var(--ink);
  }
  .about b {
    font-weight: 620;
  }
  .about .by {
    font-weight: 400;
    color: var(--ink-3);
  }
  .row .btn.sm {
    text-decoration: none;
    color: var(--ink);
  }
  .notices {
    width: min(680px, calc(100vw - 32px));
    height: min(640px, calc(100vh - 32px));
    padding: 0;
    border: none;
    border-radius: var(--r-lg);
    background: var(--bg);
    color: var(--ink);
    box-shadow: var(--shadow-sheet);
  }
  .notices[open] {
    display: grid;
    grid-template-rows: auto 1fr;
  }
  .notices::backdrop {
    background: oklch(0 0 0 / 0.3);
  }
  .notices header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px 12px 18px;
    border-bottom: 1px solid var(--rule);
  }
  .notices pre {
    margin: 0;
    padding: 14px 18px 18px;
    overflow: auto;
    font-size: var(--t-xs);
    line-height: 1.5;
    white-space: pre-wrap;
    color: var(--ink-2);
    user-select: text;
    -webkit-user-select: text;
  }
</style>
