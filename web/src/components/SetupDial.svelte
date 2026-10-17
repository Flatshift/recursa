<script lang="ts" module>
  // The Cantor dial (tools/icons.mjs, the 64 px-and-up geometry), one group per ring so each can turn on its own.
  const RADII = [300, 222, 150, 84];
  const W = 40;
  const KEEP = 0.4;
  const WALK = [1, 0, 1];
  const TW = 0.075;
  const f = (v: number) => Math.round(v * 10) / 10;

  type Arc = { d?: string; dot?: [number, number, number]; on: boolean };
  type Ring = { arcs: Arc[]; stop?: [number, number, number] };

  function build(): Ring[] {
    const levels: [number, number][][] = [[[0, 1]], [[0.05, 0.45], [0.55, 0.95]]];
    const ci = [0, WALK[0]];
    for (let r = 2; r < RADII.length; r++) {
      levels.push(levels[r - 1].flatMap(([a, b]) => { const q = (b - a) * KEEP; return [[a, a + q], [b - q, b]] as [number, number][]; }));
      ci.push(ci[r - 1] * 2 + WALK[r - 1]);
    }
    const L = RADII.length - 1;
    const fin = levels[L][ci[L]];
    const off = 0.125 - (fin[0] + fin[1]) / 2 - L * TW;
    const at = (R: number, t: number) => [f(512 + R * Math.cos(t)), f(512 + R * Math.sin(t))];
    const rings: Ring[] = [];
    for (let r = 1; r <= L; r++) {
      const R = RADII[r];
      const ring: Ring = { arcs: [] };
      levels[r].forEach(([s0, s1], i) => {
        const on = i === ci[r];
        const a = 2 * Math.PI * (s0 + off + r * TW), b = 2 * Math.PI * (s1 + off + r * TW), cap = W / 2 / R, mid = (a + b) / 2;
        if (b - a <= 2 * cap + 0.02) {
          const [x, y] = at(R, mid);
          ring.arcs.push({ dot: [x, y, W / 2], on });
          if (on) ring.stop = [x, y, (W / 2) * 1.3];
        } else {
          const [a0, a1] = [at(R, a + cap), at(R, b - cap)];
          ring.arcs.push({ d: `M${a0}A${R},${R} 0 ${b - a - 2 * cap > Math.PI ? 1 : 0} 1 ${a1}`, on });
          if (on) ring.stop = [...at(R, mid), W * 0.62] as [number, number, number];
        }
      });
      rings.push(ring);
    }
    return rings;
  }
  const RINGS = build();
</script>

<script lang="ts">
  import { fade } from 'svelte/transition';
  import { reduceMotion } from '../lib/motion';

  // The mark tuning itself in while a run sets up. It stands for nothing being measured: the rings turn, settle into
  // the mark one by one (root, TLD, name), light its path, and let go again. Under reduced motion it is the still mark.
  // `still` holds the locked mark; turning it off starts from the lock and lets go, turning it back on settles into
  // the next lock and stays.
  let { still = false }: { still?: boolean } = $props();
  const CYCLE = 4400;
  // Where each ring starts, in degrees, and which way it turns. A ring has half-turn symmetry, so a full turn per cycle
  // brings it back to where it began.
  const FROM = [-140, 200, -260];
  // When each ring locks (outer first) and when they all let go, as fractions of the cycle.
  const LOCK = [0.3, 0.37, 0.44];
  const LET_GO = 0.76;
  const OUT = 'cubic-bezier(0.22, 1, 0.36, 1)';
  const IN = 'cubic-bezier(0.64, 0, 0.78, 0)';

  // Every ring is locked between the last LOCK and LET_GO; a still dial rests at HOLD.
  const HOLD = 0.6;
  const SETTLED = LET_GO - 0.04;
  const SETTLE = 1200;

  // Runs `make`'s animation unless still. Leaving still starts at the lock (the still pose) so nothing jumps; becoming
  // still ends the animation at the next locked moment, now if the rings are locked already, and holds there.
  function driven(make: () => Animation, still: boolean) {
    let a: Animation | null = null;
    let first = true;
    const set = (still: boolean) => {
      if (reduceMotion) return;
      if (!still && a?.playState === 'running') {
        a.effect?.updateTiming({ iterations: Infinity, fill: 'none' });
        a.updatePlaybackRate(1);
      } else if (!still) {
        a?.cancel();
        a = make();
        if (!first) a.currentTime = HOLD * CYCLE;
      } else if (a) {
        const t = (a.currentTime as number) / CYCLE;
        const k = Math.floor(t), p = t - k;
        const end = p <= SETTLED ? k + Math.max(p, HOLD) : k + 1 + HOLD;
        a.effect?.updateTiming({ iterations: end, fill: 'forwards' });
        // Settle within SETTLE ms, so the mark locks in while the results are still arriving.
        const left = (end - t) * CYCLE;
        if (left > SETTLE) a.updatePlaybackRate(left / SETTLE);
      }
      first = false;
    };
    set(still);
    return { update: set, destroy: () => a?.cancel() };
  }

  function turn(el: SVGGElement, { i, still }: { i: number; still: boolean }) {
    const from = FROM[i], to = from + (from < 0 ? 360 : -360);
    const make = () =>
      el.animate(
        [
          { offset: 0, transform: `rotate(${from}deg)`, easing: OUT },
          { offset: LOCK[i], transform: 'rotate(0deg)' },
          { offset: LET_GO, transform: 'rotate(0deg)', easing: IN },
          { offset: 1, transform: `rotate(${to}deg)` },
        ],
        { duration: CYCLE, iterations: Infinity },
      );
    const d = driven(make, still);
    return { update: (p: { still: boolean }) => d.update(p.still), destroy: d.destroy };
  }

  // The kept arc lights as its ring locks and dims as the rings let go; its stop grows in just after.
  function light(el: SVGElement, { i, stop, still }: { i: number; stop?: boolean; still: boolean }) {
    const on = LOCK[i] + (stop ? 0.015 : 0);
    const make = () => el.animate(
      stop
        ? [
            { offset: 0, opacity: 0, transform: 'scale(0)' },
            { offset: on, opacity: 0, transform: 'scale(0)', easing: OUT },
            { offset: on + 0.07, opacity: 1, transform: 'scale(1)' },
            { offset: LET_GO - 0.04, opacity: 1, transform: 'scale(1)' },
            { offset: LET_GO + 0.02, opacity: 0, transform: 'scale(1)' },
            { offset: 1, opacity: 0, transform: 'scale(0)' },
          ]
        : [
            { offset: 0, opacity: 0 },
            { offset: on, opacity: 0, easing: OUT },
            { offset: on + 0.05, opacity: 1 },
            { offset: LET_GO - 0.04, opacity: 1 },
            { offset: LET_GO + 0.02, opacity: 0 },
            { offset: 1, opacity: 0 },
          ],
      { duration: CYCLE, iterations: Infinity },
    );
    const d = driven(make, still);
    return { update: (p: { still: boolean }) => d.update(p.still), destroy: d.destroy };
  }

  const STOP = ['tld', 'uncached', 'answer'];
</script>

<div class="dial" aria-hidden="true" in:fade={{ duration: reduceMotion ? 0 : 360 }} out:fade={{ duration: reduceMotion ? 0 : 220 }}>
  <svg viewBox="170 170 684 684" width="132" height="132" fill="none" stroke-width={W} stroke-linecap="round">
    <circle class="root" cx="512" cy="512" r={RADII[0]} pathLength="1" />
    {#each RINGS as ring, i}
      <g class="ring" use:turn={{ i, still }}>
        {#each ring.arcs as a}
          {#if a.d}
            <path class="arc" d={a.d} />
            {#if a.on}<path class="path" d={a.d} use:light={{ i, still }} />{/if}
          {:else if a.dot}
            <circle class="arc" cx={a.dot[0]} cy={a.dot[1]} r={a.dot[2]} />
          {/if}
        {/each}
        {#if ring.stop}
          <circle class="stop {STOP[i]}" cx={ring.stop[0]} cy={ring.stop[1]} r={ring.stop[2]} style:transform-origin="{ring.stop[0]}px {ring.stop[1]}px" use:light={{ i, stop: true, still }} />
        {/if}
      </g>
    {/each}
  </svg>
</div>

<style>
  .dial {
    display: grid;
    place-items: center;
    width: 220px;
    height: 220px;
    /* A pool of surface under the mark, so the placeholder rows fade out around it instead of running through it. */
    background: radial-gradient(closest-side, var(--surface) 62%, color-mix(in oklch, var(--surface) 0%, transparent));
  }
  svg {
    overflow: visible;
  }
  .ring,
  .stop {
    transform-box: view-box;
  }
  .ring {
    transform-origin: 512px 512px;
  }
  .root,
  .arc {
    stroke: color-mix(in oklch, var(--ink) 24%, var(--surface));
  }
  circle.arc {
    stroke: none;
    fill: color-mix(in oklch, var(--ink) 24%, var(--surface));
  }
  .root {
    stroke-dasharray: 1;
    stroke-dashoffset: 1;
    transform: rotate(-90deg);
    transform-box: view-box;
    transform-origin: 512px 512px;
    animation: draw 900ms var(--ease-out) 60ms forwards;
  }
  .path {
    stroke: var(--ink);
  }
  .stop.tld {
    fill: var(--s-tld);
  }
  .stop.uncached {
    fill: var(--s-uncached);
  }
  .stop.answer {
    fill: var(--brand-answer);
  }
  @keyframes draw {
    to {
      stroke-dashoffset: 0;
    }
  }
  /* Reduced motion: the still mark, path lit. */
  @media (prefers-reduced-motion: reduce) {
    .root {
      animation: none;
      stroke-dashoffset: 0;
    }
  }
</style>
