import { api } from './api';
import type { CuratedIncident, Resolver, StatusFeed } from './types';

interface StatusResponse {
  live: { feeds: StatusFeed[]; fetched_at: number | null };
  history: CuratedIncident[];
  measuring: boolean;
}

class StatusStore {
  data = $state<StatusResponse | null>(null);
  loading = $state(false);
  error = $state<string | null>(null);
  #inflight: Promise<void> | null = null;

  load(force = false): Promise<void> {
    if (this.#inflight) return this.#inflight;
    if (this.data && !force) return Promise.resolve();
    this.loading = true;
    this.#inflight = api
      .get<StatusResponse>(`/api/status${force ? '?force=1' : ''}`)
      .then((d) => {
        this.data = d;
        this.error = null;
      })
      .catch((e) => {
        this.error = (e as Error).message;
      })
      .finally(() => {
        this.loading = false;
        this.#inflight = null;
      });
    return this.#inflight ?? Promise.resolve();
  }

  /** Resolver ids with any monitoring history from this network; null until loaded. */
  recorded = $state<Set<string> | null>(null);

  async loadRecorded(): Promise<void> {
    try {
      this.recorded = new Set(await api.get<string[]>('/api/monitor/recorded'));
    } catch {
      /* the caution just stays off */
    }
  }

  /** Whether anything records this resolver's reliability: its operator's status feed, or monitoring here.
   *  Undefined until both have loaded, so nothing is claimed early. */
  hasRecord(r: Resolver): boolean | undefined {
    if (!this.data || !this.recorded) return undefined;
    if (this.feedFor(r.status_feed)?.ok) return true;
    return [r.id, r.custom_id, ...(r.aliases ?? [])].some((id) => !!id && this.recorded!.has(id));
  }

  feedFor(feed: string | null | undefined): StatusFeed | undefined {
    if (!feed) return undefined;
    return this.data?.live.feeds.find((f) => f.feed === feed);
  }

  incidentsFor(id: string): CuratedIncident[] {
    return (this.data?.history ?? []).filter((i) => i.providers.includes(id) || i.providers.includes('*'));
  }
}

export const status = new StatusStore();

const STATE_LABEL: Record<string, string> = {
  operational: 'Operational',
  degraded_performance: 'Degraded',
  partial_outage: 'Partial outage',
  major_outage: 'Major outage',
  under_maintenance: 'Maintenance',
  unknown: 'Unknown',
};

/** The feed's state for you. A regional problem elsewhere leaves you "operational", so say where. */
export function stateLabel(f: StatusFeed): string {
  const s = STATE_LABEL[f.state] ?? f.state;
  if (f.locality !== 'elsewhere' || !f.global_state || f.global_state === f.state) return s;
  return f.client?.country ? `${s} in ${f.client.country}` : `${s} for you`;
}

/** The report about other places, short enough for a table cell ("degraded in Asia"); null when there is none. */
export function elsewhereNote(f: StatusFeed): string | null {
  if (f.locality !== 'elsewhere' || !f.global_state || !f.elsewhere?.length) return null;
  const said = (STATE_LABEL[f.global_state] ?? f.global_state).toLowerCase();
  return `${said} in ${f.elsewhere.join(' and ')}`;
}

/** What the operator's report means for where you are, in a sentence; null when it has nothing to add. */
export function localNote(f: StatusFeed): string | null {
  if (!f.locality) return null;
  const said = STATE_LABEL[f.global_state ?? f.state] ?? f.state;
  const you = f.client?.country ?? 'your location';
  switch (f.locality) {
    case 'elsewhere':
      // The state label already names your country when it is known.
      return `${said} in ${f.elsewhere?.join(' and ')}.${f.client?.country ? '' : ` Not affecting ${you}.`}`;
    case 'local':
      return f.where?.includes(you) ? `${said} in ${f.where}.` : `${said} in ${f.where}, which includes ${you}.`;
    case 'global':
      return `${said}. The operator doesn’t say where, so this may reach ${you}.`;
    case 'unknown':
      return `${said} in ${f.where}. Your location isn’t known, so this may or may not reach you.`;
  }
}

export function stateGlyph(s: string): 'pass' | 'partial' | 'fail' | 'unknown' | 'info' {
  if (s === 'operational') return 'pass';
  if (s === 'degraded_performance' || s === 'under_maintenance') return 'partial';
  if (s === 'partial_outage' || s === 'major_outage') return 'fail';
  return 'unknown';
}
