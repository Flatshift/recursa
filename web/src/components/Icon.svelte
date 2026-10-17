<script lang="ts">
  // A small, consistent stroke icon set (16px grid, 1.6 stroke).
  type Name =
    | 'check'
    | 'cross'
    | 'dash'
    | 'warn'
    | 'info'
    | 'external'
    | 'play'
    | 'stop'
    | 'sun'
    | 'moon'
    | 'monitor'
    | 'download'
    | 'plus'
    | 'trash'
    | 'chevron-down'
    | 'chevron-right'
    | 'close'
    | 'refresh'
    | 'half'
    | 'pulse'
    | 'clock'
    | 'lock'
    | 'sidebar'
    | 'inspector'
    | 'list'
    | 'book'
    | 'gear'
    | 'bookmark'
    | 'search';
  let { name, size = 16 }: { name: Name; size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill="none"
  stroke="currentColor"
  stroke-width="1.6"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === 'check'}
    <path d="M3.5 8.5l3 3 6-7" />
  {:else if name === 'cross'}
    <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
  {:else if name === 'dash'}
    <path d="M4.5 8h7" />
  {:else if name === 'warn'}
    <path d="M8 2.5l6 10.5H2L8 2.5z" /><path d="M8 6.5v3" /><path d="M8 11.4v.1" />
  {:else if name === 'info'}
    <circle cx="8" cy="8" r="5.8" /><path d="M8 7.3v3.6" /><path d="M8 5.1v.1" />
  {:else if name === 'external'}
    <path d="M9.5 3H13v3.5M13 3L7.5 8.5M11 9.5V13H3V5h3.5" />
  {:else if name === 'play'}
    <path d="M5 3.5l7.5 4.5L5 12.5z" fill="currentColor" stroke="none" />
  {:else if name === 'stop'}
    <rect x="4" y="4" width="8" height="8" rx="1.2" fill="currentColor" stroke="none" />
  {:else if name === 'sun'}
    <circle cx="8" cy="8" r="2.8" /><path d="M8 1.5v1.6M8 12.9v1.6M1.5 8h1.6M12.9 8h1.6M3.4 3.4l1.1 1.1M11.5 11.5l1.1 1.1M3.4 12.6l1.1-1.1M11.5 4.5l1.1-1.1" />
  {:else if name === 'moon'}
    <path d="M13 9.6A5.5 5.5 0 016.4 3a5.5 5.5 0 106.6 6.6z" />
  {:else if name === 'monitor'}
    <rect x="2" y="3" width="12" height="8" rx="1.2" /><path d="M6 13.5h4M8 11v2.5" />
  {:else if name === 'download'}
    <path d="M8 2.5v8M4.5 7.5L8 11l3.5-3.5M3 13.5h10" />
  {:else if name === 'plus'}
    <path d="M8 3.5v9M3.5 8h9" />
  {:else if name === 'bookmark'}
    <path d="M4.5 2.5h7v11l-3.5-2.6-3.5 2.6z" />
  {:else if name === 'trash'}
    <path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5" />
  {:else if name === 'chevron-down'}
    <path d="M4 6l4 4 4-4" />
  {:else if name === 'chevron-right'}
    <path d="M6 4l4 4-4 4" />
  {:else if name === 'clock'}
    <circle cx="8" cy="8" r="5.6" />
    <path d="M8 4.6V8l2.4 1.5" />
  {:else if name === 'close'}
    <path d="M4 4l8 8M12 4l-8 8" />
  {:else if name === 'refresh'}
    <path d="M13 8a5 5 0 11-1.5-3.6M13 2.5v3h-3" />
  {:else if name === 'half'}
    <circle cx="8" cy="8" r="5.5" /><path d="M8 2.5a5.5 5.5 0 010 11z" fill="currentColor" stroke="none" />
  {:else if name === 'pulse'}
    <path d="M1.5 8.5h3l1.5-4 3 8 1.5-4h4" />
  {:else if name === 'sidebar'}
    <rect x="2" y="3" width="12" height="10" rx="1.6" /><path d="M6.2 3v10" />
  {:else if name === 'inspector'}
    <rect x="2" y="3" width="12" height="10" rx="1.6" /><path d="M9.8 3v10" />
  {:else if name === 'list'}
    <path d="M5.5 4.5H13M5.5 8H13M5.5 11.5H13" /><path d="M3 4.5v.1M3 8v.1M3 11.5v.1" />
  {:else if name === 'book'}
    <path d="M8 4.2C6.6 3.2 4.7 2.9 2.5 3.2v9c2.2-.3 4.1 0 5.5 1 1.4-1 3.3-1.3 5.5-1v-9c-2.2-.3-4.1 0-5.5 1z" /><path d="M8 4.2v9" />
  {:else if name === 'gear'}
    <circle cx="8" cy="8" r="2.2" /><path
      d="M8 1.8v1.5M8 12.7v1.5M14.2 8h-1.5M3.3 8H1.8M12.4 3.6l-1.1 1.1M4.7 11.3l-1.1 1.1M12.4 12.4l-1.1-1.1M4.7 4.7L3.6 3.6"
    /><circle cx="8" cy="8" r="4.6" />
  {:else if name === 'search'}
    <circle cx="7" cy="7" r="4.2" /><path d="M10.2 10.2l3.3 3.3" />
  {:else if name === 'lock'}
    <rect x="3.5" y="7" width="9" height="6.5" rx="1.2" /><path d="M5.5 7V5a2.5 2.5 0 015 0v2" />
  {/if}
</svg>
