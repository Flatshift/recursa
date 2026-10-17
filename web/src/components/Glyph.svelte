<script lang="ts">
  // Status glyph: shape + color, always paired with text by the caller.
  import Icon from './Icon.svelte';
  import type { CheckStatus } from '../lib/types';
  let { status, size = 14 }: { status: CheckStatus | 'warn'; size?: number } = $props();
  const name = $derived(
    status === 'pass' ? 'check' : status === 'fail' ? 'cross' : status === 'partial' ? 'half' : status === 'warn' ? 'warn' : status === 'info' ? 'info' : 'dash',
  );
</script>

<span class="glyph {status}" style:--s="{size}px"><Icon name={name as any} size={size} /></span>

<style>
  .glyph {
    display: inline-grid;
    place-items: center;
    width: var(--s);
    height: var(--s);
    flex: none;
  }
  .pass {
    color: var(--good);
  }
  .fail {
    color: var(--critical);
  }
  .partial,
  .warn {
    color: var(--warning);
  }
  .info {
    color: var(--ink-3);
  }
  .unknown {
    color: var(--ink-3);
  }
</style>
