<script lang="ts">
  import { api, confirmAction } from '../lib/api';
  import { app } from '../lib/store.svelte';
  import { plural } from '../lib/format';
  import Icon from './Icon.svelte';

  /** The names the Cached test asks, in rotation: a line under the test that says which list, and a sheet to see them, add and
   *  remove them, import a CSV or text file of your own, and keep lists by name. */
  let { names = $bindable() }: { names: string[] } = $props();

  // Imports read this much of a file at most: plenty for 50 names, and a top-million list still loads its first rows.
  const READ_BYTES = 512 * 1024;

  type Saved = { name: string; names: string[] };
  type Parsed = { names: string[]; total: number; skipped: { text: string; why: string }[] };

  let saved = $state<Saved[]>([]);
  let defaults = $state<string[]>(app.boot?.defaults.cached_names ?? []);
  let max = $state(50);
  let adding = $state('');
  let editing = $state(false);
  let text = $state('');
  let naming = $state(false);
  let listName = $state('');
  let busy = $state(false);
  let message = $state<{ text: string; err: boolean } | null>(null);
  let sheet = $state<HTMLDialogElement>();
  let before: string[] = [];

  $effect(() => {
    api
      .get<{ lists: Saved[]; default: string[]; max: number }>('/api/cached-lists')
      .then((r) => {
        saved = r.lists;
        defaults = r.default;
        max = r.max;
      })
      .catch(() => {
        /* the list still works without saved lists */
      });
  });

  const same = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);
  // Which list the names are, if any: the default, a saved one, or a list edited since.
  const which = $derived(same(names, defaults) ? '#default' : (saved.find((l) => same(l.names, names))?.name ?? '#custom'));
  const savedNow = $derived(saved.find((l) => l.name === which));
  const listLabel = $derived(which === '#default' ? 'the default list' : which === '#custom' ? 'an edited list' : `“${which}”`);

  const say = (t: string, err = false) => (message = { text: t, err });

  function open() {
    before = [...names];
    editing = naming = false;
    adding = listName = '';
    message = null;
    sheet?.showModal();
  }
  // Escape cancels too: the dialog's cancel event puts the names back before it closes.
  const restore = () => (names = before);
  function cancel() {
    restore();
    sheet?.close();
  }
  async function done() {
    if (editing && !(await applyText())) return;
    sheet?.close();
  }

  async function parse(t: string): Promise<Parsed> {
    return api.post<Parsed>('/api/cached-lists/parse', { text: t });
  }
  const skippedNote = (r: Parsed) =>
    r.skipped.length ? ` Skipped ${r.skipped.length}: “${r.skipped[0].text}” ${r.skipped[0].why}${r.skipped.length > 1 ? ', and more' : ''}.` : '';

  async function add(e: SubmitEvent) {
    e.preventDefault();
    if (!adding.trim()) return;
    try {
      const r = await parse(adding);
      const fresh = r.names.filter((n) => !names.includes(n));
      if (!r.names.length) return say(r.skipped.length ? `“${r.skipped[0].text}” ${r.skipped[0].why}.` : 'Nothing to add.', true);
      if (!fresh.length) return say('Already in the list.', true);
      const room = Math.max(0, max - names.length);
      names = [...names, ...fresh.slice(0, room)];
      adding = '';
      say(fresh.length > room ? `Added ${room}; a list holds at most ${max} names.` : `Added ${Math.min(fresh.length, room)}.${skippedNote(r)}`, fresh.length > room);
    } catch (err) {
      say((err as Error).message, true);
    }
  }

  const remove = (n: string) => {
    names = names.filter((x) => x !== n);
    message = null;
  };

  function pick(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    if (v === '#default') names = [...defaults];
    else if (v !== '#custom') names = [...(saved.find((l) => l.name === v)?.names ?? names)];
    message = null;
  }

  async function importFile(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = '';
    if (!file) return;
    try {
      // A long file is read from the top, cut back to its last whole line.
      const cut = file.size > READ_BYTES;
      let t = await file.slice(0, READ_BYTES).text();
      if (cut) t = t.slice(0, t.lastIndexOf('\n') + 1);
      const r = await parse(t);
      if (!r.names.length) return say(`No usable names in ${file.name}.${skippedNote(r)}`, true);
      names = r.names.slice(0, max);
      const first = cut || r.total > max ? ` (the first ${names.length}${cut ? '' : ` of ${r.total}`})` : '';
      say(`Loaded ${names.length} names from ${file.name}${first}.${skippedNote(r)}`);
    } catch (err) {
      say((err as Error).message, true);
    }
  }

  function edit() {
    text = names.join('\n');
    editing = true;
    message = null;
  }
  async function applyText() {
    try {
      const r = await parse(text);
      if (!r.names.length) return (say(`No usable names.${skippedNote(r)}`, true), false);
      names = r.names.slice(0, max);
      editing = false;
      say(`${names.length} names.${r.total > max ? ` Only the first ${max} fit.` : ''}${skippedNote(r)}`);
      return true;
    } catch (err) {
      say((err as Error).message, true);
      return false;
    }
  }

  async function store(next: Saved[], done: string) {
    busy = true;
    try {
      saved = (await api.put<{ lists: Saved[] }>('/api/cached-lists', { lists: next })).lists;
      say(done);
      return true;
    } catch (err) {
      say((err as Error).message, true);
      return false;
    } finally {
      busy = false;
    }
  }
  async function saveList(e: SubmitEvent) {
    e.preventDefault();
    const name = listName.trim();
    if (!name) return;
    const rest = saved.filter((l) => l.name.toLowerCase() !== name.toLowerCase());
    if (await store([...rest, { name, names: [...names] }], `Saved “${name}”.`)) {
      naming = false;
      listName = '';
    }
  }
  async function deleteList() {
    if (!savedNow || !(await confirmAction(`Delete the saved list “${savedNow.name}”?`, 'Delete'))) return;
    await store(saved.filter((l) => l.name !== savedNow.name), `Deleted “${savedNow.name}”.`);
  }
</script>

<div class="cn">
  <span>Asks <span class="num">{plural(names.length, 'name')}</span> from {listLabel}</span>
  <button type="button" class="btn sm" onclick={open} aria-haspopup="dialog">Edit names…</button>
</div>

<dialog class="sheet" bind:this={sheet} aria-labelledby="cn-title" aria-describedby="cn-lead" oncancel={restore}>
  <header>
    <h2 id="cn-title">Names for the Cached test</h2>
    <p id="cn-lead" class="lead">
      Each round asks the next name, so the answers are each resolver's round trip plus its front end. Your own most-used sites
      show what your repeat lookups cost.
    </p>
  </header>

  <div class="body">
    <div class="row">
      <label class="pick">
        <span class="visually-hidden">List</span>
        <select value={which} onchange={pick} aria-label="Name list">
          <option value="#default">Default ({defaults.length})</option>
          {#each saved as l (l.name)}<option value={l.name}>{l.name} ({l.names.length})</option>{/each}
          {#if which === '#custom'}<option value="#custom" disabled>Edited list</option>{/if}
        </select>
      </label>
      {#if savedNow}
        <button type="button" class="btn ghost sm" onclick={deleteList} disabled={busy}><Icon name="trash" size={13} />Delete list</button>
      {:else if !naming}
        <button type="button" class="btn ghost sm" onclick={() => (naming = true)} disabled={same(names, defaults)}>Save as…</button>
      {/if}
      <button type="button" class="btn ghost sm" onclick={() => (names = [...defaults])} disabled={same(names, defaults)}>Reset to default</button>
    </div>

    {#if naming}
      <form class="row" onsubmit={saveList}>
        <input class="inp" bind:value={listName} placeholder="Name this list" maxlength="40" aria-label="Name for this list" />
        <button class="btn sm" type="submit" disabled={busy || !listName.trim()}>
          {saved.some((l) => l.name.toLowerCase() === listName.trim().toLowerCase()) ? 'Overwrite' : 'Save'}
        </button>
        <button class="btn ghost sm" type="button" onclick={() => ((naming = false), (listName = ''))}>Cancel</button>
      </form>
    {/if}

    {#if editing}
      <textarea class="ta" bind:value={text} rows="8" spellcheck="false" aria-label="Names, one per line"></textarea>
      <div class="row">
        <button class="btn sm" type="button" onclick={applyText}>Use these</button>
        <button class="btn ghost sm" type="button" onclick={() => (editing = false)}>Cancel</button>
        <span class="hint">One per line, or separated by commas. Lines starting with # are ignored.</span>
      </div>
    {:else}
      <ul class="chips" aria-label="Names the Cached test asks">
        {#each names as n (n)}
          <li class="chip">
            <span class="mono">{n}</span>
            <button type="button" aria-label="Remove {n}" onclick={() => remove(n)} disabled={names.length <= 1}><Icon name="close" size={11} /></button>
          </li>
        {/each}
      </ul>

      <form class="row" onsubmit={add}>
        <!-- Opening the sheet starts in this field, not on the list picker above it. -->
        <!-- svelte-ignore a11y_autofocus -->
        <input class="inp" autofocus bind:value={adding} placeholder="Add a name, like example.com" aria-label="Add a name" disabled={names.length >= max} />
        <button class="btn sm" type="submit" disabled={!adding.trim() || names.length >= max}><Icon name="plus" size={13} />Add</button>
        <button class="btn ghost sm" type="button" onclick={edit}>Edit as text</button>
      </form>

      <div class="import">
        <label class="btn sm file">
          Import CSV or text file…
          <input type="file" accept=".csv,.tsv,.txt,.list,.hosts,text/csv,text/tab-separated-values,text/plain" onchange={importFile} />
        </label>
        <p class="hint">
          A CSV with a domain column, a ranked list such as Tranco’s, a hosts file, or one name per line. It replaces the list, up to
          the first {max} names.
        </p>
      </div>
    {/if}

    <p class="msg" class:err={message?.err} aria-live="polite">{message?.text ?? ''}</p>
    <p class="fine">
      Every resolver you test is asked these names, so leave out any you don't want them to see. A name that doesn't resolve is left
      out of the run, and the run says so.
    </p>
  </div>

  <footer>
    <span class="hint num">{names.length} of {max}</span>
    <button class="btn ghost sm" type="button" onclick={cancel}>Cancel</button>
    <button class="btn primary sm" type="button" onclick={done}>Done</button>
  </footer>
</dialog>

<style>
  /* Sits in the Cached test's hint column: checkbox 16 + key 10 + two 8px gaps. */
  .cn {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    margin: 2px 0 6px 42px;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .sheet {
    width: min(560px, calc(100vw - 32px));
    max-height: min(680px, calc(100vh - 32px));
    padding: 0;
    border: none;
    border-radius: var(--r-lg);
    background: var(--bg);
    color: var(--ink);
    box-shadow: var(--shadow-sheet);
  }
  .sheet[open] {
    display: grid;
    grid-template-rows: auto 1fr auto;
  }
  .sheet::backdrop {
    background: oklch(0 0 0 / 0.3);
  }
  .sheet header {
    display: grid;
    gap: 4px;
    padding: 16px 18px 12px;
    border-bottom: 1px solid var(--rule);
  }
  h2 {
    margin: 0;
    font-size: var(--t-md);
    font-weight: 600;
  }
  .sheet footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 18px;
    border-top: 1px solid var(--rule);
  }
  .sheet footer .hint {
    margin-right: auto;
  }
  .hint {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .body {
    display: grid;
    align-content: start;
    gap: 12px;
    padding: 14px 18px;
    overflow: auto;
  }
  .lead,
  .fine {
    font-size: var(--t-xs);
    color: var(--ink-3);
    max-width: 62ch;
  }
  .import {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 12px;
    align-items: center;
    padding-top: 12px;
    border-top: 1px solid var(--rule);
  }
  .import .hint {
    line-height: 1.4;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .pick select {
    max-width: 220px;
  }
  .inp {
    flex: 1 1 200px;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--rule-2);
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: var(--t-sm);
  }
  .ta {
    width: 100%;
    padding: 8px;
    border: 1px solid var(--rule-2);
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--ink);
    font-family: ui-monospace, 'SF Mono', Menlo, monospace;
    font-size: var(--t-xs);
    resize: vertical;
  }
  .chips {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 4px 1px 9px;
    border: 1px solid var(--rule-2);
    border-radius: 14px;
    background: var(--surface);
    font-size: var(--t-xs);
  }
  .chip button {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border: 0;
    border-radius: 50%;
    background: none;
    color: var(--ink-3);
    cursor: default;
    padding: 0;
  }
  .chip button:hover:not(:disabled) {
    background: var(--surface-3);
    color: var(--ink);
  }
  .chip button:disabled {
    opacity: 0.35;
  }
  .file {
    position: relative;
    cursor: default;
  }
  .file input {
    position: absolute;
    inset: 0;
    opacity: 0;
    cursor: default;
    width: 100%;
  }
  .msg {
    min-height: 1.2em;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .msg.err {
    color: var(--critical);
  }
</style>
