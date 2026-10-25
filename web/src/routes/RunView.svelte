<script lang="ts">
  import { untrack } from 'svelte';
  import { app, RUN_VIEWS, type RunView } from '../lib/store.svelte';
  import { selectKey } from '../lib/select';
  import { shortcut } from '../lib/keys';
  import { HIT_SHARE, blendRows, buildRows, rank, verdict, type Row, type SortKey } from '../lib/verdict';
  import type { RunState, TestKind, Transport } from '../lib/types';
  import { TEST_HINT, TEST_LABEL, TRANSPORT_LABEL, duration, label, ms, plural, checkMs, runEstimateMs, when } from '../lib/format';
  import { menu, SEPARATOR } from '../lib/menu.svelte';
  import { reduceMotion } from '../lib/motion';
  import { isCheck, phaseTab } from '../lib/phases';
  import { status } from '../lib/status.svelte';
  import { onResolverContext } from '../lib/resolverMenu';
  import Toolbar from '../components/Toolbar.svelte';
  import RunProgress from '../components/RunProgress.svelte';
  import Verdict from '../components/Verdict.svelte';
  import RankList from '../components/RankList.svelte';
  import Ladder from '../components/Ladder.svelte';
  import CapMatrix, { type CapView } from '../components/CapMatrix.svelte';
  import LatencyTable from '../components/LatencyTable.svelte';
  import CdnPanel from '../components/CdnPanel.svelte';
  import CdnLive from '../components/CdnLive.svelte';
  import PathLive from '../components/PathLive.svelte';
  import ProbesLive from '../components/ProbesLive.svelte';
  import Inspector from '../components/Inspector.svelte';
  import Shortlist, { type Pick } from '../components/Shortlist.svelte';
  import Info from '../components/Info.svelte';
  import Icon from '../components/Icon.svelte';
  import { mobile } from '../lib/api';
  import { showConditions } from '../components/InstrumentStrip.svelte';

  const run = $derived(app.run);
  const running = $derived(run?.status === 'running');
  const live = $derived(run ? buildRows(run, app.boot?.resolvers ?? [], HIT_SHARE[app.mix]) : []);
  // While measuring, every view takes a fresh copy of the rows at most every 2 s, so they settle instead of
  // jittering, and the order and the numbers always come from the same copy.
  const SNAP_MS = 2000;
  let snapshot = $state.raw<Row[] | null>(null);
  let lastSnap = 0;
  $effect(() => {
    const current = live;
    if (!running) {
      snapshot = null;
      return;
    }
    // Reads of the snapshot must not make this effect depend on its own output.
    untrack(() => {
      const now = Date.now();
      if (!snapshot || now - lastSnap > SNAP_MS || snapshot.length !== current.length) {
        lastSnap = now;
        snapshot = current;
      }
    });
  });
  // Between copies the views glide from the last one to the newest over the same 2 s, frame by frame, so bars,
  // scores and counts move steadily like a live feed instead of stepping every 2 s.
  let glided = $state.raw<Row[] | null>(null);
  $effect(() => {
    const to = snapshot;
    // A new copy mid-glide starts from wherever the glide had got to.
    const from = untrack(() => glided);
    if (!to || !from || reduceMotion) {
      glided = to;
      return;
    }
    const start = performance.now();
    let id = requestAnimationFrame(function step(now) {
      const t = Math.min(1, (now - start) / SNAP_MS);
      glided = blendRows(from, to, t);
      if (t < 1) id = requestAnimationFrame(step);
    });
    return () => cancelAnimationFrame(id);
  });
  const rows = $derived(running && glided ? glided : live);

  let test = $state<TestKind | 'all'>('all');
  let sortKey = $state<SortKey>('rank');
  // Each transport is its own row, so a resolver measured over several appears several times.
  const transports = $derived([...new Set(rows.map((r) => r.target.transport))] as Transport[]);
  const multi = $derived(transports.length > 1);
  // Wire floors by resolver, for the ladder; null until the path phase has measured one.
  const floors = $derived.by(() => {
    const out: Record<string, number> = {};
    for (const [id, p] of Object.entries(run?.paths ?? {})) if (p.rtt_ms != null) out[id] = p.rtt_ms;
    return Object.keys(out).length ? out : null;
  });
  let transport = $state<Transport | 'all'>('all');
  const shown = $derived(transport === 'all' ? rows : rows.filter((r) => r.target.transport === transport));

  const ordered = $derived(rank(shown, sortKey, test));

  const v = $derived(run && !running ? verdict(run, rows, HIT_SHARE[app.mix]) : null);
  let capView = $state<CapView>('all');

  // Screen readers hear the result when a run finishes while it is open, not when an old run is opened.
  let finished = $state<string | null>(null);
  let prev: { id: string; running: boolean } | null = null;
  $effect(() => {
    const cur = { id: run?.id ?? '', running };
    untrack(() => {
      if (prev?.id === cur.id && prev.running && !cur.running) finished = cur.id;
      prev = cur;
    });
  });
  const announce = $derived(v && finished === run?.id ? `Run finished. ${v.sentence}` : '');
  // Whether anything records the recommended resolver's reliability. Not having a record isn't a clean
  // record, and operators that publish no status page would otherwise win by saying nothing.
  $effect(() => {
    status.load();
    status.loadRecorded();
  });
  const caution = $derived.by(() => {
    const best = v?.best;
    const r = best && run?.resolvers.find((x) => x.id === best.target.resolver_id);
    if (!best || !r || status.hasRecord(r) !== false) return null;
    const why = r.status_feed ? 'its status feed couldn’t be read' : 'it publishes no status feed';
    return `${label(best.target.provider, best.target.variant)}: ${why} and you haven’t monitored it from this network, so how often it fails is unknown. Fastest isn’t the same as dependable.`;
  });
  // What a live run is doing before it has anything to show.
  const waiting = $derived(
    !running
      ? ''
      : run?.phase === 'setup'
        ? 'Setting up: checking your network and reaching each resolver'
        : run?.phase === 'warmup'
          ? 'Warming up: a few unrecorded queries prime each resolver’s cache and network path'
          : 'First answers are coming in; a leader shows once every resolver has answered a few times',
  );
  const placeholders = $derived(run && !run.targets.length ? run.resolvers.length || run.config.resolvers.length || 6 : 0);
  const leader = $derived(running ? rank(rows).find((r) => !r.baseline && (r.eligible || (r.measured && !r.intercepted))) : null);
  // The checks after the latency rounds: the ranking holds, the picks still wait on them.
  const checking = $derived(running && isCheck(run?.phase ?? ''));
  const liveVerdict = $derived.by(() => {
    if (!run || !running) return null;
    // Name a leader only once every target has answered and the leader has a few answers, not after one lucky query.
    const settling = rows.some((r) => !r.measured && (r.target.state === 'pending' || r.target.state === 'ready'));
    const answers = leader ? Object.values(leader.dist).reduce((n, d) => n + d.n, 0) : 0;
    if (leader && leader.composite != null && !settling && answers >= 3) {
      const who = label(leader.target.provider, leader.target.variant) + (multi ? ` over ${TRANSPORT_LABEL[leader.target.transport]}` : '');
      return {
        sentence: `${who} leads so far.`,
        detail: checking
          ? `${ms(leader.composite)} speed score over every round. The picks settle as the remaining checks finish.`
          : `${ms(leader.composite)} speed score on the rounds run so far. The ranking and the picks settle once every round has run.`,
      };
    }
    const n = new Set(run.targets.map((t) => t.resolver_id)).size || run.config.resolvers.length;
    return { sentence: `Measuring ${plural(n, 'resolver')} from this network.`, detail: checking ? 'Latency is measured. The picks settle as the remaining checks finish.' : `${waiting}.` };
  });
  const warns = $derived(run?.notes.filter((n) => n.level === 'warn') ?? []);
  const infos = $derived(run?.notes.filter((n) => n.level === 'info') ?? []);
  const inspected = $derived(app.inspect ? run?.resolvers.find((r) => r.id === app.inspect) : undefined);

  // Toolbar text.
  let now = $state(Date.now());
  $effect(() => {
    if (!running) return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });
  // When the current phase began: once the checks after the latency rounds start, time left is what remains of theirs.
  let phaseAt = $state({ phase: '', at: 0 });
  $effect(() => {
    const p = run?.phase ?? '';
    untrack(() => {
      if (p !== phaseAt.phase) phaseAt = { phase: p, at: Date.now() };
    });
  });
  // While measuring, say how long is left, and why when the run outlasts its estimate.
  function elapsed(r: RunState): string {
    const spent = now - r.started_at;
    const checks = checkMs(new Set(r.targets.map((t) => t.resolver_id)).size, r.config);
    const order = Object.keys(checks);
    const at = order.indexOf(r.phase);
    if (at >= 0) {
      const rest = Math.max(0, checks[r.phase] - (now - phaseAt.at)) + order.slice(at + 1).reduce((a, k) => a + checks[k], 0);
      return rest >= 3000 ? `${duration(spent)}, about ${duration(rest)} left` : `${duration(spent)}, finishing up`;
    }
    // Never less than the checks still to come, so time left doesn't jump when they begin.
    const later = order.reduce((a, k) => a + checks[k], 0);
    const left = runEstimateMs(r.targets.length, new Set(r.targets.map((t) => t.resolver_id)).size, r.config) - spent;
    const timeouts = r.samples.filter((s) => s.o === 'timeout').length;
    if (left >= later || !timeouts) return `${duration(spent)}, about ${duration(Math.max(left, later))} left`;
    return `${duration(spent)}, longer than estimated: ${plural(timeouts, 'query', 'queries')} waited out the timeout`;
  }
  const title = $derived(!run ? 'Results' : running ? 'Measuring' : run.status === 'cancelled' ? 'Run stopped' : 'Results');
  const subtitle = $derived(
    run
      ? [
          when(run.started_at),
          plural(new Set(run.targets.map((t) => t.resolver_id)).size, 'resolver'),
          `${run.samples.length.toLocaleString()} queries`,
          running ? elapsed(run) : duration((run.finished_at ?? now) - run.started_at),
          run.status === 'cancelled' ? 'partial results' : '',
        ]
          .filter(Boolean)
          .join(' · ')
      : '',
  );

  // The tab whose content is filling while the run works: a dot marks it, so a view the run isn't on still says where
  // the action is.
  // Intel ends by naming the CDN edges' networks, which shows on the CDN view.
  const liveTab = $derived(running && run ? (run.phase === 'intel' ? (run.cdn ? 'cdn' : 'caps') : (phaseTab(run.phase) ?? 'latency')) : null);

  // The phone folds Numbers into Overview.
  const PHONE_VIEWS = RUN_VIEWS.filter((v) => v !== 'numbers');
  $effect(() => {
    if (mobile && app.runView === 'numbers') app.setRunView('overview');
  });

  const VIEW_LABEL: Record<RunView, string> = {
    overview: 'Overview',
    latency: 'Latency',
    path: 'Path',
    cdn: 'CDN',
    caps: 'Capabilities',
    numbers: 'Numbers',
  };

  const tabs = $derived<(TestKind | 'all')[]>(['all', ...(run?.config.tests ?? [])]);
  function onKey(e: KeyboardEvent) {
    const tag = (e.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA' || menu.open) return;
    // ⌥⌘1–6 (Ctrl+Shift+1–6 on Windows) switch views; the desktop menu sends the same commands.
    if (((e.altKey && e.metaKey) || (e.ctrlKey && e.shiftKey)) && /^Digit[1-6]$/.test(e.code)) {
      e.preventDefault();
      app.setRunView(RUN_VIEWS[Number(e.code.slice(5)) - 1]);
      return;
    }
    if (e.key === 'Escape' && app.inspect) {
      app.inspect = null;
      return;
    }
    if (selectKey(e)) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const n = Number(e.key);
    if (app.runView === 'latency' && n >= 1 && n <= tabs.length) test = tabs[n - 1];
  }
  const open = (id: string) => (app.inspect = id);

  // The run's row in history, for its saved mark.
  const row = $derived(run ? (app.boot?.runs.find((r) => r.id === run.id) ?? null) : null);

  // Everything else about this run, under one "…": the same menu a run has in the sidebar.
  // Phone: the bar keeps only back, the title and this menu, so saving, a new run and the run's measurement
  // conditions move in here.
  function runMenu(e: MouseEvent, id: string) {
    const phone = mobile
      ? [
          { id: 'new', label: 'New Run' },
          ...(row ? [{ id: 'save', label: row.saved ? 'Unsave Run' : 'Save Run' }] : []),
          { id: 'conditions', label: 'Measurement Conditions' },
          SEPARATOR,
        ]
      : [];
    const exports = app.boot?.caps.export ? [{ id: 'csv', label: 'Export Samples as CSV…' }, { id: 'json', label: 'Export Run as JSON…' }, SEPARATOR] : [];
    menu.show(
      e,
      [
        ...phone,
        ...exports,
        { id: 'delete', label: 'Delete Run…' },
      ],
      (pick) => {
        if (pick === 'new') app.navigate('/');
        else if (pick === 'save') app.setSaved(id, !row?.saved);
        else if (pick === 'conditions') showConditions();
        else if (pick === 'delete') app.deleteRun(id);
        else app.exportRun(pick as 'csv' | 'json');
      },
      e.currentTarget as HTMLElement,
    );
  }

  // Each view starts at the top.
  let scroller: HTMLDivElement | undefined = $state();
  $effect(() => {
    app.runView;
    untrack(() => scroller?.scrollTo({ top: 0 }));
  });

  // Inspecting a resolver splits the view: the shortlist on the left, its detail on the right. Phones use a sheet.
  const split = $derived(!!inspected && !mobile);
  let bodyW = $state(1200);
  const narrow = $derived(bodyW < 720);
  // One line per resolver in ranking order, scored as the ladder sorts; a resolver on several transports lists once.
  const picks = $derived.by<Pick[]>(() => {
    const seen = new Set<string>();
    const out: Pick[] = [];
    for (const r of ordered) {
      const rid = r.target.resolver_id;
      if (seen.has(rid)) continue;
      seen.add(rid);
      out.push({ id: rid, name: label(r.target.provider, r.target.variant), score: r.composite });
    }
    return out;
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="view-root">
  <Toolbar {title} {subtitle}>
    {#snippet center()}
      {#if run && !mobile}
        <div class="segmented views" role="tablist" aria-label="Results view">
          {#each RUN_VIEWS as view, i}
            <button
              role="tab"
              aria-selected={app.runView === view}
              aria-controls="run-panel"
              title="{VIEW_LABEL[view]} ({shortcut(`alt+cmd+${i + 1}`)})"
              onclick={() => app.setRunView(view)}
              >{VIEW_LABEL[view]}{#if liveTab === view}<span class="live" aria-hidden="true"></span><span class="visually-hidden">, in progress</span>{/if}</button
            >
          {/each}
        </div>
        <label class="views-compact">
          <span class="visually-hidden">Results view</span>
          <select value={app.runView} onchange={(e) => app.setRunView((e.currentTarget as HTMLSelectElement).value as RunView)}>
            {#each RUN_VIEWS as view}<option value={view}>{VIEW_LABEL[view]}</option>{/each}
          </select>
        </label>
      {/if}
    {/snippet}
    {#snippet actions()}
      {#if run && running}
        <button class="btn" onclick={() => app.cancelRun()}><Icon name="stop" size={11} />Stop</button>
      {:else if run && mobile}
        <button class="tb-icon" aria-label="Run actions" onclick={(e) => runMenu(e, run.id)}><Icon name="more" size={18} /></button>
      {:else if run}
        {#if row}
          {@const saved = row.saved}
          <button
            class="tb-icon save"
            class:on={saved}
            aria-pressed={saved}
            aria-label={saved ? 'Unsave run' : 'Save run'}
            title={saved ? 'Saved: kept when runs are cleared. Click to unsave.' : 'Save run: keep it when runs are cleared or cleaned up'}
            onclick={() => app.setSaved(run.id, !saved)}><Icon name="bookmark" size={15} /></button
          >
        {/if}
        <button class="tb-icon" aria-label="Run actions" title="Export or delete this run" onclick={(e) => runMenu(e, run.id)}><Icon name="more" size={16} /></button>
        <button
          class="tb-icon"
          aria-label={app.inspect ? 'Hide inspector' : 'Show inspector'}
          aria-pressed={!!app.inspect}
          title="{app.inspect ? 'Hide' : 'Show'} inspector ({shortcut('alt+cmd+I')})"
          onclick={() => app.toggleInspector()}><Icon name="inspector" size={16} /></button
        >
        <button class="btn primary" onclick={() => app.navigate('/')}>New run</button>
      {/if}
    {/snippet}
  </Toolbar>

  {#if run}
    <div class="view-body" bind:clientWidth={bodyW}>
      <div class="col">
        {#if mobile}
          <div class="viewstrip" role="tablist" aria-label="Results view">
            {#each PHONE_VIEWS as view}
              <button role="tab" class="vtab" class:on={app.runView === view} aria-selected={app.runView === view} aria-controls="run-panel" onclick={() => app.setRunView(view)}
                >{VIEW_LABEL[view]}{#if liveTab === view}<span class="live" aria-hidden="true"></span><span class="visually-hidden">, in progress</span>{/if}</button
              >
            {/each}
          </div>
        {/if}
        <RunProgress {run} />
        <p class="visually-hidden" aria-live="polite">{announce}</p>
        {#if split && inspected}
          <div class="split">
            <Shortlist groups={[['', picks]]} selected={app.inspect} back="All results" collapsed={narrow} onselect={open} onback={() => (app.inspect = null)} />
            {#key inspected.id}
              <Inspector resolver={inspected} {run} {rows} onclose={() => (app.inspect = null)} />
            {/key}
          </div>
        {:else}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="content scroll"
            id="run-panel"
            role="tabpanel"
            tabindex="0"
            aria-label={VIEW_LABEL[app.runView]}
            bind:this={scroller}
            oncontextmenu={onResolverContext}
          >
            {#if app.runView === 'overview'}
              <div class="overview">
                {#each warns as n}
                  <div class="notice warn" role="status">
                    <Icon name="warn" size={16} />
                    <p>{n.text}</p>
                  </div>
                {/each}

                {#if v}
                  <Verdict {v} cdn={!!run.cdn} {caution} samples={run.samples} onopen={open} />
                  {#if mobile && v.caveats.length}
                    <details class="notes">
                      <summary>{v.caveats.length === 1 ? 'One caveat' : `${v.caveats.length} caveats`}</summary>
                      {#each v.caveats as c}<p class="caveat">{c}</p>{/each}
                    </details>
                  {:else}
                    {#each v.caveats as c}<p class="caveat">{c}</p>{/each}
                  {/if}
                  {#if mobile && rows.some((r) => r.measured)}
                    <!-- The phone has no Numbers tab: the same table sits under the answer. -->
                    <details class="notes numbers">
                      <summary>Medians and p95 for every test</summary>
                      <LatencyTable rows={ordered} tests={run.config.tests} onopen={open} />
                    </details>
                  {/if}
                {:else if running}
                  <div>
                    <Verdict v={null} live={liveVerdict} cdn={run.config.cdn} onopen={open}>
                      <RankList
                        rows={rank(rows)}
                        samples={run.samples}
                        tests={run.config.tests}
                        {multi}
                        live
                        warm={run.warm ?? []}
                        {placeholders}
                        onopen={open}
                        onmore={() => app.setRunView('latency')}
                      />
                    </Verdict>
                  </div>
                {/if}

                {#if infos.length}
                  <ul class="infos">
                    {#each infos as n}<li><Icon name="info" size={14} />{n.text}</li>{/each}
                  </ul>
                {/if}
              </div>
            {:else if app.runView === 'path'}
              {#if running && run.phase !== 'path' && !Object.keys(run.paths).length}
                <div class="empty"><p>The path check runs after the latency tests: it traces the route to each resolver and times the wire to it.</p></div>
              {:else}
                <PathLive {run} rows={ordered} />
              {/if}
            {:else if app.runView === 'latency'}
              <div class="bar chrome">
                <div class="segmented" role="group" aria-label="Test shown">
                  {#each tabs as t, i}
                    <button aria-pressed={test === t} onclick={() => (test = t)} title="Shortcut: {i + 1}">
                      {#if t !== 'all'}<i class="key {t}"></i>{/if}{t === 'all' ? 'All tests' : TEST_LABEL[t]}
                    </button>
                  {/each}
                </div>
                {#if multi}
                  <label>
                    <span class="visually-hidden">Transport shown</span>
                    <select bind:value={transport}>
                      <option value="all">All transports</option>
                      {#each transports as t}<option value={t}>{TRANSPORT_LABEL[t]} only</option>{/each}
                    </select>
                  </label>
                {/if}
                <label>
                  <span class="visually-hidden">Sort by</span>
                  <select bind:value={sortKey}>
                    <option value="rank">Ranked</option>
                    {#if multi}<option value="resolver">By resolver</option>{/if}
                    <option value="success">Answered</option>
                    <option value="p95">Slowest 5%</option>
                    <option value="name">Name</option>
                  </select>
                </label>
                <Info label="How to read the latency chart">
                  <p>
                    {test === 'all'
                      ? 'Every answered query, by test. Bars span the middle 50%, the tick marks the median, the whisker reaches the 95th percentile.'
                      : TEST_HINT[test]}
                  </p>
                  {#if run.config.tests.includes('cached') && run.config.cached_names?.length}
                    <p>
                      The Cached test asked {run.config.cached_names.length} names, in turn: <span class="mono">{run.config.cached_names.join(', ')}</span>.
                    </p>
                  {/if}
                  {#if running}
                    <p>Hollow rings are the warm-up queries, which prime caches and network paths and never count.</p>
                  {/if}
                  {#if floors}
                    <p>The thin line behind a row is the wire floor: the best TCP handshake to that resolver's address. No genuine answer lands left of it.</p>
                  {/if}
                  <p>Select a row to inspect it. Keys 1–{tabs.length} switch tests.</p>
                </Info>
              </div>
              <div class="panel ladder">
                <Ladder
                  rows={ordered}
                  samples={run.samples}
                  {test}
                  tests={run.config.tests}
                  live={running}
                  warm={run.warm ?? []}
                  {waiting}
                  {placeholders}
                  starting={running && run.phase === 'setup'}
                  multi={multi && transport === 'all'}
                  floors={floors ?? {}}
                  onopen={open}
                />
              </div>
            {:else if app.runView === 'cdn'}
              {#if running && run.config.cdn}
                <CdnLive {run} rows={ordered} />
              {:else if run.cdn?.version === 2 && Object.keys(run.cdn.resolvers).length}
                <CdnPanel {run} cdn={run.cdn} onopen={open} />
              {:else}
                <div class="empty">
                  <p>
                    {#if !run.config.cdn}
                      This run skipped CDN mapping. Turn on <b>CDN mapping quality</b> in the test plan to see which edges each resolver
                      sends you to.
                    {:else}
                      No CDN results: none of the resolvers answered the CDN lookups.
                    {/if}
                  </p>
                </div>
              {/if}
            {:else if app.runView === 'caps' && running && run.phase === 'probes'}
              <ProbesLive {run} rows={ordered} />
            {:else if app.runView === 'caps'}
              <CapMatrix {run} rows={ordered} {v} bind:view={capView} onopen={open} />
            {:else if app.runView === 'numbers'}
              <div class="fill">
                <div class="bar chrome">
                  <p class="lead">Every target’s median and p95 for each test, in milliseconds.</p>
                  <Info label="About the table"><p>Select a column heading to sort. Select a resolver to inspect it. For every sample, export the run as CSV: Control-click it in the sidebar.</p></Info>
                </div>
                <LatencyTable rows={ordered} tests={run.config.tests} onopen={open} />
              </div>
            {/if}
          </div>
        {/if}
      </div>

      {#if inspected && mobile}
        {#key inspected.id}
          <Inspector resolver={inspected} {run} {rows} onclose={() => (app.inspect = null)} />
        {/key}
      {/if}
    </div>
  {:else if app.runError}
    <div class="view-body">
      <div class="content empty">
        <h2>Couldn't load this run</h2>
        <p class="ink-2">{app.runError} It may have been deleted, or the link came from another computer.</p>
        <button class="btn primary" onclick={() => app.navigate('/')}>Back to Benchmark</button>
      </div>
    </div>
  {:else}
    <div class="view-body"><div class="content"><p class="muted">Loading run…</p></div></div>
  {/if}
</div>

<style>
  .tb-icon.save.on :global(svg) {
    fill: currentColor;
  }
  .col {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .col > .content,
  .split {
    flex: 1;
  }
  .split {
    display: flex;
    min-height: 0;
  }
  .views button {
    min-width: 76px;
    justify-content: center;
  }
  .views-compact {
    display: none;
  }
  .views-compact select {
    height: 28px;
    font-size: var(--t-xs);
  }
  /* Phone: five views (Numbers folds into Overview) as a strip of tabs under the bar, all in view at once. */
  .viewstrip {
    flex: none;
    display: flex;
    gap: 2px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--rule);
    background: var(--surface);
  }
  .vtab {
    flex: 1 1 auto;
    min-width: 0;
    min-height: 44px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--r-md);
    background: transparent;
    color: var(--ink-2);
    font-size: var(--t-sm);
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .vtab:active {
    background: var(--surface-2);
  }
  .vtab.on {
    background: var(--side-sel);
    color: var(--ink);
    font-weight: 620;
  }
  /* The tab the live run is filling: a slow breath, still under reduced motion. */
  .live {
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-left: 6px;
    border-radius: 50%;
    background: var(--accent);
    vertical-align: 0.1em;
    animation: breathe 2400ms ease-in-out infinite;
  }
  @keyframes breathe {
    50% {
      opacity: 0.3;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .live {
      animation: none;
    }
  }
  /* Not enough room for six tabs: a pop-up button, as a Mac toolbar would. */
  @container toolbar (max-width: 900px) {
    .views {
      display: none;
    }
    .views-compact {
      display: block;
    }
  }
  .overview {
    display: grid;
    gap: 14px;
    /* Centered, so a full-screen window doesn't leave one wide empty margin. */
    max-width: 1440px;
    margin-inline: auto;
  }
  .notice {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 12px 14px;
    border-radius: var(--r-lg);
    font-size: var(--t-sm);
  }
  .notice p {
    max-width: 100ch;
  }
  .notice.warn {
    background: var(--warn-soft);
    border: 1px solid color-mix(in oklch, var(--warning) 45%, transparent);
  }
  .notice :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--warning);
  }
  .caveat {
    margin-top: -6px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .infos {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 6px;
  }
  .infos li {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    font-size: var(--t-sm);
    color: var(--ink-2);
    max-width: 110ch;
  }
  .infos :global(svg) {
    flex: none;
    margin-top: 3px;
    color: var(--ink-3);
  }
  :global(html.mobile) .overview {
    gap: 10px;
  }
  :global(html.mobile) .caveat {
    margin-top: 0;
    font-size: var(--t-sm);
  }
  .notes {
    font-size: var(--t-sm);
    color: var(--ink-3);
  }
  .notes summary {
    min-height: 40px;
    display: flex;
    align-items: center;
    color: var(--ink-2);
    cursor: default;
  }
  .notes summary::-webkit-details-marker {
    display: none;
  }
  .notes summary::before {
    content: '';
    width: 6px;
    height: 6px;
    margin-right: 10px;
    border-right: 1.5px solid currentColor;
    border-bottom: 1.5px solid currentColor;
    transform: rotate(-45deg);
    transition: transform var(--dur) var(--ease-out);
  }
  .notes[open] summary::before {
    transform: rotate(45deg);
  }
  .notes[open] summary {
    margin-bottom: 4px;
  }
  .notes p + p {
    margin-top: 8px;
  }
  .notes.numbers {
    margin-top: 8px;
    border-top: 1px solid var(--rule);
    color: var(--ink);
  }
  :global(html.mobile) .infos li {
    font-size: var(--t-sm);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  .bar select {
    height: 30px;
  }
  .lead {
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .ladder {
    padding: 4px 14px 10px;
  }
  /* Table views fill the pane; the table scrolls inside it under a fixed header. */
  .fill {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .fill > .bar {
    flex: none;
  }
  .empty {
    display: grid;
    gap: 12px;
    justify-items: start;
    align-content: start;
    padding-top: 48px;
    max-width: 64ch;
    color: var(--ink-2);
  }
  .empty h2 {
    font-size: var(--t-xl);
    color: var(--ink);
  }
  @media (max-width: 560px) {
    .ladder {
      padding: 2px 8px 8px;
    }
  }
</style>
