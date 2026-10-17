<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { api, mobile, saveExport } from '../lib/api';
  import { app, readLocal, writeLocal } from '../lib/store.svelte';
  import type { NtpEntry, NtpResult, NtpRun } from '../lib/types';
  import { plural } from '../lib/format';
  import { MAX_CUSTOM, addrs, idMap, nameOf, num, sgn } from '../lib/ntp';
  import Icon from '../components/Icon.svelte';
  import Info from '../components/Info.svelte';
  import Toolbar from '../components/Toolbar.svelte';
  import TimeMonitor from '../components/TimeMonitor.svelte';
  import TimeWorking from '../components/TimeWorking.svelte';
  import SetupDial from '../components/SetupDial.svelte';
  import { fade } from 'svelte/transition';
  import { reduceMotion } from '../lib/motion';

  type Row = { e: NtpEntry; r: NtpResult | null };
  type NtpInfo = {
    catalog: NtpEntry[];
    run: NtpRun | null;
    spacing_s: number;
    cooldown_s: number;
    max_samples: number;
    asked: Record<string, { at: number; kiss?: string | null; until?: number | null }>;
    now: number;
  };

  let catalog = $state<NtpEntry[]>([]);
  let run = $state<NtpRun | null>(null);
  let spacing = $state(8);
  let cooldown = $state(600);
  let maxSamples = $state(8);
  let asked = $state<Record<string, { at: number; kiss?: string | null; until?: number | null }>>({});
  let now = $state(Date.now());
  let picked = $state<string[]>([]);
  let samples = $state(4);
  let nts = $state(true);
  let customHosts = $state<string[]>([]);
  let customText = $state('');
  let view = $state<'accuracy' | 'features' | 'monitor'>('accuracy');
  let sort = $state<{ key: string; dir: 1 | -1 }>({ key: 'fit', dir: 1 });
  let loaded = $state(false);
  let starting = $state(false);

  const KEY = 'recursa.ntp';
  const GROUPS: [NtpEntry['kind'], string, string][] = [
    ['service', 'Public services', 'Companies and networks that run time servers for anyone.'],
    ['lab', 'National time labs', 'Servers run by national metrology institutes and observatories.'],
    ['pool', 'The NTP pool', 'Volunteers’ servers, a few handed out per lookup, grouped by region.'],
  ];

  // True until the user picks, or a saved pick is restored: then the selection is ours to set, and it follows
  // where you are once that is known.
  let untouched = $state(true);

  function remember() {
    untouched = false;
    writeLocal(KEY, { picked, samples, nts, customHosts });
  }

  async function load() {
    try {
      const r = await api.get<NtpInfo>('/api/ntp');
      catalog = r.catalog;
      run = r.run;
      spacing = r.spacing_s;
      cooldown = r.cooldown_s;
      maxSamples = r.max_samples;
      asked = r.asked;
      now = r.now;
      const saved = readLocal<{ picked?: string[]; samples?: number; nts?: boolean; customHosts?: string[] }>(KEY, {});
      const known = new Set(catalog.map((e) => e.id));
      untouched = saved.picked === undefined;
      picked = (saved.picked ?? catalog.filter((e) => e.default).map((e) => e.id)).filter((id) => known.has(id));
      samples = Math.min(saved.samples ?? 4, maxSamples);
      nts = saved.nts ?? true;
      customHosts = (saved.customHosts ?? []).filter((h) => typeof h === 'string').slice(0, MAX_CUSTOM);
      loaded = true;
    } catch (e) {
      app.notify((e as Error).message, 'error');
    }
  }
  $effect(() => {
    untrack(load);
  });

  // While a run is going, ask for its progress once a second.
  let timer: ReturnType<typeof setInterval> | null = null;
  $effect(() => {
    const live = run?.status === 'running';
    if (live && !timer) {
      let busy = false;
      timer = setInterval(async () => {
        if (busy) return;
        busy = true;
        try {
          const r = await api.get<Pick<NtpInfo, 'run' | 'asked' | 'now'>>('/api/ntp?live=1');
          run = r.run;
          asked = r.asked;
          now = r.now;
        } catch {
          /* the next tick tries again */
        } finally {
          busy = false;
        }
      }, 1000);
    } else if (!live && timer) {
      clearInterval(timer);
      timer = null;
    }
  });
  onDestroy(() => timer && clearInterval(timer));

  async function start() {
    starting = true;
    lanes = planLanes();
    remember();
    try {
      run = await api.post<NtpRun>('/api/ntp/run', { servers: picked, custom: customHosts, samples, nts });
    } catch (e) {
      app.notify((e as Error).message, 'error');
    } finally {
      starting = false;
    }
  }
  const stop = () => api.post('/api/ntp/cancel').catch(() => {});

  const running = $derived(run?.status === 'running');
  // The server list is open until there is a run to look at. It is state of its own, not `open={!run}`: that
  // is compiled into the same effect as the summary's count, so every pick would shut a list the user opened.
  const hasRun = $derived(!!run);
  let planOpen = $state(true);
  $effect(() => {
    planOpen = !hasRun;
  });
  // Each address waits a random time up to one spacing before its first query, then one spacing between queries.
  const eta = $derived(samples * spacing + 6);
  // The most one run could send: four IPv4 addresses and one IPv6 per name, every query. Most names have one.
  const budget = $derived((picked.length + customHosts.length) * 6 * samples);
  // What the closed setup will ask, by name, so it reads as a choice rather than a count.
  const chosen = $derived.by(() => {
    const names = [...picked.map((id) => nameOf(byId, id)), ...customHosts];
    return names.length > 4 ? `${names.slice(0, 3).join(', ')} and ${names.length - 3} more` : names.join(', ');
  });
  const at = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  /** When a server may next be asked, and why not yet. */
  function resting(e: NtpEntry): { until: number; kiss: string | null } | null {
    const a = asked[e.nts_only ? (e.nts ?? e.host) : e.host];
    if (!a) return null;
    if (a.until && a.until > now) return { until: a.until, kiss: a.kiss ?? null };
    const next = a.at + cooldown * 1000;
    return next > now ? { until: next, kiss: null } : null;
  }
  const resters = $derived(catalog.filter((e) => picked.includes(e.id) && resting(e)));
  const toggle = (id: string) => {
    picked = picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id];
    remember();
  };
  // The catalog names countries in English; the vantage gives a code. Anycast services answer from nearby
  // wherever you are, so they count as local everywhere.
  const myCountry = $derived.by(() => {
    const code = app.env?.vantage?.geo?.country;
    if (!code) return null;
    try {
      return new Intl.DisplayNames('en', { type: 'region' }).of(code) ?? null;
    } catch {
      return null;
    }
  });
  const nearby = $derived(myCountry ? catalog.filter((e) => e.country === myCountry || e.anycast) : []);
  $effect(() => {
    if (!loaded || !untouched) return;
    picked = (nearby.length ? nearby : catalog.filter((e) => e.default)).map((e) => e.id);
  });

  // Which preset the pick matches, if any, so the well shows the state rather than a row of actions.
  const sortedIds = (ids: string[]) => [...ids].sort().join('\n');
  const presets = $derived([
    { id: 'near', label: 'Near me', ids: nearby.map((e) => e.id), off: !nearby.length, hint: nearby.length ? `The ${nearby.length} servers in ${myCountry}, plus anycast ones that answer from nearby. Nothing abroad.` : 'Your country isn’t known yet.' },
    { id: 'defaults', label: 'Defaults', ids: catalog.filter((e) => e.default).map((e) => e.id), off: false, hint: 'A few well-run services and labs: enough to judge your clock and your network.' },
    { id: 'labs', label: 'Labs', ids: catalog.filter((e) => e.kind === 'lab').map((e) => e.id), off: false, hint: 'Every national metrology institute and observatory in the list.' },
    { id: 'nts', label: 'NTS', ids: catalog.filter((e) => !!e.nts).map((e) => e.id), off: false, hint: 'Servers that offer authenticated time, to see which verify from here.' },
    { id: 'all', label: 'All', ids: catalog.map((e) => e.id), off: false, hint: `All ${catalog.length} servers. A lot of strangers’ infrastructure to ask at once; the defaults say as much about your clock.` },
  ]);
  const preset = $derived(presets.find((p) => p.ids.length && sortedIds(p.ids) === sortedIds(picked)) ?? null);
  const groupIds = (kind: NtpEntry['kind']) => catalog.filter((e) => e.kind === kind).map((e) => e.id);
  const pickGroup = (kind: NtpEntry['kind'], on: boolean) => {
    const ids = groupIds(kind);
    picked = on ? [...new Set([...picked, ...ids])] : picked.filter((id) => !ids.includes(id));
    remember();
  };
  const inGroup = (kind: NtpEntry['kind']) => picked.filter((id) => byId.get(id)?.kind === kind).length;

  // The sheet shown while a run is going. The engine reports only how many servers have finished, so the sheet is
  // drawn from the plan instead: one lane per server, its queries placed where the pacing will send them (a random
  // start within one spacing, then one spacing apart), and a cursor sweeping the clock across them.
  type Lane = { name: string; host: string; first: number; gap: number };
  let lanes = $state<Lane[]>([]);
  const spread = (i: number) => (i * 0.618034) % 1;
  function planLanes(): Lane[] {
    const out: Lane[] = catalog.filter((e) => picked.includes(e.id) && !resting(e)).map((e, i) => ({ name: e.name, host: e.host, first: spread(i) * spacing, gap: spacing }));
    customHosts.forEach((h, i) => out.push({ name: h, host: 'yours', first: spread(i + out.length) * 2, gap: 2 }));
    return out;
  }
  let clock = $state(Date.now());
  $effect(() => {
    if (!running) return;
    const t = setInterval(() => (clock = Date.now()), reduceMotion ? 1000 : 200);
    return () => clearInterval(t);
  });
  const elapsed = $derived(running && run ? Math.max(0, (clock - run.started_at) / 1000) : 0);
  const span = $derived(run ? run.samples * (run.spacing_s ?? spacing) + 6 : eta);
  const liveRows = $derived.by((): Lane[] => {
    if (!run) return [];
    const out = [...lanes];
    // After a reload, or for servers the engine added, the lane has no name: a placeholder keeps the count honest.
    for (let i = out.length; i < run.total; i++) out.push({ name: '', host: '', first: spread(i) * (run.spacing_s ?? spacing), gap: run.spacing_s ?? spacing });
    return out.slice(0, run.total);
  });
  const progress = $derived(run ? Math.min(1, Math.max(run.total ? run.done / run.total : 0, elapsed / span)) : 0);
  const left = $derived(Math.max(0, Math.round(span - elapsed)));
  const ticks = $derived(Array.from({ length: Math.floor(span / 10) + 1 }, (_, i) => i * 10));
  // The dial tunes in while the first queries are still waiting their random start, then locks and lifts.
  const settling = $derived(elapsed >= (run?.spacing_s ?? spacing) - 1.4);
  const tuning = $derived(elapsed < (run?.spacing_s ?? spacing) + 0.2);

  /** A server you typed has no catalog entry; this stands in for one. */
  const yours = (r: NtpResult): NtpEntry => ({
    id: r.id,
    name: r.port ? `${r.host}:${r.port}` : r.host,
    operator: 'You',
    kind: 'custom',
    host: r.host,
    port: r.port ?? null,
    nts_documented: false,
    nts_only: false,
    default: false,
  });
  const byId = $derived(idMap(catalog));
  const rows = $derived<Row[]>(
    (run?.results ?? []).flatMap((r) => {
      const e = byId.get(r.id) ?? (r.custom ? yours(r) : null);
      return e ? [{ e, r }] : [];
    }),
  );

  // Who is worth naming. A server far away can't be pinned down to less than half its round trip, so only
  // servers whose offset is known to within 25 ms can win on accuracy.
  // A server you typed is shown beside the rest but isn't named: these are recommendations of public servers.
  const answered = $derived(rows.filter((x) => x.r && x.r.ok > 0 && !x.r.nts_only && !x.r.custom));
  // Pools are measured but never named: a pool's row is a median over several volunteers' servers, which no single server can match.
  const pinned = $derived(answered.filter((x) => x.e.kind !== 'pool' && (x.r!.bound_ms ?? Infinity) <= 25 && (x.r!.stratum ?? 99) <= 3));
  const best = (xs: Row[], f: (r: NtpResult) => number | null | undefined) =>
    xs.map((x) => ({ x, v: f(x.r!) })).filter((o): o is { x: Row; v: number } => o.v != null).sort((a, b) => a.v - b.v)[0]?.x ?? null;
  // A fast server is only worth naming if its clock is fit to use: synchronized to a clock within three steps, and not
  // far from what the others say.
  const sound = (r: NtpResult) => (r.stratum ?? 99) <= 3 && (r.deviation_ms == null || Math.abs(r.deviation_ms) <= 50);
  // Jitter from fewer than three answers at an address (two other offsets to compare) is too thin to rank on.
  const enough = (r: NtpResult) => r.addrs.filter((a) => a.ok > 0).every((a) => a.ok >= 3);
  const picks = $derived({
    // Agreeing with the consensus is a gate, not a score: a server closest to the median just shares its path error. Of those that
    // agree and whose own addresses agree, the one with the smallest error ceiling against UTC is the one that can be trusted most.
    accurate: best(
      pinned.filter((x) => x.r!.agrees === true && !x.r!.addrs_disagree),
      (r) => r.distance_ms ?? r.bound_ms,
    ),
    fastest: best(answered.filter((x) => x.e.kind !== 'pool' && sound(x.r!)), (r) => r.delay_min_ms),
    steadiest: best(pinned.filter((x) => enough(x.r!)), (r) => r.jitter_ms),
  });

  type Role = { kind: 'use' | 'spare' | 'far' | 'avoid' | 'pool' | 'none'; label: string; why: string; order: number };
  // Where each server stands for the recommendation: in the set, passed over (and why), too far to rank, or left out.
  const role = (x: Row): Role => {
    const r = x.r!;
    const adv = c?.advice;
    const i = adv?.set.indexOf(r.id) ?? -1;
    if (i >= 0) return { kind: 'use', label: `Use ${i + 1}`, why: 'In the recommended set: it agrees with the others, and no server ranked ahead of it shares its operator or upstream clock.', order: i };
    const avoid = adv?.avoid.find((a) => a.id === r.id);
    if (avoid) return { kind: 'avoid', label: 'Leave out', why: avoid.why, order: 3000 };
    const passed = adv?.passed.find((a) => a.id === r.id);
    if (passed) return { kind: 'spare', label: 'Spare', why: passed.why, order: 100 + (adv?.passed.indexOf(passed) ?? 0) };
    if (x.e.kind === 'pool') return { kind: 'pool', label: 'Pool', why: "Measured but never named: a pool row is a median over several volunteers' servers.", order: 2000 };
    if (x.e.leap?.toLowerCase().includes('smear')) return { kind: 'spare', label: 'Spare', why: "It smears the leap second, which shouldn't be mixed with servers that step.", order: 400 };
    if (r.agrees === false) return { kind: 'spare', label: 'Spare', why: 'It disagrees with the others by more than both error ceilings allow.', order: 450 };
    if ((r.bound_ms ?? Infinity) > 25)
      return { kind: 'far', label: 'Too far', why: "Its round trip is too long to pin its offset down to within 25 ms from here, so it isn't ranked against nearer servers. It is still fine as a fallback.", order: 500 + (r.bound_ms ?? 0) / 1000 };
    return { kind: 'none', label: '', why: '', order: 1000 };
  };

  const pickRows = $derived([
    { goal: 'Tightest error bound', pick: picks.accurate, say: (r: NtpResult) => `off by at most ${num(r.distance_ms ?? r.bound_ms)} ms from UTC, and agrees with the others` },
    { goal: 'Lowest latency', pick: picks.fastest, say: (r: NtpResult) => `${num(r.delay_min_ms)} ms round trip` },
    { goal: 'Steadiest', pick: picks.steadiest, say: (r: NtpResult) => `${num(r.jitter_ms, 2)} ms jitter over ${r.ok} answers` },
  ]);

  const val = (x: Row, k: string): number | string => {
    const r = x.r!;
    switch (k) {
      case 'name':
        return x.e.name.toLowerCase();
      case 'fit':
        return role(x).order;
      case 'stratum':
        return r.stratum ?? 99;
      case 'offset':
        return r.offset_ms ?? Infinity;
      case 'deviation':
        return r.deviation_ms == null ? Infinity : Math.abs(r.deviation_ms);
      case 'bound':
        return r.bound_ms ?? Infinity;
      case 'distance':
        return r.distance_ms ?? r.bound_ms ?? Infinity;
      case 'delay':
        return r.delay_min_ms ?? Infinity;
      case 'jitter':
        return r.jitter_ms ?? Infinity;
      case 'ok':
        return r.sent ? -r.ok / r.sent : 0;
      default:
        return 0;
    }
  };
  const sorted = $derived(
    [...rows].sort((a, b) => {
      // Servers that answered come before the ones that didn't, whatever the sort.
      const ga = a.r!.ok > 0 ? 0 : 1;
      const gb = b.r!.ok > 0 ? 0 : 1;
      if (ga !== gb) return ga - gb;
      const [va, vb] = [val(a, sort.key), val(b, sort.key)];
      return (typeof va === 'string' ? va.localeCompare(vb as string) : (va as number) - (vb as number)) * sort.dir;
    }),
  );
  // Rows opened to show how their numbers were worked out.
  const worked = new SvelteSet<string>();
  const toggleWork = (id: string) => (worked.has(id) ? worked.delete(id) : worked.add(id));
  const sortBy = (k: string) => (sort = { key: k, dir: sort.key === k ? (sort.dir === 1 ? -1 : 1) : 1 });
  const aria = (k: string) => (sort.key === k ? (sort.dir === 1 ? 'ascending' : 'descending') : 'none');

  const c = $derived(run?.consensus ?? null);
  // What the size of the error means for what the clock is used for. Judged on the best estimate, and on the worst case
  // for the all-clear: a login code (TOTP) steps every 30 s, and Kerberos tolerates 5 minutes by default.
  const verdict = $derived.by(() => {
    if (!c) return '';
    const ms = Math.abs(c.median_ms);
    if (ms + c.bound_ms <= 100) return 'That is fine for everything ordinary: logs, TLS, login codes and Kerberos all tolerate far more.';
    if (ms < 1000) return "That is small, but timestamps won't line up exactly with other machines.";
    if (ms < 30_000) return 'That is enough to notice when comparing logs or files across machines. Login codes, which step every 30 seconds, still work.';
    if (ms < 300_000) return 'That is enough for one-time login codes, which step every 30 seconds, to fail.';
    return 'That is more than Kerberos tolerates by default (5 minutes) and enough to fail login codes.';
  });

  const NTS_WHY: Record<string, string> = {
    aead_failure: "the reply's authenticator doesn't open with the negotiated key, so it can't be trusted",
    missing_authenticator: 'the reply has no NTS authenticator',
    unauthenticated_response: 'it answered with plain, unauthenticated NTP',
    invalid_unique_id: "the reply doesn't carry this query's unique identifier",
    malformed_extensions: 'the reply has malformed NTS extension fields',
    missing_cookies: 'it handed out no cookies to authenticate with',
    ke_handshake_failed: 'the NTS-KE handshake failed',
    certificate_invalid: 'its certificate is not valid',
    timeout: 'no answer in time',
    network: 'a network error',
    dns: "the key-exchange name didn't resolve",
  };
  const ntsWhy = (kind?: string | null) => (kind && NTS_WHY[kind]) || 'no answer';

  // Why a server gave no time, in words. The stable name is in the title for anyone scripting against the export.
  const WHY: Record<string, string> = {
    dns: "The name didn't resolve.",
    no_address: 'The name has no address to ask.',
    timeout: 'No answer.',
    kiss: 'It told us to stop.',
    unsynchronized: "It answered, but its own clock isn't synchronized (leap indicator 3 or stratum 16), so it isn't fit to use.",
    bad_reply: 'It answered with something that isn’t an NTP reply.',
    network: 'The network refused the query.',
    internal: "The check couldn't start.",
  };
  const whyNone = (r: NtpResult) =>
    r.kiss
      ? `It told us to stop (${r.kiss}).`
      : `${(r.error_kind && WHY[r.error_kind]) || `No answer${r.error ? `: ${r.error}` : '.'}`}${r.error_kind === 'timeout' ? ` That's from this network; it may answer elsewhere.` : ''}`;

  async function download(format: 'csv' | 'json') {
    if (!run) return;
    try {
      const saved = await saveExport(run.id, format);
      if (saved) app.notify(`Saved ${saved}.`);
    } catch (e) {
      app.notify((e as Error).message, 'error');
    }
  }

  function addCustom() {
    const t = customText.trim();
    if (!t) return;
    if (customHosts.length >= MAX_CUSTOM) return app.notify(`At most ${MAX_CUSTOM} servers of your own per run.`);
    if (!customHosts.includes(t)) customHosts = [...customHosts, t];
    customText = '';
    remember();
  }
  const dropCustom = (t: string) => {
    customHosts = customHosts.filter((x) => x !== t);
    remember();
  };

  function ntsCell(x: Row): { text: string; tone: 'yes' | 'no' | 'none'; title: string } {
    const k = x.r?.nts;
    if (k?.ok && k.authenticated === true) return { text: 'Verified', tone: 'yes', title: `The key exchange on port ${k.port} worked, and a time query sealed with its keys (AES-SIV-CMAC-256) got a reply that verified. Authenticated time works here.${k.server ? ` It points clients to ${k.server}${k.ntp_port ? `:${k.ntp_port}` : ''}.` : ''}` };
    if (k?.ok && k.authenticated === false) return { text: 'Handshake only', tone: 'no', title: `The key exchange worked, but the authenticated time query did not verify: ${ntsWhy(k.timed?.error_kind ?? k.error_kind)}.` };
    if (k?.ok) return { text: 'Yes', tone: 'yes', title: `The key exchange on port ${k.port} worked and gave ${k.cookies} cookies (AES-SIV-CMAC-256).${k.server ? ` It points clients to ${k.server}${k.ntp_port ? `:${k.ntp_port}` : ''}.` : ''}` };
    if (k) return { text: 'No', tone: 'no', title: `The key exchange on port ${k.port} didn't complete: ${ntsWhy(k.error_kind)}${k.error ? ` (${k.error})` : ''}.` };
    if (x.e.nts_documented) return { text: 'Documented', tone: 'none', title: 'Its operator documents NTS; it was not checked in this run.' };
    return { text: '—', tone: 'none', title: 'Not checked.' };
  }
  const v6Cell = (r: NtpResult | null) =>
    r?.ipv6 === 'answers' ? 'Yes' : r?.ipv6 === 'unreachable' ? 'Yes, no route here' : r?.ipv6 === 'no address' ? 'No' : '—';
  const kindLabel = (k: string) => (k === 'service' ? 'Service' : k === 'lab' ? 'Time lab' : k === 'custom' ? 'Yours' : 'Pool');
</script>

<div class="view-root">
  <Toolbar title="Time" subtitle="How accurate and how fast public NTP servers are from this network">
    {#snippet center()}
      <div class="segmented" role="tablist" aria-label="View">
        <button role="tab" aria-selected={view === 'accuracy'} onclick={() => (view = 'accuracy')}>{mobile ? 'Accuracy' : 'Accuracy & latency'}</button>
        <button role="tab" aria-selected={view === 'features'} onclick={() => (view = 'features')}>Features</button>
        <button role="tab" aria-selected={view === 'monitor'} onclick={() => (view = 'monitor')}>Over time</button>
      </div>
    {/snippet}
    {#snippet actions()}
      {#if view === 'monitor' || mobile}
        <!-- the monitor has its own switch; the phone's Start lives in the footer bar -->
      {:else if running}
        <button class="btn" onclick={stop}><Icon name="stop" size={12} />Stop</button>
      {:else}
        {#if !mobile && run?.results.length && run.status !== 'running'}
          <button class="btn" onclick={() => download('csv')} title="One row per server, with a schema_version column and named failure reasons.">Export CSV</button>
          <button class="btn" onclick={() => download('json')} title="The whole result, with a schema_version.">Export JSON</button>
        {/if}
        <button class="btn primary" disabled={!(picked.length || customHosts.length) || starting || !loaded} title={picked.length || customHosts.length ? 'Start' : 'Pick at least one server.'} onclick={start}>
          <Icon name="play" size={12} />Start
        </button>
      {/if}
    {/snippet}
  </Toolbar>

  <div class="view-body">
    {#if view === 'monitor'}
      <TimeMonitor {catalog} />
    {/if}
    <div class="content scroll wrap" style:display={view === 'monitor' ? 'none' : null}>
      <section class="setup panel" aria-labelledby="setup-h">
        <div class="head">
          <div class="ht">
            <h2 id="setup-h">Test setup</h2>
            <Info label="How gently a run asks">
              <p>
                A run looks like an NTP client that has just started: at most {maxSamples} packets to any address, at least {spacing} seconds apart, each
                address beginning at a random moment so nothing arrives as one burst. An address that answers none of three queries in a row isn't asked
                again.
              </p>
              <p>
                A server is left alone for {Math.round(cooldown / 60)} minutes after it is asked (at most {Math.floor((3600 / cooldown) * maxSamples)} packets an hour
                to an address, fewer than an ordinary client polling every 64 seconds sends), and one that sends a Kiss-o'-Death is left alone for an hour
                (RATE) or a week (DENY, RSTR). Names that give several addresses, like the pool, are measured at up to four of them.
              </p>
              <p>
                Time is measured over plain NTP. The NTS check completes the key exchange and then sends one authenticated query per address (up to the
                same number of queries), so “Verified” means the reply was sealed with keys only that server could have.
              </p>
            </Info>
          </div>
          <p class="sum">
            <span class="num">{plural(picked.length + customHosts.length, 'server')} · {samples} queries each · NTS {nts ? 'checked' : 'off'} · {running ? 'measuring' : `about ${eta} s`}</span>
            {#if !planOpen && chosen}<span class="chosen">{chosen}</span>{/if}
          </p>
          {#if hasRun}
            <button class="btn sm" type="button" aria-expanded={planOpen} aria-controls="setup-body" disabled={running} onclick={() => (planOpen = !planOpen)}>{planOpen ? 'Done' : 'Change'}</button>
          {/if}
        </div>
        {#if planOpen}
          <div class="body" id="setup-body">
            <div class="controls">
              <fieldset>
                <legend>Which servers</legend>
                <div class="seg xs five" role="group" aria-label="Presets">
                  {#each presets as p (p.id)}
                    <button type="button" class="segbtn" aria-pressed={preset?.id === p.id} disabled={p.off || running} title={p.hint} onclick={() => ((picked = [...p.ids]), remember())}>{p.label}</button>
                  {/each}
                </div>
                <p class="hint">
                  {#if preset}{preset.hint}{:else if picked.length}Your own pick: {plural(picked.length, 'server')} from the list.{:else}Pick a preset, or tick servers in the list.{/if}
                </p>
              </fieldset>
              <fieldset>
                <legend>Queries per server</legend>
                <div class="seg xs three" role="radiogroup" aria-label="Queries per server">
                  {#each [4, 6, 8].filter((n) => n <= maxSamples) as n (n)}
                    <label class="segbtn">
                      <input type="radio" name="samples" value={n} bind:group={samples} onchange={remember} disabled={running} />
                      <span>{n}</span>
                    </label>
                  {/each}
                </div>
                <p class="hint">{spacing} seconds apart, so a run takes about {eta} seconds and sends at most {budget} packets in all.</p>
              </fieldset>
              <fieldset>
                <legend>Authenticated time</legend>
                <label class="toggle">
                  <input type="checkbox" bind:checked={nts} onchange={remember} disabled={running} />
                  Check NTS
                </label>
                <p class="hint">Completes the key exchange with servers that offer it, then sends sealed queries to see whether the answers verify.</p>
              </fieldset>
              <fieldset>
                <legend>Your own server</legend>
                <form class="custom" onsubmit={(ev) => (ev.preventDefault(), addCustom())}>
                  <input
                    type="text"
                    bind:value={customText}
                    placeholder="ntp.lan or 192.168.1.1:1123"
                    aria-label="Host name or address of a time server"
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    disabled={running || customHosts.length >= MAX_CUSTOM}
                  />
                  <button class="btn sm" type="submit" disabled={running || !customText.trim() || customHosts.length >= MAX_CUSTOM}>Add</button>
                </form>
                {#if customHosts.length}
                  <ul class="chips">
                    {#each customHosts as h (h)}
                      <li>
                        <span class="mono">{h}</span>
                        <button class="x" type="button" aria-label="Remove {h}" disabled={running} onclick={() => dropCustom(h)}>×</button>
                      </li>
                    {/each}
                  </ul>
                {/if}
                <p class="hint">
                  A name or address, with a port if it isn't 123; <span class="mono">[2001:db8::1]:123</span> for IPv6. It is measured beside the rest but never votes, and is asked every 2 seconds rather than {spacing}.
                </p>
              </fieldset>
            </div>
            <div class="list">
              {#each GROUPS as [kind, title, blurb] (kind)}
                {@const n = groupIds(kind).length}
                <section class="group" aria-label={title}>
                  <div class="gh">
                    <h3>{title} <span class="cnt num">{inGroup(kind)} of {n}</span></h3>
                    <span class="gbtns">
                      <button class="btn ghost sm" type="button" disabled={running || inGroup(kind) === n} onclick={() => pickGroup(kind, true)}>All</button>
                      <button class="btn ghost sm" type="button" disabled={running || !inGroup(kind)} onclick={() => pickGroup(kind, false)}>None</button>
                    </span>
                  </div>
                  <p class="hint">{blurb}</p>
                  <ul class="picker">
                    {#each catalog.filter((e) => e.kind === kind) as e (e.id)}
                      <li>
                        <label class:on={picked.includes(e.id)}>
                          <input type="checkbox" checked={picked.includes(e.id)} disabled={running} onchange={() => toggle(e.id)} aria-label={e.name} />
                          <span class="nm">{e.name}{#if e.nts}<span class="tag" title="Offers authenticated time (NTS)">NTS</span>{/if}</span>
                          <span class="host mono">{e.host}</span>
                          <span class="where">{e.anycast ? 'anycast' : (e.country ?? '')}</span>
                          {#if resting(e)}
                            {@const w = resting(e)!}
                            <span class="rest">{w.kiss ? `told us to stop (${w.kiss}); left alone until ${at(w.until)}` : `asked a few minutes ago; ready at ${at(w.until)}`}</span>
                          {/if}
                        </label>
                      </li>
                    {/each}
                  </ul>
                </section>
              {/each}
            </div>
          </div>
        {/if}
      </section>

      {#if running && run}
        <section class="live panel" aria-label="Measuring" in:fade={{ duration: reduceMotion ? 0 : 200 }}>
          <div class="lhead">
            <p role="status">
              <b>Measuring {plural(run.total, 'server')}</b>
              <span class="muted num">{run.done} of {run.total} finished · about {left} s left</span>
            </p>
            <span class="track" aria-hidden="true"><span class="fill" style:transform="scaleX({progress})"></span></span>
          </div>
          <div class="sheet" style:--p={Math.min(1, elapsed / span)} aria-hidden="true">
            <div class="axis">
              {#each ticks as t (t)}<span style:left="{(t / span) * 100}%">{t} s</span>{/each}
            </div>
            <div class="rows">
              {#each liveRows as l, i (i)}
                <div class="lrow">
                  <div class="who">
                    {#if l.name}
                      <span class="nm">{l.name}</span>
                      <span class="host mono">{l.host}</span>
                    {:else}
                      <span class="ghost" style:width="{120 + ((i * 37) % 60)}px" style:height="11px"></span>
                      <span class="ghost" style:width="90px" style:height="8px" style:margin-top="5px"></span>
                    {/if}
                  </div>
                  <div class="lane">
                    {#each { length: run.samples } as _, j (j)}
                      {@const t = l.first + j * l.gap}
                      <i class="q" class:sent={elapsed >= t} style:left="{(t / span) * 100}%"></i>
                    {/each}
                  </div>
                  <div class="ncol">
                    <span class="ghost" style:width="54px" style:height="14px"></span>
                    <span class="ghost" style:width="72px" style:height="8px" style:margin-top="5px"></span>
                  </div>
                </div>
              {/each}
              <div class="cursor"></div>
              {#if tuning}
                <div class="starting"><SetupDial still={settling} /></div>
              {/if}
            </div>
          </div>
        </section>
      {/if}

      {#if picked.length > 30 && !running}
        <p class="note" role="note">
          <Icon name="info" size={14} />
          <span>{picked.length} servers is a lot of strangers' infrastructure to ask at once. The defaults give the same picture of your clock and your network.</span>
        </p>
      {/if}

      {#if resters.length && !running}
        <p class="note" role="note">
          <Icon name="info" size={14} />
          <span>
            {resters.length === 1 ? 'One server you picked is' : `${resters.length} servers you picked are`} being left alone to be gentle:
            {resters.map((e) => e.name).join(', ')}. They were asked a few minutes ago (or told us to stop) and will be skipped.
          </span>
        </p>
      {/if}

      {#if run?.added?.length}
        <p class="note" role="note">
          <Icon name="info" size={14} />
          <span>Only servers you typed were picked, so {run.added.join(', ')} {run.added.length === 1 ? 'was' : 'were'} added to judge them against.</span>
        </p>
      {/if}

      {#if run?.skipped?.length}
        <p class="note" role="note">
          <Icon name="info" size={14} />
          <span>
            Left alone in the last run, to be gentle:
            {run.skipped.map((s) => `${byId.get(s.id)?.name ?? s.host} (${s.why === 'kiss' ? `it told us to stop, ${s.kiss}` : 'asked a few minutes before'}; ready ${at(s.until)})`).join(', ')}.
          </span>
        </p>
      {/if}

      {#if run && run.results.length && !running}
        <section class="summary panel">
          <div class="head">
          {#if c && c.advice.set.length}
            <div class="ht">
              <h2>Use {c.advice.set.map((id) => nameOf(byId, id)).join(', ')}</h2>
              <Info label="How these servers were chosen">
                <p>
                  Best first, by how far each can be wrong about the time from this network and how much its answers wander. They agree
                  with one another, and each comes from a different operator and upstream clock, so one failure can't take them all.
                  Pools and servers that smear the leap second are left out.
                </p>
              </Info>
            </div>
            {#if c.advice.set.length < 3}<p class="lead">Fewer than three qualified, so a bad clock couldn't be outvoted: run again with more servers.</p>{/if}
          {:else if c}
            <h2>No server qualified</h2>
            <p class="lead">Each needs to agree with the others, be synchronized, and come from a different operator and upstream.</p>
          {:else}
            <h2>Too few independent servers answered to rank them</h2>
          {/if}
          </div>
          <dl class="picks">
            {#each pickRows as p (p.goal)}
              <div>
                <dt>{p.goal}</dt>
                <dd>{#if p.pick}{@const ro = role(p.pick)}<b>{p.pick.e.name}</b> <span class="muted">{p.say(p.pick.r!)}{#if ro.kind === 'spare' || ro.kind === 'avoid'}. Not in the set: {ro.why.charAt(0).toLowerCase() + ro.why.slice(1)}{/if}</span>{:else}<span class="muted">—</span>{/if}</dd>
              </div>
            {/each}
            {#if c && c.advice.avoid.length}
              <div>
                <dt>Leave out</dt>
                <dd>
                  {#each c.advice.avoid as a, i (a.id)}{i ? '; ' : ''}<b>{nameOf(byId, a.id)}</b> <span class="muted">{a.why}</span>{/each}
                </dd>
              </div>
            {/if}
          </dl>
          {#if c}
            <p class="lead clock">
              <b>Your clock:</b>
              {#if Math.abs(c.median_ms) <= c.bound_ms}
                matches public time to within what this test can measure. The servers' median is {Math.abs(c.median_ms).toFixed(2)} ms {c.median_ms > 0 ? 'ahead of' : 'behind'} yours, which is less than the ± {num(c.bound_ms)} ms
                that one round trip leaves unresolved, so it can't be told from exact.
              {:else}
                {Math.abs(c.median_ms).toFixed(1)} ms {c.median_ms > 0 ? 'behind' : 'ahead of'} the servers' median (± {num(c.bound_ms)} ms).
              {/if}
              {c.voters} servers close enough to judge{#if c.independent < c.voters} ({c.independent} independent: those of one operator or one upstream count once){/if} agree within {c.spread_ms.toFixed(2)} ms across their middle half,
              but they are all reached over your connection and share its error, so that agreement can't shrink the ±. No server is the truth.
              {verdict}
            </p>
          {/if}
        </section>
      {/if}

      {#if run && run.results.length}
        {#if mobile}
          <!-- Phone: one row per server, its numbers in a small grid under the name. -->
          <div class="mlist">
            {#each sorted as x (x.e.id)}
              {#if view === 'accuracy'}
                {@const r = x.r!}
                <div class="mrow" class:none={r.ok === 0}>
                  <div class="mtop">
                    {#if r.ok > 0 && !r.nts_only}
                      <button class="how" aria-expanded={worked.has(x.e.id)} aria-controls="work-{x.e.id}" onclick={() => toggleWork(x.e.id)}>
                        <Icon name={worked.has(x.e.id) ? 'chevron-down' : 'chevron-right'} size={12} />
                        <span class="nm">{x.e.name}</span>
                      </button>
                    {:else}
                      <span class="nm">{x.e.name}</span>
                    {/if}
                    {#if r.ok > 0 && !r.nts_only}
                      {@const ro = role(x)}
                      {#if ro.kind !== 'none'}
                        <span class="role {ro.kind}">
                          {#if ro.kind === 'use'}<Icon name="check" size={12} />{:else if ro.kind === 'avoid'}<Icon name="warn" size={12} />{:else}<Icon name="dash" size={12} />{/if}
                          {ro.label}
                        </span>
                      {/if}
                    {/if}
                  </div>
                  <span class="host mono">{r.host}{#if addrs(r) > 1}&nbsp;·&nbsp;{addrs(r)} addresses{/if}</span>
                  {#if r.custom}<span class="host">yours · doesn't vote{r.port ? ` · port ${r.port}` : ''}</span>{/if}
                  {#if r.selection === 'falseticker'}<span class="host off">falseticker: its clock disagrees with the majority</span>{/if}
                  {#if r.addrs_disagree}<span class="host off">its addresses disagree by {num(r.addr_spread_ms)} ms</span>{/if}
                  {#if r.nts_only}
                    <p class="muted mnote">NTS key exchange only: this host doesn’t answer plain NTP, so only the handshake was checked.</p>
                  {:else if r.ok === 0}
                    <p class="muted mnote">{whyNone(r)}</p>
                  {:else}
                    {@const ro = role(x)}
                    {#if ro.kind === 'spare'}<span class="why">{ro.why}</span>{/if}
                    <dl class="mm">
                      <div><dt>Offset</dt><dd class="num">{sgn(r.offset_ms)} ms</dd></div>
                      <div><dt>±</dt><dd class="num">{num(r.bound_ms)}</dd></div>
                      <div><dt>Error ≤</dt><dd class="num" class:win={picks.accurate?.r === r}>{num(r.distance_ms ?? r.bound_ms)}</dd></div>
                      <div>
                        <dt>vs others</dt>
                        <dd class="num">
                          {sgn(r.deviation_ms)}
                          {#if r.agrees === true}<span class="ok" aria-label="agrees"><Icon name="check" size={12} /></span>
                          {:else if r.agrees === false}<span class="off" aria-label="disagrees"><Icon name="warn" size={12} /></span>{/if}
                        </dd>
                      </div>
                      <div><dt>Latency</dt><dd class="num" class:win={picks.fastest?.r === r}>{num(r.delay_min_ms)} ms</dd></div>
                      <div><dt>Jitter</dt><dd class="num" class:win={picks.steadiest?.r === r}>{r.jitter_ms == null ? '—' : `${num(r.jitter_ms, 2)} ms`}</dd></div>
                      <div><dt>Source</dt><dd><span class="num">{r.stratum}</span> <span class="mono muted">{r.refid}</span></dd></div>
                      <div><dt>Answered</dt><dd class="num">{r.ok}/{r.sent}</dd></div>
                    </dl>
                    {#if worked.has(x.e.id)}
                      <div class="mwork" id="work-{x.e.id}"><TimeWorking {r} /></div>
                    {/if}
                  {/if}
                </div>
              {:else}
                {@const n = ntsCell(x)}
                <div class="mrow">
                  <span class="nm">{#if x.e.link}<a href={x.e.link} target="_blank" rel="noreferrer">{x.e.name}</a>{:else}{x.e.name}{/if}</span>
                  <span class="host mono">{x.e.host}</span>
                  <dl class="mm">
                    <div><dt>Operator</dt><dd>{x.e.operator}</dd></div>
                    <div><dt>Kind</dt><dd>{kindLabel(x.e.kind)}</dd></div>
                    <div><dt>Source</dt><dd>{#if x.r?.stratum != null}<span class="num">{x.r.stratum}</span> <span class="mono muted">{x.r.refid}</span>{:else}—{/if}</dd></div>
                    <div><dt>NTS</dt><dd><span class="nts {n.tone}">{n.text}</span></dd></div>
                    <div><dt>IPv6</dt><dd>{v6Cell(x.r)}</dd></div>
                    <div><dt>Leap second</dt><dd>{x.e.leap ?? '—'}</dd></div>
                    <div><dt>Anycast</dt><dd>{x.e.anycast == null ? '—' : x.e.anycast ? 'Yes' : 'No'}</dd></div>
                    <div><dt>Where</dt><dd>{x.e.country ?? '—'}</dd></div>
                    {#if x.e.policy}<div class="wide"><dt>Policy and limits</dt><dd>{x.e.policy}</dd></div>{/if}
                  </dl>
                </div>
              {/if}
            {/each}
          </div>
        {:else}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex (a scrollable region must be reachable by keyboard) -->
        <div class="tablewrap panel" role="region" aria-label="Results" tabindex="0">
          {#if view === 'accuracy'}
            <table class="sticky-head">
              <thead>
                <tr>
                  <th scope="col" class="first" aria-sort={aria('name')}><button onclick={() => sortBy('name')}>Server</button></th>
                  <th scope="col" aria-sort={aria('fit')} title="Where each server stands for the recommendation: in the set to use together (best first), a spare passed over for a stated reason, too far to rank, or left out. Pools are measured but never named.">
                    <button onclick={() => sortBy('fit')}>Fit</button>
                  </th>
                  <th scope="col" aria-sort={aria('stratum')} title="Stratum 1 gets its time from a clock; each step down is one more server away. The reference ID names the source.">
                    <button onclick={() => sortBy('stratum')}>Source</button>
                  </th>
                  <th scope="col" class="r" aria-sort={aria('offset')} title="How far ahead of this computer's clock the server's is, from the query with the smallest delay.">
                    <button onclick={() => sortBy('offset')}>Offset</button>
                  </th>
                  <th scope="col" class="r" aria-sort={aria('bound')} title="The most the offset can be wrong by because the path may be slower one way: half the smallest round trip.">
                    <button onclick={() => sortBy('bound')}>±</button>
                  </th>
                  <th scope="col" class="r" aria-sort={aria('distance')} title="The most the offset can be wrong by against true time (UTC): half the server's own root delay, plus its root dispersion, plus the ±. This is NTP's own error ceiling.">
                    <button onclick={() => sortBy('distance')}>Error ≤</button>
                  </th>
                  <th scope="col" class="r" aria-sort={aria('deviation')} title="How far this server's clock is from the median of the other servers. Your own clock cancels out of this, so it says nothing about your computer, only whether the server is in step with the rest.">
                    <button onclick={() => sortBy('deviation')}>vs others</button>
                  </th>
                  <th scope="col" class="r" aria-sort={aria('delay')} title="The smallest round trip over all queries, less the server's own time.">
                    <button onclick={() => sortBy('delay')}>Latency</button>
                  </th>
                  <th scope="col" class="r" aria-sort={aria('jitter')} title="How much the offset moved between queries.">
                    <button onclick={() => sortBy('jitter')}>Jitter</button>
                  </th>
                  <th scope="col" class="r" aria-sort={aria('ok')}><button onclick={() => sortBy('ok')}>Answered</button></th>
                </tr>
              </thead>
              <tbody>
                {#each sorted as x (x.e.id)}
                  {@const r = x.r!}
                  <tr class:none={r.ok === 0}>
                    <th scope="row" class="first">
                      {#if r.ok > 0 && !r.nts_only}
                        <button class="how" aria-expanded={worked.has(x.e.id)} aria-controls="work-{x.e.id}" onclick={() => toggleWork(x.e.id)} title="Show how this server's numbers are worked out">
                          <Icon name={worked.has(x.e.id) ? 'chevron-down' : 'chevron-right'} size={12} />
                          <span class="nm">{x.e.name}</span>
                        </button>
                      {:else}
                        <span class="nm">{x.e.name}</span>
                      {/if}
                      <span class="host mono">{r.host}{#if addrs(r) > 1}&nbsp;·&nbsp;{addrs(r)} addresses{/if}</span>
                      {#if r.custom}<span class="host" title="A server you typed is measured and judged against the others, but it is never one of the servers the consensus is made of.">yours · doesn't vote{r.port ? ` · port ${r.port}` : ''}</span>{/if}
                      {#if r.selection === 'falseticker'}<span class="host off" title="Its error interval misses the one most servers share, so it can't be right if they are. It is left out of the consensus.">falseticker: its clock disagrees with the majority</span>{/if}
                      {#if r.addrs_disagree}<span class="host off" title="Two of its addresses differ by {num(r.addr_spread_ms)} ms, more than their error bounds allow, so at least one is wrong. The row shows their median.">its addresses disagree by {num(r.addr_spread_ms)} ms</span>{/if}
                    </th>
                    {#if r.nts_only}
                      <td colspan="9" class="muted">NTS key exchange only: this host doesn’t answer plain NTP, so only the handshake was checked.</td>
                    {:else if r.ok === 0}
                      <td colspan="9" class="muted" title={r.error_kind ? `reason: ${r.error_kind}` : undefined}>{whyNone(r)}</td>
                    {:else}
                      {@const ro = role(x)}
                      <td class="fit">
                        {#if ro.kind !== 'none'}
                          <span class="role {ro.kind}" title={ro.why}>
                            {#if ro.kind === 'use'}<Icon name="check" size={12} />{:else if ro.kind === 'avoid'}<Icon name="warn" size={12} />{:else}<Icon name="dash" size={12} />{/if}
                            {ro.label}
                          </span>
                          {#if ro.kind === 'spare'}<span class="why">{ro.why}</span>{/if}
                        {/if}
                      </td>
                      <td class="src"><span class="num">{r.stratum}</span> <span class="mono muted">{r.refid}</span></td>
                      <td class="r num">{sgn(r.offset_ms)} ms</td>
                      <td class="r num muted">{num(r.bound_ms)}</td>
                      <td class="r num muted" class:win={picks.accurate?.r === r}>{num(r.distance_ms ?? r.bound_ms)}</td>
                      <td class="r num">
                        {sgn(r.deviation_ms)}
                        {#if r.agrees === true}<span class="ok" title="Within what this server's error ceiling and the consensus's allow, so it agrees." aria-label="agrees"><Icon name="check" size={12} /></span>
                        {:else if r.agrees === false}<span class="off" title="Further from the consensus than both error ceilings can explain." aria-label="disagrees"><Icon name="warn" size={12} /></span>{/if}
                      </td>
                      <td class="r num" class:win={picks.fastest?.r === r}>{num(r.delay_min_ms)} ms{#if r.delay_p95_ms != null}<span class="muted">&nbsp;·&nbsp;p95 {num(r.delay_p95_ms)}</span>{/if}</td>
                      <td class="r num" class:win={picks.steadiest?.r === r} title={r.jitter_ms == null ? 'Needs two answers to compare.' : undefined}>{r.jitter_ms == null ? '—' : `${num(r.jitter_ms, 2)} ms`}</td>
                      <td class="r num">{r.ok}/{r.sent}</td>
                    {/if}
                  </tr>
                  {#if worked.has(x.e.id) && r.ok > 0 && !r.nts_only}
                    <tr class="working" id="work-{x.e.id}">
                      <td colspan="10"><TimeWorking {r} /></td>
                    </tr>
                  {/if}
                {/each}
              </tbody>
            </table>
          {:else}
            <table class="sticky-head feat">
              <thead>
                <tr>
                  <th scope="col" class="first">Server</th>
                  <th scope="col">Operator</th>
                  <th scope="col">Kind</th>
                  <th scope="col">Source</th>
                  <th scope="col" title="Network Time Security (RFC 8915): authenticated time. Checked by completing the key exchange.">NTS</th>
                  <th scope="col">IPv6</th>
                  <th scope="col" title="What the operator says it does with a leap second.">Leap second</th>
                  <th scope="col">Anycast</th>
                  <th scope="col">Where</th>
                  <th scope="col">Policy and limits</th>
                </tr>
              </thead>
              <tbody>
                {#each sorted as x (x.e.id)}
                  {@const n = ntsCell(x)}
                  <tr>
                    <th scope="row" class="first">
                      <span class="nm">{#if x.e.link}<a href={x.e.link} target="_blank" rel="noreferrer">{x.e.name}</a>{:else}{x.e.name}{/if}</span>
                      <span class="host mono">{x.e.host}</span>
                    </th>
                    <td>{x.e.operator}</td>
                    <td>{kindLabel(x.e.kind)}</td>
                    <td>{#if x.r?.stratum != null}<span class="num">{x.r.stratum}</span> <span class="mono muted">{x.r.refid}</span>{:else}—{/if}</td>
                    <td><span class="nts {n.tone}" title={n.title}>{n.text}</span></td>
                    <td>{v6Cell(x.r)}</td>
                    <td>{x.e.leap ?? '—'}</td>
                    <td>{x.e.anycast == null ? '—' : x.e.anycast ? 'Yes' : 'No'}</td>
                    <td>{x.e.country ?? '—'}</td>
                    <td class="pol">{x.e.policy ?? '—'}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
        </div>
        {/if}
        <p class="fine foot">
          Offset and latency come from the NTP exchange in RFC 5905. “−” means the server's clock is behind yours. “Latency” is the round trip, not the one-way delay. A
          field that shows “—” means the operator publishes nothing we could find, or it wasn't measured here. See Method for how each number is worked out.
        </p>
      {:else if loaded && !running}
        <p class="empty">
          <b>Nothing measured yet.</b> Pick servers above and press Start. Each one is asked for the time several times, at least {spacing} seconds apart, and the answers are compared.
        </p>
      {/if}
      {#if mobile}
        <div class="startbar">
          <p class="est num">{plural(picked.length + customHosts.length, 'server')} · {running ? 'measuring' : `about ${eta} s`}</p>
          {#if running}
            <button class="btn" onclick={stop}><Icon name="stop" size={14} />Stop</button>
          {:else}
            <button class="btn primary" disabled={!(picked.length || customHosts.length) || starting || !loaded} onclick={start}><Icon name="play" size={14} />Start</button>
          {/if}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .wrap > :global(*) {
    max-width: 1280px;
  }
  .wrap {
    display: grid;
    gap: 18px;
    align-content: start;
  }
  /* ---- Setup: the controls at the left, the server sheet at the right ---- */
  .setup .head {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    align-items: center;
    padding: 12px 16px 12px 18px;
  }
  .setup .ht {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .setup h2 {
    font-size: var(--t-sm);
    font-weight: 620;
  }
  .sum {
    display: flex;
    flex: 1;
    min-width: 0;
    gap: 10px;
    align-items: baseline;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .sum .num {
    flex: none;
  }
  .chosen {
    min-width: 0;
    color: var(--ink-3);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .body {
    display: grid;
    grid-template-columns: 340px minmax(0, 1fr);
    border-top: 1px solid var(--rule);
  }
  .controls {
    padding: 6px 22px 22px;
    border-right: 1px solid var(--rule);
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 18px 0 0;
    min-width: 0;
  }
  legend {
    display: block;
    padding: 0;
    margin-bottom: 8px;
    font-size: var(--t-xs);
    font-weight: 620;
    color: var(--ink-3);
  }
  .hint,
  .fine {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  fieldset .hint {
    margin-top: 6px;
    max-width: 44ch;
    line-height: 1.4;
  }
  .fine {
    max-width: 90ch;
  }
  .foot {
    margin-top: -6px;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 2px 0;
    font-size: var(--t-sm);
    cursor: default;
  }

  .list {
    display: grid;
    gap: 22px;
    padding: 18px 22px 24px;
    align-content: start;
  }
  .gh {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .gh h3 {
    flex: 1;
    font-size: var(--t-sm);
    font-weight: 620;
  }
  .cnt {
    margin-left: 6px;
    font-weight: 400;
    color: var(--ink-3);
  }
  .gbtns {
    display: inline-flex;
    gap: 2px;
  }
  .picker {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: 2px 18px;
  }
  .picker label {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    grid-template-areas:
      'box nm where'
      'box host where'
      '. rest rest';
    column-gap: 9px;
    align-items: baseline;
    padding: 4px 6px 4px 4px;
    margin: 0 -4px;
    border-radius: var(--r-sm);
    font-size: var(--t-sm);
    cursor: default;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .picker label:hover {
    background: var(--surface-2);
  }
  .picker input {
    grid-area: box;
    align-self: center;
  }
  .picker .nm {
    grid-area: nm;
    color: var(--ink-2);
  }
  .picker label.on .nm {
    color: var(--ink);
  }
  .nm {
    display: block;
    font-weight: 560;
  }
  .tag {
    display: inline-block;
    margin-left: 6px;
    padding: 0 5px;
    border: 1px solid var(--rule-2);
    border-radius: 3px;
    font-size: var(--t-2xs);
    font-weight: 600;
    line-height: 1.5;
    color: var(--ink-3);
    vertical-align: 1px;
  }
  .host {
    display: block;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .picker .host {
    grid-area: host;
  }
  .where {
    grid-area: where;
    align-self: center;
    font-size: var(--t-2xs);
    color: var(--ink-3);
    white-space: nowrap;
  }
  .rest {
    grid-area: rest;
    font-size: var(--t-xs);
    color: var(--ink-3);
    font-style: italic;
  }
  .custom {
    display: flex;
    gap: 8px;
  }
  .custom input {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--rule-2);
    border-radius: var(--r-md);
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: var(--t-sm);
  }
  .chips {
    list-style: none;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 8px 0 0;
    padding: 0;
  }
  .chips li {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px 2px 10px;
    border: 1px solid var(--rule-2);
    border-radius: 999px;
    font-size: var(--t-xs);
  }
  .chips .x {
    border: 0;
    background: none;
    color: var(--ink-3);
    cursor: pointer;
    font-size: var(--t-sm);
    line-height: 1;
    padding: 2px 6px;
  }
  @media (max-width: 900px) {
    .body {
      grid-template-columns: minmax(0, 1fr);
    }
    .controls {
      border-right: 0;
      border-bottom: 1px solid var(--rule);
    }
  }

  /* ---- Measuring: the plan drawn as a sheet, the clock sweeping across it ---- */
  .live {
    --who: 230px;
    --ncol: 96px;
    --gap: 18px;
    overflow: hidden;
  }
  .lhead {
    display: grid;
    gap: 8px;
    padding: 14px 18px 12px;
    border-bottom: 1px solid var(--rule);
    background: var(--surface-2);
  }
  .lhead p {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 10px;
    align-items: baseline;
    font-size: var(--t-sm);
  }
  .lhead b {
    font-weight: 620;
  }
  .lhead .muted {
    font-size: var(--t-xs);
  }
  .track {
    display: block;
    height: 3px;
    border-radius: 2px;
    background: var(--rule);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--accent);
    transform-origin: left;
    transition: transform 300ms var(--ease-out);
  }
  .sheet {
    position: relative;
    padding: 0 18px 10px;
  }
  .axis {
    position: relative;
    height: 22px;
    margin-left: calc(var(--who) + var(--gap));
    margin-right: calc(var(--ncol) + var(--gap));
  }
  .axis span {
    position: absolute;
    top: 5px;
    transform: translateX(-50%);
    font-size: var(--t-2xs);
    font-variant-numeric: tabular-nums;
    color: var(--ink-3);
  }
  .axis span:first-child {
    transform: none;
  }
  .rows {
    position: relative;
  }
  .lrow {
    display: grid;
    grid-template-columns: var(--who) minmax(0, 1fr) var(--ncol);
    gap: var(--gap);
    align-items: center;
    min-height: 44px;
    border-top: 1px solid var(--rule);
  }
  .lrow .who {
    display: grid;
    min-width: 0;
    padding: 6px 0;
  }
  .lrow .nm,
  .lrow .host {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .lrow .nm {
    color: var(--ink-2);
  }
  /* The lane is this server's share of the run: a dot where each query is planned, a ring once it has gone out. */
  .lane {
    position: relative;
    height: 14px;
    background: linear-gradient(var(--rule-2), var(--rule-2)) center / 100% 1px no-repeat;
  }
  .q {
    position: absolute;
    top: 50%;
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--rule-2);
    transform: translate(-50%, -50%);
  }
  .q.sent {
    width: 10px;
    height: 10px;
    background: var(--surface);
    border: 1.25px solid var(--brand-answer);
    opacity: 0.85;
    animation: ring-in 220ms var(--ease-out);
  }
  @keyframes ring-in {
    from {
      transform: translate(-50%, -50%) scale(0.3);
    }
  }
  .ncol {
    display: grid;
    justify-items: end;
  }
  .cursor {
    position: absolute;
    top: 0;
    bottom: 0;
    left: calc(var(--who) + var(--gap) + (100% - var(--who) - var(--ncol) - 2 * var(--gap)) * var(--p, 0));
    border-left: 1px solid var(--ink);
    opacity: 0.55;
    transition: left 220ms linear;
    pointer-events: none;
  }
  .starting {
    position: absolute;
    inset: 0 0 auto;
    height: min(100%, 480px);
    z-index: 1;
    display: grid;
    grid-template-columns: var(--who) minmax(0, 1fr) var(--ncol);
    gap: var(--gap);
    align-items: center;
    pointer-events: none;
  }
  .starting > :global(.dial) {
    grid-column: 2;
    justify-self: center;
  }
  @media (max-width: 720px) {
    .live {
      --who: 140px;
      --ncol: 0px;
    }
    .ncol {
      display: none;
    }
    .lrow {
      grid-template-columns: var(--who) minmax(0, 1fr);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .q.sent {
      animation: none;
    }
    .cursor,
    .fill {
      transition: none;
    }
  }

  .note {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    font-size: var(--t-xs);
    color: var(--ink-2);
  }
  .note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .summary {
    padding: 22px 26px;
    display: grid;
    gap: 14px 40px;
    grid-template-columns: minmax(0, 1fr) minmax(260px, 400px);
    grid-template-areas:
      'head picks'
      'clock picks';
    align-items: start;
  }
  .summary .head {
    grid-area: head;
    display: grid;
    gap: 12px;
  }
  .summary .ht {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .summary .lead.clock {
    grid-area: clock;
  }
  @media (max-width: 1000px) {
    .summary {
      grid-template-columns: minmax(0, 1fr);
      grid-template-areas:
        'head'
        'picks'
        'clock';
    }
  }
  .summary h2 {
    font-size: var(--t-xl);
    font-weight: 660;
    letter-spacing: -0.018em;
    line-height: 1.22;
  }
  .lead {
    font-size: var(--t-sm);
    color: var(--ink-2);
    max-width: 72ch;
  }
  .picks {
    grid-area: picks;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 6px 28px;
    margin: 0;
  }
  .picks > div {
    padding-top: 8px;
    border-top: 1px solid var(--rule);
  }
  .picks dt {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .picks dd {
    margin: 2px 0 0;
    font-size: var(--t-sm);
  }
  .tablewrap {
    overflow: auto;
    max-height: calc(100vh - 150px);
    min-height: 240px;
  }
  table {
    width: 100%;
    border-collapse: separate;
    border-spacing: 0;
    font-size: var(--t-sm);
  }
  th,
  td {
    padding: 7px 12px;
    border-bottom: 1px solid var(--rule);
    text-align: left;
    vertical-align: top;
    background: var(--surface);
  }
  thead th {
    background: var(--surface-2);
    font-size: var(--t-xs);
    font-weight: 560;
    color: var(--ink-3);
    white-space: nowrap;
  }
  thead th button {
    all: unset;
    cursor: default;
  }
  thead th[aria-sort='ascending'] button::after {
    content: ' ↑';
  }
  thead th[aria-sort='descending'] button::after {
    content: ' ↓';
  }
  th.first {
    position: sticky;
    left: 0;
    z-index: 1;
    min-width: 230px;
    font-weight: 400;
    color: var(--ink);
  }
  thead th.first {
    z-index: 3;
    background: var(--surface-2);
  }
  .r {
    text-align: right;
    white-space: nowrap;
  }
  .src {
    white-space: nowrap;
  }
  tr.none td,
  tr.none th .nm {
    color: var(--ink-3);
  }
  .fit {
    white-space: nowrap;
  }
  .role {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 8px;
    border: 1px solid var(--rule-2);
    border-radius: 12px;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .role.use {
    background: var(--accent-soft);
    border-color: color-mix(in oklab, var(--accent) 50%, var(--rule));
    color: var(--ink);
    font-weight: 600;
  }
  .role.avoid {
    border-color: color-mix(in oklab, var(--warning) 60%, var(--rule));
    color: var(--warning);
  }
  .fit .why {
    display: block;
    margin-top: 3px;
    max-width: 24ch;
    white-space: normal;
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .how {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    cursor: default;
    color: var(--ink);
  }
  .how:hover {
    color: var(--accent-text);
  }
  .how:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
    border-radius: var(--r-sm);
  }
  tr.working td {
    background: var(--surface-2);
    white-space: normal;
    padding: 4px 16px 6px 20px;
  }
  td.win,
  .mm dd.win {
    font-weight: 640;
    color: var(--ink);
  }
  .lead.clock {
    padding-top: 10px;
    border-top: 1px solid var(--rule);
  }
  .ok {
    color: var(--good);
    margin-left: 4px;
    vertical-align: -1px;
  }
  .off {
    color: var(--warning);
    margin-left: 4px;
    vertical-align: -1px;
  }
  .host.off {
    margin-left: 0;
    vertical-align: baseline;
  }
  .nts.yes {
    color: var(--good);
    font-weight: 600;
  }
  .nts.no {
    color: var(--ink-2);
  }
  .feat td {
    font-size: var(--t-xs);
  }
  .feat .pol {
    min-width: 280px;
    color: var(--ink-2);
  }
  .empty {
    font-size: var(--t-sm);
    color: var(--ink-2);
    max-width: 72ch;
    padding: 0 4px;
  }
  /* Phone: the setup line wraps under its heading instead of running off the edge. */
  :global(html.mobile) .sum {
    flex-basis: 100%;
    flex-wrap: wrap;
    gap: 2px 10px;
  }
  :global(html.mobile) .sum .num {
    flex: initial;
  }
  :global(html.mobile) .chosen {
    white-space: normal;
  }
  .mlist {
    display: grid;
    border-top: 1px solid var(--rule);
  }
  .mrow {
    display: grid;
    gap: 3px;
    padding: 12px 2px;
    border-bottom: 1px solid var(--rule);
    min-width: 0;
  }
  .mrow.none {
    color: var(--ink-3);
  }
  .mtop {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    min-height: 32px;
  }
  .mrow .nm {
    font-size: var(--t-base);
    font-weight: 620;
  }
  .mnote {
    font-size: var(--t-sm);
  }
  .mm {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 8px 12px;
    margin: 8px 0 0;
  }
  .mm dt {
    font-size: var(--t-2xs);
    color: var(--ink-3);
  }
  .mm dd {
    margin: 0;
    font-size: var(--t-sm);
    overflow-wrap: anywhere;
  }
  .mm .wide {
    grid-column: 1 / -1;
  }
  .mrow .why {
    font-size: var(--t-xs);
    color: var(--ink-3);
  }
  .mwork {
    margin-top: 10px;
    overflow-x: auto;
  }
  /* Phone: Start sits in a footer that stays in reach while the page scrolls; it cancels the content padding to run edge to edge. */
  .startbar {
    position: sticky;
    /* Sticky honours the scroller's bottom padding; cancel it so the bar meets the screen edge. */
    bottom: calc(-32px - var(--inset-b));
    z-index: var(--z-sticky);
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 12px;
    margin: 16px -16px calc(-32px - var(--inset-b));
    padding: 10px 16px calc(10px + var(--inset-b));
    border-top: 1px solid var(--rule);
    background: var(--surface);
  }
  .startbar .est {
    font-size: var(--t-sm);
    color: var(--ink-2);
  }
  .startbar .btn {
    min-width: 112px;
    min-height: 44px;
    font-size: var(--t-base);
  }
  /* Phone: five presets wrap to two rows, and Add matches the input's height. */
  :global(html.mobile) .seg.five {
    grid-template-columns: repeat(3, 1fr);
  }
  :global(html.mobile) .custom {
    align-items: stretch;
  }
  :global(html.mobile) .custom input {
    height: auto;
    min-height: 44px;
    font-size: 16px;
  }
  :global(html.mobile) .custom .btn {
    min-height: 44px;
  }
</style>
