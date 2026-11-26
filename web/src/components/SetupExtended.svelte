<script lang="ts">
  import { tick } from 'svelte';
  import { app } from '../lib/store.svelte';
  import { api } from '../lib/api';
  import { gapOf, label, shortWhen } from '../lib/format';
  import { buildRows, rank } from '../lib/verdict';
  import { MAX_PICK, MIN_PICK, minutesOf, shortlist } from '../lib/extended';
  import type { RunRow, RunState } from '../lib/types';
  import Icon from './Icon.svelte';
  import ResolverMark from './ResolverMark.svelte';

  // The slots, in the order the run will take them: the mark and colour of each are the ones its results use.
  const picked = $derived(
    app.extPick.map((id) => {
      const r = app.resolvers.find((x) => x.id === id);
      return { id, name: r ? label(r.provider, r.variant) : id, provider: r?.provider ?? id, variant: r?.variant ?? '' };
    }),
  );

  // Removing the last filled slot takes its button away: keyboard focus moves to the last one left, or to the list.
  let slots = $state<HTMLElement>();
  async function remove(id: string, e: Event) {
    const held = document.activeElement === e.currentTarget;
    app.extPick = app.extPick.filter((x) => x !== id);
    if (!held) return;
    await tick();
    if (slots && !slots.contains(document.activeElement)) ([...slots.querySelectorAll<HTMLElement>('.x')].at(-1) ?? slots).focus();
  }

  // What a screen reader hears when the picks change. When every slot is in use, the sentence by the list says so.
  let live = $state('');
  let was: typeof picked | undefined;
  $effect(() => {
    const now = picked;
    const prev = was ?? now;
    was = now;
    const added = now.filter((p) => !prev.some((w) => w.id === p.id));
    const gone = prev.filter((w) => !now.some((p) => p.id === w.id));
    if (!added.length && !gone.length) return;
    const count = `${now.length} of ${MAX_PICK} picked.`;
    if (added.length + gone.length > 1) live = count;
    else live = `${(added[0] ?? gone[0]).name} ${added.length ? 'added' : 'removed'}.${now.length < MAX_PICK ? ` ${count}` : ''}`;
  });

  // An offer, never a default: the fastest of the latest Accurate run, and the resolver in use.
  let offer = $state<{ at: number; ids: string[] } | null>(null);

  async function find(runs: RunRow[]) {
    // The newest finished runs, in case the latest was a Quick or Extended one.
    for (const row of runs.filter((r) => r.status === 'done' && !r.summary.mode).slice(0, 5)) {
      let run: RunState;
      try {
        run = await api.get<RunState>(`/api/runs/${row.id}`);
      } catch {
        continue;
      }
      if (run.config.mode !== 'accurate') continue;
      const seen = new Set<string>();
      const ranked = rank(buildRows(run, app.resolvers))
        .filter((r) => r.measured && !seen.has(r.target.resolver_id) && !!seen.add(r.target.resolver_id))
        .map((r) => ({ id: r.target.resolver_id }));
      const current = run.resolvers.filter((r) => r.group === 'system').map((r) => r.id);
      // Extended runs over IPv4, so a resolver without an IPv4 address is not offered.
      const ids = shortlist(ranked, current)
        .map((r) => r.id)
        .filter((id) => app.resolvers.find((r) => r.id === id)?.ipv4.length)
        .slice(0, MAX_PICK);
      return ids.length >= MIN_PICK ? { at: run.started_at, ids } : null;
    }
    return null;
  }

  $effect(() => {
    let stale = false;
    find(app.boot?.runs ?? []).then((o) => stale || (offer = o));
    return () => {
      stale = true;
    };
  });

  const offered = $derived(!!offer && (offer.ids.length !== app.extPick.length || offer.ids.some((id) => !app.extPick.includes(id))));
  const samples = $derived(app.draft?.ext_samples === 10000 ? 10000 : 5000);
  const mins = $derived(minutesOf(samples, gapOf(app.draft?.pace)));
</script>

<div class="ext">
  <h2 class="section-title">Extended test</h2>
  <p class="lede">
    Each resolver you pick gets the same <span class="num">{samples.toLocaleString()}</span> lookups, paced evenly: mostly common
    names asked again, as in ordinary use, and a fifth of them less common sites asked once. That is enough to tell whether a gap
    of a few milliseconds is real or a tie.
  </p>

  <ol class="slots" aria-label="Resolvers to compare" tabindex="-1" bind:this={slots}>
    {#each { length: MAX_PICK } as _, i}
      {@const r = picked[i]}
      <li class="slot" class:empty={!r}>
        {#if r}
          <ResolverMark t={i} />
          <span class="sname" title={r.name}><b>{r.provider}</b>{#if r.variant}<span>{r.variant}</span>{/if}</span>
          <button class="btn ghost sm x" aria-label="Remove {r.name} from the comparison" title="Remove" onclick={(e) => remove(r.id, e)}
            ><Icon name="close" size={13} /></button
          >
        {:else}
          <span class="snote">{i < MIN_PICK ? 'Pick a resolver below' : 'Optional'}</span>
        {/if}
      </li>
    {/each}
  </ol>
  <p class="visually-hidden" aria-live="polite">{live}</p>

  {#if offer && offered}
    <p class="offer">
      <button
        class="btn sm"
        title="The three fastest, and the resolver you use now if it is not one of them."
        onclick={() => (app.extPick = offer!.ids)}>Use the fastest from your last Accurate run</button
      >
      <span class="at num">{shortWhen(offer.at)}</span>
    </p>
  {/if}

  <p class="notice">
    <Icon name="info" size={14} />
    <span
      >This takes about <b class="num">{mins} minutes</b>. Recursa keeps this computer awake while it runs. Stay on this network; closing
      the lid pauses the test. You can stop at any time and keep the results so far.</span
    >
  </p>
</div>

<style>
  .ext {
    container-type: inline-size;
    margin-bottom: 26px;
  }
  .lede {
    margin-top: 6px;
    max-width: 72ch;
    font-size: var(--t-sm);
    line-height: 1.5;
    color: var(--ink-2);
  }
  .slots {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 8px;
    margin: 14px 0 0;
    padding: 0;
    list-style: none;
  }
  @container (max-width: 800px) {
    .slots {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  .slot {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    min-height: 46px;
    padding: 0 5px 0 11px;
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    background: var(--surface);
  }
  .slot.empty {
    padding-right: 11px;
    border-style: dashed;
    border-color: var(--rule-2);
    background: none;
  }
  .slot :global(svg) {
    flex: none;
  }
  .sname {
    flex: 1;
    display: grid;
    min-width: 0;
    line-height: 1.3;
  }
  .sname > * {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sname b {
    font-size: var(--t-sm);
    font-weight: 540;
  }
  .sname span {
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .snote {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .x {
    flex: none;
    width: 28px;
    padding: 0;
    justify-content: center;
  }
  .offer {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 10px;
    margin-top: 8px;
  }
  .at {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .notice {
    display: flex;
    gap: 8px;
    max-width: 72ch;
    margin-top: 14px;
    font-size: var(--t-xs);
    line-height: 1.5;
    color: var(--ink-2);
  }
  .notice :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--ink-3);
  }
  :global(html.mobile) .x {
    width: 44px;
    height: 44px;
  }
</style>
