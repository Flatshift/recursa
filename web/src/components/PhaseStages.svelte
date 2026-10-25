<script lang="ts" module>
  export interface Stage {
    label: string;
    value: string;
    /** Said after the value in the warning colour: what went wrong so far. */
    note?: string;
    /** 0 to 1. */
    share: number;
    /** A colour of the step's own: its marker, and its meter while running and once done. */
    tone?: string;
    state: 'wait' | 'now' | 'done';
  }
</script>

<script lang="ts">
  // The steps of one live phase, each with its own meter: what the engine is doing now, and what is left.
  let { stages, label }: { stages: Stage[]; label: string } = $props();
</script>

<ol class="stages" aria-label={label}>
  {#each stages as s}
    <li class={s.state} class:toned={s.tone} style:--tone={s.tone}>
      <span class="sl">{s.label}</span>
      <span class="sv num">{s.value}{#if s.note}, <span class="note">{s.note}</span>{/if}</span>
      <span class="meter" aria-hidden="true"><span style:scale="{s.share} 1"></span></span>
    </li>
  {/each}
</ol>

<style>
  .stages {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 10px 18px;
    margin: 0;
    padding: 0;
    list-style: none;
    max-width: 920px;
    /* Under max-width, auto-fit sizes for four columns when measured; without this a grid parent widens to fit them. */
    min-width: 0;
  }
  li {
    display: grid;
    gap: 3px;
    color: var(--ink-3);
    transition: color 300ms var(--ease-out);
  }
  .sl {
    font-size: var(--t-sm);
    font-weight: 600;
  }
  .sv {
    font-size: var(--t-xs);
    min-height: 1.4em;
  }
  .now {
    color: var(--ink);
  }
  .now .sv,
  .done {
    color: var(--ink-2);
  }
  .meter {
    display: block;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .meter > span {
    display: block;
    height: 100%;
    background: var(--ink-3);
    transform-origin: left;
    transition: scale 500ms var(--ease-out);
  }
  .now .meter > span {
    background: var(--ink);
  }
  .done .meter > span {
    background: var(--good);
  }
  .note {
    color: var(--warning);
  }

  /* A toned step: a marker by its name, hollow until the step starts; the meter fills in the step's colour, with a
     light sweep across it while it runs. */
  .toned .sl::before {
    content: '';
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: 7px;
    border-radius: 50%;
    vertical-align: 1px;
    box-shadow: inset 0 0 0 1.5px var(--tone);
    transition: background-color 300ms var(--ease-out);
  }
  .toned:not(.wait) .sl::before {
    background: var(--tone);
  }
  .toned .meter {
    background: color-mix(in srgb, var(--tone) 14%, var(--surface-3));
  }
  .toned .meter > span {
    position: relative;
    overflow: hidden;
    background: linear-gradient(90deg, color-mix(in srgb, var(--tone) 55%, var(--surface)), var(--tone));
  }
  .toned.done .meter > span {
    background: var(--tone);
  }
  .toned.now .meter > span::after {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(90deg, transparent, oklch(1 0 0 / 0.55), transparent) no-repeat;
    background-size: 40% 100%;
    animation: sweep 1.6s var(--ease-out) infinite;
  }
  @keyframes sweep {
    from {
      background-position: -60% 0;
    }
    to {
      background-position: 160% 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    li,
    .meter > span {
      transition: none;
    }
    .toned.now .meter > span::after {
      animation: none;
    }
  }
</style>
