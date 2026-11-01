<script lang="ts">
  // The network intel while it runs, as a board of answers. Each resolver gets a tile per check (its route origin,
  // whether invalid routes are filtered over IPv4 and IPv6, and the network its queries leave from); a tile turns over
  // when the resolver's report lands, filled with its group's colour when the check passes. Every resolver that
  // answered is checked at once; the route-filtering test is a live session with Check My DNS that can take half a
  // minute, so the rows land one by one. Rows stay put, fastest cached lookup first.
  import type { Resolver, RovCheck, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { answered, egressCell, roaCell, type Cell } from '../lib/cells';
  import { ms } from '../lib/format';
  import { GLYPH } from './Glyph.svelte';
  import Icon from './Icon.svelte';
  import PhaseStages, { type Stage } from './PhaseStages.svelte';

  let { run, rows }: { run: RunState; rows: Row[] } = $props();

  const GROUPS: { title: string; tone: string; checks: string[]; n: number }[] = [
    { title: 'Route origin', tone: 'var(--s-cached)', checks: ['RPKI'], n: 1 },
    { title: 'Invalid routes filtered', tone: 'var(--s-tld)', checks: ['IPv4', 'IPv6'], n: 2 },
    { title: 'Queries leave from', tone: 'var(--w-amber)', checks: ['Network'], n: 3 },
  ];
  const TOTAL = 4;

  // Rows keep one order for the whole phase: fastest cached lookup first, from the phases that already ran.
  const cached = $derived(new Map(rows.flatMap((r) => (r.resolver ? [[r.target.resolver_id, r.dist.cached?.p50 ?? null]] : []))));
  const lanes = $derived.by(() => {
    const at = (r: Resolver) => cached.get(r.id) ?? Infinity;
    return [...run.resolvers].sort((a, b) => (at(a) === at(b) ? 0 : at(a) - at(b)));
  });
  const state = (r: Resolver) => (run.intel[r.id] ? 'done' : answered(run, r.id) ? 'run' : 'off');

  const rov = (c: RovCheck): Cell => ({
    status: c.status === 'filtered' ? 'pass' : c.status === 'unfiltered' ? 'fail' : 'unknown',
    text: c.status === 'filtered' ? 'Filtered' : c.status === 'unfiltered' ? 'Not filtered' : 'Not tested',
    prov: c.status === 'not_tested' ? 'unknown' : 'verified',
    evidence: c.detail,
  });
  // Every reported resolver's answers, one per tile in board order.
  const answers = $derived.by(() => {
    const out: Record<string, { status: Cell['status']; text: string; tip: string }[]> = {};
    for (const r of run.resolvers) {
      const v = run.intel[r.id];
      if (!v) continue;
      const roa = roaCell(run, r.id);
      const eg = egressCell(run, r.id);
      const fam = v.rov ? [rov(v.rov.v4), rov(v.rov.v6)] : [];
      out[r.id] = [
        { status: roa.status, text: roa.text, tip: `RPKI route origin: ${roa.text}${roa.evidence ? ` · ${roa.evidence}` : ''}` },
        ...(['IPv4', 'IPv6'] as const).map((f, i) => {
          const c = fam[i];
          return c
            ? { status: c.status, text: c.text, tip: `${f}: ${c.text}, ${c.evidence}` }
            : { status: 'unknown' as const, text: 'Not tested', tip: `${f}: not tested` };
        }),
        { status: eg.status, text: eg.text, tip: `Queries leave from ${eg.text}${eg.evidence ? ` · ${eg.evidence}` : ''}` },
      ];
    }
    return out;
  });
  const reported = $derived(Object.keys(answers).length);
  const flying = $derived(lanes.filter((r) => state(r) === 'run').length);
  // How many reported resolvers pass each check, and how many networks the queries leave from, for the foot.
  const tally = $derived(Array.from({ length: 3 }, (_, i) => Object.values(answers).filter((a) => a[i].status === 'pass').length));
  const nets = $derived(new Set(Object.values(answers).map((a) => a[3].text)).size);

  const naming = $derived(run.cdn_naming);
  const stages = $derived.by((): Stage[] => {
    const total = run.total || run.resolvers.length;
    const out: Stage[] = [
      {
        label: 'Check each resolver',
        value: `${reported} of ${total}${flying ? `, ${flying} in flight` : ''}`,
        share: total ? reported / total : 1,
        state: reported >= total ? 'done' : 'now',
        tone: 'var(--s-cached)',
      },
    ];
    if (run.config.cdn && (naming || run.cdn)) {
      const named = naming?.done ?? 0, toName = naming?.total ?? 0;
      out.push({
        label: 'Name the CDN edges',
        value: naming ? `${named} of ${toName} edges` : 'done',
        share: naming && toName ? named / toName : 1,
        state: !naming || named >= toName ? 'done' : 'now',
        tone: 'var(--w-rose)',
      });
    }
    return out;
  });
</script>

<section class="intel" aria-labelledby="intel-h" aria-busy="true">
  <header>
    <h2 id="intel-h">Network intel</h2>
    <p class="lede">
      Each resolver’s network is looked up: whether its route origin is authorised, whether invalid routes are filtered on its path
      (a live test with Check My DNS by DNS-OARC, up to half a minute each), and where its queries leave for the Internet. A filled
      tile is a check it passes.
    </p>
  </header>

  <PhaseStages {stages} label="Network intel progress" />

  <div class="board" role="list">
    <div class="lane head" aria-hidden="true">
      <span></span>
      <span class="tiles">
        {#each GROUPS as g}
          <span class="group" style:--tone={g.tone} style:--n={g.n} style:--cols={g.checks.length}>
            <span class="gt">{g.title}</span>
            {#each g.checks as c}<span class="code">{c}</span>{/each}
          </span>
        {/each}
      </span>
      <span class="t">Passed</span>
    </div>

    {#each lanes as r, n (r.id)}
      {@const st = state(r)}
      {@const got = answers[r.id]}
      {@const passed = got?.slice(0, 3).filter((a) => a.status === 'pass').length ?? 0}
      <div class="lane {st}" role="listitem" aria-label="{r.provider} {r.variant}: {got ? `${passed} of 3 checks passed, queries leave from ${got[3].text}` : st === 'off' ? 'never answered' : 'checking'}" style:--i={n}>
        <span class="who">
          <span class="name"><b>{r.provider}</b> <span class="var">{r.variant}</span></span>
          {#if cached.get(r.id) != null}<span class="lat num">{ms(cached.get(r.id))} cached</span>{/if}
        </span>
        <span class="tiles" aria-hidden="true">
          <span class="group" style:--tone={GROUPS[0].tone} style:--n={GROUPS[0].n} style:--cols={1}>
            <span class="tile {got?.[0].status ?? ''}" style:--k={0} title={got?.[0].tip}>{#if got}<Icon name={GLYPH[got[0].status]} size={12} />{/if}</span>
          </span>
          <span class="group" style:--tone={GROUPS[1].tone} style:--n={GROUPS[1].n} style:--cols={2}>
            {#each [1, 2] as k}
              <span class="tile {got?.[k].status ?? ''}" style:--k={k} title={got?.[k].tip}>{#if got}<Icon name={GLYPH[got[k].status]} size={12} />{/if}</span>
            {/each}
          </span>
          <span class="group" style:--tone={GROUPS[2].tone} style:--n={GROUPS[2].n} style:--cols={1}>
            <span class="tile net {got ? 'info' : ''}" style:--k={3} title={got?.[3].tip}>{#if got}{got[3].text}{/if}</span>
          </span>
        </span>
        <span class="t num" aria-hidden="true">
          {#if got}<b>{passed}</b><span class="of">/3</span>{:else}{st === 'off' ? 'silent' : 'checking'}{/if}
        </span>
      </div>
    {/each}

    <div class="lane foot" aria-label="Resolvers passing each check: {tally.join(', ')} of {reported}; queries leave from {nets} networks">
      <span class="who">Passing</span>
      <span class="tiles" aria-hidden="true">
        {#each GROUPS as g, gi}
          <span class="group" style:--tone={g.tone} style:--n={g.n} style:--cols={g.checks.length}>
            {#if gi < 2}
              {#each g.checks as _, ci}
                {@const c = tally[gi + ci]}
                <span class="count">
                  <span class="col" style:--share={reported ? c / reported : 0}></span>
                  <span class="num">{c}</span>
                </span>
              {/each}
            {:else}
              <span class="count nets"><span class="num">{nets}</span> {nets === 1 ? 'network' : 'networks'}</span>
            {/if}
          </span>
        {/each}
      </span>
      <span class="t num">of {reported}</span>
    </div>
  </div>
</section>

<style>
  .intel {
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
  /* One raised sheet, as the probes board is. */
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
  /* The rows still being checked sit on a band. */
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
  .off .name {
    color: var(--ink-3);
  }

  /* Three groups side by side, the network column widest, so the columns line up down the board. */
  .tiles {
    grid-area: tiles;
    display: flex;
    gap: 12px;
    perspective: 600px;
  }
  .group {
    flex: var(--n) 1 0;
    display: grid;
    grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
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

  .tile {
    display: grid;
    place-items: center;
    height: 30px;
    border-radius: var(--r-sm);
    background: var(--surface-3);
    color: var(--ink-3);
  }
  /* In flight: a light runs along the row while the checks ask their questions. */
  .run .tile {
    animation: ask 1.1s ease-in-out calc(var(--k) * 90ms) infinite;
  }
  /* Answered: each tile turns over in turn, left to right. */
  .done .tile {
    animation: turn 420ms var(--ease-out) calc(var(--k) * 48ms) backwards;
  }
  .tile.pass {
    background: var(--tone);
    color: oklch(0.2 0.01 285);
  }
  .tile.fail {
    color: var(--critical);
  }
  .tile.partial {
    color: var(--warning);
  }
  /* The network is a name, not a verdict: ink on the tile, in the row's own size. */
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
