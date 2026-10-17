export type Transport = 'udp' | 'tcp' | 'dot' | 'doh';
export type TestKind = 'cached' | 'uncached' | 'tld';
export type Outcome =
  | 'ok'
  | 'nxdomain'
  | 'nodata'
  | 'servfail'
  | 'refused'
  | 'rcode'
  | 'hijack'
  | 'blocked'
  | 'from_cache'
  | 'timeout'
  | 'unreachable'
  | 'neterr';

export interface Published {
  dnssec?: boolean | null;
  ecs?: boolean | null;
  qmin?: boolean | null;
  filtering: string[];
  doq?: boolean | null;
  dnscrypt?: boolean | null;
  odoh?: boolean | null;
  logging?: string | null;
  jurisdiction?: string | null;
}

export interface Resolver {
  id: string;
  provider: string;
  name: string;
  variant: string;
  group: 'public' | 'legacy' | 'system' | 'custom';
  default: boolean;
  udp: boolean;
  ipv4: string[];
  ipv6: string[];
  dot?: string | null;
  doh?: string | null;
  status_feed?: string | null;
  published: Published;
  notes?: string | null;
  links: { home?: string | null; privacy?: string | null };
  /** On a system resolver: the custom entry that names it (same address). */
  custom_id?: string;
  /** Custom entries folded into this catalog entry. */
  aliases?: string[];
  /** On a system resolver that is a local forwarder (dnsmasq, Pi-hole, a router's DNS proxy): what it is. */
  forwarder?: string | null;
  /** What the operator publishes about how much one client may send; only present when it publishes something. */
  limits?: { qps?: number | null; per?: string | null; text: string; source: string } | null;
}

interface Summary {
  sent: number;
  ok: number;
  timeouts: number;
  errors: number;
  success: number;
  min: number | null;
  p25: number | null;
  p50: number | null;
  p75: number | null;
  p90: number | null;
  p95: number | null;
  p99: number | null;
  max: number | null;
  mean: number | null;
  sd: number | null;
  iqr: number | null;
}

export interface Target {
  idx: number;
  resolver_id: string;
  provider: string;
  variant: string;
  transport: Transport;
  family: 'v4' | 'v6';
  ip: string;
  endpoint: string;
  bootstrap: string | null;
  state: 'pending' | 'ready' | 'failed' | 'unreachable' | 'dropped';
  reason: string | null;
  setup_ms: number | null;
  stats: Partial<Record<TestKind, Summary>>;
  intercepted: boolean;
  hops: number | null;
  /** Share of Cached-test answers given from the resolver's cache, judged by TTL (absent in older runs). */
  cache_hits?: number | null;
}

export interface Sample {
  t: number;
  k: TestKind;
  i: number;
  us: number;
  o: Outcome;
  at: number;
  /** IP TTL the answer arrived with (0 = unknown). */
  h: number;
  /** DNS TTL of the first answer record, in seconds (absent for negative answers and older runs). */
  d?: number;
}

/** A warm-up answer: shown while the run warms up, never recorded. */
export interface WarmSample {
  t: number;
  k: TestKind;
  us: number;
}

export type CheckStatus = 'pass' | 'fail' | 'partial' | 'info' | 'unknown';

export interface Check {
  key: string;
  status: CheckStatus;
  present: boolean | null;
  value: string;
  evidence: string;
}

interface FilterSample {
  domain: string;
  blocked: boolean | null;
  how: string;
}

interface FilterCategory {
  tested: number;
  blocked: number;
  source: string;
  samples: FilterSample[];
}

interface ProbeReport {
  resolver_id: string;
  via: string;
  checks: Check[];
  egress: string[];
  /** ECS as Google's nameservers saw it. */
  ecs: string | null;
  /** ECS as Akamai's nameservers saw it; resolvers can treat the two differently. */
  ecs_akamai: string | null;
  nsid: string | null;
  pop: string | null;
  filtering: Record<string, FilterCategory>;
  block_style: string | null;
  duration_ms: number;
}

interface IpInfo {
  ip: string;
  asn?: number | null;
  prefix?: string | null;
  holder?: string | null;
  rpki?: {
    /** valid, invalid_asn, invalid_length, unknown (no ROA covers it), or not_checked (see `state` and `reason`). */
    status: string;
    state?: 'off' | 'syncing' | 'waiting' | 'error' | 'ready';
    reason?: string;
    roas?: { origin: string; prefix: string; max_length: number; validity: string }[];
    validator?: string | null;
    /** When the route-origin data was synced, ms since the epoch. */
    vrp_time?: number;
  } | null;
  rov?: { ratio: number; organization: string; updated: string } | null;
  private?: boolean;
  error?: string;
  fetched_at?: number;
}

interface ResolverIntel {
  service: IpInfo | null;
  egress: IpInfo | null;
}

export interface Edge {
  ip: string;
  /** Minimum handshake RTT: the best estimate of path latency. */
  rtt_ms: number | null;
  /** Older runs predate the median and the refused flag. */
  median_ms?: number | null;
  samples: number[];
  /** Answered with a reset: still a round trip. */
  refused?: boolean;
  asn: number | null;
  holder: string | null;
  on_net: boolean;
}

export type AsYou = 'same' | 'near' | 'different';
export type HostStatus = 'ok' | 'failed' | 'blocked' | 'unreachable' | 'off_cdn';

/** One hostname through one resolver. */
interface HostMapping {
  ips: string[];
  /** Last name in the CNAME chain, the one the CDN's own nameservers answer for. */
  target: string | null;
  /** Repeated lookups returned different edges. */
  rotated: boolean;
  status: HostStatus;
  error: string | null;
  /** S: median over the edges of each one's best round trip. */
  rtt_ms: number | null;
  /** p: S minus the best any resolver (or the CDN's own answer) found for this name. */
  penalty_ms: number | null;
  /** The penalty is under the noise floor (or 5% of the best). */
  same: boolean | null;
  as_you: AsYou | null;
}

/** One resolver's result for a CDN. */
export interface Mapping {
  rtt_ms: number | null;
  penalty_ms: number | null;
  /** Same edges (or as near) as the CDN gives this computer's own address; null without a reference. */
  as_you: AsYou | null;
  error: string | null;
  worst_host: [string, number] | null;
  hosts_used: number;
  hosts_total: number;
  hosts: Record<string, HostMapping>;
}

/** The CDN's own answer for this computer: its nameserver asked directly, no resolver in between. */
export interface Reference {
  name: string;
  server: string;
  ips: string[];
  rtt_ms: number | null;
  penalty_ms: number | null;
  error: string | null;
}

interface Coverage {
  cdns_used: number;
  cdns_total: number;
  hosts_used: number;
  hosts_total: number;
}

interface ResolverCdn {
  mappings: Record<string, Mapping>;
  /** The headline: mean penalty over the steered CDNs every ranked resolver has. */
  penalty_ms: number | null;
  tier?: number | null;
  worst: [string, number] | null;
  coverage?: Coverage;
  basis: 'on_net' | 'ecs' | 'ecs_substitute' | 'resolver_location' | 'unknown';
  ecs: string | null;
  ecs_akamai: string | null;
  ecs_is_client: boolean | null;
}

interface HostInfo {
  name: string;
  /** Every resolver (and the CDN's own answer) returned the same edges. */
  uniform: boolean;
  /** A steered name that came back uniform, or an anycast control that didn't. */
  unexpected: boolean;
  you: Reference | null;
}

export interface CdnInfo {
  id: string;
  name: string;
  role: 'steered' | 'control';
  hosts: HostInfo[];
  best_ms: number | null;
}

export interface CdnReport {
  /** 2 for the multi-hostname test; older reports have none and are not shown. */
  version: number;
  /** The noise floor δ: how far two handshakes to one edge differ by chance in this run. */
  noise_ms: number;
  cdns: CdnInfo[];
  edges: Record<string, Edge>;
  resolvers: Record<string, ResolverCdn>;
}

interface PathInfo {
  rtt_ms: number | null;
  port: number;
  samples: number[];
}

interface Note {
  level: 'info' | 'warn';
  text: string;
  resolver_id: string | null;
}

export interface RunConfig {
  resolvers: string[];
  transports: Transport[];
  family: 'v4' | 'v6' | 'both';
  tests: TestKind[];
  samples: number;
  mode: 'accurate' | 'quick';
  probes: boolean;
  filters: { threats: boolean; ads: boolean; adult: boolean; plain_threats: boolean; system_threats: boolean };
  cdn: boolean;
  /** The names the Cached test asks, in rotation. */
  cached_names: string[];
}

interface Calibration {
  clock: string;
  kernel_rx: boolean;
  samples: number;
  floor_p50_us: number | null;
  floor_p99_us: number | null;
  wake_p50_us: number | null;
  wake_p99_us: number | null;
  at: number;
}

export interface Vantage {
  ipv4: string | null;
  ipv6: string | null;
  ipv6_reachable: boolean;
  network?: IpInfo;
  geo?: { country?: string } | null;
}

export interface Interception {
  verdict: 'direct' | 'intercepted' | 'partial' | 'unknown';
  checked_at: number;
  link: { interface: string; wifi: boolean } | null;
  results: {
    ip: string;
    name: string;
    path_ms: number | null;
    udp_ms: number | null;
    hops: number | null;
    on_lan: boolean;
    intercepted: boolean;
  }[];
  /** Who answered the first locally answered address, from CHAOS questions, TCP and a name it hadn't seen. */
  fingerprint?: {
    ip: string;
    name: string;
    kind: 'match' | 'elsewhere' | 'other' | 'passes' | 'silent';
    summary: string;
    udp: { version: string | null; id: string | null };
    tcp: { version: string | null; id: string | null; port53: 'intercepted' | 'direct' | 'closed' | 'unknown' };
  } | null;
}

export interface Env {
  interception?: Interception | null;
  platform: string;
  version: string;
  /** Git commit and build time of the running binary. */
  build: string;
  clock: string;
  kernel_rx: boolean;
  calibration: Calibration | null;
  vantage: Vantage | null;
  system: string[];
  db: string;
  /** Whether this build can check route origins on this computer. */
  rpki_local?: boolean;
}

/** The local route-origin (RPKI) check, as `GET /api/rpki` reports it. */
export interface RpkiStatus {
  enabled: boolean;
  state: 'off' | 'syncing' | 'ready' | 'error';
  /** On, but nothing has synced yet and nothing is running (a measurement is in progress). */
  waiting: boolean;
  last_sync: number | null;
  vrps: number;
  disk_bytes: number;
  syncing_since: number | null;
  /** Publication points the running sync has read. */
  points: number;
  error: string | null;
  validator: string;
}

/** How the link to the gateway behaved while a run measured. Times in ms; samples are [ms into the run, µs], 0 µs = lost. */
export interface LinkStats {
  gateway: string;
  sent: number;
  lost: number;
  p50: number;
  p95: number;
  max: number;
  stalls: number;
  samples: [number, number][];
  /** What the Mac itself was doing, and the cause named from it. Absent in runs from before cause detection. */
  host?: LinkHost;
}

/** Why a noisy link stalled, as far as this Mac could tell: null when the link was not noisy. */
export type LinkCause = 'busy' | 'traffic' | 'awdl' | 'driver' | 'unknown';

/** One row per second: [ms into the run, loopback echo µs, tick lag µs, en0 rx bps, en0 tx bps, en0 errors+drops, awdl0 bps]. null = not readable. */
type HostSample = [number, number | null, number, number | null, number | null, number | null, number | null];

interface LinkHost {
  samples: HostSample[];
  cause: LinkCause | null;
}

export interface LinkUnavailable {
  state: 'unavailable';
  reason: string;
}

export interface RunState {
  id: string;
  started_at: number;
  finished_at: number | null;
  status: 'running' | 'done' | 'cancelled' | 'failed';
  phase: string;
  done: number;
  total: number;
  config: RunConfig;
  env: Env;
  resolvers: Resolver[];
  targets: Target[];
  samples: Sample[];
  /** Warm-up answers, present only while the run is live. */
  warm?: WarmSample[];
  probes: Record<string, ProbeReport>;
  intel: Record<string, ResolverIntel>;
  notes: Note[];
  cdn: CdnReport | null;
  /** Absent in runs from before the link check, and when it is turned off. */
  link?: LinkStats | LinkUnavailable;
  paths: Record<string, PathInfo>;
  /** The most queries per second Recursa sent to each resolver in any one second. */
  rates?: Record<string, number>;
  seq: number;
}

export interface RunRow {
  id: string;
  started_at: number;
  finished_at: number | null;
  status: string;
  /** Kept by hand: skipped by Clear all runs and by automatic cleanup. */
  saved: boolean;
  summary: {
    /** The fastest pick; `tied_with` names the next resolver when the run can't tell them apart (older runs were picked with the CDN cost included and have none). */
    best?: { resolver_id: string; label: string; transport: Transport; composite_ms: number; success: number; tied_with?: string | null } | null;
    resolvers?: number;
    targets?: number;
    queries?: number;
    vantage?: string | null;
  };
}

/** Automatic cleanup of unsaved runs; `null` switches a rule off. */
export interface Housekeeping {
  keep_runs: number | null;
  keep_days: number | null;
}

export interface MonitorConfig {
  enabled: boolean;
  interval_min: number;
  resolvers: string[];
  retention_days: number;
}

/** One history cell: [start, samples, answered, p50 ms, p95 ms, histogram]. The histogram is
 *  `[first bin, counts…]` over eighth-of-a-decade log-ms bins, bin 0 starting at 0.32 ms. */
export type MonitorCell = [number, number, number, number | null, number | null, number[]];

interface MonitorOutage {
  start: number;
  end: number;
  cycles: number;
  recovered: number | null;
}

export interface MonitorHist {
  resolver_id: string;
  cycles: number;
  uptime: number | null;
  query_success: number | null;
  p50: number | null;
  p95: number | null;
  first: number | null;
  last: number | null;
  buckets: MonitorCell[];
  outages: MonitorOutage[];
}

/** `cell`: one history cell in ms (an hour, or several for long windows). */
export interface MonitorHistory {
  hours: number;
  since: number;
  cell: number;
  resolvers: MonitorHist[];
}

export interface Bootstrap {
  env: Env;
  resolvers: Resolver[];
  defaults: RunConfig;
  monitor: MonitorConfig;
  active_run: string | null;
  runs: RunRow[];
  method: {
    cached_names: string[];
    uncached_zones: string[];
    cdns: { id: string; name: string; role: 'steered' | 'control'; hosts: string[] }[];
  };
}

export interface StatusFeed {
  feed: string;
  name: string;
  page: string;
  ok: boolean;
  /** The operator's state as it applies to where you are (see `locality`). */
  state: string;
  /** The operator's own overall state. */
  global_state?: string;
  /** How the report relates to you: `elsewhere` (only other places, so `state` is operational), `local`
   *  (names your country or region), `global` (names no place), `unknown` (names places, yours isn't known). */
  locality?: 'elsewhere' | 'local' | 'global' | 'unknown';
  /** Where the report applies and reaches you, and where it applies and doesn't. */
  where?: string;
  elsewhere?: string[];
  client?: { code: string; country: string };
  component?: string | null;
  affected?: string[];
  incidents: {
    title: string;
    status: string;
    started: string | null;
    resolved: string | null;
    url: string | null;
    update: string | null;
  }[];
}

export interface CuratedIncident {
  id: string;
  providers: string[];
  date: string;
  start: string | null;
  end: string | null;
  kind: 'outage' | 'routing' | 'degradation' | 'ecosystem' | 'user-reported' | 'change';
  confidence: 'operator' | 'unconfirmed';
  scope: string;
  title: string;
  summary: string;
  cause: string;
  source: string;
}

/** A time server in the NTP catalog: what its operator says, not what was measured. */
export interface NtpEntry {
  id: string;
  name: string;
  operator: string;
  kind: 'service' | 'lab' | 'pool' | 'custom';
  host: string;
  port?: number | null;
  country?: string | null;
  nts?: string | null;
  nts_documented: boolean;
  nts_only: boolean;
  leap?: string | null;
  anycast?: boolean | null;
  policy?: string | null;
  link?: string | null;
  default: boolean;
}

interface NtpAddr {
  ip: string;
  sent: number;
  ok: number;
  kiss?: string | null;
  error?: string | null;
  /** Why it failed, as a short stable name (`timeout`, `kiss`, `unsynchronized`, `bad_reply`, `network`, ...). */
  error_kind?: string | null;
  offset_ms?: number | null;
  delay_min_ms?: number | null;
  delay_ms?: number | null;
  bound_ms?: number | null;
  /** Root distance: the most the offset can be wrong by against UTC itself. */
  distance_ms?: number | null;
  jitter_ms?: number | null;
}

interface NtsKe {
  ok: boolean;
  host: string;
  port: number;
  ms?: number | null;
  cookies: number;
  aead?: number | null;
  server?: string | null;
  ntp_port?: number | null;
  error?: string | null;
  /** Why the key exchange failed: `timeout`, `network`, `certificate_invalid`, `ke_handshake_failed`, `missing_cookies`, `dns`. */
  error_kind?: string | null;
  /** A time query sealed with the negotiated keys got a reply that verified. Null when none was tried. */
  authenticated?: boolean | null;
  /** What the authenticated queries found; its `error_kind` says why one didn't verify (`aead_failure`, `unauthenticated_response`, ...). */
  timed?: NtpAddr | null;
}

export interface NtpResult {
  id: string;
  host: string;
  addrs: NtpAddr[];
  error?: string | null;
  /** Why nothing was measured, as a short stable name. */
  error_kind?: string | null;
  /** A host you typed: judged against the others, never one of them. */
  custom?: boolean;
  port?: number | null;
  /** The server's headline numbers: the median over its addresses. */
  offset_ms?: number | null;
  delay_min_ms?: number | null;
  delay_ms?: number | null;
  delay_p95_ms?: number | null;
  jitter_ms?: number | null;
  /** The most the offset can be wrong by from path asymmetry alone: half the smallest delay. */
  bound_ms?: number | null;
  /** The most the offset can be wrong by against UTC itself (root distance): the server's own error plus the path bound. */
  distance_ms?: number | null;
  /** How far apart its addresses' offsets are, lowest to highest. */
  addr_spread_ms?: number | null;
  /** Two of its addresses disagree by more than their error bounds allow. */
  addrs_disagree?: boolean;
  stratum?: number | null;
  refid?: string | null;
  root_delay_ms?: number | null;
  root_dispersion_ms?: number | null;
  sent: number;
  ok: number;
  kiss?: string | null;
  deviation_ms?: number | null;
  /** Whether the deviation is within what this server's and the consensus's error bounds allow; null with no consensus. */
  agrees?: boolean | null;
  /** Among the majority whose error intervals overlap, or outside it; absent if it didn't vote or no majority overlapped. */
  selection?: 'truechimer' | 'falseticker' | null;
  nts?: NtsKe | null;
  /** Absent when IPv6 wasn't tried. */
  ipv6?: 'answers' | 'unreachable' | 'no address' | null;
  nts_only: boolean;
  /** Who runs it, from the catalogue. */
  operator?: string;
}

export interface NtpRun {
  id: string;
  status: 'running' | 'done' | 'cancelled';
  started_at: number;
  finished_at: number | null;
  total: number;
  done: number;
  samples: number;
  nts: boolean;
  results: NtpResult[];
  /** bound_ms: how far the median can be wrong, the surest voter's error ceiling (voters share this computer's path). */
  consensus: {
    median_ms: number;
    spread_ms: number;
    voters: number;
    /** Voters that fail independently: one per operator and per upstream server. The verdict needs three of these. */
    independent: number;
    bound_ms: number;
    /** Up to four servers to configure together, best first; servers to leave out with the first reason found; and qualified servers passed over, with why. */
    advice: { set: string[]; avoid: { id: string; why: string }[]; passed: { id: string; why: string }[] };
  } | null;
  /** Reference servers added because every server in the run was one you typed. */
  added?: string[];
  /** Servers left out because they were asked a few minutes ago, or told us to stop. */
  skipped?: { id: string; host: string; why: 'recent' | 'kiss'; until: number; kiss?: string | null }[];
  spacing_s?: number;
}

/** What the background NTP monitor asks, and how often. */
export interface NtpMonitorConfig {
  enabled: boolean;
  interval_min: number;
  /** Catalogue ids, or hosts you typed. */
  servers: string[];
  retention_days: number;
}

export interface NtpMonitorPoint {
  ts: number;
  offset_ms: number | null;
  lo_ms: number | null;
  hi_ms: number | null;
  bound_ms: number | null;
  delay_ms: number | null;
  sent: number;
  ok: number;
}

interface NtpMonitorStats {
  sent: number;
  ok: number;
  up_pct: number;
  flaps: number;
  offset_median_ms: number | null;
  offset_spread_ms: number | null;
  delay_median_ms: number | null;
  delay_p95_ms: number | null;
  /** Median delay for each hour of the day (UTC). */
  delay_by_hour: (number | null)[];
  falseticks: number;
  voted: number;
  last_ts: number | null;
  last_ok: boolean | null;
  last_kind: string | null;
  stratum: number | null;
}

export interface NtpMonitorServer {
  id: string;
  stats: NtpMonitorStats;
  points: NtpMonitorPoint[];
}

interface NtpMonitorClock {
  points: { ts: number; median_ms: number; bound_ms: number; independent: number }[];
  cycles: number;
  median_ms: number | null;
  spread_ms: number | null;
  trend_ms_per_hour: number | null;
}

export interface NtpMonitorResp {
  config: NtpMonitorConfig;
  last: number | null;
  paused: boolean;
  history: { since: number; until: number; bucket_ms: number; cycles: number; servers: NtpMonitorServer[]; clock: NtpMonitorClock };
}

/** A Deep Dive: a short, fixed look at one resolver. Only round trips and response fields are measured; the cache findings are inferred. */
export interface DeepDive {
  transport: string;
  name: string;
  queries: number;
  duration_ms: number;
  latency: {
    samples_ms: number[];
    stats: Summary;
    /** At least one reply had no kernel receive stamp, so the times include scheduler wake-up. */
    user_space_timed: boolean;
  };
  cache: {
    cold_ms: number | null;
    cold_samples_ms: number[];
    warm_ms: number | null;
    recursion_cost_ms: number | null;
  };
  ttl: {
    first: number | null;
    last: number | null;
    elapsed_s: number;
    suggests: 'counting_down' | 'unchanged' | 'refreshed' | 'irregular' | 'undetermined';
  };
  response: {
    rcode: number;
    rcode_name: string;
    flags: { qr: boolean; aa: boolean; tc: boolean; rd: boolean; ra: boolean; ad: boolean; cd: boolean };
    answers: number;
    authority: number;
    additional: number;
    size_bytes: number;
    edns: { udp_size: number; version: number; dnssec_ok: boolean } | null;
    ede: { code: number; name: string; text: string }[];
    nsid: string | null;
  } | null;
  /** Every query sent, in order; `ms` is null when nothing usable came back. */
  timeline: { kind: DeepDiveStep; ms: number | null }[];
  aborted: string | null;
}

export type DeepDiveStep = 'popular' | 'random' | 'repeat' | 'last';
