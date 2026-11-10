<script lang="ts">
  import type { Transport } from '../lib/types';
  import { TRANSPORT_LABEL } from '../lib/format';

  /** Which transport a row measured, as a filled badge. The label carries the meaning; the one neutral fill only sets it off from the row. */
  let { t, v6 = false }: { t: Transport; v6?: boolean } = $props();
</script>

<span class="tchip mono"><span class="visually-hidden">, </span>{TRANSPORT_LABEL[t]}{v6 ? '·v6' : ''}</span>

<style>
  .tchip {
    display: inline-block;
    margin-left: 6px;
    padding: 0 6px;
    border-radius: var(--r-sm);
    background: var(--tx);
    font-size: var(--t-2xs);
    line-height: 16px;
    font-weight: 600;
    color: var(--ink);
    vertical-align: 1px;
    white-space: nowrap;
  }
</style>
