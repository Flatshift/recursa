<script lang="ts">
  // The capability probes while they run, as a board of answers. Each resolver gets one tile per check, grouped as the
  // Capabilities view groups them; a tile turns over to its glyph when the resolver's report lands, filled with its
  // group's colour when the check passes. Rows stay put, fastest cached lookup first, and the foot of each column counts
  // how many resolvers pass that check so far. Eight are probed at a time.
  import type { CheckStatus, Resolver, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { answered, capCell } from '../lib/cells';
  import { CAP, CAP_KEYS } from '../lib/caps';
  import type { Stage } from './PhaseStages.svelte';
  import LiveBoard, { type BoardGroup, type Phase } from './LiveBoard.svelte';

  let { run, rows }: { run: RunState; rows: Row[] } = $props();

  const AT_ONCE = 8;
  // The column heads, short enough for a tile.
  const CODE: Record<string, string> = {
    dnssec: 'DNSSEC',
    ecs: 'ECS',
    qmin: 'QNAME min',
    rebind: 'Rebinding',
    filter_malware: 'Malware',
    filter_phishing: 'Phishing',
    filter_ads: 'Ads',
    ede: 'EDE',
    edns_version: 'EDNS',
    cookies: 'Cookies',
    tcp: 'TCP',
    dot: 'DoT',
    doh: 'DoH',
  };
  const GROUPS: BoardGroup[] = [
    { title: 'Privacy & security', tone: 'var(--s-cached)', checks: CAP_KEYS.privacy },
    {
      title: 'Blocking',
      tone: 'var(--w-amber)',
      // A resolver that filters nothing answers all three with a note rather than a verdict: one cell says so.
      none: 'No filtering',
      checks: CAP_KEYS.blocking,
    },
    { title: 'Protocol', tone: 'var(--s-tld)', checks: CAP_KEYS.protocol },
  ];
  const TOTAL = GROUPS.reduce((n, g) => n + g.checks.length, 0);

  // The engine works through the run's own order, so the next few without a report are the ones in flight.
  const flying = $derived(new Set(run.resolvers.filter((r) => !run.probes[r.id] && answered(run, r.id)).slice(0, AT_ONCE).map((r) => r.id)));
  const phase = (r: Resolver): Phase => (run.probes[r.id] ? 'done' : !answered(run, r.id) ? 'off' : flying.has(r.id) ? 'run' : 'queued');

  // Every probed resolver's answers, one per check in board order. Unprobed ones have none yet, rather than the
  // operator's published claims the finished views fall back to.
  const answers = $derived.by(() => {
    const out: Record<string, { status: CheckStatus; tip: string }[]> = {};
    for (const r of run.resolvers) {
      if (!run.probes[r.id]) continue;
      out[r.id] = GROUPS.flatMap((g) =>
        g.checks.map((key) => {
          const c = capCell(run, r, key);
          return { status: c.status, tip: `${CAP[key].label}: ${c.text}` };
        }),
      );
    }
    return out;
  });
  // How many probed resolvers pass each check, for the foot of its column.
  const tally = $derived(
    Array.from({ length: TOTAL }, (_, i) => Object.values(answers).filter((a) => a[i].status === 'pass').length),
  );

  const stages = $derived.by((): Stage[] => {
    const total = run.total || run.resolvers.length;
    return [
      {
        label: 'Probe each resolver',
        value: `${run.done} of ${total}${flying.size ? `, ${flying.size} in flight` : ''}`,
        share: total ? run.done / total : 1,
        state: run.done >= total ? 'done' : 'now',
        tone: 'var(--s-tld)',
      },
    ];
  });
</script>

<LiveBoard
  {run}
  {rows}
  name="probes"
  heading="Probing capabilities"
  lede="Each resolver is asked about privacy, blocking and protocol support, {AT_ONCE} at a time, fastest cached lookup first. A filled tile is a check it passes."
  stagesLabel="Probe progress"
  {stages}
  groups={GROUPS}
  heads={CODE}
  {answers}
  {tally}
  scored={TOTAL}
  {phase}
  words={{ run: 'probing', queued: 'queued' }}
  cascade
  askMs={60}
  turnMs={32}
/>
