<script lang="ts" module>
  import type { DeepDiveStep } from '../lib/types';

  // The dive's fixed sequence (crates/engine/src/deepdive.rs), drawn before it runs and filled in after.
  const GROUPS: { kind: DeepDiveStep; n: number; label: string; what: string }[] = [
    { kind: 'popular', n: 2, label: 'Popular name', what: 'Fetch, then cache' },
    { kind: 'random', n: 3, label: 'Random names', what: 'Never cached' },
    { kind: 'repeat', n: 12, label: 'Repeats', what: 'Round trips' },
    { kind: 'last', n: 1, label: 'Last', what: 'TTL' },
  ];
</script>

<script lang="ts">
  import type { DeepDive } from '../lib/types';
  import { ms } from '../lib/format';
  import { reduceMotion } from '../lib/motion';

  let { dive, busy }: { dive: DeepDive | null; busy: boolean } = $props();

  const STAGGER = reduceMotion ? 0 : 45;
  const STUB = 6;

  // Log scale from a little under the fastest answer, so a cache hit and a full lookup both stay readable.
  const scale = $derived.by(() => {
    const v = (dive?.timeline ?? []).flatMap((t) => (t.ms != null && t.ms > 0 ? [t.ms] : []));
    if (!v.length) return null;
    const lo = Math.min(...v) / 1.6, hi = Math.max(...v);
    return (x: number) => (hi <= lo ? 60 : 12 + 88 * (Math.log(x / lo) / Math.log(hi / lo)));
  });

  const groups = $derived.by(() => {
    let at = 0;
    return GROUPS.map((g) => {
      const slots = Array.from({ length: g.n }, (_, j) => {
        const i = at + j;
        const t = dive?.timeline[i];
        const tone = g.kind === 'random' ? 'cold' : g.kind === 'popular' && j === 0 ? 'first' : 'warm';
        return { i, tone, sent: !!t, ms: t?.ms ?? null };
      });
      at += g.n;
      const got = slots.flatMap((s) => (s.ms != null ? [s.ms] : []));
      const sorted = [...got].sort((a, b) => a - b);
      const sum =
        !dive || !got.length
          ? null
          : g.kind === 'popular' && got.length === 2
            ? `${ms(got[0])} → ${ms(got[1])}`
            : got.length > 1
              ? `${ms(sorted[Math.floor((sorted.length - 1) / 2)])} median`
              : ms(got[0]);
      return { ...g, slots, sum };
    });
  });

  function height(s: { sent: boolean; ms: number | null }) {
    if (!dive || !s.sent || s.ms == null || !scale) return `${STUB}px`;
    return `${scale(s.ms)}%`;
  }
  function tip(s: { i: number; sent: boolean; ms: number | null }, label: string) {
    const n = `Query ${s.i + 1} · ${label}`;
    if (!dive) return n;
    return !s.sent ? `${n} · not sent` : s.ms == null ? `${n} · no answer` : `${n} · ${ms(s.ms)}`;
  }
</script>

<figure class="trace" class:measured={!!dive} class:busy aria-label={dive ? 'Each query in the order sent, by round trip' : 'The queries a deep dive sends'}>
  <div class="row">
    {#each groups as g (g.kind)}
      {#if g.kind === 'last'}
        <div class="wait" aria-hidden="true"><span>2 s</span></div>
      {/if}
      <div class="group">
        <div class="plot">
          {#each g.slots as s (s.i)}
            <span class="slot" title={tip(s, g.label)}>
              <span
                class="bar {s.tone}"
                class:miss={dive && s.sent && s.ms == null}
                class:unsent={dive && !s.sent}
                style:height={height(s)}
                style:--d="{s.i * STAGGER}ms"
              ></span>
            </span>
          {/each}
        </div>
        <div class="label">
          <span class="name">{g.label}</span>
          <span class="what num">{g.sum ?? (dive ? '—' : g.what)}</span>
        </div>
      </div>
    {/each}
  </div>
  <figcaption>
    {#if dive}Each bar is one query, in the order sent. Height is the round trip on a log scale.{:else}The 18 queries, in order. Each
      bar fills in with its round trip.{/if}
  </figcaption>
</figure>

<style>
  .trace {
    margin: 0;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 14px 18px;
  }
  .plot {
    display: flex;
    align-items: flex-end;
    gap: 4px;
    height: 76px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--rule);
  }
  .slot {
    display: flex;
    align-items: flex-end;
    width: 14px;
    height: 100%;
  }
  .bar {
    width: 100%;
    border-radius: 3px;
    background: var(--tone);
    opacity: 0.55;
    transition:
      height 260ms var(--ease-out),
      opacity 260ms var(--ease-out);
  }
  .bar.first {
    --tone: var(--s-neutral);
  }
  .bar.cold {
    --tone: var(--s-uncached);
  }
  .bar.warm {
    --tone: var(--s-cached);
  }
  /* Measured: each bar grows to its round trip in the order it was sent. */
  .measured .bar {
    opacity: 1;
    transition:
      height 620ms var(--ease-out) var(--d),
      opacity 200ms var(--ease-out) var(--d);
  }
  .measured .bar.miss {
    background: none;
    border: 1px dashed var(--ink-3);
  }
  .measured .bar.unsent {
    opacity: 0.2;
  }
  .busy .bar {
    opacity: 0.3;
  }
  .wait {
    align-self: stretch;
    display: flex;
    align-items: flex-end;
    padding-bottom: 33px;
  }
  .wait span {
    padding: 0 6px;
    border-bottom: 1px dashed var(--ink-3);
    color: var(--ink-3);
    font-size: var(--t-xs);
    line-height: 1.6;
  }
  .label {
    display: grid;
    margin-top: 6px;
    font-size: var(--t-xs);
    line-height: 1.35;
  }
  .name {
    color: var(--ink-2);
    font-weight: 560;
  }
  .what {
    color: var(--ink-3);
    white-space: nowrap;
  }
  figcaption {
    margin-top: 12px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  @media (prefers-reduced-motion: reduce) {
    .bar,
    .measured .bar {
      transition: none;
    }
  }
</style>
