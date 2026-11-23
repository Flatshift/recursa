<script lang="ts">
  // A resolver's colour and shape together: the same on the table, the legend and the chart, so colour is never the only cue.
  import { colourOf, markOf, markPath } from '../lib/extended';

  let { t, size = 10 }: { t: number; size?: number } = $props();
  const r = $derived(size / 2 - 1);
</script>

<svg class="mark" width={size} height={size} viewBox="0 0 {size} {size}" aria-hidden="true"><path d={markPath(markOf(t), size / 2, size / 2, r)} fill={colourOf(t)} /></svg>

<style>
  .mark {
    flex: none;
    vertical-align: -1px;
  }
</style>
