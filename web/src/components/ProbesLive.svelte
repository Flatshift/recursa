<script lang="ts">
  // The capability probes while they run, as a board of answers. Each resolver gets one tile per check, grouped as the
  // Capabilities view groups them; a tile turns over to its glyph when the resolver's report lands, filled with its
  // group's colour when the check passes. Rows stay put, fastest cached lookup first, and the foot of each column counts
  // how many resolvers pass that check so far. Eight are probed at a time.
  import type { CheckStatus, Resolver, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { capCell } from '../lib/cells';
  import { CAP, CAP_KEYS } from '../lib/caps';
  import { ms } from '../lib/format';
  import { GLYPH } from './Glyph.svelte';
  import Icon from './Icon.svelte';
  import PhaseStages, { type Stage } from './PhaseStages.svelte';

  let { run, rows }: { run: RunState; rows: Row[] } = $props();

  const AT_ONCE = 8;
  // The column heads, short enough for a tile.
  const CODE: Record<string, string> = {
    dnssec: 'DNSSEC',
    ecs: 'ECS',
    qmin: 'QNAME min',
    rebind: 'Rebinding',
    filter_malware: 'Malware',
    filter_phishing: 'Phishing',
    filter_ads: 'Ads',
    ede: 'EDE',
    edns_version: 'EDNS',
    cookies: 'Cookies',
    tcp: 'TCP',
    dot: 'DoT',
    doh: 'DoH',
  };
  const GROUPS: { title: string; tone: string; none?: string; checks: string[] }[] = [
    { title: 'Privacy & security', tone: 'var(--s-cached)', checks: CAP_KEYS.privacy },
    {
      title: 'Blocking',
      tone: 'var(--w-amber)',
      // A resolver that filters nothing answers all three with a note rather than a verdict: one cell says so.
      none: 'No filtering',
      checks: CAP_KEYS.blocking,
    },
    { title: 'Protocol', tone: 'var(--s-tld)', checks: CAP_KEYS.protocol },
  ];
  const TOTAL = GROUPS.reduce((n, g) => n + g.checks.length, 0);

  // Rows keep one order for the whole phase: fastest cached lookup first, from the phase that already ran.
  const cached = $derived(new Map(rows.flatMap((r) => (r.resolver ? [[r.target.resolver_id, r.dist.cached?.p50 ?? null]] : []))));
  const lanes = $derived.by(() => {
    const at = (r: Resolver) => cached.get(r.id) ?? Infinity;
    return [...run.resolvers].sort((a, b) => (at(a) === at(b) ? 0 : at(a) - at(b)));
  });
  // The engine works through the run's own order, so the next few without a report are the ones in flight.
  const flying = $derived(new Set(run.resolvers.filter((r) => !run.probes[r.id]).slice(0, AT_ONCE).map((r) => r.id)));
  const phase = (r: Resolver) => (run.probes[r.id] ? 'done' : flying.has(r.id) ? 'run' : 'queued');

  // Every probed resolver's answers, one per check in board order. Unprobed ones have none yet, rather than the
  // operator's published claims the finished views fall back to.
  const answers = $derived.by(() => {
    const out: Record<string, { status: CheckStatus; tip: string }[]> = {};
    for (const r of run.resolvers) {
      if (!run.probes[r.id]) continue;
      out[r.id] = GROUPS.flatMap((g) =>
        g.checks.map((key) => {
          const c = capCell(run, r, key);
          return { status: c.status, tip: `${CAP[key].label}: ${c.text}` };
        }),
      );
    }
    return out;
  });
  const probed = $derived(Object.keys(answers).length);
  // How many probed resolvers pass each check, for the foot of its column.
  const tally = $derived(
    Array.from({ length: TOTAL }, (_, i) => Object.values(answers).filter((a) => a[i].status === 'pass').length),
  );
  const score = (id: string) => answers[id]?.filter((a) => a.status === 'pass').length ?? 0;
  // Each group's first column, so its tiles and tallies can be found in the flat lists.
  const start = GROUPS.map((_, g) => GROUPS.slice(0, g).reduce((n, x) => n + x.checks.length, 0));

  const stages = $derived.by((): Stage[] => {
    const total = run.total || run.resolvers.length;
    return [
      {
        label: 'Probe each resolver',
        value: `${run.done} of ${total}${flying.size ? `, ${flying.size} in flight` : ''}`,
        share: total ? run.done / total : 1,
        state: run.done >= total ? 'done' : 'now',
        tone: 'var(--s-tld)',
      },
    ];
  });
</script>

<section class="probes" aria-labelledby="probes-h" aria-busy="true">
  <header>
    <h2 id="probes-h">Probing capabilities</h2>
    <p class="lede">
      Each resolver is asked about privacy, blocking and protocol support, {AT_ONCE} at a time, fastest cached lookup first.
      A filled tile is a check it passes.
    </p>
  </header>

  <PhaseStages {stages} label="Probe progress" />

  <div class="board" role="list">
    <div class="lane head" aria-hidden="true">
      <span></span>
      <span class="tiles">
        {#each GROUPS as g}
          <span class="group" style:--tone={g.tone} style:--n={g.checks.length}>
            <span class="gt">{g.title}</span>
            {#each g.checks as key}<span class="code">{CODE[key]}</span>{/each}
          </span>
        {/each}
      </span>
      <span class="t">Passed</span>
    </div>

    {#each lanes as r, n (r.id)}
      {@const st = phase(r)}
      {@const got = answers[r.id]}
      <div
        class="lane cascade {st}"
        role="listitem"
        aria-label="{r.provider} {r.variant}: {got ? `${score(r.id)} of ${TOTAL} checks passed` : st === 'run' ? 'probing' : 'queued'}"
        style:--i={n}
      >
        <span class="who">
          <span class="name"><b>{r.provider}</b> <span class="var">{r.variant}</span></span>
          {#if cached.get(r.id) != null}<span class="lat num">{ms(cached.get(r.id))} cached</span>{/if}
        </span>
        <span class="tiles" aria-hidden="true">
          {#each GROUPS as g, gi}
            {@const mine = got?.slice(start[gi], start[gi] + g.checks.length)}
            <span class="group" style:--tone={g.tone} style:--n={g.checks.length}>
              {#if g.none && mine?.every((a) => a.status === 'info')}
                <span class="tile na" style:--k={start[gi]} title={mine.map((a) => a.tip).join('\n')}>{g.none}</span>
              {:else}
                {#each g.checks as _, ci}
                  {@const k = start[gi] + ci}
                  {@const a = mine?.[ci]}
                  <span class="tile {a?.status ?? ''}" style:--k={k} title={a?.tip}>
                    {#if a}<Icon name={GLYPH[a.status]} size={12} />{/if}
                  </span>
                {/each}
              {/if}
            </span>
          {/each}
        </span>
        <span class="t num" aria-hidden="true">
          {#if got}<b>{score(r.id)}</b><span class="of">/{TOTAL}</span>{:else}{st === 'run' ? 'probing' : 'queued'}{/if}
        </span>
      </div>
    {/each}

    <div class="lane foot" aria-label="Resolvers passing each check: {tally.join(', ')} of {probed}">
      <span class="who">Passing</span>
      <span class="tiles" aria-hidden="true">
        {#each GROUPS as g, gi}
          <span class="group" style:--tone={g.tone} style:--n={g.checks.length}>
            {#each g.checks as _, ci}
              {@const c = tally[start[gi] + ci]}
              <span class="count">
                <span class="col" style:--share={probed ? c / probed : 0}></span>
                <span class="num">{c}</span>
              </span>
            {/each}
          </span>
        {/each}
      </span>
      <span class="t num">of {probed}</span>
    </div>
  </div>
</section>

<style>
  .probes {
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
  .lat {
    color: var(--ink-3);
  }
  .queued .name {
    color: var(--ink-3);
  }

  /* Three groups side by side, each as wide as its share of the checks, so the columns line up down the board. */
  .tiles {
    grid-area: tiles;
    display: flex;
    gap: 12px;
    perspective: 600px;
  }
  .group {
    flex: var(--n) 1 0;
    display: grid;
    grid-template-columns: repeat(var(--n), minmax(0, 1fr));
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
  /* Column names read upward, so thirteen of them fit over tiles a finger wide. */
  .code {
    justify-self: center;
    writing-mode: vertical-rl;
    transform: rotate(180deg);
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
  .queued .tile {
    opacity: 0.45;
  }
  /* In flight: a light runs along the row while the probe asks its questions. */
  .run .tile {
    animation: ask 1.1s ease-in-out calc(var(--k) * 60ms) infinite;
  }
  /* Answered: each tile turns over in turn, left to right. */
  .done .tile {
    animation: turn 420ms var(--ease-out) calc(var(--k) * 32ms) backwards;
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

  .t {
    grid-area: t;
    text-align: right;
    color: var(--ink-2);
  }
  .t b {
    color: var(--ink);
    font-size: var(--t-md);
  }
  .of {
    color: var(--ink-3);
  }
  .queued .t {
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
  /* A slim gauge per column: the track is every probed resolver, the fill those that pass. */
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
