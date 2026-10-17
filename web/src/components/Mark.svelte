<script lang="ts">
  // The Cantor dial at small sizes: outer ring and the inner arcs, with the answer dot in the brand colour.
  let { size = 20 }: { size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="170 170 684 684" fill="none" stroke="currentColor" stroke-width="100" stroke-linecap="round" aria-hidden="true">
  <g opacity="0.45">
    <circle cx="512" cy="512" r="272" />
    <path d="M376.7,526.1A136,136 0 0 1 526.1,376.7" />
  </g>
  <path d="M647.3,497.9A136,136 0 0 1 497.9,647.3" />
  <circle class="answer" cx="608.2" cy="608.2" r="70" fill="currentColor" stroke="none" />
</svg>

<style>
  svg {
    flex: none;
  }
  .answer {
    fill: var(--brand-answer);
  }
</style>
