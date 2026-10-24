<script lang="ts">
  import type { Verdict } from '../lib/verdict';
  import { CDN_ROUND_TRIPS, HIT_SHARE, MIX_LABEL, RELIABLE, TIMEOUT_COST_MS, WEIGHTS } from '../lib/verdict';
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
  }: {
    v: Verdict | null;
    /** While the run is measuring: what to say in place of the verdict. The picks hold ghosts. */
    live?: { sentence: string; detail: string } | null;
    cdn?: boolean;
    caution?: string | null;
    onopen: (id: string) => void;
    children?: Snippet;
  } = $props();

  const sentence = $derived(v?.sentence ?? live?.sentence ?? '');
  const detail = $derived(v?.detail ?? live?.detail ?? '');
  // Phone: a long explanation shows three lines until asked for the rest, so the ranking reaches the first screen.
  let more = $state(false);
  const clamp = $derived(mobile && !more && detail.length > 150);
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
        {#each v.picks as p}
          <div class="pick" title={p.evidence}>
            <dt>{p.goal}</dt>
            <dd>
              <button class="name" onclick={() => onopen(p.resolverId)}>{p.name}</button>
              <span class="fig" class:num={/^[\d±+]/.test(p.figure)}><Provenance kind={p.verified ? 'verified' : 'published'} />{p.figure}</span>
              {#if p.note}<span class="note">{p.note}</span>{/if}
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
              <span class="ghost" style:width="{90 + ((i * 41) % 50)}px" style:height="12px" style:margin="3px 0 5px"></span>
              <span class="ghost" style:width="{48 + ((i * 17) % 20)}px" style:height="13px" style:margin="0 0 5px"></span>
              <span class="ghost" style:width="{60 + ((i * 29) % 30)}%" style:height="8px"></span>
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
  /* One small card per goal: the goal, who wins it, the deciding figure, a short note. The full sentence is the
     card's title; the cards wrap to whatever the width allows. */
  .picks {
    margin: 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 8px;
    align-content: start;
  }
  .pick {
    display: grid;
    align-content: start;
    gap: 4px;
    min-width: 0;
    padding: 10px 12px 11px;
    background: var(--surface);
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
  }
  dt {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  dd {
    margin: 0;
    display: grid;
    gap: 3px;
    min-width: 0;
  }
  .name {
    justify-self: start;
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
  .fig {
    display: flex;
    align-items: baseline;
    gap: 6px;
    margin-top: 2px;
    font-size: var(--t-base);
    font-weight: 600;
    letter-spacing: -0.01em;
    line-height: 1.2;
  }
  .note {
    font-size: var(--t-xs);
    color: var(--ink-2);
    line-height: 1.35;
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
      grid-template-columns: repeat(4, minmax(0, 1fr));
      margin-top: 2px;
    }
  }
  @container verdict (max-width: 760px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
      gap: 20px;
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
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }
  :global(html.mobile) .pick {
    padding: 10px 12px;
  }
  :global(html.mobile) dt {
    font-size: var(--t-sm);
  }
  :global(html.mobile) .name {
    font-size: var(--t-base);
  }
  :global(html.mobile) .note {
    font-size: var(--t-sm);
  }
</style>
