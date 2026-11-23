<script lang="ts">
  // The Extended test's setup: finalists from the latest saved Accurate run, the cost in time and exposure, and Start.
  import { untrack } from 'svelte';
  import { app } from '../lib/store.svelte';
  import { api } from '../lib/api';
  import { label, ms, timeOfDay, when } from '../lib/format';
  import { buildRows, rank } from '../lib/verdict';
  import { MAX_PICK, mergedMedian, minutesOf, shortlist, toggled } from '../lib/extended';
  import type { RunRow, RunState } from '../lib/types';
  import Icon from './Icon.svelte';

  interface Finalist {
    id: string;
    name: string;
    addr: string;
    median: number | null;
    current: boolean;
  }

  // null while looking, false when there is no Accurate run to take finalists from.
  let source = $state<{ at: number; list: Finalist[] } | null | false>(null);

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
        .map((r) => ({
          id: r.target.resolver_id,
          name: label(r.target.provider, r.target.variant),
          addr: r.target.ip,
          median: mergedMedian(Object.values(r.dist).map((d) => d.values)),
        }));
      const current = run.resolvers.filter((r) => r.group === 'system').map((r) => r.id);
      return { at: run.started_at, list: shortlist(ranked, current) };
    }
    return false as const;
  }

  $effect(() => {
    const runs = app.boot?.runs ?? [];
    find(runs).then((s) => {
      source = s;
      if (!s) return;
      untrack(() => {
        // Start from the shortlist; keep earlier ticks that are still on it.
        const ids = s.list.map((f) => f.id);
        const kept = app.extPick.filter((id) => ids.includes(id));
        app.extPick = kept.length ? kept : ids.slice(0, MAX_PICK);
      });
    });
  });

  const n = $derived(app.extPick.length);
  const mins = minutesOf(5000, 300);
  const today = (t: number) => new Date(t).toDateString() === new Date().toDateString();
</script>

<div class="ext">
  <h2 id="pick-h" class="h">When two resolvers are too close to call</h2>
  <p class="lede">
    Quick and Accurate tell you who is in the running. Extended sends the finalists the same <span class="num">5,000</span> lookups, slowly, over about
    <span class="num">{mins}</span> minutes, so a gap of a millisecond or two can be called real or called a tie, and you can see whether it held the whole time.
  </p>

  {#if source === null}
    <p class="muted" role="status">Looking for your latest Accurate run.</p>
  {:else if source === false}
    <div class="none" role="note">
      <p><b>There is no saved Accurate run to choose finalists from.</b> Run an Accurate test first; the closest resolvers from it are offered here.</p>
      <button class="btn" type="button" onclick={() => app.draft && (app.draft.mode = 'accurate')}>Set up an Accurate run</button>
    </div>
  {:else}
    <fieldset class="picks">
      <legend class="pickhead">
        <span>Shortlist, from {today(source.at) ? `today's Accurate run at ${timeOfDay(source.at)}` : `the Accurate run of ${when(source.at)}`}</span>
        <span><span class="num">{n}</span> of {MAX_PICK} chosen</span>
      </legend>
      {#each source.list as f}
        {@const on = app.extPick.includes(f.id)}
        <label class="pick" class:on>
          <input type="checkbox" checked={on} disabled={!on && n >= MAX_PICK} onchange={() => (app.extPick = toggled(app.extPick, f.id))} />
          <span class="who"><b>{f.name}</b> <span class="addr num">{f.addr}</span>{#if f.current} <span class="chip">Current</span>{/if}</span>
          <span class="was"><span class="num">{ms(f.median)}</span> median</span>
        </label>
      {/each}
    </fieldset>

    <div class="notice" role="note">
      <Icon name="info" size={14} />
      <span>
        This takes about <b class="num">{mins} minutes</b>. Recursa keeps this computer awake while it runs. Stay on this network; closing the lid pauses the test.
        You can stop at any time and keep the results so far.
      </span>
    </div>

    <p class="names">
      <b>The names.</b> <span class="num">500</span> common names that ship with Recursa, each asked three times, and <span class="num">3,500</span> less common
      sites drawn from today's Tranco list. Recursa downloads that list when the run starts; without a connection to it, the run uses Recursa's own list
      instead. The less common names are not screened for content, so sites of every kind will show in the DNS log of your router or filter.
    </p>

    <div class="go">
      <button class="btn primary" type="button" disabled={n < 2} onclick={() => app.startRun()}><Icon name="play" size={12} />Start Extended test</button>
      <p>
        {#if n < 2}Tick at least two resolvers to compare.{:else}<span class="num">5,000</span> lookups to each of {n} resolvers: <span class="num">{(5000 * n).toLocaleString()}</span>
          queries, about <span class="num">3.3</span> a second to each resolver.{/if}
      </p>
    </div>
  {/if}
</div>

<style>
  .ext {
    display: grid;
    gap: 18px;
    max-width: 46rem;
  }
  .h {
    margin: 0;
    font-size: var(--t-xl);
    font-weight: 620;
    color: var(--ink);
  }
  .lede,
  .names,
  .muted {
    margin: 0;
    color: var(--ink-2);
    font-size: var(--t-base);
    line-height: 1.55;
  }
  .names b {
    color: var(--ink);
  }
  .picks {
    margin: 0;
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: 8px;
    overflow: hidden;
  }
  .pickhead {
    float: left;
    width: 100%;
    display: flex;
    justify-content: space-between;
    padding: 10px 14px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    border-bottom: 1px solid var(--rule);
  }
  .pick {
    clear: both;
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--rule);
    cursor: pointer;
  }
  .pick:last-child {
    border-bottom: 0;
  }
  .pick:has(input:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .pick:has(input:disabled) {
    cursor: default;
    color: var(--ink-3);
  }
  .who b {
    font-weight: 600;
    color: var(--ink);
  }
  .addr,
  .was {
    color: var(--ink-2);
    font-size: var(--t-sm);
  }
  .chip {
    font-size: var(--t-xs);
    padding: 1px 6px;
    border: 1px solid var(--rule-strong, var(--rule));
    border-radius: 4px;
    color: var(--ink-2);
  }
  .notice,
  .none {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 12px 14px;
    border: 1px solid var(--rule);
    border-radius: 8px;
    background: var(--surface);
    font-size: var(--t-sm);
    color: var(--ink-2);
    line-height: 1.5;
  }
  .none {
    flex-direction: column;
  }
  .none p {
    margin: 0;
  }
  .none b,
  .notice b {
    color: var(--ink);
  }
  .go {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .go p {
    margin: 0;
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
</style>
