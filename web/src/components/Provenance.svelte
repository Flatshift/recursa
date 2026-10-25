<script lang="ts">
  // The provenance mark: ● measured from this network · ○ published by the operator · – unknown.
  // `titled` off where a surrounding tooltip already explains the mark, so two never stack.
  let { kind, titled = true }: { kind: 'verified' | 'published' | 'unknown'; titled?: boolean } = $props();
  const title = $derived(
    kind === 'verified'
      ? 'Measured from your network in this run'
      : kind === 'published'
        ? 'Published by the operator; not verified here'
        : 'Unknown: neither measured nor documented',
  );
</script>

<span class="prov {kind}" title={titled ? title : undefined} aria-label={title} role="img"></span>

<style>
  .prov {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
    vertical-align: 0.08em;
  }
  .verified {
    background: var(--ink);
  }
  .published {
    border: 1.5px solid var(--ink-2);
  }
  .unknown {
    width: 7px;
    height: 1.5px;
    border-radius: 1px;
    background: var(--ink-3);
    vertical-align: 0.3em;
  }
</style>
