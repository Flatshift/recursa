<script lang="ts">
  import { CDN_LOGOS } from '../lib/cdnLogos';

  /** A CDN's mark in the table's ink; a lettered badge where there is no mark to show. Decorative: the name sits beside it. */
  let { id, name }: { id: string; name: string } = $props();
  const logo = $derived(CDN_LOGOS[id]);
</script>

{#if logo}
  <svg class="logo" viewBox="0 0 {logo.vb ?? 24} {logo.vb ?? 24}" aria-hidden="true"
    ><path fill="currentColor" fill-rule={logo.evenodd ? 'evenodd' : 'nonzero'} d={logo.d} /></svg
  >
{:else}
  <span class="logo badge" aria-hidden="true">{name.trim().charAt(0).toUpperCase()}</span>
{/if}

<style>
  .logo {
    display: block;
    width: 16px;
    height: 16px;
    color: var(--ink-2);
  }
  .badge {
    display: grid;
    place-items: center;
    box-sizing: border-box;
    border: 1.5px solid currentColor;
    border-radius: 4px;
    font-size: 0.5625rem;
    font-weight: 700;
    line-height: 1;
    color: var(--ink-3);
  }
</style>
