<script lang="ts" module>
  import type { CheckStatus } from '../lib/types';
  /** The icon each status draws, for callers that want it without the glyph's colour. */
  export const GLYPH = { pass: 'check', fail: 'cross', partial: 'half', warn: 'warn', info: 'info', unknown: 'dash' } as const satisfies Record<CheckStatus | 'warn', string>;
</script>

<script lang="ts">
  // Status glyph: shape + color, always paired with text by the caller.
  import Icon from './Icon.svelte';
  let { status, size = 14 }: { status: CheckStatus | 'warn'; size?: number } = $props();
  const name = $derived(GLYPH[status]);
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
  .info,
  .unknown {
    color: var(--ink-3);
  }
</style>
