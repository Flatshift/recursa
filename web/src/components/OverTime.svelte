<script lang="ts">
  // Response time over time on one shared axis: the medians together on a log scale, then, behind a toggle, one small
  // panel per series with its median line and its variation (half, p25 to p75, and nine in ten, p5 to p95), and under each a strip of lost queries.
  // Gaps and left-out stretches are hatched across every panel and named. A stretch where the series slowed together
  // keeps its data under a plain tint, with a cap whose shape says where the cause sits. Pointer and arrow keys share one cursor.
  import { flushSync, onDestroy, untrack } from 'svelte';
  import { Tween } from 'svelte/motion';
  import type { OverTimeGap, OverTimeMark, OverTimeOutage, OverTimeSeries, OverTimeSlow } from '../lib/overtime';
  import { BUBBLES, FEED, POUR, REVEAL, ZOOM, feedDip, feedDots, feedPath, pourAt, ramp, reduceMotion, wander } from '../lib/motion';
  import { SLOW_FAMILY, bandPath, blendSeries, fitSpans, lastFilled, linePath, lossClass, lossHeight, overlayValues, slotAt, slotGrid, spreadLabels, timeTicks } from '../lib/overtime';
  import { logDomain, logScale, logTicks, tickLabel } from '../lib/scale';
  import { ms } from '../lib/format';
  import { readLocal, writeLocal } from '../lib/store.svelte';
  import Icon from './Icon.svelte';

  let {
    series,
    gaps,
    slows = [],
    marks = [],
    outages = [],
    cursor = $bindable(null),
    formatTime,
    unit,
    totals,
    totalsLabel,
    slots,
    pour,
  }: {
    series: OverTimeSeries[];
    gaps: OverTimeGap[];
    slows?: OverTimeSlow[];
    marks?: OverTimeMark[];
    outages?: OverTimeOutage[];
    /** The slot under the pointer or the keyboard, shared with whatever else lists these times. */
    cursor?: number | null;
    formatTime: (t: number) => string;
    /** What one cell is called: "15-second bin", "hour". */
    unit: string;
    /** Each series' median over the whole chart, read out under it until a bin is picked. */
    totals?: (number | null | undefined)[];
    /** What `totals` covers: "Whole run", "So far". */
    totalsLabel?: string;
    /** While a run sends: the slots it will fill. The chart keeps that width and fills from the left, easing each update in. */
    slots?: number;
    /** The chart taking over from the filled vessel: how full it was (0 to 1). Read once, as the chart opens; the liquid
     *  then pours into the warm-up sliver while the rest of the chart fades in. */
    pour?: number;
  } = $props();

  const uid = $props.id();
  const PAD_L = 44;
  const OV_TOP = 30;
  const OV_H = 150;
  // A value axis stacks its labels, so a decade needs far less room for 1-2-5 ticks than a time axis does.
  const TICK_ROOM = 46;
  const MINOR_ROOM = 22;
  const OV_BOTTOM = OV_TOP + OV_H;

  let W = $state(0);
  let sel = $state<number | null>(null);
  let tip = $state<{ x: number; y: number; text: string } | null>(null);
  let live = $state('');
  // The series whose end label is pointed at or focused; with three or more series only it shows its variation.
  let pick = $state<number | null>(null);
  // Middle halves: every one shaded and blended at once, or only the line nearest the pointer's.
  let shade = $state<'all' | 'nearest'>(readLocal<string>('recursa.middleHalf', 'all') === 'nearest' ? 'nearest' : 'all');
  function setShade(v: typeof shade) {
    shade = v;
    writeLocal('recursa.middleHalf', v);
  }
  // The header switch's width, so the caption beside it is cut short of it.
  let switchW = $state(0);
  // The median line nearest the pointer in the medians chart.
  let near = $state<number | null>(null);

  const grid = $derived(slotGrid(series));
  // The slots with data; a live run is drawn to all it will fill, so the chart keeps one width as it fills.
  const edge = $derived(grid?.n ?? 0);
  const growing = $derived(slots != null);
  const N = $derived(grid ? Math.max(edge, slots ?? 0) : 0);
  const wide = $derived(W >= 620);
  const padR = $derived(wide ? 128 : 10);
  const pw = $derived(Math.max(40, W - PAD_L - padR));
  const slotW = $derived(N ? pw / N : pw);
  // Six identity colours (--r-1 to --r-6); past that nothing is coloured by identity and the panels carry the names.
  const many = $derived(series.length > 6);
  const colour = (i: number) => (many ? 'var(--ink)' : `var(--r-${i + 1})`);
  const panelH = $derived(many ? 44 : 160);
  const pitch = $derived(panelH + 38);
  const first = $derived(OV_BOTTOM + 34);
  const rowTop = (i: number) => first + i * pitch;
  const lastBottom = $derived(series.length ? rowTop(series.length - 1) + panelH + 16 : OV_BOTTOM);
  const H = $derived(lastBottom + 30);
  // The medians chart ends at its own time axis; the panels open below it in a second chart that keeps these
  // coordinates, so its view starts at B0 and its tints at BT, just above the first panel's name.
  const HA = OV_BOTTOM + 30;
  const B0 = $derived(first - 28);
  const BT = $derived(first - 22);
  let open = $state(false);

  const xb = (i: number) => PAD_L + i * slotW;
  const xm = (i: number) => xb(i) + slotW / 2;
  const startOf = (i: number) => (grid ? grid.base + i * grid.width : 0);
  const tx = (t: number) => (grid ? Math.min(PAD_L + pw, Math.max(PAD_L, PAD_L + ((t - grid.base) / grid.width) * slotW)) : PAD_L);

  function domainOf(pick: (c: NonNullable<OverTimeSeries['cells'][number]>) => (number | null)[]): [number, number] {
    let lo = Infinity;
    let hi = 0;
    for (const s of series)
      for (const c of s.cells) if (c) for (const v of pick(c)) if (v != null && v > 0) (lo = Math.min(lo, v)), (hi = Math.max(hi, v));
    return logDomain(lo, hi);
  }
  const wantOv = $derived(domainOf((c) => overlayValues(c)));
  const wantP = $derived(domainOf((c) => [c.p5 ?? c.p50, c.p95 ?? c.p50]));
  // While a run is live each axis eases to its new bounds (in log space, as it is drawn); otherwise it snaps.
  const toLog = (d: [number, number]): [number, number] => [Math.log10(d[0]), Math.log10(d[1])];
  const zOv = new Tween(untrack(() => toLog(wantOv)), ZOOM);
  const zP = new Tween(untrack(() => toLog(wantP)), ZOOM);
  $effect.pre(() => {
    const a = toLog(wantOv);
    const b = toLog(wantP);
    untrack(() => {
      zOv.set(a, growing ? ZOOM : { duration: 0 });
      zP.set(b, growing ? ZOOM : { duration: 0 });
    });
  });
  const eased = (z: [number, number], want: [number, number]): [number, number] => {
    const [a, b] = toLog(want);
    return Math.abs(z[0] - a) < 1e-4 && Math.abs(z[1] - b) < 1e-4 ? want : [10 ** z[0], 10 ** z[1]];
  };
  const domOv = $derived(eased(zOv.current, wantOv));
  const domP = $derived(eased(zP.current, wantP));
  const yOv = $derived(logScale(domOv, OV_BOTTOM, OV_TOP));
  const inOv = (t: number) => t >= domOv[0] && t <= domOv[1];
  const inP = (t: number) => t >= domP[0] && t <= domP[1];
  const ticksOv = $derived(logTicks(wantOv, OV_H, TICK_ROOM).filter(inOv));
  const ticksP = $derived(logTicks(wantP, panelH, TICK_ROOM).filter(inP));
  // Unlabelled lines at 2 and 5 where the labels had to drop to decades.
  const minorP = $derived(logTicks(wantP, panelH, MINOR_ROOM).filter((t) => !ticksP.includes(t) && inP(t)));

  // Between a live run's updates each cell eases from what was drawn to its new value; the read-outs keep the figures.
  let from = untrack(() => series);
  let to = untrack(() => series);
  const step = new Tween(1, ZOOM);
  $effect.pre(() => {
    const next = series;
    untrack(() => {
      if (next === to) return;
      const same = growing && next.length === to.length && next.every((s, i) => s.id === to[i].id);
      from = same ? blendSeries(from, to, step.current) : next;
      to = next;
      step.set(0, { duration: 0 });
      step.set(1, same ? ZOOM : { duration: 0 });
    });
  });
  const drawn = $derived(step.current >= 1 ? series : blendSeries(from, to, step.current));

  // A new bin's line draws on from the last point to it over about one update, rather than appearing at once.
  const lastSlot = $derived(series.reduce((m, s) => Math.max(m, lastFilled(s.cells)), 0));
  // Poured from the vessel: the lines wait at their start until the liquid lands, then draw on from there.
  const pourLevel = untrack(() => (pour != null && slots != null && !reduceMotion ? Math.min(1, Math.max(0, pour)) : null));
  const firstSlot = untrack(() => Math.min(...series.map((s) => s.cells.findIndex((c) => c)).filter((k) => k >= 0)));
  const pourT = new Tween(pourLevel != null ? 0 : 1);
  const pouring = $derived(pourT.current < 1);
  let poured = pourLevel == null;
  const reveal = new Tween(untrack(() => (pourLevel != null && Number.isFinite(firstSlot) ? firstSlot - 0.5 : lastSlot)), REVEAL);
  $effect.pre(() => {
    const at = lastSlot;
    untrack(() => poured && reveal.set(at, growing && at > reveal.target ? REVEAL : { duration: 0 }));
  });
  const drawing = $derived(growing && (pouring || reveal.current < reveal.target));
  const revealX = $derived(drawing ? xm(reveal.current) : W);

  const hwLine = $derived(Math.min(slotW / 2, 4));
  const slotXs = $derived(Array.from({ length: N }, (_, i) => xm(i)));
  const cellsOf = (s: OverTimeSeries) => Array.from({ length: N }, (_, i) => s.cells[i] ?? null);

  const together = $derived(
    drawn.map((s, i) => {
      const cells = cellsOf(s);
      const ys = cells.map((c) => (c?.p50 != null ? yOv(c.p50) : null));
      const last = lastFilled(ys);
      const at = (k: 'p25' | 'p75') => cells.map((c) => (c?.[k] != null ? yOv(c[k]!) : null));
      return { i, d: linePath(slotXs, ys, hwLine), band: bandPath(slotXs, at('p75'), at('p25'), slotW / 2), ys, last, ly: last >= 0 ? ys[last]! : 0 };
    }),
  );
  /** Where a line ends as drawn: its last point, or mid-draw the point the reveal has reached on the way to it. */
  function tipOf(t: (typeof together)[number]): [number, number] {
    const r = reveal.current;
    if (!drawing || r >= t.last) return [xm(t.last), t.ly];
    const k = Math.floor(r);
    const a = t.ys[k];
    const b = t.ys[k + 1];
    return a != null && b != null ? [xm(r), a + (b - a) * (r - k)] : [Math.min(xm(t.last), revealX), t.ly];
  }
  // End-of-line names, pushed apart so close lines keep separate labels.
  const ends = $derived.by(() => {
    const seen = together.filter((t) => t.last >= 0);
    if (many) return [];
    const tips = seen.map(tipOf);
    const ys = spreadLabels(tips.map((p) => p[1]), 16, OV_TOP + 6, OV_BOTTOM - 4);
    return seen.map((t, k) => ({ i: t.i, x: tips[k][0], y: tips[k][1], labelY: ys[k] }));
  });
  // Two series always show their middle halves; with more, bands would pile up, so one shows while its name is pointed at.
  const fewSeries = $derived(series.length <= 2);
  // The series picked out: its name pointed at, the line nearest the pointer, or the one picked with the arrow keys.
  const hot = $derived(many ? null : (pick ?? near ?? sel));
  const switching = $derived(!fewSeries && !many);
  const blend = $derived(switching && shade === 'all');
  const bandOn = (i: number) => (fewSeries || blend ? true : hot === i);
  // Beside the switch, which names the shading, the caption only says what the line is or how to pick one.
  const caption = $derived(
    fewSeries
      ? 'line is each median, shaded is its variation'
      : many || blend
        ? 'line is each median'
        : 'point near a line to shade it',
  );
  // Characters of caption that fit beside the title, short of the switch at the plot's right edge.
  const captionFit = $derived(Math.max(8, Math.floor((pw - (switching ? switchW + 12 : 0)) / 6.2) - 20));
  const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, Math.max(1, n - 1))}…` : s);

  const panels = $derived(
    drawn.map((s, i) => {
      const top = rowTop(i);
      const bottom = top + panelH;
      const y = logScale(domP, bottom, top);
      const cells = cellsOf(s);
      const ys = (pick: 'p5' | 'p25' | 'p50' | 'p75' | 'p95') => cells.map((c) => (c?.[pick] != null ? y(c[pick]!) : null));
      const bars: { x: number; h: number; cls: string }[] = [];
      const thin: number[] = [];
      const empty: [number, number][] = [];
      let lost = 0;
      let from = -1;
      cells.forEach((c, k) => {
        // Past the data a live run has not got there yet; that is not a stretch without checks.
        if (k >= edge) return;
        if ((!c || (c.sent === 0 && c.cls !== 'shared')) && !gapAt(k)?.soft) {
          if (from < 0) from = k;
        } else if (from >= 0) {
          empty.push([from, k]);
          from = -1;
        }
        if (!c) return;
        lost += c.lost;
        const cls = lossClass(c);
        if (cls === 'thin') thin.push(k);
        else if (cls) bars.push({ x: xb(k), h: lossHeight(c, cls) * 8, cls });
      });
      if (from >= 0) empty.push([from, edge]);
      return {
        i,
        top,
        bottom,
        bandOuter: bandPath(slotXs, ys('p95'), ys('p5'), slotW / 2),
        bandInner: bandPath(slotXs, ys('p75'), ys('p25'), slotW / 2),
        median: linePath(slotXs, ys('p50'), hwLine),
        bars,
        thin,
        empty,
        lost,
        outs: outages
          .filter((o) => o.id === s.id)
          .map((o) => ({ x0: tx(o.t0), x1: tx(o.t1), shared: o.shared }))
          .filter((o) => o.x1 > o.x0),
        marks: marks
          .filter((m) => m.id === s.id && grid && m.t >= grid.base && m.t < grid.base + N * grid.width)
          .map((m) => ({ x: tx(m.t), t: m.t, title: m.title })),
        y,
      };
    }),
  );

  // Gaps and slow stretches named above the first panel; a name that would run into another stays off (its tooltip
  // still says it), and gaps are placed first.
  const boxes = $derived.by(() => {
    const wOf = (s: string) => s.length * 6.2;
    const place = <T extends { t0: number; t1: number; label: string }>(list: T[]) =>
      list
        .map((g) => ({ g, x0: tx(g.t0), x1: tx(g.t1) }))
        .filter((b) => b.x1 > b.x0)
        .map((b) => {
          const w = wOf(b.g.label);
          const end = b.x0 - 6 - w >= PAD_L;
          return { ...b, anchor: end ? 'end' : 'start', tx: end ? b.x0 - 6 : b.x0 + 4, span: (end ? [b.x0 - 6 - w, b.x0 - 6] : [b.x0 + 4, b.x0 + 4 + w]) as [number, number] };
        });
    const placed = place(gaps);
    const slow = place(slows);
    // A soft gap is part of the plan: named inside the plot, a word to a line when it is narrow, and not in the lane above.
    const hard = placed.filter((p) => !p.g.soft);
    const fit = fitSpans([[PAD_L, PAD_L + 112], ...hard.map((p) => p.span), ...slow.map((p) => p.span)]).slice(1);
    const linesIn = (label: string, w: number) =>
      [[label], label.split(' ')].find((ls) => ls.every((l) => w >= wOf(l) + 8)) ?? [];
    return {
      gaps: placed.map((p) => ({
        ...p,
        named: !p.g.soft && fit[hard.indexOf(p)],
        inside: p.g.soft ? linesIn(p.g.label, p.x1 - p.x0) : [],
      })),
      slows: slow.map((p, k) => ({ ...p, w: Math.max(3, p.x1 - p.x0), family: SLOW_FAMILY[p.g.kind], named: fit[hard.length + k] })),
    };
  });
  const gapBoxes = $derived(boxes.gaps);

  // The pour: the vessel becomes the plot and its liquid tips into the warm-up sliver while the rest of the chart
  // fades in and the chart grows from the vessel's height to its own. Without a sliver to pour into, the chart just opens.
  const warm = $derived(gapBoxes.find((b) => b.g.soft));
  const pf = $derived(
    pouring && warm
      ? pourAt(pourT.current, {
          w: W,
          floor: OV_BOTTOM,
          level: (pourLevel ?? 1) * OV_BOTTOM,
          left: warm.x0,
          right: warm.x0 + Math.max(3, warm.x1 - warm.x0),
          top: OV_TOP,
          end: W - padR,
        })
      : null,
  );
  const chartIn = $derived(pouring ? ramp(pourT.current, 0.2, 0.5) : undefined);
  const landed = $derived(pouring ? ramp(pourT.current, POUR.arrive, POUR.arrive + 0.15) : undefined);
  const settled = $derived(pouring ? ramp(pourT.current, 0.8, 1) : undefined);

  // The warm-up's dots. The pour carries them in with the liquid; while the run sends they drift across the warm-up
  // and bend onto the lines at its edge, since the names it cached are what the repeat lookups ask.
  const warmEnd = $derived(warm ? warm.x0 + Math.max(3, warm.x1 - warm.x0) : 0);
  const feed = $derived(warm ? feedDots(warm.x0, warmEnd, OV_TOP, OV_H) : null);
  const feeding = $derived(growing && !reduceMotion && !pf);
  // Where each line crosses the warm-up's edge, and its first point past it.
  const intakes = $derived.by(() => {
    const k = slotXs.findIndex((x) => x >= warmEnd);
    if (!warm || k < 0) return [];
    return together.flatMap((t) => {
      const y = t.ys[k];
      const before = k > 0 ? t.ys[k - 1] : null;
      if (y == null) return [];
      const along: [number, number][] = [];
      for (let i = k; i < t.ys.length && t.ys[i] != null && xm(i) - warmEnd < FEED.reach + slotW; i++) along.push([xm(i), t.ys[i]!]);
      return [{ along, edge: before == null ? y : y + ((before - y) * (xm(k) - warmEnd)) / (xm(k) - xm(k - 1)) }];
    });
  });
  /** The line a dot in row `y` feeds, the nearest at the edge: how far off it is there, and its points past it. */
  const intake = (y: number) => {
    const to = intakes.reduce<(typeof intakes)[number] | null>((m, n) => (m == null || Math.abs(n.edge - y) < Math.abs(m.edge - y) ? n : m), null);
    return to ? { dy: Math.round(to.edge - y), along: to.along.map(([x, ly]): [number, number] => [Math.round(x), Math.round(ly - y)]) } : { dy: 0, along: [] };
  };
  // The lines have stopped easing to a live update: until then a dot keeps the round it has.
  const still = $derived(step.current >= 1 && domOv === wantOv);
  type Flow = { on: boolean; still: boolean; x: number; dy: number; along: [number, number][]; p: number };
  let flowT0: number | undefined;
  /** One dot's round: from the axis across the warm-up at an even drift, bending onto its line by the edge, then along
   *  the line into the plot, fading. It rests where the round starts it, and holds where it is when the run stops. */
  function flow(node: SVGGElement, o: Flow) {
    let anim: Animation | undefined;
    let key = '';
    const set = (o: Flow) => {
      if (!o.on) {
        // Stopped: a dot on its way out goes back to wait behind the axis.
        if (anim && feed && ((anim.currentTime as number) % feed.total) > feed.travel) anim.currentTime = 0;
        anim?.pause();
        return;
      }
      const k = `${feed?.from} ${warmEnd} ${o.x} ${o.dy} ${o.along}`;
      if (!feed || (anim?.playState === 'running' && (k === key || !o.still))) return;
      // One clock for every dot, so they stay evenly spread when the chart's width changes their round.
      flowT0 ??= performance.now();
      const at = o.p * feed.total + performance.now() - flowT0;
      anim?.cancel();
      key = k;
      anim = node.animate(
        feedPath(feed, warmEnd, o.dy, o.along).map((s) => ({
          transform: `translate(${(s.x - o.x).toFixed(1)}px, ${s.y.toFixed(1)}px)`,
          opacity: s.o,
          offset: s.at,
        })),
        { duration: feed.total, iterations: Infinity },
      );
      anim.currentTime = at;
    };
    set(o);
    return { update: set, destroy: () => anim?.cancel() };
  }
  /** One dot hanging in the liquid: it wanders about its place while the run sends, and holds where it is after. */
  function hang(node: SVGCircleElement, o: { on: boolean; i: number }) {
    let anim: Animation | undefined;
    const set = (o: { on: boolean; i: number }) => {
      if (!o.on) return anim?.pause();
      if (!anim) {
        const w = wander(o.i);
        anim = node.animate(
          w.path.map(([x, y]) => ({ transform: `translate(${x.toFixed(2)}px, ${y.toFixed(2)}px)` })),
          { duration: w.ms, iterations: Infinity },
        );
        anim.currentTime = w.p * w.ms;
      }
      anim.play();
    };
    set(o);
    return { update: set, destroy: () => anim?.cancel() };
  }
  let ot: HTMLDivElement;
  let held = $state(pourLevel != null);
  let started = false;
  const pourTimers: ReturnType<typeof setTimeout>[] = [];
  onDestroy(() => pourTimers.forEach(clearTimeout));
  $effect(() => {
    if (started || pourLevel == null || !(W > 0 && grid && series.length)) return;
    started = true;
    untrack(() => {
      const done = () => {
        poured = true;
        reveal.set(lastSlot, { duration: warm ? POUR.draw : 0 });
      };
      if (!warm) {
        pourT.set(1, { duration: 0 }).then(done);
        held = false;
        return;
      }
      // Measured as the growth starts, once everything it opens onto is drawn.
      pourTimers.push(setTimeout(() => {
        ot.style.height = 'auto';
        const h = ot.offsetHeight;
        ot.style.height = `${OV_BOTTOM}px`;
        if (h <= OV_BOTTOM) {
          held = false;
          return;
        }
        const grow = ot.animate([{ height: `${OV_BOTTOM}px` }, { height: `${h}px` }], {
          duration: POUR.grow,
          easing: 'cubic-bezier(0.65, 0, 0.35, 1)',
          fill: 'forwards',
        });
        grow.finished
          .then(() => {
            held = false;
            flushSync();
            grow.cancel();
          })
          .catch(() => {});
      }, POUR.hold + POUR.growAt * POUR.ms));
      // The lines start out of the sliver as the liquid lands in it, while it settles.
      pourTimers.push(setTimeout(done, POUR.hold + POUR.arrive * POUR.ms));
      pourT.set(1, { duration: POUR.ms, delay: POUR.hold });
    });
  });

  const cur = $derived(cursor != null && cursor >= 0 && cursor < N ? cursor : null);
  const pillText = $derived(cur == null ? '' : formatTime(startOf(cur)));
  const pillW = $derived(pillText.length * 6.4 + 16);
  const pillX = $derived(cur == null ? 0 : Math.max(PAD_L + pillW / 2, Math.min(PAD_L + pw - pillW / 2, xm(cur))));

  const axis = $derived.by(() => {
    if (!grid) return [];
    const ts = timeTicks(grid.base, grid.base + N * grid.width, wide ? 9 : 4);
    const items = ts.map((t) => ({ t, x: tx(t), label: formatTime(t) })).filter((it, k, a) => !k || it.label !== a[k - 1].label);
    const spans = items.map((it) => [it.x - it.label.length * 3.2, it.x + it.label.length * 3.2] as [number, number]);
    const ok = fitSpans(spans, 10);
    return items.filter((it, k) => ok[k] && spans[k][1] <= W);
  });

  const gapAt = (i: number) => gaps.find((g) => g.t1 > startOf(i) && g.t0 < startOf(i + 1));
  const slowAt = (i: number) => slows.find((s) => s.t1 > startOf(i) && s.t0 < startOf(i + 1));
  function summary(si: number, i: number): string {
    const s = series[si];
    const c = s.cells[i] ?? null;
    const t0 = c?.t0 ?? startOf(i);
    const t1 = c?.t1 ?? startOf(i + 1);
    const gap = gapAt(i);
    const parts = [`${formatTime(t0)} – ${formatTime(t1)}`];
    if (!c || c.sent === 0) parts.push(gap?.label ?? c?.note ?? 'no checks');
    else {
      parts.push(`${c.sent - c.lost} of ${c.sent} answered`, `median ${ms(c.p50)}`, `p95 ${ms(c.p95)}`);
      if (gap) parts.push(gap.label);
      const slow = slowAt(i);
      if (slow) parts.push(slow.label);
      if (c.note) parts.push(c.note);
    }
    const out = outages.find((o) => o.id === s.id && !o.shared && o.t1 > t0 && o.t0 < t1);
    if (out) parts.push(out.label);
    return parts.join(' · ');
  }
  /** One series at one slot, for the read-out line. */
  function readOf(si: number, i: number): string {
    const c = series[si].cells[i] ?? null;
    if (!c || c.sent === 0) return gapAt(i)?.label ?? c?.note ?? 'no checks';
    const spread = c.p5 != null && c.p95 != null ? `, 9 in 10 between ${ms(c.p5, false)} and ${ms(c.p95)}` : '';
    return `${ms(c.p50)}${spread}, ${c.lost} lost`;
  }
  const shownSeries = $derived(many && sel != null ? [sel] : series.map((_, k) => k));
  // Every charted check, so the read-out has figures before a bin is picked.
  const pooled = $derived(series.map((s) => s.cells.reduce((a, c) => (c ? { sent: a.sent + c.sent, lost: a.lost + c.lost } : a), { sent: 0, lost: 0 })));
  /** The read-out row for one series: its median, then the rest in words; the whole chart when no bin is picked. */
  function readParts(si: number, i: number | null): [string, string] {
    if (i == null) {
      const p = pooled[si];
      return [ms(totals?.[si]), p.sent ? `${p.lost.toLocaleString()} of ${p.sent.toLocaleString()} lost` : 'no checks'];
    }
    const c = series[si].cells[i] ?? null;
    if (!c || c.sent === 0) return ['—', gapAt(i)?.label ?? c?.note ?? 'no checks'];
    const spread = c.p5 != null && c.p95 != null ? `9 in 10 between ${ms(c.p5, false)} and ${ms(c.p95)}, ` : '';
    return [ms(c.p50), `${spread}${c.lost} of ${c.sent} lost`];
  }
  const hint = $derived(`oint at the chart, or use the arrow keys, to read one ${unit}`);
  const plain = (i: number) => {
    const slow = slowAt(i);
    return `${formatTime(startOf(i))}. ${slow ? `${slow.label}. ` : ''}${shownSeries.map((k) => `${series[k].name}: ${readOf(k, i)}`).join('; ')}`;
  };

  function panelAt(py: number): number | null {
    for (let i = 0; i < series.length; i++) if (py >= rowTop(i) - 22 && py <= rowTop(i) + panelH + 14) return i;
    return null;
  }
  /** `off` is where the chart's view starts: 0 for the medians, which have no panels, B0 for the panels. */
  function hover(e: PointerEvent, off: number) {
    const box = (e.currentTarget as SVGElement).getBoundingClientRect();
    const px = e.clientX - box.left;
    const py = e.clientY - box.top + off;
    const i = slotAt(px, PAD_L, pw, N);
    if (i == null || i >= edge) return leave();
    cursor = i;
    const p = off ? panelAt(py) : null;
    sel = p;
    if (!off) near = nearest(i, py);
    if (p == null) tip = null;
    else {
      const m = panels[p].marks.find((k) => Math.abs(k.x - px) < 6 && py - panels[p].top < 10);
      tip = { x: e.clientX, y: box.top + panels[p].top - off, text: m ? `Operator reported, ${formatTime(m.t)}: ${m.title}` : summary(p, i) };
    }
  }
  /** The series whose median at slot `i` is closest to height `py`, if within reach of the pointer. */
  function nearest(i: number, py: number): number | null {
    if (many || py < OV_TOP - 8 || py > OV_BOTTOM + 8) return null;
    let best: number | null = null;
    let gap = 28;
    for (const t of together) {
      const y = t.ys[i];
      if (y != null && Math.abs(y - py) < gap) [best, gap] = [t.i, Math.abs(y - py)];
    }
    return best;
  }
  function leave() {
    tip = null;
    cursor = null;
    sel = null;
    near = null;
  }
  function key(e: KeyboardEvent) {
    if (!N) return;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight' || e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      const at = cur ?? lastSlot;
      cursor = e.key === 'Home' ? 0 : e.key === 'End' ? lastSlot : Math.min(edge - 1, Math.max(0, at + (e.key === 'ArrowLeft' ? -1 : 1)));
      live = plain(cursor);
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      const d = e.key === 'ArrowUp' ? -1 : 1;
      sel = Math.min(series.length - 1, Math.max(0, (sel ?? (d > 0 ? -1 : series.length)) + d));
      cursor ??= lastSlot;
      live = plain(cursor);
    }
  }
  function focus() {
    cursor ??= lastSlot;
    live = plain(cursor);
  }
  const label = $derived(
    `Response time over time, one ${unit} per step. ${series.map((s) => `${s.name}${s.sub ? `, ${s.sub}` : ''}`).join('; ')}. Left and right arrows read each ${unit}${many ? '; up and down arrows pick a series' : ''}.`,
  );
  const labelEach = $derived(`Each series on its own panel, one ${unit} per step. Left and right arrows read each ${unit}; up and down arrows pick a series.`);
</script>

{#snippet shape(i: number, x: number, y: number)}
  {#if i % 4 === 0}<circle class="m" cx={x} cy={y} r="3.6" />
  {:else if i % 4 === 1}<rect class="m" x={x - 3.3} y={y - 3.3} width="6.6" height="6.6" />
  {:else if i % 4 === 2}<path class="m" d="M{x},{y - 4.2}L{x + 4.2},{y + 3.4}L{x - 4.2},{y + 3.4}Z" />
  {:else}<path class="m" d="M{x},{y - 4.4}L{x + 4.4},{y}L{x},{y + 4.4}L{x - 4.4},{y}Z" />{/if}
{/snippet}

<!-- The time axis along y, with the cursor from top down to it and the time pill under it. -->
{#snippet timeAxis(y: number, top: number)}
  {#each axis as a}
    <line class="axis" x1={a.x} x2={a.x} y1={y} y2={y + 4} />
    {#if cur == null || Math.abs(a.x - pillX) > pillW / 2 + a.label.length * 3.2 + 4}
      <text class="tick" x={a.x} y={y + 18} text-anchor="middle">{a.label}</text>
    {/if}
  {/each}
  <line class="axis" x1={PAD_L} x2={PAD_L + pw} y1={y} y2={y} />
  {#if cur != null}
    <rect class="cur" x={xb(cur)} y={top} width={Math.max(2, slotW)} height={y - top} />
    <rect class="pill" x={pillX - pillW / 2} y={y + 6} width={pillW} height="18" rx="4" />
    <text class="pilltext" x={pillX} y={y + 19} text-anchor="middle">{pillText}</text>
  {/if}
{/snippet}

<div class="ot" bind:clientWidth={W} bind:this={ot} style:height={held ? `${OV_BOTTOM}px` : undefined} style:overflow={held ? 'clip' : undefined}>
  {#if W > 0 && grid && series.length}
    {#if switching}
      <div class="halves" style:right="{padR}px" style:opacity={chartIn} bind:clientWidth={switchW}>
        <span id="{uid}-halves">Variation</span>
        <div class="segmented" role="group" aria-labelledby="{uid}-halves">
          <button aria-pressed={shade === 'all'} onclick={() => setShade('all')}>All</button>
          <button aria-pressed={shade === 'nearest'} onclick={() => setShade('nearest')}>Nearest</button>
        </div>
      </div>
    {/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <svg
      width={W}
      height={HA}
      viewBox="0 0 {W} {HA}"
      role="group"
      tabindex="0"
      aria-label={label}
      onpointermove={(e) => hover(e, 0)}
      onpointerleave={leave}
      onkeydown={key}
      onfocus={focus}
    >
      <defs>
        <pattern id="{uid}-hatch" width="5" height="5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
          <rect width="5" height="5" class="hatch-bg" />
          <line x1="0" y1="0" x2="0" y2="5" class="hatch" />
        </pattern>
        <!-- The warm-up's bubbles in the panels, one in four tinted by the kind of name asked; they drift toward the plot while the run sends. -->
        <pattern id="{uid}-bubbles" x={PAD_L} y={OV_TOP} width="48" height="144" patternUnits="userSpaceOnUse">
          {#each BUBBLES as [cx, cy, r, tint]}<circle {cx} {cy} {r} class="bub {tint}" />{/each}
          {#if growing && !reduceMotion}<animateTransform attributeName="patternTransform" type="translate" from="0 0" to="48 0" dur="{48 / FEED.v}s" repeatCount="indefinite" />{/if}
        </pattern>
        <pattern id="{uid}-comb" width="4" height="5" patternUnits="userSpaceOnUse">
          <line x1="0.5" y1="0" x2="0.5" y2="5" class="comb" />
        </pattern>
        <clipPath id="{uid}-reveal"><rect x="0" y="0" width={revealX} height={H} /></clipPath>
        <clipPath id="{uid}-reveal-slot"><rect x="0" y="0" width={revealX + (drawing ? slotW / 2 : 0)} height={H} /></clipPath>
        {#if pf}
          <clipPath id="{uid}-pour"><rect x={pf.clip.x} y="0" width={pf.clip.w} height={OV_BOTTOM} rx={pf.clip.rx} /></clipPath>
          <clipPath id="{uid}-liquid"><path d={pf.d} /></clipPath>
        {:else if warm}
          <clipPath id="{uid}-feed"><rect x={warm.x0} y="0" width={W} height={OV_BOTTOM} /></clipPath>
        {/if}
      </defs>

      {#each gapBoxes as b}
        {#if b.g.soft}
          {#if !pf}
            <rect class="gap liquid" x={b.x0} y={OV_TOP} width={Math.max(3, b.x1 - b.x0)} height={OV_H} />
            <line class="ref lead" x1={b.x1} x2={b.x1} y1={OV_TOP} y2={OV_BOTTOM} />
          {/if}
        {:else}
          <g opacity={chartIn}>
            <rect class="gap" x={b.x0} y={OV_TOP} width={Math.max(3, b.x1 - b.x0)} height={OV_H} fill="url(#{uid}-hatch)" />
            <line class="ref" x1={b.x0} x2={b.x0} y1={OV_TOP} y2={OV_BOTTOM} />
            <line class="ref" x1={b.x1} x2={b.x1} y1={OV_TOP} y2={OV_BOTTOM} />
            {#if b.named}
              <line class="ref" x1={b.x0} x2={b.x0} y1={OV_TOP - 14} y2={OV_TOP} />
              <text class="note" x={b.tx} y={OV_TOP - 6} text-anchor={b.anchor}>{b.g.label}</text>
            {/if}
          </g>
        {/if}
      {/each}
      {#if pf}
        <!-- The vessel's liquid on its way into the sliver: tint and its waterline. -->
        <g class="gap" clip-path="url(#{uid}-pour)">
          <path class="liquid" d={pf.d} />
          <path class="waterline" d={pf.surface} opacity={1 - (settled ?? 1)} />
        </g>
        <rect class="frame" x={pf.frame.x} y={pf.frame.y} width={pf.frame.w} height={pf.frame.h} rx={pf.frame.rx} opacity={pf.frame.op} />
        <line class="ref lead" x1={pf.wall.x} x2={pf.wall.x} y1={pf.wall.y} y2={OV_BOTTOM} opacity={pf.wall.op} />
      {/if}
      {#if warm && feed}
        <!-- The warm-up's dots, under the lines they feed. -->
        <g class="gap" clip-path="url(#{uid}-{pf ? 'liquid' : 'feed'})">
          {#each feed.dots as b, i (i)}
            {@const to = intake(b.y)}
            <!-- Poured in already leaning toward its line, where the flow picks it up. -->
            {@const [cx, cy] = pf ? pf.at((b.x - warm.x0) / (warmEnd - warm.x0), (OV_BOTTOM - b.y - feedDip(b.x, feed.from, warmEnd, to.dy)) / OV_H) : [b.x, b.y]}
            <g use:flow={{ on: feeding, still, x: b.x, dy: to.dy, along: to.along, p: b.p }}>
              <circle class="bub {b.tint}" {cx} {cy} r={b.r} use:hang={{ on: growing && !reduceMotion, i }} />
            </g>
          {/each}
        </g>
      {/if}
      <g opacity={chartIn}>
      {#each boxes.slows as b}
        <rect class="slow" x={b.x0} y={OV_TOP} width={b.w} height={OV_H} />
        {#if b.family === 'host'}<rect class="cap" x={b.x0} y={OV_TOP} width={b.w} height="5" fill="url(#{uid}-comb)" />
        {:else if b.family === 'up'}<rect class="cap hollow" x={b.x0 + 0.5} y={OV_TOP + 0.5} width={b.w - 1} height="4" />
        {:else}<rect class="cap solid" x={b.x0} y={OV_TOP} width={b.w} height={b.family === 'net' ? 5 : 1} />{/if}
        {#if b.named}
          <line class="ref" x1={b.x0} x2={b.x0} y1={OV_TOP - 14} y2={OV_TOP} />
          <text class="note" x={b.tx} y={OV_TOP - 6} text-anchor={b.anchor}>{b.g.label}</text>
        {/if}
      {/each}

      <!-- Medians together -->
      <text class="rowname" x={PAD_L} y="12">Medians together<tspan class="sub" dx="8">{clip(caption, captionFit)}</tspan></text>
      <text class="tick" x={PAD_L - 8} y={OV_TOP - 6} text-anchor="end">ms</text>
      {#each ticksOv as g}
        <line class="grid" x1={PAD_L} x2={PAD_L + pw} y1={yOv(g)} y2={yOv(g)} />
        <text class="tick" x={PAD_L - 8} y={yOv(g) + 3.5} text-anchor="end">{tickLabel(g)}</text>
      {/each}
      <g clip-path={growing ? `url(#${uid}-reveal)` : undefined}>
        <g class="bands">
          {#each together as t (t.i)}
            {#if t.band && bandOn(t.i)}
              <path class="ov-band" class:blend class:hot={blend && hot === t.i} class:faded={blend && hot != null && hot !== t.i} style:--c={colour(t.i)} d={t.band} />
            {/if}
          {/each}
        </g>
        {#each together as t (t.i)}
          {#if t.d && !(many && sel === t.i)}
            <path class="ln" class:neutral={many} class:dim={hot != null && hot !== t.i} style:--c={many ? 'var(--s-neutral)' : colour(t.i)} d={t.d} />
          {/if}
        {/each}
      </g>
      {#each gapBoxes as b}
        {#each b.inside as line, i}
          <text class="note in" x={(b.x0 + b.x1) / 2} y={OV_TOP + 14 + i * 12} text-anchor="middle" opacity={settled}>{line}</text>
        {/each}
      {/each}
      {#if many && sel != null && together[sel]?.d}<path class="ln pick" d={together[sel].d} clip-path={growing ? `url(#${uid}-reveal)` : undefined} />{/if}
      <g opacity={landed}>
      {#each ends as e (e.i)}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <g
          style:--c={colour(e.i)}
          class="end"
          class:dim={hot != null && hot !== e.i}
          role="group"
          tabindex="0"
          aria-label="{series[e.i].name}: variation shown while focused"
          onpointerenter={() => (pick = e.i)}
          onpointerleave={() => (pick = null)}
          onfocus={() => (pick = e.i)}
          onblur={() => (pick = null)}
        >
          {#if wide}
            <rect class="hit" x={e.x + 2} y={e.labelY - 9} width={padR - 4} height="18" />
            {@render shape(e.i, e.x + 10, e.labelY)}
            <text class="endlabel" x={e.x + 20} y={e.labelY + 4}>{clip(series[e.i].name, Math.floor((padR - 24) / 6.4))}</text>
          {:else}
            {@render shape(e.i, e.x, e.y)}
          {/if}
        </g>
      {/each}
      </g>

      {@render timeAxis(OV_BOTTOM, OV_TOP)}
      </g>
    </svg>

    <button class="each" style:opacity={chartIn} aria-expanded={open} aria-controls="{uid}-each" onclick={() => (open = !open)}>
      <Icon name={open ? 'chevron-down' : 'chevron-right'} size={12} />{open ? 'Hide each resolver' : `Show each resolver (${series.length})`}
    </button>

    {#if open}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
      <svg
        id="{uid}-each"
        width={W}
        height={H - B0}
        viewBox="0 {B0} {W} {H - B0}"
        role="group"
        tabindex="0"
        aria-label={labelEach}
        onpointermove={(e) => hover(e, B0)}
        onpointerleave={leave}
        onkeydown={key}
        onfocus={focus}
      >
        {#if sel != null && panels[sel]}
          <rect class="sel" x="0" y={panels[sel].top - 22} width={W} height={panelH + 36} />
        {/if}

        {#each gapBoxes as b}
          {#if b.g.soft}
            {#each panels as p (p.i)}
              <rect class="gap liquid" x={b.x0} y={p.top} width={Math.max(3, b.x1 - b.x0)} height={panelH} />
              <rect class="gap" x={b.x0} y={p.top} width={Math.max(3, b.x1 - b.x0)} height={panelH} fill="url(#{uid}-bubbles)" />
              <line class="ref lead" x1={b.x1} x2={b.x1} y1={p.top} y2={p.bottom} />
            {/each}
          {:else}
            <rect class="gap" x={b.x0} y={BT} width={Math.max(3, b.x1 - b.x0)} height={lastBottom - BT} fill="url(#{uid}-hatch)" />
            <line class="ref" x1={b.x0} x2={b.x0} y1={BT} y2={lastBottom} />
            <line class="ref" x1={b.x1} x2={b.x1} y1={BT} y2={lastBottom} />
          {/if}
        {/each}
        {#each boxes.slows as b}
          <rect class="slow" x={b.x0} y={BT} width={b.w} height={lastBottom - BT} />
        {/each}

        {#each panels as p (p.i)}
          {@const s = series[p.i]}
          <g style:--c={colour(p.i)}>
            {#if !many}{@render shape(p.i, PAD_L + 4, p.top - 14)}{/if}
            <text class="rowname" x={PAD_L + (many ? 0 : 14)} y={p.top - 10}>{s.name}{#if s.sub}<tspan class="sub" dx="8">{clip(s.sub, Math.max(8, Math.floor(pw / 6.2) - s.name.length - 4))}</tspan>{/if}</text>
            {#each minorP as g}<line class="grid minor" x1={PAD_L} x2={PAD_L + pw} y1={p.y(g)} y2={p.y(g)} />{/each}
            {#each ticksP as g}
              <line class="grid" x1={PAD_L} x2={PAD_L + pw} y1={p.y(g)} y2={p.y(g)} />
              <text class="tick" x={PAD_L - 8} y={p.y(g) + 3.5} text-anchor="end">{tickLabel(g)}</text>
            {/each}
            {#each p.empty as [a, z]}<rect class="empty" x={xb(a)} y={p.top} width={(z - a) * slotW} height={panelH} />{/each}
            <g clip-path={growing ? `url(#${uid}-reveal-slot)` : undefined}>
              {#if p.bandOuter}<path class="band2" d={p.bandOuter} />{/if}
              {#if p.bandInner}<path class="band1" d={p.bandInner} />{/if}
              {#each p.outs as o}
                <rect class="out" class:shared={o.shared} x={o.x0} y={p.top} width={o.x1 - o.x0} height={panelH} />
                <line class="out-edge" class:shared={o.shared} x1={o.x0} x2={o.x0} y1={p.top} y2={p.bottom} />
              {/each}
            </g>
            {#if p.median}
              <g clip-path={growing ? `url(#${uid}-reveal)` : undefined}>
                <path class="med-ring" d={p.median} />
                <path class="med" d={p.median} />
              </g>
            {/if}
            <g clip-path={growing ? `url(#${uid}-reveal-slot)` : undefined}>
              {#each p.thin as k}<rect class="thin" x={xb(k)} y={p.top} width={slotW} height={panelH + 14} fill="url(#{uid}-hatch)" />{/each}
            </g>
            {#each p.marks as m}<circle class="mark" cx={m.x} cy={p.top + 5} r="3.5" />{/each}
            <rect class="rail" x={PAD_L} y={p.bottom + 6} width={pw} height="8" />
            <text class="tick" x={PAD_L - 8} y={p.bottom + 13.5} text-anchor="end">lost</text>
            <g clip-path={growing ? `url(#${uid}-reveal-slot)` : undefined}>
              {#each p.bars as b}<rect class="loss {b.cls}" x={b.x + 0.5} y={p.bottom + 14 - b.h} width={Math.max(1.5, slotW - 1)} height={b.h} />{/each}
            </g>
            {#if wide}<text class="railnote" x={PAD_L + pw + 10} y={p.bottom + 14}>{p.lost.toLocaleString()} lost</text>{/if}
          </g>
        {/each}

        {@render timeAxis(lastBottom, BT)}
      </svg>
    {/if}

    <!-- One row per series at rest and on hover alike, so picking a bin never moves the page. -->
    <div class="read" style:opacity={chartIn} class:picking={many && sel != null} aria-hidden="true">
      <p class="rhead">
        {#if cur != null}<b class="num">{formatTime(startOf(cur))}</b> · one {unit}{:else if totalsLabel}<b>{totalsLabel}</b> · p{hint}{:else}P{hint}{/if}
      </p>
      {#each series as s, k (k)}
        {@const r = readParts(k, cur)}
        <span class="rd" class:on={many && sel === k} style:--c={colour(k)}>
          <span class="who">{#if !many}<svg class="sw" width="12" height="12" viewBox="-6 -6 12 12">{@render shape(k, 0, 0)}</svg>{/if}{s.name}</span>
          <span class="num med">{r[0]}</span>
          <span class="more">{r[1]}</span>
        </span>
      {/each}
    </div>
    <p class="visually-hidden" aria-live="polite">{live}</p>
  {/if}
</div>

{#if tip}
  <div class="tip chart-tip" style:left="{tip.x}px" style:top="{tip.y}px" role="tooltip">{tip.text}</div>
{/if}

<style>
  .ot {
    position: relative;
    width: 100%;
    /* The warm-up's liquid, the same as the vessel's that pours into it. */
    --liquid: color-mix(in oklch, var(--ink) 5%, var(--surface));
  }
  /* The middle-half switch sits on the medians title row, right-aligned to the plot. */
  .halves {
    position: absolute;
    top: 0;
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .halves .segmented button {
    height: 18px;
    padding: 0 8px;
  }
  svg[role='group'] {
    display: block;
    overflow: visible;
    border-radius: var(--r-sm);
  }
  svg[role='group']:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 4px;
  }
  text {
    fill: var(--ink);
    font-family: inherit;
  }
  .tick,
  .railnote,
  .note {
    font-size: var(--t-2xs);
    fill: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }
  .note {
    fill: var(--ink-2);
  }
  .rowname {
    font-size: var(--t-sm);
    font-weight: 600;
  }
  .sub {
    font-size: var(--t-xs);
    font-weight: 400;
    fill: var(--ink-3);
  }
  .endlabel {
    font-size: var(--t-xs);
    font-weight: 600;
  }
  .m {
    fill: var(--c);
    stroke: var(--surface);
    stroke-width: 1;
  }
  .grid {
    stroke: var(--rule);
    stroke-width: 1;
  }
  .grid.minor {
    stroke-opacity: 0.5;
  }
  .axis {
    stroke: var(--rule-2);
    stroke-width: 1;
  }
  .ref {
    stroke: var(--ink-3);
    stroke-width: 1;
    stroke-opacity: 0.6;
  }
  .sel {
    fill: var(--surface-2);
    fill-opacity: 0.6;
  }
  .hatch-bg {
    fill: var(--surface-2);
  }
  .hatch {
    stroke: var(--ink-3);
    stroke-width: 1;
    stroke-opacity: 0.55;
  }
  .gap {
    pointer-events: none;
  }
  .bub {
    fill: none;
    stroke: var(--ink-3);
    stroke-width: 1;
    stroke-opacity: 0.55;
  }
  .bub.c {
    fill: var(--s-cached);
    stroke: none;
  }
  .bub.u {
    fill: var(--s-uncached);
    stroke: none;
  }
  .ref.lead {
    stroke-dasharray: 2 3;
  }
  .liquid {
    fill: var(--liquid);
  }
  .waterline {
    fill: none;
    stroke: var(--ink-3);
    stroke-width: 1;
  }
  .frame {
    fill: none;
    stroke: var(--rule);
    stroke-width: 1;
  }
  .note.in {
    paint-order: stroke;
    stroke: var(--liquid);
    stroke-width: 4px;
    stroke-linejoin: round;
  }
  .slow {
    fill: var(--ink);
    fill-opacity: 0.06;
    pointer-events: none;
  }
  .cap {
    pointer-events: none;
  }
  .cap.solid {
    fill: var(--ink-2);
  }
  .cap.hollow {
    fill: none;
    stroke: var(--ink-2);
    stroke-width: 1;
  }
  .comb {
    stroke: var(--ink-2);
    stroke-width: 1;
  }
  .empty {
    fill: var(--surface-2);
  }
  .thin {
    opacity: 0.85;
    pointer-events: none;
  }
  .ln,
  .med,
  .med-ring {
    fill: none;
    stroke-linejoin: round;
    stroke-linecap: round;
    shape-rendering: geometricPrecision;
  }
  .ln {
    stroke: var(--c);
    stroke-width: 1.75;
  }
  .ln.neutral {
    stroke-width: 1;
    stroke-opacity: 0.7;
  }
  .ln.dim,
  .end.dim {
    opacity: 0.35;
  }
  .ov-band {
    fill: var(--c);
    fill-opacity: 0.16;
    pointer-events: none;
    transition: fill-opacity 120ms;
  }
  /* Bands blend with each other only, so overlaps deepen without muddying the grid. */
  .bands {
    isolation: isolate;
  }
  .ov-band.blend {
    fill-opacity: 0.14;
    mix-blend-mode: multiply;
  }
  .ov-band.blend.hot {
    fill-opacity: 0.3;
  }
  .ov-band.blend.faded {
    fill-opacity: 0.05;
  }
  :global(:root[data-theme='dark']) .ov-band.blend {
    mix-blend-mode: screen;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root:not([data-theme='light'])) .ov-band.blend {
      mix-blend-mode: screen;
    }
  }
  .end {
    outline: none;
  }
  .end:focus-visible .endlabel {
    text-decoration: underline;
  }
  .hit {
    fill: transparent;
  }
  .ln.pick {
    stroke: var(--ink);
    stroke-width: 2;
  }
  .med-ring {
    stroke: var(--surface);
    stroke-width: 4;
  }
  .med {
    stroke: var(--c);
    stroke-width: 1.5;
  }
  .band2 {
    fill: var(--c);
    fill-opacity: 0.13;
  }
  .band1 {
    fill: var(--c);
    fill-opacity: 0.26;
  }
  .out {
    fill: var(--critical);
    fill-opacity: 0.16;
  }
  .out-edge {
    stroke: var(--critical);
    stroke-width: 1.5;
  }
  .out.shared {
    fill: var(--ink-3);
  }
  .out-edge.shared {
    stroke: var(--ink-3);
  }
  .mark {
    fill: var(--surface);
    stroke: var(--ink-2);
    stroke-width: 1.5;
    pointer-events: none;
  }
  .rail {
    fill: var(--surface-2);
  }
  .loss {
    shape-rendering: crispEdges;
  }
  .loss.up {
    fill: var(--rule-2);
  }
  .loss.some {
    fill: var(--warning);
  }
  .loss.most {
    fill: var(--serious);
  }
  .loss.down {
    fill: var(--critical);
  }
  .loss.shared {
    fill: var(--ink-3);
  }
  .cur {
    fill: var(--ink);
    fill-opacity: 0.09;
    pointer-events: none;
  }
  .pill {
    fill: var(--ink);
  }
  .pilltext {
    fill: var(--surface);
    font-size: var(--t-2xs);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .read {
    margin-top: 6px;
    font-size: var(--t-xs);
    color: var(--ink-2);
    display: grid;
    grid-template-columns: max-content max-content minmax(0, 1fr);
    gap: 2px 14px;
    align-items: center;
  }
  .rhead {
    grid-column: 1 / -1;
    margin: 0 0 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .read b {
    color: var(--ink);
  }
  .each {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin: 2px 0 6px;
    padding: 0;
    border: 0;
    background: none;
    font-size: var(--t-xs);
    color: var(--ink-2);
    text-decoration: underline;
    text-decoration-color: var(--rule-2);
  }
  .each:hover {
    color: var(--ink);
    text-decoration-color: var(--ink-3);
  }
  .rd {
    display: contents;
  }
  .rd .who {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    color: var(--ink);
    font-weight: 600;
    white-space: nowrap;
  }
  .rd .med {
    min-width: 8ch;
    text-align: right;
    color: var(--ink);
  }
  .rd .more {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .picking .rd:not(.on) > * {
    color: var(--ink-3);
  }
  .sw {
    flex: none;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .tip {
    color: var(--ink);
    padding: 5px 9px;
  }
</style>
