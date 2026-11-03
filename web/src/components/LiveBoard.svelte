<script lang="ts" module>
  import type { CheckStatus } from '../lib/types';

  /** Where a resolver's row is: report landed, being asked, waiting its turn, or never answered. */
  export type Phase = 'done' | 'run' | 'queued' | 'off';

  /** One answer, as its tile shows it. */
  export interface BoardCell {
    status: CheckStatus;
    tip: string;
    /** What the tile says in a named group. */
    text?: string;
  }

  /** A run of columns under one heading. */
  export interface BoardGroup {
    title: string;
    tone: string;
    /** One per column, as the column head (or the key of it, see the board's `heads`). */
    checks: string[];
    /** Flex share of the board. Without one the group is as wide as it has columns; with one its tiles sit one to a column. */
    n?: number;
    /** What the single tile says when every answer in the group is a note rather than a verdict. */
    none?: string;
    /** Its tiles carry a name (the answer's text), not a verdict. `noun` counts them in the foot; `lead` introduces one aloud. */
    named?: { noun: string; lead: string };
  }
</script>

<script lang="ts">
  // A live phase as a board of answers, one row per resolver and one tile per check, grouped under headings. A tile turns
  // over when the resolver's report lands, filled with its group's colour when the check passes. Rows keep one order for
  // the whole phase, fastest cached lookup first, and the foot counts how many resolvers pass each check so far. What
  // the checks are and how they are answered is the caller's; the board lays them out.
  import type { Resolver, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { ms } from '../lib/format';
  import { GLYPH } from './Glyph.svelte';
  import Icon from './Icon.svelte';
  import PhaseStages, { type Stage } from './PhaseStages.svelte';

  let {
    run,
    rows,
    name,
    heading,
    lede,
    stagesLabel,
    stages,
    groups,
    heads,
    answers,
    tally,
    namedCount = 0,
    scored,
    phase,
    words,
    cascade = false,
    upright = false,
    announce = false,
    askMs,
    turnMs,
  }: {
    run: RunState;
    rows: Row[];
    /** The board's class, and the stem of its heading's id. */
    name: string;
    heading: string;
    lede: string;
    stagesLabel: string;
    stages: Stage[];
    groups: BoardGroup[];
    /** The head of each column, for groups whose `checks` are keys rather than heads. */
    heads?: Record<string, string>;
    /** Every reported resolver's answers, one per column in board order. */
    answers: Record<string, BoardCell[]>;
    /** How many reported resolvers pass each scored column. */
    tally: number[];
    /** How many distinct names the named group's tiles carry, for the foot. */
    namedCount?: number;
    /** How many of a row's first answers count towards its score. */
    scored: number;
    phase: (r: Resolver) => Phase;
    /** What a row says while its report is awaited, by phase. */
    words: Record<string, string>;
    /** Rows settle in one after another as the board arrives. */
    cascade?: boolean;
    /** Column heads read across, not upward. */
    upright?: boolean;
    /** Each answered tile is announced on its own; otherwise the row's label says it all. */
    announce?: boolean;
    /** Stagger between neighbouring tiles while their row is asked, and while its report turns them over. */
    askMs: number;
    turnMs: number;
  } = $props();

  // Rows keep one order for the whole phase: fastest cached lookup first, from the phases that already ran.
  const cached = $derived(new Map(rows.flatMap((r) => (r.resolver ? [[r.target.resolver_id, r.dist.cached?.p50 ?? null]] : []))));
  const lanes = $derived.by(() => {
    const at = (r: Resolver) => cached.get(r.id) ?? Infinity;
    return [...run.resolvers].sort((a, b) => (at(a) === at(b) ? 0 : at(a) - at(b)));
  });
  // Each group's first column, so its tiles and tallies can be found in the flat lists.
  const start = $derived(groups.map((_, g) => groups.slice(0, g).reduce((n, x) => n + x.checks.length, 0)));
  const named = $derived(groups.find((g) => g.named)?.named);
  const namedAt = $derived(start[groups.findIndex((g) => g.named)]);
  const reported = $derived(Object.keys(answers).length);

  const passed = (got: BoardCell[]) => got.slice(0, scored).filter((a) => a.status === 'pass').length;
  // What a row says of itself to a screen reader.
  const spoken = (got: BoardCell[] | undefined, st: Phase) =>
    got ? `${passed(got)} of ${scored} checks passed${named ? `, ${named.lead} ${got[namedAt].text}` : ''}` : st === 'off' ? 'never answered' : words[st];
</script>

<section class={name} aria-labelledby="{name}-h" aria-busy="true">
  <header>
    <h2 id="{name}-h">{heading}</h2>
    <p class="lede">{lede}</p>
  </header>

  <PhaseStages {stages} label={stagesLabel} />

  <div class="board" class:upright role="list" style:--ask-step="{askMs}ms" style:--turn-step="{turnMs}ms">
    <div class="lane head" aria-hidden="true">
      <span></span>
      <span class="tiles">
        {#each groups as g}
          <span class="group" style:--tone={g.tone} style:--n={g.n ?? g.checks.length} style:--cols={g.n ? g.checks.length : undefined}>
            <span class="gt">{g.title}</span>
            {#each g.checks as c}<span class="code">{heads?.[c] ?? c}</span>{/each}
          </span>
        {/each}
      </span>
      <span class="t">Passed</span>
    </div>

    {#each lanes as r, n (r.id)}
      {@const st = phase(r)}
      {@const got = answers[r.id]}
      <div class="lane {st}" class:cascade role="listitem" aria-label="{r.provider} {r.variant}: {spoken(got, st)}" style:--i={n}>
        <span class="who">
          <span class="name"><b>{r.provider}</b> <span class="var">{r.variant}</span></span>
          {#if cached.get(r.id) != null}<span class="lat num">{ms(cached.get(r.id))} cached</span>{/if}
        </span>
        <span class="tiles" aria-hidden={announce ? undefined : 'true'}>
          {#each groups as g, gi}
            {@const mine = got?.slice(start[gi], start[gi] + g.checks.length)}
            <span class="group" style:--tone={g.tone} style:--n={g.n ?? g.checks.length} style:--cols={g.n ? g.checks.length : undefined}>
              {#if g.none && mine?.every((a) => a.status === 'info')}
                <span class="tile na" style:--k={start[gi]} title={mine.map((a) => a.tip).join('\n')}>{g.none}</span>
              {:else}
                {#each g.checks as _, ci}
                  {@const k = start[gi] + ci}
                  {@const a = mine?.[ci]}
                  <span
                    class="tile {a ? (g.named ? 'info' : a.status) : ''}"
                    class:net={g.named}
                    style:--k={k}
                    title={a?.tip}
                    role={announce && a ? 'img' : undefined}
                    aria-label={announce ? a?.tip : undefined}
                  >
                    {#if a && g.named}{a.text}{:else if a}<Icon name={GLYPH[a.status]} size={12} />{/if}
                  </span>
                {/each}
              {/if}
            </span>
          {/each}
        </span>
        <span class="t num" aria-hidden="true">
          {#if got}<b>{passed(got)}</b><span class="of">/{scored}</span>{:else}{st === 'off' ? 'silent' : words[st]}{/if}
        </span>
      </div>
    {/each}

    <div
      class="lane foot"
      aria-label="Resolvers passing each check: {tally.join(', ')} of {reported}{named ? `; ${named.lead} ${namedCount} ${named.noun}s` : ''}"
    >
      <span class="who">Passing</span>
      <span class="tiles" aria-hidden="true">
        {#each groups as g, gi}
          <span class="group" style:--tone={g.tone} style:--n={g.n ?? g.checks.length} style:--cols={g.n ? g.checks.length : undefined}>
            {#if g.named}
              <span class="count nets"><span class="num">{namedCount}</span> {namedCount === 1 ? g.named.noun : `${g.named.noun}s`}</span>
            {:else}
              {#each g.checks as _, ci}
                {@const c = tally[start[gi] + ci]}
                <span class="count">
                  <span class="col" style:--share={reported ? c / reported : 0}></span>
                  <span class="num">{c}</span>
                </span>
              {/each}
            {/if}
          </span>
        {/each}
      </span>
      <span class="t num">of {reported}</span>
    </div>
  </div>
</section>

<style>
  section {
    display: grid;
    gap: 18px;
    container-type: inline-size;
  }
  h2 {
    margin: 0;
    font-size: var(--t-lg);
  }
  .lede {
    margin: 4px 0 0;
    max-width: 68ch;
    color: var(--ink-2);
    font-size: var(--t-sm);
  }
  /* One raised sheet, as the live ranking is. */
  .board {
    display: grid;
    padding: 14px 10px 12px;
    background: var(--surface);
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
  }
  .lane {
    display: grid;
    grid-template-columns: minmax(120px, 190px) minmax(0, 1fr) 6ch;
    grid-template-areas: 'who tiles t';
    align-items: center;
    column-gap: 16px;
    position: relative;
    padding: 5px 8px;
    border-radius: var(--r-md);
    font-size: var(--t-xs);
  }
  /* The rows being asked right now sit on a band. */
  .lane.run {
    background: var(--surface-2);
  }
  /* A row whose report just landed washes once and settles. */
  .lane.done::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: var(--ink);
    opacity: 0;
    pointer-events: none;
    animation: landed 900ms var(--ease-out);
  }
  .who {
    grid-area: who;
    display: grid;
    min-width: 0;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--t-sm);
  }
  .var,
  .lat,
  .queued .name,
  .off .name {
    color: var(--ink-3);
  }

  /* The groups side by side, each as wide as its share, so the columns line up down the board. */
  .tiles {
    grid-area: tiles;
    display: flex;
    gap: 12px;
    perspective: 600px;
  }
  .group {
    flex: var(--n) 1 0;
    display: grid;
    grid-template-columns: repeat(var(--cols, var(--n)), minmax(0, 1fr));
    gap: 4px;
  }

  .head {
    align-items: end;
    padding-bottom: 8px;
    color: var(--ink-2);
    font-size: var(--t-2xs);
  }
  .gt {
    grid-column: 1 / -1;
    padding-bottom: 6px;
    margin-bottom: 4px;
    border-bottom: 3px solid var(--tone);
    border-radius: 0 0 2px 2px;
    color: var(--ink);
    font-weight: 600;
    white-space: nowrap;
  }
  .code {
    justify-self: center;
    white-space: nowrap;
    line-height: 1;
  }
  /* Column names read upward, so thirteen of them fit over tiles a finger wide. */
  .board:not(.upright) .code {
    writing-mode: vertical-rl;
    transform: rotate(180deg);
  }

  .tile {
    display: grid;
    place-items: center;
    height: 30px;
    border-radius: var(--r-sm);
    background: var(--surface-3);
    color: var(--ink-3);
  }
  .queued .tile {
    opacity: 0.45;
  }
  /* In flight: a light runs along the row while the checks ask their questions. */
  .run .tile {
    animation: ask 1.1s ease-in-out calc(var(--k) * var(--ask-step)) infinite;
  }
  /* Answered: each tile turns over in turn, left to right. */
  .done .tile {
    animation: turn 420ms var(--ease-out) calc(var(--k) * var(--turn-step)) backwards;
  }
  .tile.pass {
    background: var(--tone);
    color: oklch(0.2 0.01 285);
  }
  .tile.na {
    grid-column: 1 / -1;
    background: none;
    border: 1px dashed var(--rule);
    font-size: var(--t-2xs);
    white-space: nowrap;
    overflow: hidden;
  }
  .tile.fail {
    color: var(--critical);
  }
  .tile.partial {
    color: var(--warning);
  }
  /* A name, not a verdict: ink on the tile, in the row's own size. */
  .tile.net {
    padding: 0 8px;
    font-size: var(--t-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    display: block;
    line-height: 30px;
  }
  .tile.net.info {
    color: var(--ink);
    background: color-mix(in oklch, var(--tone) 18%, var(--surface-3));
  }

  .t {
    grid-area: t;
    text-align: right;
    color: var(--ink-2);
  }
  .t b {
    color: var(--ink);
    font-size: var(--t-md);
  }
  .of,
  .queued .t,
  .off .t {
    color: var(--ink-3);
  }

  .foot {
    align-items: end;
    margin-top: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--rule);
    border-radius: 0;
    color: var(--ink-3);
  }
  .foot .who {
    color: var(--ink-2);
  }
  .count {
    display: grid;
    justify-items: center;
    gap: 4px;
  }
  .count.nets {
    grid-auto-flow: column;
    justify-content: start;
    align-items: baseline;
    gap: 5px;
    padding-left: 8px;
    white-space: nowrap;
  }
  /* A slim gauge per column: the track is every reported resolver, the fill those that pass. */
  .col {
    position: relative;
    width: 6px;
    height: 22px;
    border-radius: 3px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .col::after {
    content: '';
    position: absolute;
    inset: 0;
    background: var(--tone);
    transform: scaleY(var(--share));
    transform-origin: bottom;
    transition: transform 500ms var(--ease-out);
  }
  .count .num {
    color: var(--ink-2);
  }

  @keyframes turn {
    from {
      opacity: 0;
      transform: rotateX(-90deg);
    }
  }
  @keyframes landed {
    from {
      opacity: 0.07;
    }
  }
  @keyframes ask {
    50% {
      background: color-mix(in oklch, var(--tone) 30%, var(--surface-3));
    }
  }

  /* Narrow: name and score on one line, the tiles under them. */
  @container (max-width: 560px) {
    .lane {
      grid-template-columns: 1fr auto;
      grid-template-areas: 'who t' 'tiles tiles';
      row-gap: 6px;
    }
    .head > :not(.tiles),
    .foot > .t {
      display: none;
    }
    .who {
      grid-auto-flow: column;
      justify-content: start;
      gap: 8px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .run .tile,
    .done .tile,
    .lane.done::after {
      animation: none;
    }
    .col::after {
      transition: none;
    }
  }
</style>
