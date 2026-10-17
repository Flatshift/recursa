import { api } from './api';
import type { Check, RunRow, RunState } from './types';

/** Probe checks per resolver, per finished run. Run bodies are large; only this part is kept. */
const cache = new Map<string, Record<string, Check[]>>();

/**
 * The newest measured result of one probe check for each resolver, looking back over the last
 * few finished runs (a resolver left out of the latest run keeps its earlier measurement).
 */
export async function latestChecks(runs: RunRow[], key: string, lookback = 6): Promise<Map<string, Check>> {
  const out = new Map<string, Check>();
  for (const row of runs.filter((r) => r.status === 'done').slice(0, lookback)) {
    let probes = cache.get(row.id);
    if (!probes) {
      try {
        const run = await api.get<RunState>(`/api/runs/${row.id}`);
        probes = Object.fromEntries(Object.entries(run.probes ?? {}).map(([id, p]) => [id, p.checks]));
      } catch {
        continue;
      }
      cache.set(row.id, probes);
    }
    for (const [id, checks] of Object.entries(probes)) {
      if (out.has(id)) continue;
      const c = checks.find((c) => c.key === key && c.status !== 'unknown');
      if (c) out.set(id, c);
    }
  }
  return out;
}
