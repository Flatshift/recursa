<script lang="ts">
  import type { Verdict } from '../lib/verdict';
  import { CDN_ROUND_TRIPS, HIT_SHARE, MIX_LABEL, RELIABLE, TIMEOUT_COST_MS, WEIGHTS } from '../lib/verdict';
  import { pct } from '../lib/format';
  import { app } from '../lib/store.svelte';
  import Provenance from './Provenance.svelte';
  import PickGraph from './PickGraph.svelte';
  import RunTimeline from './RunTimeline.svelte';
  import type { Sample } from '../lib/types';
  import { logDomain } from '../lib/scale';
  import Info from './Info.svelte';
  import Icon from './Icon.svelte';
  import Mark from './Mark.svelte';
  import { mobile } from '../lib/api';

  import type { Snippet } from 'svelte';

  let {
    v,
    live = null,
    cdn = false,
    caution = null,
    samples = [],
    onopen,
    children,
  }: {
    v: Verdict | null;
    /** While the run is measuring: what to say in place of the verdict. The picks hold ghosts. */
    live?: { sentence: string; detail: string } | null;
    cdn?: boolean;
    caution?: string | null;
    /** Every answer in the run, for the picks' dot graphs. */
    samples?: Sample[];
    onopen: (id: string) => void;
    children?: Snippet;
  } = $props();

  const sentence = $derived(v?.sentence ?? live?.sentence ?? '');
  const detail = $derived(v?.detail ?? live?.detail ?? '');
  // Phone: a long explanation shows three lines until asked for the rest, so the ranking reaches the first screen.
  let more = $state(false);
  const clamp = $derived(mobile && !more && detail.length > 150);
  // One log axis for every dot graph in the sheet, so the speed picks read against each other.
  const domain = $derived.by(() => {
    let lo = Infinity;
    let hi = 0;
    const want = (v?.picks ?? []).flatMap((p) => (p.graph?.kind === 'dots' ? [p.graph] : []));
    for (const s of samples) {
      if (s.o !== 'ok' || !want.some((g) => g.target === s.t && g.tests.includes(s.k))) continue;
      lo = Math.min(lo, s.us / 1000);
      hi = Math.max(hi, s.us / 1000);
    }
    return logDomain(lo, hi);
  });
  // The parts list doubles as the Best from here bar's legend.
  const swatch = (p: NonNullable<Verdict['picks']>[number], k: string) => (p.graph?.kind === 'stack' ? p.graph.terms.find((t) => t.label === k)?.c : undefined);
  // The timeline follows the fastest pick, the one whose dots the sheet draws first.
  const lead = $derived(v?.picks.find((p) => p.graph?.kind === 'dots'));
  // One tooltip per row, lifted above it: the pick's evidence, or the part of its graph under the pointer.
  type Tip = { x: number; y: number; h: string; v?: string; s?: string; body?: string };
  let tip = $state<Tip | null>(null);
  function hover(e: PointerEvent, p: NonNullable<Verdict['picks']>[number]) {
    if (e.pointerType !== 'mouse') return;
    const top = (e.currentTarget as HTMLElement).getBoundingClientRect().top;
    const x = Math.min(Math.max(e.clientX, 170), innerWidth - 170);
    const d = ((e.target as Element).closest('[data-h]') as HTMLElement | SVGElement | null)?.dataset;
    tip = d ? { x, y: top, h: d.h!, v: d.v, s: d.s } : { x, y: top, h: p.goal, body: p.evidence };
  }
  // The privacy checks get more height when wide, for their names over the squares.
  const tall = (p: NonNullable<Verdict['picks']>[number]) => p.graph?.kind === 'checks';
  // The last speed row labels the log axis the speed rows share.
  const axisAt = $derived((v?.picks ?? []).map((p) => p.graph?.kind).lastIndexOf('dots'));
  const ghostGoals = $derived(['Fastest answers', 'Fastest cache', 'Fastest fresh lookups', ...(cdn ? ['Nearest CDN edges'] : []), 'Best protection', 'Most private', 'Steadiest']);
</script>

<section class="verdict" aria-labelledby="verdict-h" aria-busy={!v}>
  <div class="grid">
    <div class="lead">
      <!-- The one place the brand shows inside the Mac app, so a screenshot of the results says where they came from. -->
      <p class="eyebrow"><Mark size={15} />Recursa</p>
      <h2 id="verdict-h">{sentence}</h2>
      {#if detail}
        <p class="detail" class:clamp>{detail}</p>
        {#if clamp}<button class="more" onclick={() => (more = true)}>More</button>{/if}
      {/if}
      {#if caution && mobile}
        <details class="caution-d">
          <summary><Icon name="info" size={14} />No reliability record</summary>
          <p>{caution}</p>
        </details>
      {:else if caution}
        <p class="caution" role="note"><Icon name="info" size={14} /><span><b>No reliability record.</b> {caution}</span></p>
      {/if}
      <div class="rule-line">
        How the picks work
        <Info label="How the picks work">
          <p>
            Each goal has its own pick, ranked on that goal alone; there is no single overall winner on every goal. Only resolvers that
            answered at least {pct(RELIABLE, 0)} of queries are picked.
          </p>
          <p>
            Fastest answers ranks by a speed score, in ms: cached
            {pct(WEIGHTS.cached, 0)}, uncached {pct(WEIGHTS.uncached, 0)}, TLD {pct(WEIGHTS.tld, 0)}. Cached answers are most of what
            you get, uncached lookups are the delays you notice, and TLD lookups are a small share of real traffic.
          </p>
          <p>
            Fastest cache and Fastest fresh lookups are the two halves of that score on their own: the cached test alone, and the
            uncached and TLD tests at 0.8 and 0.2. The speed score is their average, and a resolver that leads one half often trails
            the other.
          </p>
          <p>
            Each test counts its mean after the slowest tenth of answers is pulled down to the 90th percentile, so one straggler
            cannot move it. The 95th percentile and the jitter are shown beside the score rather than folded in. A test in
            which a resolver began refusing or stopped answering is left out of its score and marked limited.
          </p>
          <p>
            Timeouts and failed answers cost too: each adds {TIMEOUT_COST_MS / 1000} s, about what your computer waits before it asks again, spread over
            every query sent. A resolver that drops 1% of queries carries {TIMEOUT_COST_MS / 100} ms more.
          </p>
          <p>
            When a gap is small enough that this run’s own variation could produce it (the answers redrawn 200 times), it is
            called a tie and neither resolver is named ahead.
          </p>
          <p>
            Best from here is the one pick that adds things up: the speed figures at the traffic mix set in Settings ({pct(HIT_SHARE[app.mix], 0)}
            cache hits for a {MIX_LABEL[app.mix].toLowerCase()} network), the cached figure corrected for answers that were really fresh lookups, and the CDN edge
            penalty on the lookups that open a new connection. It is the only pick that may name a tie as its answer.
          </p>
          {#if cdn}
            <p>
              Nearest CDN edges ranks by the CDN penalty alone. A lookup is paid once, but every new connection then waits at least
              {CDN_ROUND_TRIPS} round trips on the CDN edge the resolver chose: the TCP handshake, the TLS handshake, then the first
              request. So the quickest answer isn’t always the quickest page.
            </p>
          {/if}
          <p>
            Best protection, Most private and Steadiest first filter on what the resolver did in this run, then take the fastest
            of those that qualify. Your own resolver keeps Best protection and Most private when it qualifies, since you
            needn't change anything, and Most private puts resolvers that block trackers first. An ISP's resolver can't be
            Most private: it sees who is asking.
          </p>
          <p>Plain-DNS targets that a device on your network answered are listed but never ranked.</p>
        </Info>
      </div>
    </div>

    {#if v?.picks.length}
      <dl class="picks">
        {#each v.picks as p, i}
          <!-- svelte-ignore a11y_no_static_element_interactions (pointer tooltip only; the evidence is also in the row's text) -->
          <div class="pick" class:sum={p.parts} class:tall={tall(p)} onpointermove={(e) => hover(e, p)} onpointerleave={() => (tip = null)}>
            <dt>{p.goal}</dt>
            <dd>
              <button class="name" onclick={() => onopen(p.resolverId)}>{p.name}</button>
              <span class="fig"><Provenance kind={p.verified ? 'verified' : 'published'} titled={false} />{p.figure}</span>
              <span class="graph">{#if p.graph}<PickGraph g={p.graph} {samples} {domain} axis={i === axisAt} />{/if}</span>
              <span class="note">{p.note}</span>
              <span class="visually-hidden">{p.evidence}</span>
              {#if p.parts?.length}
                <ul class="parts">
                  {#each p.parts as [k, val]}
                    {@const c = swatch(p, k)}
                    <li>{#if c}<i class="sw {c}"></i>{/if}<span class="k">{k}</span> {val}</li>
                  {/each}
                </ul>
              {/if}
            </dd>
          </div>
        {/each}
      </dl>
      {#if lead?.graph?.kind === 'dots'}
        <div class="timeline"><RunTimeline {samples} target={lead.graph.target} name={lead.name} /></div>
      {/if}
    {:else if !v}
      <dl class="picks" aria-hidden="true">
        {#each ghostGoals as g, i}
          <div class="pick">
            <dt>{g}</dt>
            <dd>
              <span class="ghost" style:width="{90 + ((i * 41) % 50)}px" style:height="12px"></span>
              <span class="ghost g-fig" style:width="{48 + ((i * 17) % 20)}px" style:height="12px"></span>
              <span class="graph"></span>
              <span class="ghost g-note" style:width="{60 + ((i * 29) % 30)}%" style:height="8px"></span>
            </dd>
          </div>
        {/each}
      </dl>
    {/if}

    {#if children}<div class="extra">{@render children()}</div>{/if}
  </div>
</section>

{#if tip}
  <div class="chart-tip pick-tip" style:left="{tip.x}px" style:top="{tip.y}px" role="tooltip">
    <div class="tt-h">{tip.h}</div>
    {#if tip.v}<div class="tt-v num">{tip.v}</div>{/if}
    {#if tip.s}<div class="tt-s">{tip.s}</div>{/if}
    {#if tip.body}<div class="tt-b">{tip.body}</div>{/if}
  </div>
{/if}

<style>
  .verdict {
    /* Sized by the card, not the window: an open inspector narrows it too. */
    /* No card: the verdict is set on the page plane, and the ranking below is the one raised sheet. */
    container: verdict / inline-size;
    padding: 10px 4px 0;
  }
  /* The sheet sits under the lead at every width: beside it, the names and notes wrap in a narrow column while
     the space under the headline stays empty. */
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 20px;
  }
  .eyebrow {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 8px;
    font-size: var(--t-sm);
    font-weight: 620;
    letter-spacing: -0.01em;
    color: var(--ink-2);
  }
  h2 {
    font-size: var(--t-2xl);
    font-weight: 600;
    letter-spacing: -0.02em;
    line-height: 1.14;
    max-width: 24ch;
    text-wrap: balance;
  }
  .detail {
    margin-top: 10px;
    color: var(--ink-2);
    max-width: 62ch;
  }
  .caution {
    display: flex;
    gap: 8px;
    margin-top: 10px;
    font-size: var(--t-sm);
    color: var(--ink-2);
    max-width: 62ch;
  }
  .caution :global(svg) {
    flex: none;
    margin-top: 3px;
  }
  .caution b {
    color: var(--ink);
    font-weight: 620;
  }
  /* The ranking spans the whole card, so its strip plots get the width the picks column would otherwise leave empty. */
  .extra {
    grid-column: 1 / -1;
    min-width: 0;
    margin: 4px -4px 0;
    padding: 18px 22px 16px;
    background: var(--surface);
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
  }
  .rule-line {
    margin-top: 12px;
    display: flex;
    align-items: center;
    gap: 2px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  /* The pick sheet: one hairline row per goal in the ranking table's own vocabulary. The goal, who wins it, the
     deciding figure, a short note; the full sentence is the row's title. The rows share their columns through the
     subgrid, so the figures stand in one column down the sheet. In two columns the note drops under the name. */
  .picks {
    margin: 0 -4px;
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr) max-content;
    align-content: start;
  }
  .pick {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: subgrid;
    column-gap: 14px;
    align-items: baseline;
    min-width: 0;
    padding: 5px 4px;
    border-top: 1px solid var(--rule);
  }
  .pick:first-child {
    border-top: 0;
  }
  .pick:hover {
    background: var(--surface-2);
  }
  dd {
    display: contents;
  }
  dt {
    font-size: var(--t-xs);
    color: var(--ink-3);
    line-height: 1.3;
  }
  .name {
    justify-self: start;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    font-weight: 620;
    font-size: var(--t-sm);
    line-height: 1.3;
    cursor: default;
    text-align: left;
  }
  .name:hover {
    text-decoration: underline;
    text-decoration-color: var(--rule-2);
    text-underline-offset: 3px;
  }
  .fig,
  .g-fig {
    justify-self: end;
  }
  .fig {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    font-size: var(--t-sm);
    font-weight: 600;
    letter-spacing: -0.01em;
    line-height: 1.3;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .note,
  .g-note {
    grid-column: 2 / -1;
    font-size: var(--t-xs);
    color: var(--ink-2);
    line-height: 1.35;
  }
  /* Best from here adds the rows above it up, so it closes the sheet under a double rule as the total: its figure
     one step larger, the terms it sums on a line beneath. */
  .pick.sum {
    margin-top: 3px;
    padding-top: 7px;
    border-top: 3px double var(--rule-2);
  }
  .sum dt {
    color: var(--ink-2);
  }
  .sum .fig {
    font-size: var(--t-md);
    letter-spacing: -0.015em;
  }
  .parts {
    grid-column: 1 / -1;
    margin: 1px 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-wrap: wrap;
    gap: 2px 16px;
    font-size: var(--t-xs);
    color: var(--ink);
    font-variant-numeric: tabular-nums;
    line-height: 1.35;
  }
  .k {
    color: var(--ink-3);
  }
  /* Each row's graph: what that goal was decided on. In two columns it takes its own line under the note; with the
     sheet's full width it is a column between the figure and the note. */
  .graph {
    grid-column: 2 / -1;
    order: 1;
    min-width: 0;
    align-self: center;
  }
  .graph:empty {
    display: none;
  }
  .parts {
    order: 2;
  }
  .sw {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: 5px;
    border-radius: 1.5px;
    vertical-align: 0;
  }
  .sw.cached {
    background: var(--s-cached);
  }
  .sw.uncached {
    background: var(--s-uncached);
  }
  .sw.neutral {
    background: var(--s-neutral);
  }
  .sw.tld {
    background: var(--s-tld);
  }
  /* Wide: the headline and its detail side by side, the picks in one row of columns, so no
     column runs long beside an empty one. */
  @container verdict (min-width: 1060px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
      gap: 22px;
    }
    .lead {
      display: grid;
      grid-template-columns: minmax(0, 0.9fr) minmax(0, 1.1fr);
      column-gap: 48px;
      align-items: start;
    }
    .lead > :global(*) {
      grid-column: 2;
    }
    .lead > h2 {
      grid-column: 1;
      grid-row: 1 / span 3;
    }
    .detail {
      margin-top: 3px;
    }
    .picks {
      grid-template-columns: max-content fit-content(240px) max-content minmax(160px, 1fr) fit-content(40ch);
      margin-top: 2px;
      /* The graphs take the spare width, so they get more height too. */
      --pg-h: 24px;
    }
    .pick {
      padding-block: 7px;
    }
    .pick.tall {
      --pg-h: 34px;
    }
    .note,
    .g-note,
    .graph {
      grid-column: auto;
      order: 0;
    }
    .graph:empty {
      display: block;
    }
    .parts {
      grid-column: 2 / -1;
    }
  }
  /* Below wide: the note in its own column, the graph taking the spare width. */
  @container verdict (width < 1060px) {
    .picks {
      grid-template-columns: max-content fit-content(240px) max-content minmax(120px, 1fr) fit-content(36ch);
      --pg-h: 24px;
    }
    .note,
    .g-note,
    .graph {
      grid-column: auto;
      order: 0;
    }
    .graph:empty {
      display: block;
    }
    .parts {
      grid-column: 2 / -1;
    }
  }
  /* The run in time only where the window has room to spare: wide, and tall enough that it sits on the first screen
     under the sheet. Anywhere smaller it is left out, not squeezed. */
  .timeline {
    display: none;
    margin-top: 6px;
  }
  @media (min-height: 900px) {
    @container verdict (min-width: 1060px) {
      .timeline {
        display: block;
      }
    }
  }
  /* Narrow: the note takes what the name and figure leave, the graph a short column. */
  @container verdict (max-width: 760px) {
    .picks {
      grid-template-columns: max-content fit-content(200px) max-content minmax(80px, 150px) minmax(0, 1fr);
      --pg-h: 16px;
    }
  }
  /* Narrower still (five columns would leave the note a sliver): the goal above, the name and figure on one line, the
     note beneath. */
  @container verdict (max-width: 660px) {
    .picks {
      grid-template-columns: minmax(0, 1fr) max-content;
      --pg-h: 24px;
    }
    .pick {
      padding: 7px 4px;
    }
    dt,
    .note,
    .g-note,
    .graph,
    .parts {
      grid-column: 1 / -1;
    }
    .graph {
      order: 1;
    }
    .graph:empty {
      display: none;
    }
  }
  @media (max-width: 900px) {
    .verdict {
      padding: 4px 0 0;
    }
  }
  /* Phone: the sentence, then the ranking, then the sheet. The lead's parts join the grid so the ranking can sit
     between the sentence and the picks. */
  :global(html.mobile) .verdict {
    padding: 0;
  }
  :global(html.mobile) .grid {
    gap: 10px;
  }
  :global(html.mobile) .lead {
    display: contents;
  }
  :global(html.mobile) .eyebrow {
    display: none;
  }
  :global(html.mobile) h2 {
    font-size: var(--t-lg);
    max-width: none;
  }
  :global(html.mobile) .detail {
    margin-top: -4px;
    font-size: var(--t-sm);
  }
  .detail.clamp {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    overflow: hidden;
  }
  .more {
    justify-self: start;
    min-height: 32px;
    margin-top: -10px;
    padding: 0;
    border: 0;
    background: none;
    font-size: var(--t-sm);
    font-weight: 560;
    color: var(--ink);
  }
  .caution-d {
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .caution-d summary {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 32px;
    padding: 0 12px;
    border: 1px solid var(--rule);
    border-radius: 999px;
    color: var(--ink);
    font-weight: 560;
    list-style: none;
  }
  .caution-d summary::-webkit-details-marker {
    display: none;
  }
  .caution-d p {
    margin-top: 8px;
  }
  :global(html.mobile) .extra {
    order: 1;
    margin: 2px 0 0;
    padding: 12px 12px 10px;
  }
  :global(html.mobile) .rule-line {
    order: 2;
    margin-top: 6px;
    font-size: var(--t-sm);
  }
  :global(html.mobile) .picks {
    order: 3;
    grid-template-columns: minmax(0, 1fr) max-content;
    --pg-h: 24px;
    margin: 0;
  }
  :global(html.mobile) .pick {
    padding: 8px 0;
  }
  :global(html.mobile) dt,
  :global(html.mobile) .note,
  :global(html.mobile) .g-note,
  :global(html.mobile) .graph,
  :global(html.mobile) .parts {
    grid-column: 1 / -1;
  }
  :global(html.mobile) dt {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .name,
  :global(html.mobile) .fig {
    font-size: var(--t-base);
  }
  :global(html.mobile) .note,
  :global(html.mobile) .parts {
    font-size: var(--t-sm);
  }
  .pick-tip {
    white-space: normal;
    max-width: 40ch;
  }
  .tt-b {
    margin-top: 2px;
    line-height: 1.4;
  }
</style>
