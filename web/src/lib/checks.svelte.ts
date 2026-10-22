// The checks a resolver's Inspector and page can run on demand, with their results kept for the session.
// The engine runs one check at a time, so one runs here at a time too.

import { api } from "./api";
import { app } from "./store.svelte";
import type {
  DeepChecks,
  DeepDive,
  LiveBlocking,
  Resolver,
  Trace,
} from "./types";

export type CheckKind = "trace" | "dive" | "checks" | "live";

/** Results by resolver id. Live-feed checks with the same `sample_taken_at` tested the same hosts, so they compare directly. */
export const results = $state<{
  trace: Record<string, Trace>;
  dive: Record<string, DeepDive>;
  checks: Record<string, DeepChecks>;
  live: Record<string, LiveBlocking>;
}>({ trace: {}, dive: {}, checks: {}, live: {} });

/** The last error from each check, by `kind:rid`; cleared when that check runs again. */
export const errors = $state<Record<string, string>>({});

/** The check in flight; `all` while Run all checks is working through its list. */
export const running = $state<{
  now: { kind: CheckKind; rid: string; all: boolean } | null;
}>({ now: null });

/** How long the engine reuses one sample of live hosts (`LIVE_SAMPLE_TTL_MS`). */
export const LIVE_SAMPLE_TTL_MS = 30 * 60_000;

const PATH: Record<CheckKind, string> = {
  trace: "trace",
  dive: "deep-dive",
  checks: "deep-checks",
  live: "live-blocking",
};

export const errorOf = (kind: CheckKind, rid: string): string | null =>
  errors[`${kind}:${rid}`] ?? null;
export const isRunning = (kind: CheckKind, rid: string): boolean =>
  running.now?.kind === kind && running.now.rid === rid;

/** Why no check can start right now, for the disabled buttons' tooltips; null when one can. */
export function blocker(): string | null {
  if (running.now) return "Another check is running";
  if (app.run?.status === "running") return "A benchmark is running";
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
  const trace = r.ipv4.length > 0 && app.env?.trace === "ok";
  return trace ? ["trace", "dive", "checks"] : ["dive", "checks"];
}

export const RUN_ALL_TITLE =
  "Trace path, sample 18 queries and check behaviour, one after another";
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

/** What a running check is doing, for its section's status line. */
export function phase(kind: CheckKind, name: string, ip: string): string {
  switch (kind) {
    case "trace":
      return `Probing the path to ${ip}…`;
    case "dive":
      return "Sending 18 queries…";
    case "checks":
      return "Checking behaviour: truncation, TTLs, DNSSEC, caches…";
    case "live":
      return `Confirming live hosts, then asking ${name}…`;
  }
}
