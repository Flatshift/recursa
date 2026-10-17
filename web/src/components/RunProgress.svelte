<script lang="ts">
  import type { RunState } from '../lib/types';

  let { run }: { run: RunState } = $props();

  const NAMES: Record<string, string> = {
    setup: 'Setup',
    warmup: 'Warm-up',
    cached: 'Cached',
    uncached: 'Uncached',
    tld: 'TLD',
    path: 'Path check',
    probes: 'Probes',
    cdn: 'CDN mapping',
    intel: 'Network intel',
  };
  const phases = $derived([
    'setup',
    'warmup',
    ...run.config.tests,
    'path',
    ...(run.config.probes ? ['probes'] : []),
    ...(run.config.cdn ? ['cdn'] : []),
    'intel',
  ]);
  const idx = $derived(run.phase === 'done' ? phases.length : phases.indexOf(run.phase));
  const running = $derived(run.status === 'running');
  const frac = $derived(run.total > 0 ? Math.min(1, run.done / run.total) : 0);
</script>

{#if running}
  <div class="prog chrome">
    <ol class="rail" aria-label="Run progress">
      {#each phases as p, i}
        <li class:done={i < idx} class:now={i === idx} aria-current={i === idx ? 'step' : undefined}>
          <span class="lbl">{NAMES[p] ?? p}</span>
          <span class="track"><span class="fill" style:transform="scaleX({i < idx ? 1 : i === idx ? frac : 0})"></span></span>
        </li>
      {/each}
    </ol>
    <p class="visually-hidden" aria-live="polite">{NAMES[run.phase] ?? run.phase} phase</p>
  </div>
{/if}

<style>
  .prog {
    padding: 10px 28px 12px;
    border-bottom: 1px solid var(--rule);
    background: var(--bg);
  }
  .rail {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(0, 1fr);
    gap: 6px;
  }
  .rail li {
    display: grid;
    gap: 5px;
    min-width: 0;
  }
  .lbl {
    font-size: var(--t-xs);
    color: var(--ink-3);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rail li.now .lbl {
    color: var(--ink);
    font-weight: 600;
  }
  .rail li.done .lbl {
    color: var(--ink-2);
  }
  .track {
    height: 3px;
    border-radius: 2px;
    background: var(--rule);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--accent);
    transform-origin: left;
    transition: transform 300ms var(--ease-out);
  }
  @media (max-width: 640px) {
    .rail {
      grid-auto-flow: row;
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
  }
</style>
