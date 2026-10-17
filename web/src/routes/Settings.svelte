<script lang="ts">
  // App settings. In the desktop app this is its own window (⌘, or Ctrl+,); in the browser, a page.
  import { app } from '../lib/store.svelte';
  import { api, desktop, loginItem } from '../lib/api';
  import type { Housekeeping, MonitorConfig, RpkiStatus } from '../lib/types';
  import { ago, pct, plural } from '../lib/format';
  import { HIT_SHARE, MIX_LABEL, type Mix } from '../lib/verdict';
  import Toolbar from '../components/Toolbar.svelte';
  import Icon from '../components/Icon.svelte';

  const mixes: Mix[] = ['home', 'office', 'lab'];
  const themes = [
    { v: 'system', icon: 'monitor', label: 'Match system' },
    { v: 'light', icon: 'sun', label: 'Light' },
    { v: 'dark', icon: 'moon', label: 'Dark' },
  ] as const;

  const cfg = $derived(app.boot?.monitor ?? null);
  let login = $state<boolean | null>(null);
  let saving = $state(false);
  let error = $state<string | null>(null);

  async function load() {
    if (desktop) login = await loginItem.get().catch(() => null);
  }
  $effect(() => {
    load();
  });

  async function saveMonitor(next: MonitorConfig) {
    saving = true;
    error = null;
    try {
      const saved = await api.put<MonitorConfig>('/api/monitor', next);
      if (app.boot) app.boot = { ...app.boot, monitor: saved };
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
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

  const standalone = desktop;

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
  <div class="settings">
    <section>
      <h2>Appearance</h2>
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
    </section>

    <section>
      <h2>Background monitoring</h2>
      {#if cfg}
        <label class="row check">
          <input
            type="checkbox"
            checked={cfg.enabled}
            disabled={saving}
            onchange={(e) => saveMonitor({ ...cfg!, enabled: (e.currentTarget as HTMLInputElement).checked })}
          />
          <span>Check resolvers in the background while Recursa is running</span>
        </label>
        <div class="row">
          <label class="lbl" for="interval">Every</label>
          <select
            id="interval"
            value={cfg.interval_min}
            disabled={saving}
            onchange={(e) => saveMonitor({ ...cfg!, interval_min: Number((e.currentTarget as HTMLSelectElement).value) })}
          >
            {#each [1, 5, 10, 15, 30, 60] as m}<option value={m}>{m} min</option>{/each}
          </select>
        </div>
        <div class="row">
          <label class="lbl" for="keep">Keep history</label>
          <select
            id="keep"
            value={cfg.retention_days}
            disabled={saving}
            onchange={(e) => saveMonitor({ ...cfg!, retention_days: Number((e.currentTarget as HTMLSelectElement).value) })}
          >
            {#each [7, 30, 90, 365] as d}<option value={d}>{d} days</option>{/each}
          </select>
        </div>
        <p class="note">
          {cfg.resolvers.length} resolvers are monitored. Choose them, and see the record, in Reliability.
        </p>
      {:else if !error}
        <p class="note">Loading…</p>
      {/if}
    </section>

    <section>
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
          weekly checks, and 90 days is about as long as a comparison stays current. Saved runs are never counted or deleted: Control-click a
          run in the sidebar to save it, or to clear all runs now.
        </p>
      {:else if !error}
        <p class="note">Loading…</p>
      {/if}
    </section>

    {#if app.env?.rpki_local}
      <section>
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
      </section>
    {/if}

    {#if desktop}
      <section>
        <h2>Startup</h2>
        <label class="row check">
          <input type="checkbox" checked={!!login} disabled={login == null} onchange={(e) => setLogin((e.currentTarget as HTMLInputElement).checked)} />
          <span>Open at login, in the menu bar only</span>
        </label>
        <p class="note">Recursa keeps monitoring from the menu bar when its window is closed.</p>
      </section>
    {/if}

    <section>
      <h2>About</h2>
      <p class="about">Recursa{app.env ? ` ${app.env.version}` : ''} <span class="by">by Brian Morisette</span></p>
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
    </section>

    {#if error}<p class="err" role="alert">{error}</p>{/if}
  </div>

  <dialog class="notices" bind:this={noticesDialog} aria-labelledby="notices-title" onclose={() => (notices = '')}>
    <header>
      <h2 id="notices-title">Third-party notices</h2>
      <button class="btn sm" onclick={() => noticesDialog?.close()}>Done</button>
    </header>
    <pre class="mono">{notices}</pre>
  </dialog>
{/snippet}

{#if standalone}
  <div class="window scroll chrome">{@render body()}</div>
{:else}
  <div class="view-root">
    <Toolbar title="Settings" />
    <div class="view-body">
      <div class="content scroll">{@render body()}</div>
    </div>
  </div>
{/if}

<style>
  .window {
    height: 100%;
    background: var(--bg);
    padding: 20px 26px 24px;
  }
  .settings {
    display: grid;
    gap: 22px;
    max-width: 560px;
  }
  section {
    display: grid;
    gap: 10px;
  }
  section + section {
    padding-top: 18px;
    border-top: 1px solid var(--rule);
  }
  h2 {
    font-size: var(--t-sm);
    font-weight: 660;
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
  .check input {
    margin: 0;
    accent-color: var(--accent);
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
    font-size: var(--t-sm);
    font-weight: 560;
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
