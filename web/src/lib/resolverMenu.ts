// The right-click menu for a resolver row, wherever it appears.

import { app } from './store.svelte';
import { copyText, SEPARATOR } from './api';
import { menu } from './menu.svelte';
import { label } from './format';
import { allKinds, blocker, canRun, confirmLive, run, runAll } from './checks.svelte';

function resolverMenu(e: MouseEvent, id: string) {
  const r = app.resolvers.find((x) => x.id === id) ?? app.run?.resolvers.find((x) => x.id === id);
  if (!r) return;
  const addr = r.ipv4[0] ?? r.ipv6[0] ?? r.doh ?? r.dot ?? '';
  const planned = app.draft?.resolvers.includes(id) ?? false;
  const idle = !blocker();
  menu.show(
    e,
    [
      { id: 'inspect', label: 'Show in Inspector' },
      SEPARATOR,
      { id: 'all', label: 'Run All Checks', enabled: idle },
      ...(app.env?.trace !== 'unsupported'
        ? [{ id: 'trace', label: 'Trace Path', enabled: canRun('trace', r) && idle }]
        : []),
      { id: 'dive', label: 'Sample 18 Queries', enabled: idle },
      { id: 'checks', label: 'Check Behaviour', enabled: idle },
      { id: 'live', label: 'Check Live Feeds', enabled: idle && canRun('live', r) },
      SEPARATOR,
      { id: 'copy-addr', label: `Copy Address${addr ? ` (${addr})` : ''}`, enabled: !!addr },
      { id: 'copy-name', label: 'Copy Name' },
      SEPARATOR,
      { id: 'plan', label: planned ? 'Leave Out of Next Benchmark' : 'Include in Next Benchmark', enabled: !!app.draft },
    ],
    (pick) => {
      if (pick === 'inspect') app.inspect = id;
      else if (pick === 'all') {
        app.inspect = id;
        void runAll(r.id, allKinds(r));
      } else if (pick === 'trace' || pick === 'dive' || pick === 'checks') {
        app.inspect = id;
        void run(pick, r.id);
      } else if (pick === 'live') {
        void confirmLive().then((ok) => {
          if (!ok) return;
          app.inspect = id;
          void run('live', r.id, { plain: false });
        });
      }
      else if (pick === 'copy-addr') copyText(addr);
      else if (pick === 'copy-name') copyText(label(r.provider, r.variant));
      else if (pick === 'plan' && app.draft) {
        app.draft.resolvers = planned ? app.draft.resolvers.filter((x) => x !== id) : [...app.draft.resolvers, id];
        app.saveDraft();
        app.notify(planned ? `${label(r.provider, r.variant)} left out of the next benchmark.` : `${label(r.provider, r.variant)} added to the next benchmark.`);
      }
    },
  );
}

/** Delegate: right-click anywhere inside a `[data-resolver]` row. */
export function onResolverContext(e: MouseEvent) {
  const row = (e.target as HTMLElement).closest<HTMLElement>('[data-resolver]');
  if (row?.dataset.resolver) resolverMenu(e, row.dataset.resolver);
}
