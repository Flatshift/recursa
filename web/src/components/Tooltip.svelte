<script lang="ts">
  // Tooltips for every `title` attribute, drawn by the page. macOS doesn't show WebKit's native
  // tooltips while the app window is in full screen, so the title moves to this layer while the
  // pointer is over its element, with the native delay, then goes back.
  import { onMount } from 'svelte';

  let tip: HTMLDivElement | undefined = $state();
  let anchor: HTMLElement | null = null;
  let stash = '';
  let muted = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let settle: ReturnType<typeof setTimeout> | undefined;
  let warmUntil = 0;
  let px = 0;
  let py = 0;

  const shown = () => !!tip?.matches(':popover-open');

  function hide() {
    clearTimeout(timer);
    if (shown()) {
      tip!.hidePopover();
      // Moving straight on to another titled element shows its tip at once, as on macOS.
      warmUntil = performance.now() + 500;
    }
  }

  function release() {
    hide();
    // A pending re-arm after a scroll would show a tip where the pointer no longer is.
    clearTimeout(settle);
    settle = undefined;
    if (anchor && !anchor.hasAttribute('title')) anchor.setAttribute('title', stash);
    anchor = null;
    stash = '';
    muted = false;
  }

  function show() {
    if (!anchor || !tip || muted || !anchor.isConnected) return;
    // The page re-rendered the title while the pointer was on it.
    const fresh = anchor.getAttribute('title');
    if (fresh != null) {
      stash = fresh;
      anchor.removeAttribute('title');
    }
    // Set directly, not through state, so the size measured below is this text's.
    tip.textContent = stash;
    tip.style.left = '0px';
    tip.style.top = '0px';
    tip.showPopover();
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    // Cells, chips and buttons get the tip right under (or over) their box, so it reads as theirs
    // even when it is tall and has to flip; anything taller than a row keeps the pointer's place.
    const r = anchor.getBoundingClientRect();
    const boxed = r.height <= 96;
    const under = boxed ? r.bottom + 6 : py + 20;
    const over = (boxed ? r.top - 6 : py - 10) - h;
    tip.style.left = `${Math.max(6, Math.min(px, innerWidth - w - 6))}px`;
    tip.style.top = `${under + h < innerHeight - 6 ? under : Math.max(6, over)}px`;
  }

  function arm(t: HTMLElement | null, x: number, y: number) {
    release();
    // An empty title is how markup turns an ancestor's tooltip off.
    const title = t?.getAttribute('title');
    if (!t || !title?.trim()) return;
    anchor = t;
    stash = title;
    t.removeAttribute('title');
    px = x;
    py = y;
    timer = setTimeout(show, performance.now() < warmUntil ? 40 : 700);
  }

  function over(e: PointerEvent) {
    if (e.pointerType === 'touch') return;
    const target = e.target as Element;
    const t = target.closest?.('[title]') as HTMLElement | null;
    // Still inside the current element (its own title is off it now), unless a titled child took over.
    if (anchor && anchor.contains(target) && !(t && t !== anchor && anchor.contains(t))) return;
    arm(t, e.clientX, e.clientY);
  }

  function out(e: PointerEvent) {
    if (anchor && !(e.relatedTarget instanceof Node && anchor.contains(e.relatedTarget))) release();
  }

  function move(e: PointerEvent) {
    if (e.pointerType === 'touch') return;
    px = e.clientX;
    py = e.clientY;
    // After a scroll or a re-render the pointer sits on something it never entered.
    if (!anchor) over(e);
  }

  // A click or a key dismisses the tip until the pointer leaves the element, as natively.
  function dismiss() {
    if (!anchor) return;
    muted = true;
    hide();
  }

  // Scrolling moves the content under the pointer: drop the tip, and once the scroll settles
  // (trackpad inertia keeps it going) start over for whatever is under the pointer now.
  function scrolled() {
    if (!anchor && !settle) return;
    release();
    warmUntil = 0;
    clearTimeout(settle);
    settle = setTimeout(() => {
      settle = undefined;
      const el = px || py ? document.elementFromPoint(px, py) : null;
      arm((el?.closest('[title]') as HTMLElement | null) ?? null, px, py);
    }, 250);
  }

  onMount(() => {
    // WebKit before Safari 17 has no popovers: leave the native tooltips alone there.
    if (!('showPopover' in HTMLElement.prototype)) return;
    const opts = { capture: true, passive: true };
    document.addEventListener('pointerover', over, opts);
    document.addEventListener('pointerout', out, opts);
    document.addEventListener('pointermove', move, opts);
    document.addEventListener('pointerdown', dismiss, opts);
    document.addEventListener('keydown', dismiss, opts);
    document.addEventListener('wheel', scrolled, opts);
    document.addEventListener('scroll', scrolled, opts);
    window.addEventListener('blur', release);
    return () => {
      clearTimeout(settle);
      release();
      document.removeEventListener('pointerover', over, opts);
      document.removeEventListener('pointerout', out, opts);
      document.removeEventListener('pointermove', move, opts);
      document.removeEventListener('pointerdown', dismiss, opts);
      document.removeEventListener('keydown', dismiss, opts);
      document.removeEventListener('wheel', scrolled, opts);
      document.removeEventListener('scroll', scrolled, opts);
      window.removeEventListener('blur', release);
    };
  });
</script>

<div bind:this={tip} class="tt" popover="manual" role="tooltip"></div>

<style>
  .tt {
    position: fixed;
    inset: auto;
    margin: 0;
    max-width: min(340px, calc(100vw - 12px));
    padding: 5px 9px;
    border: 0;
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--ink);
    box-shadow: var(--shadow-pop);
    font-size: var(--t-xs);
    font-weight: 400;
    line-height: 1.45;
    text-align: left;
    white-space: pre-line;
    overflow-wrap: anywhere;
    pointer-events: none;
    user-select: none;
    animation: tt-in 120ms var(--ease-out);
  }
  @keyframes tt-in {
    from {
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .tt {
      animation: none;
    }
  }
</style>
