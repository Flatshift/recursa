// The checks the Inspector runs on one resolver and the Resolvers list runs on several, with their results kept for the
// session. The engine runs one check at a time, so one runs here at a time too.

import { api, confirmAction } from './api';
import { app } from './store.svelte';
import type {
  DeepChecks,
  DeepDive,
  LiveBlocking,
  Resolver,
  Trace,
} from './types';

export type CheckKind = 'trace' | 'dive' | 'checks' | 'live';

/** Results by resolver id. Live-feed checks with the same `sample_taken_at` tested the same hosts, so they compare directly. */
export const results = $state<{
  trace: Record<string, Trace>;
  dive: Record<string, DeepDive>;
  checks: Record<string, DeepChecks>;
  live: Record<string, LiveBlocking>;
}>({ trace: {}, dive: {}, checks: {}, live: {} });

/** When each check last returned a result, by `kind:rid`. */
export const ranAt = $state<Record<string, number>>({});

/** The last error from each check, by `kind:rid`; cleared when that check runs again. */
const errors = $state<Record<string, string>>({});

/** The check in flight; `all` while Run all checks is working through its list. `batch` while the Resolvers list works
 *  through its selection: how many resolvers are done, and whether Stop was pressed. */
const running = $state<{
  now: { kind: CheckKind; rid: string; all: boolean } | null;
  batch: { total: number; done: number; stop: boolean } | null;
}>({ now: null, batch: null });

/** How long the engine reuses one sample of live hosts (`LIVE_SAMPLE_TTL_MS`). */
export const LIVE_SAMPLE_TTL_MS = 30 * 60_000;

const PATH: Record<CheckKind, string> = {
  trace: 'trace',
  dive: 'deep-dive',
  checks: 'deep-checks',
  live: 'live-blocking',
};

export const errorOf = (kind: CheckKind, rid: string): string | null =>
  errors[`${kind}:${rid}`] ?? null;
export const isRunning = (kind: CheckKind, rid: string): boolean =>
  running.now?.kind === kind && running.now.rid === rid;

/** Why no check can start right now, for the disabled buttons' tooltips; null when one can. */
export function blocker(): string | null {
  if (running.batch) return 'Checking the selected resolvers';
  if (running.now) return 'Another check is running';
  if (app.run?.status === 'running') return 'A benchmark is running';
  return null;
}

async function exec(kind: CheckKind, rid: string, body?: unknown) {
  delete errors[`${kind}:${rid}`];
  try {
    const r = await api.post<unknown>(
      `/api/resolvers/${encodeURIComponent(rid)}/${PATH[kind]}`,
      body,
    );
    (results[kind] as Record<string, unknown>)[rid] = r;
    ranAt[`${kind}:${rid}`] = Date.now();
  } catch (e) {
    errors[`${kind}:${rid}`] = e instanceof Error ? e.message : String(e);
  }
}

export async function run(kind: CheckKind, rid: string, body?: unknown) {
  if (blocker()) return;
  running.now = { kind, rid, all: false };
  try {
    await exec(kind, rid, body);
  } finally {
    running.now = null;
  }
}

/** The checks "Run all" runs: the path when a trace can run here as the user, then the sample and the behaviour checks. */
export function allKinds(r: Resolver): CheckKind[] {
  return (['trace', 'dive', 'checks'] as const).filter((k) => canRun(k, r));
}

export const RUN_ALL_TITLE =
  'Trace path, sample 18 queries and check behaviour, one after another';
/** True while "Run all" is working through a resolver's checks. */
export const runningAll = (rid: string) =>
  running.now?.all === true && running.now.rid === rid;

/** The given checks one after another; an error in one is shown in its section and the rest still run. */
export async function runAll(rid: string, kinds: CheckKind[]) {
  if (blocker()) return;
  try {
    for (const kind of kinds) {
      running.now = { kind, rid, all: true };
      await exec(kind, rid);
    }
  } finally {
    running.now = null;
  }
}

/** Whether `kind` can run on `r` here: a trace needs an IPv4 address and the rights to send it; live names need DoT or
 *  DoH, or a resolver on this network. */
export function canRun(kind: CheckKind, r: Resolver): boolean {
  if (kind === 'trace') return r.ipv4.length > 0 && app.env?.trace === 'ok';
  if (kind === 'live') return !!(r.dot || r.doh) || r.group === 'system';
  return true;
}

/** The Resolvers list's batch in progress: resolvers done, how many in all, the one being checked, and whether Stop
 *  was pressed. */
export function batchProgress(): { done: number; total: number; rid: string | null; stopping: boolean } | null {
  const b = running.batch;
  return b && { done: b.done, total: b.total, rid: running.now?.rid ?? null, stopping: b.stop };
}

/** Ends the batch once the check in flight returns. */
export function stopBatch() {
  if (running.batch) running.batch.stop = true;
}

/** The given checks on each resolver in turn, skipping those a resolver can't take; an error is kept for its Inspector
 *  view and the rest still run. Live names never go over plain DNS here except to a resolver on this network. */
export async function runBatch(rs: Resolver[], kinds: CheckKind[]) {
  if (blocker() || !rs.length) return;
  running.batch = { total: rs.length, done: 0, stop: false };
  const b = running.batch;
  try {
    for (const r of rs) {
      for (const kind of kinds) {
        if (b.stop) return;
        if (!canRun(kind, r)) continue;
        running.now = { kind, rid: r.id, all: false };
        await exec(kind, r.id, kind === 'live' ? { plain: false } : undefined);
      }
      b.done++;
    }
  } finally {
    running.now = null;
    running.batch = null;
  }
}

/** Asks before Check live feeds starts from a menu, where the warning in its Inspector view isn't in sight. */
export const confirmLive = () =>
  confirmAction(
    'Check live feeds looks up real malicious hosts. Security tools on the network may flag the lookups or block this device. The names go only over DoT or DoH, or plain DNS to your own resolver on this network.',
    'Check Live Feeds',
  );

/** What a running check is doing, for its section's status line. */
export function phase(kind: CheckKind, name: string, ip: string): string {
  switch (kind) {
    case 'trace':
      return `Probing the path to ${ip}…`;
    case 'dive':
      return 'Sending 18 queries…';
    case 'checks':
      return 'Checking behaviour: truncation, TTLs, DNSSEC, caches…';
    case 'live':
      return `Confirming live hosts, then asking ${name}…`;
  }
}
