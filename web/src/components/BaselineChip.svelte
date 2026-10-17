<script lang="ts">
  /** Marks your own local forwarder in a ranking: measured and shown for reference, never the pick.
   *  `lead` puts it before a name that may be cut short, so the mark is never what gets cut. */
  let { what = null, lead = false }: { what?: string | null; lead?: boolean } = $props();
  const title = $derived(
    `${what ?? 'A local forwarder'}: a DNS forwarder on your network, such as dnsmasq. Shown for reference and never named the best pick: it isn’t an alternative to choose, it answers repeats from a cache on your own network, and it forwards the rest to whatever you choose. See Method.`,
  );
</script>

<span class="bchip" class:lead {title}><span class="visually-hidden">{lead ? '' : ', '}</span>baseline<span class="visually-hidden">{lead ? ', ' : ''}</span></span>

<style>
  .bchip {
    display: inline-block;
    margin-left: 6px;
    padding: 0 6px;
    border: 1px dashed var(--rule-2);
    border-radius: var(--r-sm);
    font-size: var(--t-2xs);
    line-height: 15px;
    font-weight: 560;
    color: var(--ink-2);
    vertical-align: 1px;
    white-space: nowrap;
  }
  .bchip.lead {
    margin: 0 6px 0 0;
  }
</style>
