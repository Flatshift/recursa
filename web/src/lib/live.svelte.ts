import type { DeepChecks, LiveBlocking } from './types';

/** The live-feed blocking checks made this session, by resolver id. Checks with the same `sample_taken_at` tested
 *  the same hosts, so their block rates compare directly. */
export const liveChecks = $state<Record<string, LiveBlocking>>({});

/** How long the engine reuses one sample of live hosts (`LIVE_SAMPLE_TTL_MS`). */
export const LIVE_SAMPLE_TTL_MS = 30 * 60_000;

/** The Deep Dive's further checks made this session, by resolver id. */
export const deeperChecks = $state<Record<string, DeepChecks>>({});

/** Whether deeper checks are running. The engine runs one check at a time, so the page's other buttons wait on it. */
export const deeper = $state<{ busy: boolean; rid: string | null }>({ busy: false, rid: null });
