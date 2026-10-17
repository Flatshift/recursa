<script lang="ts">
  import type { Verdict } from '../lib/verdict';
  import { CDN_ROUND_TRIPS, HIT_SHARE, JITTER_WEIGHT, MIX_LABEL, RELIABLE, TAIL_WEIGHT, TIMEOUT_COST_MS, WEIGHTS } from '../lib/verdict';
  import { pct } from '../lib/format';
  import { app } from '../lib/store.svelte';
  import Provenance from './Provenance.svelte';
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
    onopen,
    children,
    status,
  }: {
    v: Verdict | null;
    /** While the run is measuring: what to say in place of the verdict. The picks hold ghosts. */
    live?: { sentence: string; detail: string } | null;
    cdn?: boolean;
    caution?: string | null;
    onopen: (id: string) => void;
    children?: Snippet;
    /** While the run keeps working after the latency rounds: the line naming the check under way. */
    status?: Snippet;
  } = $props();

  const sentence = $derived(v?.sentence ?? live?.sentence ?? '');
  const detail = $derived(v?.detail ?? live?.detail ?? '');
  // Phone: a long explanation shows three lines until asked for the rest, so the ranking reaches the first screen.
  let more = $state(false);
  const clamp = $derived(mobile && !more && detail.length > 150);
  const ghostGoals = $derived(['Fastest answers', ...(cdn ? ['Nearest CDN edges'] : []), 'Best protection', 'Most private', 'Steadiest']);
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
      {#if status}{@render status()}{/if}
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
            you get, uncached lookups are the delays you notice, and TLD lookups show how fast a resolver reaches a top-level
            domain's own servers when it has to.
          </p>
          <p>
            Each test counts its median pulled {pct(TAIL_WEIGHT, 0)} of the way toward its 95th percentile, plus {pct(JITTER_WEIGHT, 0)}
            of its standard deviation, so consistency counts: a steady resolver beats an erratic one with the same median, and
            one lucky fast answer counts for nothing.
          </p>
          <p>
            Timeouts cost too: each adds {TIMEOUT_COST_MS / 1000} s, about what your computer waits before it asks again, spread over
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
        {#each v.picks as p}
          <div class="pick">
            <dt>{p.goal}</dt>
            <dd>
              <button class="name" onclick={() => onopen(p.resolverId)}>{p.name}</button>
              <span class="ev"><Provenance kind={p.verified ? 'verified' : 'published'} /> {p.evidence}</span>
            </dd>
          </div>
        {/each}
      </dl>
    {:else if !v}
      <dl class="picks" aria-hidden="true">
        {#each ghostGoals as g, i}
          <div class="pick">
            <dt>{g}</dt>
            <dd>
              <span class="ghost" style:width="{110 + ((i * 41) % 50)}px" style:height="12px" style:margin="3px 0 4px"></span>
              <span class="ghost" style:width="{70 + ((i * 29) % 25)}%" style:height="8px"></span>
            </dd>
          </div>
        {/each}
      </dl>
    {/if}

    {#if children}<div class="extra">{@render children()}</div>{/if}
  </div>
</section>

<style>
  .verdict {
    /* Sized by the card, not the window: an open inspector narrows it too. */
    /* No card: the verdict is set on the page plane, and the ranking below is the one raised sheet. */
    container: verdict / inline-size;
    padding: 10px 4px 0;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1.25fr) minmax(0, 1fr);
    gap: 24px 40px;
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
  .picks {
    margin: 0;
    display: grid;
    align-content: start;
  }
  .pick {
    display: grid;
    grid-template-columns: 132px minmax(0, 1fr);
    gap: 12px;
    padding: 9px 0;
    border-top: 1px solid var(--rule);
  }
  .pick:first-child {
    border-top: 0;
    padding-top: 2px;
  }
  dt {
    font-size: var(--t-xs);
    color: var(--ink-3);
    padding-top: 2px;
  }
  dd {
    margin: 0;
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .name {
    justify-self: start;
    padding: 0;
    border: 0;
    background: none;
    font-weight: 620;
    font-size: var(--t-sm);
    cursor: default;
    text-align: left;
  }
  .name:hover {
    text-decoration: underline;
    text-decoration-color: var(--rule-2);
    text-underline-offset: 3px;
  }
  .ev {
    font-size: var(--t-xs);
    color: var(--ink-2);
    display: flex;
    gap: 6px;
    align-items: baseline;
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
      grid-auto-flow: column;
      grid-auto-columns: minmax(0, 1fr);
      padding-top: 18px;
      border-top: 1px solid var(--rule);
    }
    .pick,
    .pick:first-child {
      grid-template-columns: minmax(0, 1fr);
      align-content: start;
      gap: 5px;
      padding: 0 18px;
      border-top: 0;
      border-left: 1px solid var(--rule);
    }
    .pick:first-child {
      padding-left: 0;
      border-left: 0;
    }
    .pick:last-child {
      padding-right: 0;
    }
    dt {
      padding-top: 0;
    }
  }
  @container verdict (max-width: 760px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
      gap: 20px;
    }
  }
  @container verdict (max-width: 440px) {
    .pick {
      grid-template-columns: 1fr;
      gap: 2px;
    }
  }
  @media (max-width: 900px) {
    .verdict {
      padding: 4px 0 0;
    }
  }
  /* Phone: the sentence, then the ranking, then the picks as one row per goal with their evidence in full. The
     lead's parts join the grid so the ranking can sit between the sentence and the picks. */
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
    margin-top: -6px;
  }
  :global(html.mobile) .pick,
  :global(html.mobile) .pick:first-child {
    grid-template-columns: 104px minmax(0, 1fr);
    gap: 10px;
    padding: 9px 0;
    border-top: 1px solid var(--rule);
  }
  :global(html.mobile) dt {
    font-size: var(--t-sm);
    padding-top: 1px;
  }
  :global(html.mobile) .name {
    font-size: var(--t-base);
  }
  :global(html.mobile) .ev {
    font-size: var(--t-sm);
    display: block;
  }
</style>
