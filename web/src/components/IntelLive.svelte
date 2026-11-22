<script lang="ts">
  // The network intel while it runs, as a board of answers. Each resolver gets a tile per check (whether invalid
  // routes are filtered over IPv4 and IPv6, and the network its queries leave from); a tile turns over
  // when the resolver's report lands, filled with its group's colour when the check passes. Every resolver that
  // answered is checked at once; the route-filtering test is a live session with Check My DNS that can take half a
  // minute, so the rows land one by one. Rows stay put, fastest cached lookup first.
  import type { Resolver, RovCheck, RunState } from '../lib/types';
  import type { Row } from '../lib/verdict';
  import { answered, egressCell, type Cell } from '../lib/cells';
  import type { Stage } from './PhaseStages.svelte';
  import LiveBoard, { type BoardGroup, type Phase } from './LiveBoard.svelte';

  let { run, rows }: { run: RunState; rows: Row[] } = $props();

  const GROUPS: BoardGroup[] = [
    { title: 'Invalid routes filtered', tone: 'var(--s-tld)', checks: ['IPv4', 'IPv6'], n: 2 },
    { title: 'Queries leave from', tone: 'var(--w-amber)', checks: ['Network'], n: 3, named: { noun: 'network', lead: 'queries leave from' } },
  ];

  const state = (r: Resolver): Phase => (run.intel[r.id] ? 'done' : answered(run, r.id) ? 'run' : 'off');

  const rov = (c: RovCheck): Cell => ({
    status: c.status === 'filtered' ? 'pass' : c.status === 'unfiltered' ? 'fail' : 'unknown',
    text: c.status === 'filtered' ? 'Filtered' : c.status === 'unfiltered' ? 'Not filtered' : 'Not tested',
    prov: c.status === 'not_tested' ? 'unknown' : 'verified',
    evidence: c.detail,
  });
  // Every reported resolver's answers, one per tile in board order.
  const answers = $derived.by(() => {
    const out: Record<string, { status: Cell['status']; text: string; tip: string }[]> = {};
    for (const r of run.resolvers) {
      const v = run.intel[r.id];
      if (!v) continue;
      const eg = egressCell(run, r.id);
      const fam = v.rov ? [rov(v.rov.v4), rov(v.rov.v6)] : [];
      out[r.id] = [
        ...(['IPv4', 'IPv6'] as const).map((f, i) => {
          const c = fam[i];
          return c
            ? { status: c.status, text: c.text, tip: `${f}: ${c.text}, ${c.evidence}` }
            : { status: 'unknown' as const, text: 'Not tested', tip: `${f}: not tested` };
        }),
        { status: eg.status, text: eg.text, tip: `Queries leave from ${eg.text}${eg.evidence ? ` · ${eg.evidence}` : ''}` },
      ];
    }
    return out;
  });
  const reported = $derived(Object.keys(answers).length);
  const flying = $derived(run.resolvers.filter((r) => state(r) === 'run').length);
  // How many reported resolvers pass each check, and how many networks the queries leave from, for the foot.
  const tally = $derived(Array.from({ length: 2 }, (_, i) => Object.values(answers).filter((a) => a[i].status === 'pass').length));
  const nets = $derived(new Set(Object.values(answers).filter((a) => a[2].status !== 'unknown').map((a) => a[2].text)).size);

  const naming = $derived(run.cdn_naming);
  const stages = $derived.by((): Stage[] => {
    const total = run.total || run.resolvers.length;
    const out: Stage[] = [
      {
        label: 'Check each resolver',
        value: `${reported} of ${total}${flying ? `, ${flying} in flight` : ''}`,
        share: total ? reported / total : 1,
        state: reported >= total ? 'done' : 'now',
        tone: 'var(--s-cached)',
      },
    ];
    if (run.config.cdn && run.cdn) {
      // The engine's count arrives with the first edge it names; until then the edges found are the total.
      const named = naming?.done ?? 0, toName = naming?.total ?? Object.keys(run.cdn.edges).length;
      out.push({
        label: 'Name the CDN edges',
        value: toName ? `${named} of ${toName} edges` : 'done',
        share: toName ? named / toName : 1,
        state: named >= toName ? 'done' : 'now',
        tone: 'var(--w-rose)',
      });
    }
    return out;
  });
</script>

<LiveBoard
  {run}
  {rows}
  name="intel"
  heading="Network intel"
  lede="Each resolver’s network is looked up: whether invalid routes are filtered on its path (a live test with Check My DNS by DNS-OARC, up to half a minute each), and where its queries leave for the Internet. A filled tile is a check it passes."
  stagesLabel="Network intel progress"
  {stages}
  groups={GROUPS}
  {answers}
  {tally}
  namedCount={nets}
  scored={2}
  phase={state}
  words={{ run: 'checking' }}
  upright
  announce
  askMs={90}
  turnMs={48}
/>
