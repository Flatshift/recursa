//! Benchmark orchestration.
//!
//! A run moves through: setup (bootstrap DoH/DoT endpoints, prepare filter test sets,
//! open connections) → warm-up (prime ARP/NAT/route caches and resolver caches; detect
//! unreachable targets) → measured phases (cached, uncached, TLD) → path check
//! (interception) → capability probes → CDN mapping → network intel. Measurement happens
//! on dedicated, prioritized OS threads. Within each round every target gets one query in
//! a freshly shuffled order, so no resolver gains from when it was tested. No other
//! network activity is allowed while measured phases run.

use crate::catalog::Resolver;
use crate::dns::{self, QuerySpec};
use crate::endpoints;
use crate::intel::Intel;
use crate::net::{self, Endpoint, Exchanger, Transport, UdpX, XErr};
use crate::probes::{self, check, rand_label};
use crate::{cdn, clock, link, stats};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Pause after each query in Accurate mode; the Deep Dive spaces its few queries the same way.
pub(crate) const ACCURATE_GAP: Duration = Duration::from_millis(4);

pub const CACHED_NAMES: &[&str] = &[
    "www.google.com",
    "www.youtube.com",
    "www.wikipedia.org",
    "www.amazon.com",
    "www.apple.com",
    "www.microsoft.com",
    "www.netflix.com",
    "www.reddit.com",
];
/// Unsigned wildcard zones: every random label resolves, and no aggressive-NSEC
/// synthesis is possible, so each query forces the resolver to ask the authority.
/// Each is hosted by a different DNS provider (NS1, Vercel, Google Cloud DNS, Route 53),
/// so the Uncached test isn't a measure of the path to one of them. Setup checks them first.
pub const UNCACHED_ZONES: &[&str] = &["github.io", "vercel.app", "firebaseapp.com", "onrender.com"];
/// TLDs for the TLD test, each run by a different registry operator (Verisign, Identity Digital, Nominet).
/// Each proves a name doesn't exist with hashed, opt-out NSEC3, from which no resolver may answer a different
/// name (RFC 8198). Setup checks them first.
const TLD_ZONES: &[&str] = &["com", "org", "uk"];

/// The zones the random-name tests rotate through, after the setup checks.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fresh {
    pub zones: Vec<String>,
    pub tlds: Vec<String>,
}

impl Fresh {
    pub(crate) fn defaults() -> Self {
        Fresh { zones: UNCACHED_ZONES.iter().map(|z| z.to_string()).collect(), tlds: TLD_ZONES.iter().map(|t| t.to_string()).collect() }
    }
}

/// What a timeout costs, in ms, when the composite counts it: a stub resolver waits about a second
/// before it asks again. The UI mirrors this.
pub const TIMEOUT_COST_MS: f64 = 1000.0;

/// Consecutive timeouts after which a target is dropped from the rest of the run.
const DROP_AFTER: u32 = 6;
/// Share of reliable answers needed to rank (mirrors the UI).
pub const RELIABLE: f64 = 0.98;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestKind {
    Cached,
    Uncached,
    Tld,
}

impl TestKind {
    const ALL: [TestKind; 3] = [TestKind::Cached, TestKind::Uncached, TestKind::Tld];

    pub fn key(self) -> &'static str {
        match self {
            TestKind::Cached => "cached",
            TestKind::Uncached => "uncached",
            TestKind::Tld => "tld",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Accurate,
    Quick,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    #[default]
    V4,
    V6,
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilterProbes {
    /// Malware and phishing blocking, with the test names blocking services publish. Live feed hosts are only
    /// looked up from a Deep Dive (see [`crate::deepdive::live_blocking`]).
    #[serde(default = "yes")]
    pub threats: bool,
    #[serde(default = "yes")]
    pub ads: bool,
    /// Adult-content blocking, with published test names. Off by default: the lookups are visible to the network.
    #[serde(default)]
    pub adult: bool,
}

impl Default for FilterProbes {
    fn default() -> Self {
        FilterProbes { threats: true, ads: true, adult: false }
    }
}

fn yes() -> bool {
    true
}
fn default_transports() -> Vec<Transport> {
    vec![Transport::Udp]
}
fn default_tests() -> Vec<TestKind> {
    TestKind::ALL.to_vec()
}
fn default_samples() -> u32 {
    30
}
const QUERY_TIMEOUT_MS: u64 = 2000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunConfig {
    pub resolvers: Vec<String>,
    #[serde(default = "default_transports")]
    pub transports: Vec<Transport>,
    #[serde(default)]
    pub family: Family,
    #[serde(default = "default_tests")]
    pub tests: Vec<TestKind>,
    #[serde(default = "default_samples")]
    pub samples: u32,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default = "yes")]
    pub probes: bool,
    #[serde(default)]
    pub filters: FilterProbes,
    /// Test how well each resolver maps this client onto CDN edges.
    #[serde(default = "yes")]
    pub cdn: bool,
    /// The names the Cached test asks, in rotation. Checked when the run starts; a name that doesn't resolve
    /// is left out and said so. A run made before this existed used the default list.
    #[serde(default = "crate::names::defaults")]
    pub cached_names: Vec<String>,
}

impl RunConfig {
    pub fn with_resolvers(resolvers: Vec<String>) -> Self {
        RunConfig {
            resolvers,
            transports: default_transports(),
            family: Family::default(),
            tests: default_tests(),
            samples: default_samples(),
            mode: Mode::default(),
            probes: true,
            filters: FilterProbes::default(),
            cdn: true,
            cached_names: crate::names::defaults(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Nxdomain,
    Nodata,
    Servfail,
    Refused,
    Rcode,
    Hijack,
    /// A filter's block answer (0.0.0.0, a loopback address, or a "blocked" or "filtered" error code).
    Blocked,
    /// An Uncached answer that came from a cache (its TTL was below the authority's). Left out of the stats.
    FromCache,
    Timeout,
    Unreachable,
    Neterr,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Sample {
    /// Target index.
    pub t: u16,
    pub k: TestKind,
    /// Round.
    pub i: u32,
    /// Round trip in microseconds (0 when there was no answer).
    pub us: u32,
    pub o: Outcome,
    /// Milliseconds since the run started.
    pub at: u32,
    /// IP TTL the answer arrived with (0 = unknown or not UDP).
    pub h: u8,
    /// DNS TTL of the first answer record, in seconds (none for negative or empty answers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d: Option<u32>,
}

/// A warm-up answer, shown live and never recorded: warm-up queries prime caches and paths and do not count.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WarmSample {
    /// Target index.
    pub t: u16,
    pub k: TestKind,
    /// Round trip in microseconds.
    pub us: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetInfo {
    pub idx: usize,
    pub resolver_id: String,
    pub provider: String,
    pub variant: String,
    pub transport: Transport,
    pub family: &'static str,
    pub ip: String,
    pub endpoint: String,
    /// How a DoT/DoH address was found (bypassing the system resolver).
    pub bootstrap: Option<String>,
    /// pending | ready | failed | unreachable | dropped
    pub state: String,
    /// Why a target isn't ready.
    pub reason: Option<String>,
    pub setup_ms: Option<f64>,
    pub stats: BTreeMap<String, stats::Summary>,
    /// Plain-DNS answers arrived faster than the network path allows, or with a TTL
    /// showing they never crossed a router: a device on this network answered.
    pub intercepted: bool,
    /// Median router hops the plain-DNS answers crossed (from their IP TTL).
    pub hops: Option<u8>,
    /// Share of Cached-test answers the resolver gave from its cache, judged by TTL (none when no answer could be judged).
    pub cache_hits: Option<f64>,
}

impl TargetInfo {
    fn new(r: &Resolver, transport: Transport, v6: bool, ip: IpAddr) -> Self {
        TargetInfo {
            idx: 0,
            resolver_id: r.id.clone(),
            provider: r.provider.clone(),
            variant: r.variant.clone(),
            transport,
            family: if v6 { "v6" } else { "v4" },
            ip: ip.to_string(),
            endpoint: String::new(),
            bootstrap: None,
            state: "pending".into(),
            reason: None,
            setup_ms: None,
            stats: BTreeMap::new(),
            intercepted: false,
            hops: None,
            cache_hits: None,
        }
    }

    fn fail(&mut self, state: &str, reason: impl Into<String>) {
        self.state = state.into();
        self.reason = Some(reason.into());
    }
}

/// Network round trip to a resolver's address, from TCP handshakes. Physics sets the
/// floor: no genuine answer can arrive faster than this.
#[derive(Clone, Debug, Serialize)]
pub struct PathInfo {
    pub rtt_ms: Option<f64>,
    pub port: u16,
    pub samples: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Note {
    pub level: &'static str,
    pub text: String,
    pub resolver_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunState {
    pub id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// running | done | cancelled | failed
    pub status: String,
    pub phase: String,
    pub done: u32,
    pub total: u32,
    pub config: RunConfig,
    pub env: Value,
    pub resolvers: Vec<Resolver>,
    pub targets: Vec<TargetInfo>,
    pub samples: Vec<Sample>,
    /// Warm-up answers, for the live view only. Cleared when the run ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warm: Vec<WarmSample>,
    pub probes: BTreeMap<String, probes::ProbeReport>,
    pub intel: BTreeMap<String, Value>,
    pub notes: Vec<Note>,
    pub cdn: Option<cdn::CdnReport>,
    /// How the link to the gateway behaved while measuring (see [`link`]); absent in runs from before the check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<Value>,
    pub paths: BTreeMap<String, PathInfo>,
    /// The most queries per second Recursa sent to each resolver in any one second (all its transports count:
    /// limits are per client address), from the measured phases.
    pub rates: BTreeMap<String, f64>,
    /// When each phase began, in ms since the run started: where a run's time went. Absent in older runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timings: Vec<(String, i64)>,
    pub seq: u64,
}

pub struct RunHandle {
    pub id: String,
    pub cancel: AtomicBool,
    state: Mutex<RunState>,
    tx: broadcast::Sender<Arc<str>>,
}

impl RunHandle {
    /// The run state. A panic on the run thread must not lock everyone out, so a
    /// poisoned lock is recovered rather than propagated.
    pub fn state(&self) -> MutexGuard<'_, RunState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Mutate state and broadcast the resulting event atomically. Snapshots are taken
    /// under the same lock, so a subscriber sees each change exactly once: either in
    /// the snapshot or as an event with a higher `seq`.
    fn publish(&self, f: impl FnOnce(&mut RunState) -> Value) {
        let mut s = self.state();
        let mut v = f(&mut s);
        s.seq += 1;
        v["seq"] = json!(s.seq);
        let _ = self.tx.send(v.to_string().into());
    }
    fn with<R>(&self, f: impl FnOnce(&mut RunState) -> R) -> R {
        f(&mut self.state())
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// Note when a step began, for `timings`.
    fn mark(&self, step: &str) {
        self.with(|s| s.timings.push((step.into(), now_ms() - s.started_at)));
    }
    fn phase(&self, phase: &str, total: u32) {
        self.mark(phase);
        self.publish(|s| {
            s.phase = phase.into();
            s.done = 0;
            s.total = total;
            json!({"type": "phase", "phase": phase, "total": total})
        });
    }
    fn emit_targets(&self) {
        self.publish(|s| json!({"type": "targets", "targets": s.targets}));
    }
    fn note(&self, level: &'static str, text: String, resolver_id: Option<String>) {
        let n = Note { level, text, resolver_id };
        self.publish(|s| {
            s.notes.push(n.clone());
            json!({"type": "note", "note": n})
        });
    }
    fn set_target(&self, idx: usize, f: impl FnOnce(&mut TargetInfo)) {
        self.with(|s| f(&mut s.targets[idx]));
    }
    /// Subscribe and take a consistent snapshot in one step.
    pub fn subscribe(&self) -> (broadcast::Receiver<Arc<str>>, String) {
        let s = self.state();
        let rx = self.tx.subscribe();
        let snap = json!({"type": "snapshot", "run": &*s, "seq": s.seq}).to_string();
        (rx, snap)
    }
}

pub struct Deps {
    pub resolvers: Vec<Resolver>,
    pub intel: Arc<Intel>,
    pub rt: tokio::runtime::Handle,
    pub env: Value,
    pub client_v6: bool,
    pub measuring: Arc<AtomicBool>,
    pub run_active: Arc<AtomicBool>,
    pub on_finish: Box<dyn FnOnce(&RunState) + Send>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn new_id() -> String {
    format!("{:x}{:04x}", now_ms(), fastrand::u16(..))
}

pub fn spawn(mut cfg: RunConfig, mut deps: Deps) -> Arc<RunHandle> {
    cfg.samples = cfg.samples.clamp(3, 500);
    cfg.tests.sort();
    cfg.tests.dedup();
    // Each transport once, in the order given ([udp, tcp, udp] would measure UDP twice).
    let mut seen = std::collections::HashSet::new();
    cfg.transports.retain(|t| seen.insert(*t));
    let selected: Vec<Resolver> = cfg.resolvers.iter().filter_map(|id| deps.resolvers.iter().find(|r| &r.id == id).cloned()).collect();
    let (tx, _) = broadcast::channel(4096);
    let id = new_id();
    let state = RunState {
        id: id.clone(),
        started_at: now_ms(),
        finished_at: None,
        status: "running".into(),
        phase: "setup".into(),
        done: 0,
        total: 0,
        config: cfg.clone(),
        env: deps.env.clone(),
        resolvers: selected.clone(),
        targets: vec![],
        samples: vec![],
        warm: vec![],
        probes: BTreeMap::new(),
        intel: BTreeMap::new(),
        notes: vec![],
        cdn: None,
        link: None,
        paths: BTreeMap::new(),
        rates: BTreeMap::new(),
        timings: vec![],
        seq: 0,
    };
    let h = Arc::new(RunHandle { id, cancel: AtomicBool::new(false), state: Mutex::new(state), tx });
    let hh = h.clone();
    deps.run_active.store(true, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("recursa-run".into())
        .spawn(move || {
            clock::prioritize_thread();
            let on_finish = std::mem::replace(&mut deps.on_finish, Box::new(|_| {}));
            let (measuring, run_active) = (deps.measuring.clone(), deps.run_active.clone());
            let crashed = catch_unwind(AssertUnwindSafe(|| run(&hh, &cfg, &selected, &deps))).is_err();
            // Always leave the app able to start another run, even after a panic.
            measuring.store(false, Ordering::SeqCst);
            if crashed {
                hh.note("warn", "Recursa hit an internal error and stopped this run early. The results so far are kept.".into(), None);
            }
            let cancelled = hh.cancelled();
            hh.with(|s| {
                s.status = if crashed {
                    "failed"
                } else if cancelled {
                    "cancelled"
                } else {
                    "done"
                }
                .into();
                s.phase = "done".into();
                s.finished_at = Some(now_ms());
                s.timings.push(("done".into(), now_ms() - s.started_at));
                s.warm.clear();
            });
            let final_state = hh.with(|s| s.clone());
            on_finish(&final_state);
            run_active.store(false, Ordering::SeqCst);
            hh.publish(|s| json!({"type": "done", "run": &*s}));
        })
        .expect("spawn run thread");
    h
}

pub(crate) fn query_name(test: TestKind, round: u32) -> String {
    query_name_in(test, round, &crate::names::defaults(), &Fresh::defaults())
}

/// The zone an Uncached query in this round asks under (every target gets the same zone in a round).
fn zone_for(round: u32, zones: &[String]) -> &str {
    zones.get(round as usize % zones.len().max(1)).map(String::as_str).unwrap_or(UNCACHED_ZONES[0])
}

/// The TLD a TLD-test query in this round asks under, likewise.
fn tld_for(round: u32, tlds: &[String]) -> &str {
    tlds.get(round as usize % tlds.len().max(1)).map(String::as_str).unwrap_or(TLD_ZONES[0])
}

/// The name for one query: the Cached test rotates through `cached`, the Uncached and TLD tests through `fresh`.
pub(crate) fn query_name_in(test: TestKind, round: u32, cached: &[String], fresh: &Fresh) -> String {
    match test {
        TestKind::Cached if !cached.is_empty() => cached[round as usize % cached.len()].clone(),
        TestKind::Cached => CACHED_NAMES[round as usize % CACHED_NAMES.len()].to_string(),
        TestKind::Uncached => format!("ub{}.{}", rand_label(12), zone_for(round, &fresh.zones)),
        TestKind::Tld => format!("ub{}.{}", rand_label(14), tld_for(round, &fresh.tlds)),
    }
}

/// A filter's block answer: 0.0.0.0/8 or a loopback address in place of the real one, or an
/// extended error saying the name was blocked, censored or filtered (EDE 15 to 17).
fn blocked(m: &dns::Message) -> bool {
    m.ede().iter().any(|(c, _)| matches!(c, 15..=17))
        || m.ipv4_answers().iter().any(|a| a.octets()[0] == 0 || a.is_loopback())
        || m.ipv6_answers().iter().any(|a| a.is_unspecified() || a.is_loopback())
}

/// A real NXDOMAIN for a name directly under a TLD carries that TLD's own SOA. One without it was made up on the
/// way (a filter that answers random-looking names itself, say), not fetched from the registry.
fn from_registry(m: &dns::Message) -> bool {
    let tld = m.qname.trim_end_matches('.').rsplit('.').next().unwrap_or("");
    !tld.is_empty() && m.authority.iter().any(|r| r.rtype == dns::TYPE_SOA && r.name.trim_end_matches('.').eq_ignore_ascii_case(tld))
}

/// The outcome, the round trip in µs, and the TTL of the first answer record. The reply is parsed after
/// the receive stamp, so none of this is inside the measured interval.
pub(crate) fn classify(test: TestKind, r: &Result<net::Reply, XErr>) -> (Outcome, u32, Option<u32>) {
    let reply = match r {
        Ok(r) => r,
        Err(XErr::Timeout) => return (Outcome::Timeout, 0, None),
        Err(XErr::Refused) => return (Outcome::Unreachable, 0, None),
        Err(_) => return (Outcome::Neterr, 0, None),
    };
    let us = (reply.rtt_ns / 1000).min(u32::MAX as u64) as u32;
    let b = &reply.bytes;
    if b.len() < 12 {
        return (Outcome::Neterr, us, None);
    }
    // Negative answers carry the zone's SOA, whose TTL says nothing about this name, so only answer records count.
    let msg = dns::parse(b).ok();
    let ttl = msg.as_ref().and_then(|m| m.answers.first()).map(|r| r.ttl);
    if msg.as_ref().is_some_and(blocked) {
        return (Outcome::Blocked, us, ttl);
    }
    let rcode = (b[3] & 0x0F) as u16;
    let ancount = u16::from_be_bytes([b[6], b[7]]);
    let o = match (test, rcode) {
        (TestKind::Tld, dns::RCODE_NXDOMAIN) if msg.as_ref().is_some_and(|m| !from_registry(m)) => Outcome::Blocked,
        (TestKind::Tld, dns::RCODE_NXDOMAIN) => Outcome::Ok,
        (TestKind::Tld, dns::RCODE_NOERROR) if ancount > 0 => Outcome::Hijack,
        (_, dns::RCODE_NOERROR) if ancount > 0 => Outcome::Ok,
        (_, dns::RCODE_NOERROR) => Outcome::Nodata,
        (_, dns::RCODE_NXDOMAIN) => Outcome::Nxdomain,
        (_, dns::RCODE_SERVFAIL) => Outcome::Servfail,
        (_, dns::RCODE_REFUSED) => Outcome::Refused,
        _ => Outcome::Rcode,
    };
    (o, us, ttl)
}

/// Endpoints this run already bootstrapped for a resolver.
fn planned_for(plan: &[(TargetInfo, Option<Endpoint>)], resolver_id: &str) -> Vec<Endpoint> {
    plan.iter().filter(|(t, _)| t.resolver_id == resolver_id).filter_map(|(_, e)| e.clone()).collect()
}

fn offers(r: &Resolver, t: Transport) -> bool {
    match t {
        Transport::Udp | Transport::Tcp => r.udp,
        Transport::Dot => r.dot.is_some(),
        Transport::Doh => r.doh.is_some(),
    }
}

fn expand(
    set: Setup,
    cfg: &RunConfig,
    selected: &[Resolver],
    client_v6: bool,
    timeout: Duration,
    h: &RunHandle,
) -> Vec<(TargetInfo, Option<Endpoint>)> {
    let families: &[bool] = match cfg.family {
        Family::V4 => &[false],
        Family::V6 => &[true],
        Family::Both => &[false, true],
    };
    let mut out = Vec::new();
    let mut not_offered: Vec<String> = Vec::new();
    for r in selected {
        let mut transports: Vec<Transport> = cfg.transports.iter().copied().filter(|t| offers(r, *t)).collect();
        if r.udp {
            let missing: Vec<&str> = cfg.transports.iter().filter(|t| !offers(r, **t)).map(|t| t.label()).collect();
            if !missing.is_empty() {
                not_offered.push(format!("{} ({})", r.label(), missing.join(", ")));
            }
        } else if transports.is_empty() {
            let t = if r.doh.is_some() { Transport::Doh } else { Transport::Dot };
            transports.push(t);
            h.note(
                "info",
                format!("{} doesn't answer plain DNS, so it was measured over {} instead.", r.label(), t.label()),
                Some(r.id.clone()),
            );
        }
        for &v6 in families {
            let Some(ip) = r.primary(v6) else {
                if v6 {
                    h.note("info", format!("{} publishes no IPv6 address.", r.label()), Some(r.id.clone()));
                }
                continue;
            };
            for &t in &transports {
                let mut info = TargetInfo::new(r, t, v6, ip);
                let ep = if v6 && !client_v6 {
                    info.fail("failed", "This network has no IPv6 connectivity.");
                    None
                } else if matches!(t, Transport::Udp | Transport::Tcp) {
                    let ep = endpoints::plain(r, t, v6);
                    info.endpoint = ep.as_ref().map(|e| e.addr.to_string()).unwrap_or_default();
                    ep
                } else if set.over(h) {
                    info.fail("failed", "Setup stopped before this endpoint's address was found.");
                    None
                } else {
                    match endpoints::encrypted(r, t, v6, timeout, set.plain) {
                        Some((ep, how)) => {
                            info.ip = ep.addr.ip().to_string();
                            info.endpoint = ep.url.clone().unwrap_or_else(|| format!("{}:853", ep.host.as_deref().unwrap_or_default()));
                            info.bootstrap = Some(how);
                            Some(ep)
                        }
                        None => {
                            info.fail("failed", "Couldn't find an address for this endpoint.");
                            None
                        }
                    }
                };
                out.push((info, ep));
            }
        }
    }
    for (i, (t, _)) in out.iter_mut().enumerate() {
        t.idx = i;
    }
    if !not_offered.is_empty() {
        h.note("info", format!("No endpoint known for these, so they weren't measured over them: {}.", not_offered.join("; ")), None);
    }
    out
}

fn prepare_filter_sets(set: Setup, cfg: &RunConfig, h: &RunHandle) -> probes::FilterSets {
    let mut sets = probes::FilterSets::default();
    if !cfg.probes || set.over(h) {
        return sets;
    }
    if cfg.filters.threats {
        let names = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        sets.categories.push(("malware".into(), names(probes::MALWARE), "Vendor blocking-test domains".into()));
        sets.categories.push(("phishing".into(), names(probes::PHISHING), "Vendor blocking-test domains".into()));
    }
    if cfg.filters.ads {
        sets.categories.push(("ads".into(), probes::ADS.iter().map(|s| s.to_string()).collect(), "Common ad and tracker hosts".into()));
    }
    if cfg.filters.adult {
        sets.categories.push((
            "adult".into(),
            probes::ADULT.iter().map(|s| s.to_string()).collect(),
            "Vendor blocking-test domains".into(),
        ));
    }
    sets
}

/// Open every planned target's transport in parallel (handshakes are timed separately).
fn open_targets(set: Setup, h: &RunHandle, plan: &[(TargetInfo, Option<Endpoint>)], timeout: Duration) -> Vec<Option<Box<dyn Exchanger>>> {
    let opened: Vec<Option<Result<Box<dyn Exchanger>, XErr>>> = std::thread::scope(|sc| {
        let handles: Vec<_> = plan
            .iter()
            .map(|(info, ep)| {
                sc.spawn(move || ep.as_ref().filter(|_| info.state == "pending" && !set.over(h)).map(|ep| net::open(ep, timeout)))
            })
            .collect();
        handles.into_iter().map(|hd| hd.join().ok().flatten()).collect()
    });
    opened
        .into_iter()
        .enumerate()
        .map(|(i, res)| match res {
            // Not opened because setup stopped: a planned, still pending target.
            None => {
                if plan[i].1.is_some() && plan[i].0.state == "pending" {
                    h.set_target(i, |t| t.fail("failed", "Setup stopped before this target was opened."));
                }
                None
            }
            Some(Ok(x)) => {
                let setup = x.setup_ns().map(|n| n as f64 / 1e6);
                h.set_target(i, |t| {
                    t.state = "ready".into();
                    t.setup_ms = setup;
                });
                Some(x)
            }
            Some(Err(e)) => {
                h.set_target(i, |t| t.fail("failed", format!("Couldn't connect: {e}")));
                None
            }
        })
        .collect()
}

/// Whether `x` answers `name` with at least one address.
fn resolves(x: &mut dyn Exchanger, name: &str, timeout: Duration) -> bool {
    let Ok(q) = dns::encode_query(fastrand::u16(..), &QuerySpec::new(name, dns::TYPE_A)) else { return false };
    x.exchange(&q, timeout).ok().and_then(|r| dns::parse(&r.bytes).ok()).is_some_and(|m| !m.ipv4_answers().is_empty())
}

/// The names the Cached test will ask. The default list is known to resolve. A list you changed is checked
/// first, because a name that doesn't resolve (a typo, a private zone) would fail every query for every
/// resolver and pull every success rate under the ranking threshold: such names are left out, and said so.
fn effective_cached_names(h: &RunHandle, set: Setup, cfg: &RunConfig, selected: &[Resolver], timeout: Duration) -> Vec<String> {
    let names = cfg.cached_names.clone();
    if !cfg.tests.contains(&TestKind::Cached) || names == crate::names::defaults() {
        return names;
    }
    // Two of the chosen resolvers are asked; a name counts if either one answers it with an address.
    let mut asks: Vec<Box<dyn Exchanger>> =
        selected.iter().take_while(|_| !set.over(h)).filter_map(|r| set.open(r, timeout)).take(2).collect();
    // A resolver that can't answer a name known to resolve says nothing about yours: don't blame the names for it.
    let control = crate::names::defaults().remove(0);
    asks.retain_mut(|x| resolves(x.as_mut(), &control, timeout));
    if asks.is_empty() {
        return names;
    }
    let (keep, left_out): (Vec<String>, Vec<String>) =
        names.into_iter().partition(|n| set.over(h) || asks.iter_mut().any(|x| resolves(x.as_mut(), n, timeout)));
    if left_out.is_empty() {
        return keep;
    }
    let (list_text, kept_text) = (left_out.join(", "), keep.len());
    if keep.is_empty() {
        h.note(
            "warn",
            format!("None of the names in your Cached list resolved ({list_text}), so the Cached test used the default names."),
            None,
        );
        let defaults = crate::names::defaults();
        h.with(|s| s.config.cached_names = defaults.clone());
        return defaults;
    }
    h.note(
        "warn",
        format!("These names didn't resolve, so the Cached test left them out: {list_text}. It used the other {kept_text}."),
        None,
    );
    h.with(|s| s.config.cached_names = keep.clone());
    keep
}

/// One query through `x`, parsed.
fn ask(x: &mut dyn Exchanger, name: &str, qtype: u16, rd: bool, timeout: Duration) -> Option<dns::Message> {
    let spec = QuerySpec { rd, ..QuerySpec::new(name, qtype) };
    let q = dns::encode_query(fastrand::u16(..), &spec).ok()?;
    x.exchange(&q, timeout).ok().and_then(|r| dns::parse(&r.bytes).ok())
}

/// The setup checks before the measured phases: when they stop, and whether they may use plain DNS. A run on
/// encrypted transports only sends none, so a resolver without a DoT or DoH endpoint is simply not asked.
#[derive(Clone, Copy)]
struct Setup {
    until: Instant,
    plain: bool,
}

/// The longest the setup checks may take in all; what is unfinished then is taken as it stands.
const SETUP_BUDGET: Duration = Duration::from_secs(40);

impl Setup {
    fn new(cfg: &RunConfig) -> Self {
        Setup { until: Instant::now() + SETUP_BUDGET, plain: cfg.transports.iter().any(|t| matches!(t, Transport::Udp | Transport::Tcp)) }
    }

    fn over(&self, h: &RunHandle) -> bool {
        h.cancelled() || Instant::now() >= self.until
    }

    fn open(&self, r: &Resolver, timeout: Duration) -> Option<Box<dyn Exchanger>> {
        open_for_run(r, &[], timeout, false, self.plain).map(|(x, _)| x)
    }
}

/// The connection a run's own checks use: the best one, or an encrypted one when the run sends no plain DNS.
fn open_for_run(
    r: &Resolver,
    planned: &[Endpoint],
    timeout: Duration,
    avoid_udp: bool,
    plain: bool,
) -> Option<(Box<dyn Exchanger>, &'static str)> {
    if plain { endpoints::open_best(r, planned, timeout, avoid_udp) } else { endpoints::open_encrypted(r, planned, timeout, false) }
}

/// Up to two of the chosen resolvers, each shown to answer a name known to resolve.
fn helpers(h: &RunHandle, set: Setup, selected: &[Resolver], timeout: Duration) -> Vec<Box<dyn Exchanger>> {
    let control = crate::names::defaults().remove(0);
    selected
        .iter()
        .take_while(|_| !set.over(h))
        .filter_map(|r| set.open(r, timeout))
        .filter_map(|mut x| resolves(x.as_mut(), &control, timeout).then_some(x))
        .take(2)
        .collect()
}

/// The zones the Uncached test will use. Each must still be unsigned (in a signed zone a resolver may answer random
/// names from NSEC records it already holds, RFC 8198) and still answer any label (a wildcard). A zone that changed is
/// left out, and said so. If none passes, or no resolver could be asked, the built-in list is used.
fn effective_uncached_zones(h: &RunHandle, set: Setup, cfg: &RunConfig, selected: &[Resolver], timeout: Duration) -> Vec<String> {
    let zones: Vec<String> = UNCACHED_ZONES.iter().map(|z| z.to_string()).collect();
    if !cfg.tests.contains(&TestKind::Uncached) {
        return zones;
    }
    let mut asks = helpers(h, set, selected, timeout);
    if asks.is_empty() {
        return zones;
    }
    let (mut keep, mut changed) = (Vec::new(), Vec::new());
    for z in zones.iter() {
        if set.over(h) {
            return zones;
        }
        let signed = asks.iter_mut().any(|x| {
            ask(x.as_mut(), z, dns::TYPE_DNSKEY, true, timeout).is_some_and(|m| m.answers.iter().any(|r| r.rtype == dns::TYPE_DNSKEY))
        });
        let wildcard = asks.iter_mut().any(|x| {
            ask(x.as_mut(), &format!("ub{}.{z}", rand_label(12)), dns::TYPE_A, true, timeout)
                .is_some_and(|m| m.rcode == dns::RCODE_NOERROR && !m.answers.is_empty())
        });
        match (signed, wildcard) {
            (false, true) => keep.push(z.clone()),
            (true, _) => changed.push(format!("{z} (now signed with DNSSEC)")),
            (false, false) => changed.push(format!("{z} (no longer answers every name)")),
        }
    }
    if changed.is_empty() {
        return keep;
    }
    if keep.is_empty() {
        h.note(
            "warn",
            format!("None of the Uncached test's zones passed the setup check ({}), so it used them anyway. Its results may include cached answers.", changed.join("; ")),
            None,
        );
        return zones;
    }
    h.note("warn", format!("The Uncached test left out {}, and used {}.", list(&changed), list(&keep)), None);
    keep
}

/// How a TLD proved that a random name under it doesn't exist.
#[derive(Debug, PartialEq)]
enum Denial {
    /// Hashed, opt-out NSEC3: no resolver may answer another name from it (RFC 8198).
    OptOut,
    /// Plain NSEC, or NSEC3 without opt-out: a resolver may answer later names from proofs it holds. With
    /// plain NSEC every `ub…` name sorts into the same gap, so after the first query none would be a miss.
    Reusable,
}

fn denial(m: &dns::Message, tld: &str) -> Option<Denial> {
    if m.rcode != dns::RCODE_NXDOMAIN {
        return None;
    }
    let under = |r: &&dns::Record| r.name.trim_end_matches('.').to_ascii_lowercase().ends_with(&format!(".{tld}"));
    let proofs: Vec<&dns::Record> =
        m.authority.iter().filter(|r| matches!(r.rtype, dns::TYPE_NSEC | dns::TYPE_NSEC3)).filter(under).collect();
    if proofs.is_empty() {
        return None;
    }
    let opt_out =
        |r: &&dns::Record| r.rtype == dns::TYPE_NSEC3 && matches!(&r.data, dns::RData::Other(d) if d.get(1).is_some_and(|f| f & 1 == 1));
    Some(if proofs.iter().all(opt_out) { Denial::OptOut } else { Denial::Reusable })
}

/// The TLDs the TLD test will use: each must still deny random names with opt-out NSEC3. One whose proof can't be
/// seen (the resolvers asked don't pass DNSSEC records on) is kept; one that changed is left out, and said so.
fn effective_tlds(h: &RunHandle, set: Setup, cfg: &RunConfig, selected: &[Resolver], timeout: Duration) -> Vec<String> {
    let tlds = Fresh::defaults().tlds;
    if !cfg.tests.contains(&TestKind::Tld) {
        return tlds;
    }
    let mut asks = helpers(h, set, selected, timeout);
    if asks.is_empty() {
        return tlds;
    }
    let (mut keep, mut changed) = (Vec::new(), Vec::new());
    for t in tlds.iter() {
        if set.over(h) {
            return tlds;
        }
        let spec = QuerySpec::new(format!("ub{}.{t}", rand_label(14)), dns::TYPE_A).with_edns(|e| e.dnssec_ok = true);
        let Ok(q) = dns::encode_query(fastrand::u16(..), &spec) else { continue };
        let seen = asks.iter_mut().find_map(|x| {
            let m = x.exchange(&q, timeout).ok().and_then(|r| dns::parse(&r.bytes).ok())?;
            denial(&m, t)
        });
        match seen {
            Some(Denial::Reusable) => changed.push(format!(".{t} (its proofs that a name doesn't exist can now answer other names)")),
            _ => keep.push(t.clone()),
        }
    }
    if changed.is_empty() {
        return keep;
    }
    if keep.is_empty() {
        h.note(
            "warn",
            format!("None of the TLD test's TLDs passed the setup check ({}), so it used them anyway. Its results may include answers made from cache.", changed.join("; ")),
            None,
        );
        return tlds;
    }
    let dotted: Vec<String> = keep.iter().map(|t| format!(".{t}")).collect();
    h.note("warn", format!("The TLD test left out {}, and used {}.", list(&changed), list(&dotted)), None);
    keep
}

/// Each Cached-test name's TTL at its own name server, so an answer can be judged: at the full TTL the resolver just
/// fetched it (a miss), below it the answer came from its cache. Asked with recursion off, and only replies marked
/// authoritative count, so a device on this network that answers DNS itself can't supply one. Names whose
/// authority can't be found are left out of the judgement. The name servers are asked over plain DNS, so a run on
/// encrypted transports only skips this.
fn authority_ttls(
    h: &RunHandle,
    set: Setup,
    cfg: &RunConfig,
    cached: &[String],
    selected: &[Resolver],
    timeout: Duration,
) -> BTreeMap<String, u32> {
    if !set.plain || !cfg.tests.contains(&TestKind::Cached) || selected.is_empty() {
        return BTreeMap::new();
    }
    let chunks: Vec<&[String]> = cached.chunks(cached.len().div_ceil(8).max(1)).collect();
    std::thread::scope(|sc| {
        let workers: Vec<_> = chunks
            .into_iter()
            .map(|names| {
                sc.spawn(move || {
                    let Some(mut x) = helpers(h, set, selected, timeout).into_iter().next() else { return Vec::new() };
                    names
                        .iter()
                        .take_while(|_| !set.over(h))
                        .filter_map(|n| authority_ttl(x.as_mut(), n, timeout).map(|t| (n.clone(), t)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().filter_map(|w| w.join().ok()).flatten().collect()
    })
}

fn authority_ttl(x: &mut dyn Exchanger, name: &str, timeout: Duration) -> Option<u32> {
    // The zone: the nearest ancestor (or the name itself) with NS records of its own.
    let labels: Vec<&str> = name.trim_end_matches('.').split('.').collect();
    let servers = (0..labels.len().saturating_sub(1)).find_map(|i| {
        let zone = labels[i..].join(".");
        let m = ask(x, &zone, dns::TYPE_NS, true, timeout)?;
        let ns: Vec<String> = m
            .answers
            .iter()
            .filter(|r| r.rtype == dns::TYPE_NS && r.name.trim_end_matches('.').eq_ignore_ascii_case(&zone))
            .filter_map(|r| if let dns::RData::Name(n) = &r.data { Some(n.clone()) } else { None })
            .collect();
        (!ns.is_empty()).then_some(ns)
    })?;
    for ns in servers.iter().take(2) {
        let Some(ip) = ask(x, ns, dns::TYPE_A, true, timeout).and_then(|m| m.ipv4_answers().first().copied()) else { continue };
        let Ok(mut auth) = UdpX::connect(SocketAddr::new(IpAddr::V4(ip), 53)) else { continue };
        let Some(m) = ask(&mut auth, name, dns::TYPE_A, false, timeout) else { continue };
        if !m.aa() {
            continue;
        }
        let first = m.answers.first().filter(|r| r.name.trim_end_matches('.').eq_ignore_ascii_case(name.trim_end_matches('.')));
        if let Some(r) = first {
            return Some(r.ttl);
        }
    }
    None
}

/// A fresh Uncached answer carries the authority's full TTL, because its name is new. One more than 2 s below the
/// highest TTL the same target gave in the same zone came from a cache, so it isn't a cache miss: it is marked and
/// left out of the stats. A resolver that rewrites TTLs (a floor or a cap) gives the same TTL every time, so nothing
/// of its is marked.
fn flag_cached_answers(h: &RunHandle, zones: &[String]) {
    let (changed, flagged) = h.with(|s| mark_cached_answers(&mut s.samples, zones));
    if changed.is_empty() {
        return;
    }
    h.publish(|_| json!({"type": "outcomes", "samples": changed}));
    for (t, answered, cached) in flagged {
        let Some(tg) = h.with(|s| s.targets.get(t as usize).cloned()) else { continue };
        h.note(
            "warn",
            format!(
                "{} {} over {} gave {cached} of its {answered} Uncached answers from a cache (their TTL was below the zone's). They aren't cache misses, so its Uncached numbers leave them out.",
                tg.provider,
                tg.variant,
                tg.transport.label()
            ),
            Some(tg.resolver_id.clone()),
        );
    }
}

/// The marking for [`flag_cached_answers`]: the changed samples, and per target (answered, from cache) where any were.
fn mark_cached_answers(samples: &mut [Sample], zones: &[String]) -> (Vec<Value>, Vec<(u16, usize, usize)>) {
    let answered = |x: &Sample| x.k == TestKind::Uncached && x.o == Outcome::Ok;
    let mut top: BTreeMap<(u16, &str), u32> = BTreeMap::new();
    for x in samples.iter().filter(|x| answered(x)) {
        if let Some(d) = x.d {
            let e = top.entry((x.t, zone_for(x.i, zones))).or_default();
            *e = (*e).max(d);
        }
    }
    let (mut changed, mut per) = (Vec::new(), BTreeMap::<u16, (usize, usize)>::new());
    for x in samples.iter_mut().filter(|x| answered(x)) {
        let e = per.entry(x.t).or_default();
        e.0 += 1;
        if let (Some(d), Some(t)) = (x.d, top.get(&(x.t, zone_for(x.i, zones))))
            && d.saturating_add(2) < *t
        {
            x.o = Outcome::FromCache;
            e.1 += 1;
            changed.push(json!({"t": x.t, "k": x.k, "i": x.i, "o": x.o}));
        }
    }
    (changed, per.into_iter().filter(|(_, (_, c))| *c > 0).map(|(t, (n, c))| (t, n, c)).collect())
}

/// How often each target answered the Cached test from its cache, judged by TTL against the authority's.
/// A TTL above the authority's means the resolver raises short TTLs, so that answer can't be judged.
fn cache_hit_rates(h: &RunHandle, cached: &[String], auth: &BTreeMap<String, u32>, custom: bool) {
    if auth.is_empty() {
        return;
    }
    let rates: Vec<(usize, f64)> = h.with(|s| {
        let mut per: BTreeMap<u16, (u32, u32)> = BTreeMap::new(); // judged, hits
        for x in s.samples.iter().filter(|x| x.k == TestKind::Cached && x.o == Outcome::Ok) {
            let name = query_name_in(TestKind::Cached, x.i, cached, &Fresh::default());
            let (Some(d), Some(&a)) = (x.d, auth.get(&name)) else { continue };
            if a < 10 || d > a {
                continue;
            }
            let e = per.entry(x.t).or_default();
            e.0 += 1;
            if d + 1 < a {
                e.1 += 1;
            }
        }
        let rates: Vec<(usize, f64)> =
            per.into_iter().filter(|(_, (n, _))| *n >= 10).map(|(t, (n, hits))| (t as usize, hits as f64 / n as f64)).collect();
        for &(t, r) in &rates {
            if let Some(tg) = s.targets.get_mut(t) {
                tg.cache_hits = Some(r);
            }
        }
        rates
    });
    let low: Vec<String> = h.with(|s| {
        let mut seen = BTreeSet::new();
        rates
            .iter()
            .filter(|(_, r)| *r < 0.7)
            .filter_map(|&(t, r)| s.targets.get(t).map(|tg| (tg.provider.clone(), r)))
            .filter(|(p, _)| seen.insert(p.clone()))
            .map(|(p, r)| format!("{p} ({:.0}%)", r * 100.0))
            .collect()
    });
    if low.is_empty() {
        return;
    }
    let why = "Large services run many separate caches behind one address, and a query reaches one of them";
    if custom {
        h.note(
            "warn",
            format!("Many Cached-test answers were fresh lookups, not cache hits, for {}. {why}, so names that aren't popular often miss. Those numbers are closer to Uncached than Cached.", list(&low)),
            None,
        );
    } else {
        h.note("info", format!("Share of Cached-test answers given from cache: {}. {why}.", list(&low)), None);
    }
}

/// A filter's block answer counts as an error, so a resolver that blocks some of the Cached test's names (a Pi-hole
/// with your own list of names, say) can fall under the ranking threshold for a reason that has nothing to do with
/// speed. One note per resolver: which names, and whether it cost the resolver its place in the ranking.
fn blocked_notes(s: &RunState, cached: &[String]) -> Vec<(String, String)> {
    #[derive(Default)]
    struct Seen {
        provider: String,
        names: BTreeSet<String>,
        random: u32,
        tests: BTreeSet<&'static str>,
        unranked: bool,
    }
    let mut per: BTreeMap<&str, Seen> = BTreeMap::new();
    for x in s.samples.iter().filter(|x| x.o == Outcome::Blocked) {
        let Some(tg) = s.targets.get(x.t as usize) else { continue };
        let e = per.entry(&tg.resolver_id).or_insert_with(|| Seen { provider: tg.provider.clone(), ..Default::default() });
        match x.k {
            TestKind::Cached => {
                e.names.insert(query_name_in(TestKind::Cached, x.i, cached, &Fresh::default()));
            }
            k => {
                e.random += 1;
                e.tests.insert(if k == TestKind::Tld { "TLD" } else { "Uncached" });
            }
        }
    }
    for tg in &s.targets {
        if let Some(e) = per.get_mut(tg.resolver_id.as_str()) {
            e.unranked |= success(tg) < RELIABLE;
        }
    }
    let threshold = format!("It answered under {:.0}% of queries, so it isn't ranked.", RELIABLE * 100.0);
    per.into_iter()
        .map(|(id, e)| {
            let mut parts = Vec::new();
            if !e.names.is_empty() {
                let n = e.names.len();
                let mut shown: Vec<String> = e.names.iter().take(5).cloned().collect();
                if n > 5 {
                    shown.push(format!("{} more", n - 5));
                }
                let (them, it) = if n == 1 { ("it", "name") } else { ("them", "names") };
                parts.push(format!(
                    "{} blocked {n} of the Cached test's {it} ({}), and a blocked answer counts as an error. To compare it on speed alone, take {them} out of the Cached list.",
                    e.provider,
                    list(&shown)
                ));
            }
            if e.random > 0 {
                let tests: Vec<String> = e.tests.iter().map(|t| t.to_string()).collect();
                parts.push(format!(
                    "{} blocked {} of the random names the {} test asks, which count as errors: its filter may treat names that look random as a threat.",
                    e.provider,
                    e.random,
                    tests.join(" and ")
                ));
            }
            if e.unranked {
                parts.push(threshold.clone());
            }
            (id.to_string(), parts.join(" "))
        })
        .collect()
}

/// The most queries per second sent to each resolver in any one-second window, from the samples.
fn peak_rates(s: &RunState) -> BTreeMap<String, f64> {
    let mut times: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for x in &s.samples {
        if let Some(t) = s.targets.get(x.t as usize) {
            times.entry(t.resolver_id.as_str()).or_default().push(x.at);
        }
    }
    times
        .into_iter()
        .map(|(id, mut at)| {
            at.sort_unstable();
            let (mut lo, mut peak) = (0, 0);
            for hi in 0..at.len() {
                while at[hi] - at[lo] >= 1000 {
                    lo += 1;
                }
                peak = peak.max(hi - lo + 1);
            }
            (id.to_string(), peak as f64)
        })
        .collect()
}

/// A run of answers that then stops: `(target, test, answered before, then not answered, how)`.
pub struct Stall {
    pub target: usize,
    pub test: TestKind,
    pub answered: usize,
    pub stopped: usize,
    pub how: &'static str,
}

/// Where a resolver answered normally and then went silent, or began refusing: at least five unanswered
/// queries in a row at the end, after at least five answered with almost none lost. That is what a rate
/// limit looks like from here, though so is a network that dropped; the note says which it can't tell.
pub fn stalls(s: &RunState) -> Vec<Stall> {
    let mut seq: BTreeMap<(u16, TestKind), Vec<(u32, Outcome)>> = BTreeMap::new();
    for x in &s.samples {
        seq.entry((x.t, x.k)).or_default().push((x.i, x.o));
    }
    let mut out = Vec::new();
    for ((t, test), mut v) in seq {
        v.sort_by_key(|(i, _)| *i);
        let bad = |o: Outcome| matches!(o, Outcome::Timeout | Outcome::Refused | Outcome::Servfail);
        let tail = v.iter().rev().take_while(|(_, o)| bad(*o)).count();
        if tail < 5 || v.len() - tail < 5 {
            continue;
        }
        let head = &v[..v.len() - tail];
        let lost = head.iter().filter(|(_, o)| bad(*o)).count();
        if lost * 10 > head.len() {
            continue;
        }
        let how = match v[v.len() - 1].1 {
            Outcome::Refused => "began refusing queries",
            Outcome::Servfail => "began returning SERVFAIL",
            _ => "stopped answering",
        };
        out.push(Stall { target: t as usize, test, answered: head.len() - lost, stopped: tail, how });
    }
    out
}

/// What the published limits make of the load Recursa sent, as notes. A resolver with a published rate gets a
/// warning only when the run came within half of it; a stall is said plainly, with the limit set beside what was sent.
fn load_notes(s: &RunState, selected: &[Resolver]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for r in selected {
        let (Some(l), Some(peak)) = (&r.limits, s.rates.get(&r.id)) else { continue };
        if let Some(qps) = l.qps.filter(|q| *peak >= q * 0.5) {
            out.push((
                format!("This run sent {} up to {peak:.0} queries per second, within reach of the {qps:.0} it publishes{}. Wait before running again.", r.label(), l.per.as_ref().map(|p| format!(" for {p}")).unwrap_or_default()),
                Some(r.id.clone()),
            ));
        }
    }
    for st in stalls(s) {
        let Some(t) = s.targets.get(st.target) else { continue };
        let r = selected.iter().find(|r| r.id == t.resolver_id);
        let peak = s.rates.get(&t.resolver_id).copied();
        let context = match (r.and_then(|r| r.limits.as_ref()), peak) {
            (Some(l), Some(p)) if l.qps.is_some() => {
                format!(
                    " {} publishes {:.0} queries per second and this run sent at most {p:.0}, so the network is the likelier cause.",
                    t.provider,
                    l.qps.unwrap_or(0.0)
                )
            }
            (Some(l), _) => format!(" {}", l.text),
            (None, _) => " It publishes no limit to compare with.".to_string(),
        };
        out.push((
            format!(
                "{} {} over {} after {} answered {} queries, and then {} the next {}. That can be a rate limit on this address, or the network.{context}",
                t.provider, t.variant, t.transport.label(), st.answered, st.test.key(), st.how, st.stopped
            ),
            Some(t.resolver_id.clone()),
        ));
    }
    out
}

fn report_load(h: &RunHandle, selected: &[Resolver]) {
    let notes = h.with(|s| {
        s.rates = peak_rates(s);
        load_notes(s, selected)
    });
    for (text, id) in notes {
        h.note("warn", text, id);
    }
}

/// Prime caches along the path and in the resolver; set aside targets that never answer.
fn warm_up(set: Setup, h: &RunHandle, xs: &mut [Option<Box<dyn Exchanger>>], timeout: Duration, cached: &[String], fresh: &Fresh) {
    let ready: Vec<usize> = (0..xs.len()).filter(|i| xs[*i].is_some()).collect();
    let planned = (cached.len() + fresh.zones.len() + fresh.tlds.len()) as u32;
    h.phase("warmup", planned * ready.len() as u32);
    let mut slots: Vec<(usize, Box<dyn Exchanger>)> = ready.iter().map(|&i| (i, xs[i].take().unwrap())).collect();
    let results: Vec<(usize, u32, Option<XErr>, bool)> = std::thread::scope(|sc| {
        let workers: Vec<_> = slots
            .iter_mut()
            .map(|(i, x)| {
                let i = *i;
                sc.spawn(move || {
                    let names = cached
                        .iter()
                        .map(|n| (TestKind::Cached, n.clone()))
                        .chain(fresh.zones.iter().map(|z| (TestKind::Uncached, format!("ub{}.{z}", rand_label(12)))))
                        .chain(fresh.tlds.iter().map(|t| (TestKind::Tld, format!("ub{}.{t}", rand_label(14)))));
                    let (mut answered, mut failures, mut last_err, mut consumed, mut stopped) = (0u32, 0u32, None, 0u32, false);
                    for (k, n) in names {
                        if set.over(h) {
                            stopped = true;
                            break;
                        }
                        consumed += 1;
                        let Ok(q) = dns::encode_query(fastrand::u16(..), &QuerySpec::new(n, dns::TYPE_A)) else {
                            h.publish(|s| {
                                s.done += 1;
                                json!({"type": "warm", "samples": [], "done": s.done, "total": s.total})
                            });
                            continue;
                        };
                        let r = x.exchange(&q, timeout);
                        let sample =
                            r.as_ref().ok().map(|r| WarmSample { t: i as u16, k, us: (r.rtt_ns / 1000).min(u32::MAX as u64) as u32 });
                        h.publish(|s| {
                            s.warm.extend(sample);
                            s.done += 1;
                            json!({"type": "warm", "samples": sample.as_slice(), "done": s.done, "total": s.total})
                        });
                        match r {
                            Ok(_) => answered += 1,
                            Err(e) => {
                                failures += 1;
                                last_err = Some(e);
                            }
                        }
                        if answered == 0 && failures >= 3 {
                            break;
                        }
                    }
                    // The queries skipped after an early stop still count, so the bar completes.
                    if consumed < planned {
                        h.publish(|s| {
                            s.done += planned - consumed;
                            json!({"type": "warm", "samples": [], "done": s.done, "total": s.total})
                        });
                    }
                    (i, answered, last_err.filter(|_| answered == 0), stopped)
                })
            })
            .collect();
        workers.into_iter().filter_map(|w| w.join().ok()).collect()
    });
    for (i, x) in slots {
        xs[i] = Some(x);
    }
    let cut = results.iter().filter(|r| r.3).count();
    if cut > 0 && !h.cancelled() {
        h.note(
            "warn",
            format!(
                "Warm-up ran out of time for {cut} of {} targets, so their cached test may include lookups that weren't cached yet.",
                results.len()
            ),
            None,
        );
    }
    for (i, answered, err, stopped) in results {
        // A target cut off by the budget or a cancel was not shown to be dead; it keeps its place.
        if answered > 0 || stopped {
            continue;
        }
        xs[i] = None;
        let reason = match err {
            Some(XErr::Refused) => "Port closed: the server refused the connection (ICMP unreachable).".to_string(),
            Some(XErr::Timeout) | None => "No answer to any warm-up query.".to_string(),
            Some(e) => format!("No answer to any warm-up query ({e})."),
        };
        h.set_target(i, |t| t.fail("unreachable", reason));
    }
}

fn run(h: &RunHandle, cfg: &RunConfig, selected: &[Resolver], deps: &Deps) {
    let timeout = Duration::from_millis(QUERY_TIMEOUT_MS);
    let t0 = Instant::now();

    // ---- Setup: everything that touches the network beyond measurement happens here.
    h.phase("setup", 0);
    let set = Setup::new(cfg);
    let sets = prepare_filter_sets(set, cfg, h);
    let plan = expand(set, cfg, selected, deps.client_v6, timeout, h);
    h.publish(|s| {
        s.targets = plan.iter().map(|(t, _)| t.clone()).collect();
        json!({"type": "targets", "targets": s.targets})
    });

    deps.measuring.store(true, Ordering::SeqCst);
    let mut xs = open_targets(set, h, &plan, timeout);
    h.emit_targets();
    let cached = effective_cached_names(h, set, cfg, selected, timeout);
    let fresh =
        Fresh { zones: effective_uncached_zones(h, set, cfg, selected, timeout), tlds: effective_tlds(h, set, cfg, selected, timeout) };
    let auth = authority_ttls(h, set, cfg, &cached, selected, timeout);
    // The link check pings the gateway from the start of warm-up to the end of the measured phases.
    let pinger = (!link::disabled()).then(|| link::start(t0));
    // Warm-up gets a budget of its own: setup may have used most of the shared one, and a target
    // left unwarmed would measure cold lookups in its cached test.
    warm_up(Setup::new(cfg), h, &mut xs, timeout, &cached, &fresh);
    h.emit_targets();

    // ---- Measured phases.
    let gap = match cfg.mode {
        Mode::Accurate => ACCURATE_GAP,
        Mode::Quick => Duration::from_millis(1),
    };
    for &test in &cfg.tests {
        if h.cancelled() {
            break;
        }
        run_phase(h, test, &mut xs, cfg, &cached, &fresh, timeout, gap, t0);
        match test {
            TestKind::Uncached => flag_cached_answers(h, &fresh.zones),
            TestKind::Cached => cache_hit_rates(h, &cached, &auth, cached != crate::names::defaults()),
            TestKind::Tld => {}
        }
        compute_stats(h, test);
        h.emit_targets();
    }
    if let Some(pinger) = pinger {
        let (value, noisy) = link::report(pinger.map(link::LinkProbe::stop));
        h.publish(|s| {
            s.link = Some(value);
            json!({"type": "link", "link": s.link})
        });
        if let Some(text) = noisy {
            h.note("warn", text, None);
        }
    }
    for (id, text) in h.with(|s| blocked_notes(s, &cached)) {
        h.note("warn", text, Some(id));
    }
    drop(xs);
    deps.measuring.store(false, Ordering::SeqCst);
    report_load(h, selected);

    // A resolver that answered nothing gets no path check, probes or CDN lookups: each of
    // those queries would only wait out its timeout (one dead server cost ~50 s).
    let answered: BTreeSet<String> =
        h.with(|s| s.targets.iter().filter(|t| t.stats.values().any(|x| x.ok > 0)).map(|t| t.resolver_id.clone()).collect());
    let silent: Vec<String> = selected.iter().filter(|r| !answered.contains(&r.id)).map(|r| r.label()).collect();
    let selected: Vec<Resolver> = selected.iter().filter(|r| answered.contains(&r.id)).cloned().collect();
    let selected = selected.as_slice();
    if !silent.is_empty() && !h.cancelled() && (cfg.probes || cfg.cdn) {
        let skipped = match (cfg.probes, cfg.cdn) {
            (true, true) => "capability probes and CDN mapping were",
            (true, false) => "capability probes were",
            _ => "CDN mapping was",
        };
        let pronoun = if silent.len() == 1 { "it" } else { "them" };
        h.note("info", format!("No answers from {}, so {skipped} skipped for {pronoun}.", list(&silent)), None);
    }

    // ---- Path check: true network RTT per resolver, and interception detection.
    // The setup budget ran out long ago, with the measured phases: this check gets its own.
    let intercepted: Vec<String> = if h.cancelled() { vec![] } else { check_paths(Setup::new(cfg), h, selected) };

    // ---- Capability probes.
    if cfg.probes && !h.cancelled() {
        h.phase("probes", selected.len() as u32);
        run_probes(h, selected, &plan, &sets, timeout, deps.client_v6, &intercepted, set.plain);
    }

    // ---- Health check: if plain DNS stopped answering during the probes (a security
    // appliance reacting, or a network change), say so and avoid it from here on.
    let mut avoid_udp = intercepted.clone();
    if cfg.probes && set.plain && !h.cancelled() && plain_dns_stopped() {
        h.note(
            "warn",
            "Plain DNS (UDP 53) from this computer stopped getting answers during the probes, while TCP and encrypted DNS still work. A threat-protection, IPS or firewall rule on your network may have blocked this computer's DNS, or the network changed mid-run. Check your router or firewall's security log. The rest of this run uses encrypted DNS.".into(),
            None,
        );
        avoid_udp = selected.iter().map(|r| r.id.clone()).collect();
    }

    // ---- CDN mapping: resolve CDN hostnames through every resolver, time the edges.
    if cfg.cdn && !h.cancelled() {
        h.phase("cdn", selected.len() as u32);
        let report = run_cdn(selected, &plan, timeout, &avoid_udp, set.plain);
        h.publish(|s| {
            s.cdn = Some(report);
            json!({"type": "cdn", "cdn": s.cdn})
        });
    }

    // ---- Intel: RPKI, ROV, egress networks. Cached on disk; never during measurement.
    if !h.cancelled() {
        h.phase("intel", selected.len() as u32);
        let egress: BTreeMap<String, Option<String>> =
            h.with(|s| s.probes.iter().map(|(k, p)| (k.clone(), p.egress.first().cloned())).collect());
        let jobs: Vec<(String, Option<IpAddr>, Option<String>)> = selected
            .iter()
            .map(|r| (r.id.clone(), r.primary(false).or_else(|| r.primary(true)), egress.get(&r.id).cloned().flatten()))
            .collect();
        let intel = deps.intel.clone();
        let results = deps.rt.block_on(async move {
            let futs = jobs.into_iter().map(|(id, ip, eg)| {
                let intel = intel.clone();
                async move { (id, intel.resolver_intel(ip, eg).await) }
            });
            futures_util::future::join_all(futs).await
        });
        for (id, v) in results {
            h.publish(|s| {
                s.intel.insert(id.clone(), v.clone());
                s.done += 1;
                json!({"type": "intel", "resolver_id": id, "intel": v, "done": s.done})
            });
        }
        explain_egress(h);
        h.mark("cdn-owners");
        enrich_cdn(h, deps);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_phase(
    h: &RunHandle,
    test: TestKind,
    xs: &mut [Option<Box<dyn Exchanger>>],
    cfg: &RunConfig,
    cached: &[String],
    fresh: &Fresh,
    timeout: Duration,
    gap: Duration,
    t0: Instant,
) {
    let mut active: Vec<usize> = (0..xs.len()).filter(|i| xs[*i].is_some()).collect();
    if active.is_empty() {
        return;
    }
    let accurate = cfg.mode == Mode::Accurate;
    // Accurate: one single-target group per thread (see `measure_paced`), in a random first-round order.
    // Quick: up to eight groups through `measure`.
    let k = if accurate {
        fastrand::shuffle(&mut active);
        active.len()
    } else {
        active.len().clamp(1, 8)
    };
    let mut groups: Vec<Vec<(usize, Box<dyn Exchanger>)>> = (0..k).map(|_| Vec::new()).collect();
    for (j, &i) in active.iter().enumerate() {
        groups[j % k].push((i, xs[i].take().unwrap()));
    }
    h.phase(test.key(), (active.len() as u32) * cfg.samples);

    let (stx, srx) = mpsc::channel::<Sample>();
    let samples = cfg.samples;
    let cancel = &h.cancel;
    let pacer = Pacer::new(gap);
    let pacer = &pacer;
    let dropped: Vec<usize> = std::thread::scope(|sc| {
        let workers: Vec<_> = groups
            .iter_mut()
            .map(|g| {
                let stx = stx.clone();
                sc.spawn(move || {
                    if accurate {
                        let (ti, x) = &mut g[0];
                        measure_paced(*ti, x.as_mut(), pacer, test, samples, cached, fresh, timeout, t0, cancel, stx).into_iter().collect()
                    } else {
                        measure(g, test, samples, cached, fresh, timeout, gap, t0, cancel, stx)
                    }
                })
            })
            .collect();
        drop(stx);

        // Batch samples to the UI at ~8 Hz so rendering never competes with measuring.
        let mut batch: Vec<Sample> = Vec::with_capacity(256);
        let mut last = Instant::now();
        let flush = |batch: &mut Vec<Sample>| {
            if batch.is_empty() {
                return;
            }
            h.publish(|s| {
                s.samples.extend_from_slice(batch);
                s.done += batch.len() as u32;
                json!({"type": "samples", "samples": batch, "done": s.done, "total": s.total})
            });
            batch.clear();
        };
        loop {
            match srx.recv_timeout(Duration::from_millis(100)) {
                Ok(s) => batch.push(s),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if last.elapsed() >= Duration::from_millis(125) || batch.len() >= 256 {
                flush(&mut batch);
                last = Instant::now();
            }
        }
        flush(&mut batch);
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    });
    for (i, x) in groups.into_iter().flatten() {
        xs[i] = Some(x);
    }
    for i in dropped {
        xs[i] = None;
        h.set_target(i, |t| t.fail("dropped", format!("Stopped after {DROP_AFTER} consecutive timeouts during the {} test.", test.key())));
    }
}

/// One measured query: encode, exchange, classify, update the target's timeout streak and send the sample.
/// Returns whether the target is now dead (`DROP_AFTER` consecutive timeouts).
#[allow(clippy::too_many_arguments)]
fn query_once(
    x: &mut dyn Exchanger,
    ti: usize,
    test: TestKind,
    round: u32,
    cached: &[String],
    fresh: &Fresh,
    timeout: Duration,
    t0: Instant,
    tx: &mpsc::Sender<Sample>,
    streak: &mut u32,
) -> bool {
    // Encoding happens before the send stamp, outside the measured interval.
    let Ok(q) = dns::encode_query(fastrand::u16(..), &QuerySpec::new(query_name_in(test, round, cached, fresh), dns::TYPE_A)) else {
        return false;
    };
    let res = x.exchange(&q, timeout);
    let (o, us, d) = classify(test, &res);
    let h = res.as_ref().ok().and_then(|r| r.ttl).unwrap_or(0);
    let mut dead = false;
    if o == Outcome::Timeout {
        *streak += 1;
        dead = *streak >= DROP_AFTER;
    } else {
        *streak = 0;
    }
    let _ = tx.send(Sample { t: ti as u16, k: test, i: round, us, o, at: t0.elapsed().as_millis() as u32, h, d });
    dead
}

/// Spaces sends from all measuring threads at least `gap` apart. The lock is held only to
/// reserve a slot, never while sleeping or exchanging.
struct Pacer {
    next: Mutex<Instant>,
    gap: Duration,
}

impl Pacer {
    fn new(gap: Duration) -> Self {
        Pacer { next: Mutex::new(Instant::now()), gap }
    }

    fn wait(&self) {
        let at = {
            let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
            let at = (*next).max(Instant::now());
            *next = at + self.gap;
            at
        };
        if let Some(d) = at.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
}

/// Accurate mode: one target, one query in flight, every send reserved through the shared pacer.
/// Returns the target index if it was dropped.
#[allow(clippy::too_many_arguments)]
fn measure_paced(
    ti: usize,
    x: &mut dyn Exchanger,
    pacer: &Pacer,
    test: TestKind,
    samples: u32,
    cached: &[String],
    fresh: &Fresh,
    timeout: Duration,
    t0: Instant,
    cancel: &AtomicBool,
    tx: mpsc::Sender<Sample>,
) -> Option<usize> {
    clock::prioritize_thread();
    let mut streak = 0u32;
    for round in 0..samples {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        pacer.wait();
        if query_once(x, ti, test, round, cached, fresh, timeout, t0, &tx, &mut streak) {
            return Some(ti);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn measure(
    g: &mut [(usize, Box<dyn Exchanger>)],
    test: TestKind,
    samples: u32,
    cached: &[String],
    fresh: &Fresh,
    timeout: Duration,
    gap: Duration,
    t0: Instant,
    cancel: &AtomicBool,
    tx: mpsc::Sender<Sample>,
) -> Vec<usize> {
    clock::prioritize_thread();
    let mut streak = vec![0u32; g.len()];
    let mut dead = vec![false; g.len()];
    let mut order: Vec<usize> = (0..g.len()).collect();
    'rounds: for round in 0..samples {
        fastrand::shuffle(&mut order);
        for &j in &order {
            if cancel.load(Ordering::Relaxed) {
                break 'rounds;
            }
            if dead[j] {
                continue;
            }
            let (ti, x) = &mut g[j];
            dead[j] = query_once(x.as_mut(), *ti, test, round, cached, fresh, timeout, t0, &tx, &mut streak[j]);
            std::thread::sleep(gap);
        }
    }
    dead.iter().enumerate().filter(|(_, d)| **d).map(|(j, _)| g[j].0).collect()
}

fn compute_stats(h: &RunHandle, test: TestKind) {
    h.with(|s| {
        let n = s.targets.len();
        let mut ms: Vec<Vec<f64>> = vec![Vec::new(); n];
        let mut counts = vec![(0u32, 0u32, 0u32); n]; // sent, timeouts, errors
        // An Uncached answer that came from a cache measured nothing the test is for: it isn't counted at all.
        for smp in s.samples.iter().filter(|x| x.k == test && x.o != Outcome::FromCache) {
            let t = smp.t as usize;
            counts[t].0 += 1;
            match smp.o {
                Outcome::Ok => ms[t].push(smp.us as f64 / 1000.0),
                Outcome::Timeout => counts[t].1 += 1,
                _ => counts[t].2 += 1,
            }
        }
        for (t, v) in ms.iter_mut().enumerate() {
            let (sent, to, err) = counts[t];
            if sent > 0 {
                s.targets[t].stats.insert(test.key().into(), stats::summarize(v, sent, to, err));
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn run_probes(
    h: &RunHandle,
    selected: &[Resolver],
    plan: &[(TargetInfo, Option<Endpoint>)],
    sets: &probes::FilterSets,
    timeout: Duration,
    client_v6: bool,
    intercepted: &[String],
    plain: bool,
) {
    let targets = h.with(|s| s.targets.clone());
    // Eight resolvers at a time, each worker taking the next as soon as it finishes, so one slow resolver
    // doesn't hold up the rest.
    let next = AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..selected.len().min(8) {
            sc.spawn(|| {
                while !h.cancelled() {
                    let Some(r) = selected.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    let avoid_udp = intercepted.contains(&r.id);
                    // A probe that panics loses only its own report, as when each had a thread of its own.
                    let probe = || probe_resolver(r, plan, &targets, sets, timeout, client_v6, avoid_udp, plain);
                    let Some(rep) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe)).ok().flatten() else {
                        continue;
                    };
                    let id = rep.resolver_id.clone();
                    h.publish(|s| {
                        s.probes.insert(id.clone(), rep.clone());
                        s.done += 1;
                        json!({"type": "probe", "resolver_id": id, "report": rep, "done": s.done})
                    });
                }
            });
        }
    });
}

/// A resolver on this network's own address space: RFC 1918, unique-local or link-local. Not shared (CGNAT,
/// Tailscale) space or a loopback stub, which forward to somewhere else.
pub(crate) fn on_local_network(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

#[allow(clippy::too_many_arguments)]
fn probe_resolver(
    r: &Resolver,
    plan: &[(TargetInfo, Option<Endpoint>)],
    targets: &[TargetInfo],
    sets: &probes::FilterSets,
    timeout: Duration,
    client_v6: bool,
    avoid_udp: bool,
    plain: bool,
) -> Option<probes::ProbeReport> {
    use probes::Status::{Fail, Info, Pass, Unknown};
    let planned = planned_for(plan, &r.id);
    let (mut ex, via) = open_for_run(r, &planned, timeout, avoid_udp, plain)?;
    let mut rep = probes::probe(&r.id, via, ex.as_mut(), timeout, sets);
    drop(ex);

    // Transports already measured in this run need no extra connection.
    let measured = |t: Transport| {
        targets
            .iter()
            .find(|x| x.resolver_id == r.id && x.transport == t && x.stats.values().any(|s| s.ok > 0))
            .map(|x| x.stats.values().map(|s| s.p50).filter(|p| p.is_finite()).fold(f64::INFINITY, f64::min))
    };
    let quick = timeout.min(Duration::from_millis(2500));
    let try_ep = |ep: &Endpoint| -> Result<f64, XErr> {
        let mut x = net::open(ep, quick)?;
        let q = dns::encode_query(fastrand::u16(..), &QuerySpec::new("example.com", dns::TYPE_A)).map_err(|e| XErr::Io(e.to_string()))?;
        Ok(x.exchange(&q, quick)?.rtt_ns as f64 / 1e6)
    };

    rep.checks.push(match (measured(Transport::Tcp), endpoints::plain(r, Transport::Tcp, false).filter(|_| r.udp)) {
        (Some(p50), _) => check("tcp", Pass, Some(true), "Yes", format!("measured in this run, median {p50:.1} ms")),
        (None, Some(_)) if !plain => check("tcp", Unknown, None, "Untested", "this run sent no plain DNS"),
        (None, Some(ep)) => match try_ep(&ep) {
            Ok(ms) => check("tcp", Pass, Some(true), "Yes", format!("{}:53 answered over TCP in {ms:.1} ms", ep.addr.ip())),
            Err(e) => check("tcp", Fail, Some(false), "No", format!("{}:53 over TCP: {e}", ep.addr.ip())),
        },
        (None, None) => check("tcp", Info, Some(false), "Not offered", "no plain DNS service"),
    });

    for (key, t) in [("dot", Transport::Dot), ("doh", Transport::Doh)] {
        let name = if t == Transport::Dot { r.dot.as_deref() } else { r.doh.as_deref() };
        rep.checks.push(match (measured(t), name) {
            (Some(p50), _) => check(key, Pass, Some(true), "Yes", format!("measured in this run, median {p50:.1} ms")),
            (None, Some(name)) => {
                let ep = planned
                    .iter()
                    .find(|e| e.transport == t)
                    .cloned()
                    .or_else(|| endpoints::encrypted(r, t, false, quick, plain).map(|(e, _)| e));
                match ep.map(|ep| try_ep(&ep)) {
                    Some(Ok(ms)) => check(key, Pass, Some(true), "Yes", format!("{name} answered in {ms:.1} ms (after handshake)")),
                    Some(Err(e)) => check(key, Fail, Some(false), "Failed", format!("{name}: {e}")),
                    None => check(key, Unknown, None, "Couldn't resolve", format!("couldn't find an address for {name}")),
                }
            }
            // Nothing to measure. Whether the operator offers it is published data, not a
            // measurement, so the UI takes it from the catalog.
            (None, None) if matches!(r.group.as_str(), "system" | "custom") => {
                check(key, Unknown, None, "Not set", "no endpoint was given for this resolver")
            }
            (None, None) => check(key, Unknown, None, "Not offered", "the operator publishes no endpoint for it"),
        });
    }

    rep.checks.push(match (endpoints::plain(r, Transport::Udp, true), client_v6, r.udp) {
        (None, _, _) => check("ipv6", Info, Some(false), "No IPv6 address", "none published"),
        (Some(_), false, _) => check("ipv6", Unknown, None, "Untested", "this network has no IPv6 connectivity"),
        (Some(_), true, true) if !plain => check("ipv6", Unknown, None, "Untested", "this run sent no plain DNS"),
        (Some(ep), true, true) => match try_ep(&ep) {
            Ok(ms) => check("ipv6", Pass, Some(true), "Reachable", format!("[{}]:53 answered in {ms:.1} ms", ep.addr.ip())),
            Err(e) => check("ipv6", Fail, Some(false), "Unreachable", format!("[{}]:53: {e}", ep.addr.ip())),
        },
        (Some(ep), true, false) => check("ipv6", Info, Some(true), "Published", format!("{} (encrypted transports only)", ep.addr.ip())),
    });
    Some(rep)
}

/// True when well-known public resolvers stop answering plain DNS while TCP 53 still
/// works: the signature of this computer's UDP DNS being blocked on the path.
fn plain_dns_stopped() -> bool {
    let ask = |t: Transport, ip: &str| {
        let ep = Endpoint { transport: t, addr: SocketAddr::new(ip.parse().unwrap(), 53), host: None, url: None };
        let q = dns::encode_query(fastrand::u16(..), &QuerySpec::new("www.google.com", dns::TYPE_A)).ok()?;
        net::open(&ep, Duration::from_millis(1500)).ok()?.exchange(&q, Duration::from_millis(1500)).ok()
    };
    let udp_dead = ["1.1.1.1", "8.8.8.8"].iter().all(|ip| ask(Transport::Udp, ip).is_none());
    udp_dead && ask(Transport::Tcp, "1.1.1.1").is_some()
}

/// Time TCP handshakes to each public resolver's address (DoT port first, then HTTPS,
/// then 53), interleaved over five rounds. Then flag plain-DNS targets whose answers
/// beat that floor: a device on this network answered them.
fn check_paths(set: Setup, h: &RunHandle, selected: &[Resolver]) -> Vec<String> {
    let jobs: Vec<(String, IpAddr, Vec<u16>)> = selected
        .iter()
        .filter(|r| r.group != "system")
        .filter_map(|r| {
            let ip = r.primary(false).or_else(|| r.primary(true)).filter(|ip| !crate::catalog::is_private(ip))?;
            let ports: Vec<u16> = [(r.dot.is_some(), 853), (r.doh.is_some(), 443), (r.udp && set.plain, 53)]
                .into_iter()
                .filter(|(on, _)| *on)
                .map(|(_, p)| p)
                .collect();
            Some((r.id.clone(), ip, ports)).filter(|(_, _, ports)| !ports.is_empty())
        })
        .collect();
    h.phase("path", jobs.len() as u32);
    let mut paths: BTreeMap<String, PathInfo> = BTreeMap::new();
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    for _ in 0..5 {
        fastrand::shuffle(&mut order);
        for &j in &order {
            if set.over(h) {
                break;
            }
            let (id, ip, ports) = &jobs[j];
            // Once a port answers, keep using it so every sample measures the same path.
            let candidates: Vec<u16> = paths.get(id).map(|p| vec![p.port]).unwrap_or_else(|| ports.clone());
            for port in candidates {
                let start = clock::now_ns();
                if std::net::TcpStream::connect_timeout(&SocketAddr::new(*ip, port), Duration::from_millis(1500)).is_ok() {
                    let ms = (clock::now_ns() - start) as f64 / 1e6;
                    let p = paths.entry(id.clone()).or_insert(PathInfo { rtt_ms: None, port, samples: vec![] });
                    p.samples.push((ms * 1000.0).round() / 1000.0);
                    p.rtt_ms = Some(p.rtt_ms.map_or(ms, |x| x.min(ms)));
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    }
    let mut flagged: Vec<String> = Vec::new();
    let mut local: Vec<String> = Vec::new();
    h.publish(|s| {
        let n = s.targets.len();
        let (mut ok_us, mut ttls): (Vec<Vec<u32>>, Vec<Vec<u8>>) = (vec![Vec::new(); n], vec![Vec::new(); n]);
        for x in s.samples.iter().filter(|x| x.o == Outcome::Ok) {
            ok_us[x.t as usize].push(x.us);
            if x.h > 0 {
                ttls[x.t as usize].push(x.h);
            }
        }
        let private: BTreeSet<&str> = s
            .resolvers
            .iter()
            .filter(|r| r.primary(false).or_else(|| r.primary(true)).is_some_and(|ip| crate::catalog::is_private(&ip)))
            .map(|r| r.id.as_str())
            .collect();
        for t in s.targets.iter_mut().filter(|t| t.transport == Transport::Udp) {
            let ttl = &mut ttls[t.idx];
            ttl.sort_unstable();
            t.hops = ttl.get(ttl.len() / 2).map(|v| net::hops(*v));
            if private.contains(t.resolver_id.as_str()) {
                continue;
            }
            // A public resolver can't be zero or one router hop away.
            let on_lan = t.hops.is_some_and(|h| h <= 1);
            let ok = &ok_us[t.idx];
            let too_fast = paths.get(&t.resolver_id).and_then(|p| p.rtt_ms).is_some_and(|rtt| {
                rtt >= 4.0 && !ok.is_empty() && ok.iter().filter(|us| (**us as f64 / 1000.0) < rtt * 0.5).count() * 5 >= ok.len()
            });
            if on_lan || too_fast {
                t.intercepted = true;
                if !flagged.contains(&t.resolver_id) {
                    flagged.push(t.resolver_id.clone());
                }
                if on_lan && !local.contains(&t.resolver_id) {
                    local.push(t.resolver_id.clone());
                }
            }
        }
        s.paths = paths;
        json!({"type": "paths", "paths": s.paths, "targets": s.targets})
    });
    if !flagged.is_empty() {
        let names: Vec<String> = selected.iter().filter(|r| flagged.contains(&r.id)).map(|r| r.label()).collect();
        let evidence = if local.is_empty() {
            "arrived faster than physics allows: quicker than a TCP handshake to the same address"
        } else {
            "came from a device on your local network: they crossed no router (their IP TTL was untouched), and some arrived faster than a TCP handshake to the real address"
        };
        h.note(
            "warn",
            format!(
                "Plain-DNS answers from {} {evidence}. Something on this network, such as a Wi-Fi access point or router DNS feature (UniFi DNS Assistance, for example) or a DNAT rule, is answering port 53 itself. Those UDP rows aren't ranked, and their probes and CDN lookups used encrypted DNS instead. Add TCP, DoT or DoH to the plan to measure the real services.",
                names.join(", ")
            ),
            None,
        );
    }
    flagged
}

fn run_cdn(
    selected: &[Resolver],
    plan: &[(TargetInfo, Option<Endpoint>)],
    timeout: Duration,
    intercepted: &[String],
    plain: bool,
) -> cdn::CdnReport {
    let mut answers: cdn::Answers = std::thread::scope(|sc| {
        let workers: Vec<_> = selected
            .iter()
            .map(|r| {
                let avoid_udp = intercepted.contains(&r.id);
                let planned = planned_for(plan, &r.id);
                sc.spawn(move || {
                    let (mut ex, _) = open_for_run(r, &planned, timeout, avoid_udp, plain)?;
                    Some((r.id.clone(), cdn::resolve_all(ex.as_mut(), timeout, 2)))
                })
            })
            .collect();
        workers.into_iter().filter_map(|w| w.join().ok().flatten()).collect()
    });
    // The name each hostname finally answers for, as most resolvers saw it.
    let mut targets: BTreeMap<String, String> = BTreeMap::new();
    for h in cdn::CDNS.iter().flat_map(|c| c.hosts) {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for m in answers.values().flat_map(|a| a.values()).filter_map(|hs| hs.get(h.name)) {
            if let (Some(t), cdn::HostStatus::Ok) = (&m.target, m.status) {
                *counts.entry(t.as_str()).or_default() += 1;
            }
        }
        if let Some((t, _)) = counts.into_iter().max_by_key(|(_, n)| *n) {
            targets.insert(h.name.to_string(), t.to_string());
        }
    }
    // The references ask each CDN's name servers over plain DNS, so an encrypted-only run skips them.
    let refs = selected
        .iter()
        .filter(|_| plain)
        .find_map(|r| open_for_run(r, &planned_for(plan, &r.id), timeout, intercepted.contains(&r.id), plain))
        .map(|(mut ex, _)| cdn::references(ex.as_mut(), &targets, timeout, !intercepted.is_empty()))
        .unwrap_or_default();
    let ips: BTreeSet<String> = answers
        .values()
        .flat_map(|c| c.values().flat_map(|h| h.values().flat_map(|x| x.ips.iter().cloned())))
        .chain(refs.values().flat_map(|r| r.ips.iter().cloned()))
        .collect();
    let edges = cdn::time_edges(&ips);
    cdn::mark_unreachable(&mut answers, &edges);
    let noise = cdn::noise_floor(&edges);
    cdn::score(answers, refs, edges, noise)
}

/// After intel: name each edge's network and explain each resolver's mapping basis.
fn enrich_cdn(h: &RunHandle, deps: &Deps) {
    let Some(ips) = h.with(|s| s.cdn.as_ref().map(|c| c.edges.keys().cloned().collect::<Vec<_>>())) else { return };
    let intel = deps.intel.clone();
    let owners: Vec<(String, Value)> = deps.rt.block_on(async move {
        use futures_util::StreamExt;
        futures_util::stream::iter(ips.into_iter().map(|ip| {
            let intel = intel.clone();
            async move {
                let v = match ip.parse::<IpAddr>() {
                    Ok(a) => intel.asn_of(a).await,
                    Err(_) => Value::Null,
                };
                (ip, v)
            }
        }))
        .buffer_unordered(6)
        .collect()
        .await
    });
    let client_asn = deps.env.pointer("/vantage/network/asn").and_then(Value::as_u64);
    let client_v4: Option<std::net::Ipv4Addr> = deps.env.pointer("/vantage/ipv4").and_then(Value::as_str).and_then(|s| s.parse().ok());
    h.publish(|s| {
        let RunState { cdn, probes, intel, .. } = s;
        if let Some(c) = cdn.as_mut() {
            for (ip, v) in owners {
                if let Some(e) = c.edges.get_mut(&ip) {
                    e.asn = v["asn"].as_u64();
                    e.holder = v["holder"].as_str().map(str::to_string);
                    e.on_net = client_asn.is_some() && e.asn == client_asn;
                }
            }
            for (id, r) in c.resolvers.iter_mut() {
                let egress_asn = intel.get(id).and_then(|v| v.pointer("/egress/asn")).and_then(Value::as_u64);
                r.ecs = probes.get(id).and_then(|p| p.ecs.clone());
                r.ecs_akamai = probes.get(id).and_then(|p| p.ecs_akamai.clone());
                r.ecs_is_client = r.ecs.as_deref().and_then(|e| ecs_contains(e, client_v4));
                r.basis = if client_asn.is_some() && egress_asn == client_asn {
                    "on_net"
                } else if r.ecs_is_client == Some(false) {
                    "ecs_substitute"
                } else if r.ecs.is_some() {
                    "ecs"
                } else if probes.contains_key(id) {
                    "resolver_location"
                } else {
                    "unknown"
                }
                .into();
            }
        }
        json!({"type": "cdn", "cdn": cdn})
    });
}

/// Does an ECS value like "203.0.113.0/24" cover the client's public IPv4 address?
fn ecs_contains(ecs: &str, client: Option<std::net::Ipv4Addr>) -> Option<bool> {
    let client = client?;
    let (net, len) = ecs.split_once('/')?;
    let net: std::net::Ipv4Addr = net.parse().ok()?;
    let len: u32 = len.parse().ok().filter(|l| (1..=32).contains(l))?;
    let mask = u32::MAX << (32 - len);
    Some(u32::from(net) & mask == u32::from(client) & mask)
}

/// Compare egress addresses (where each resolver's lookups reach authoritative servers
/// from) with the system resolver's. One matching operator means the system resolver
/// forwards there. Several unrelated operators sharing one egress means something on this
/// network is redirecting port 53, unless the port-53 check showed DNS reaching resolvers
/// directly. With no exact match, fall back to comparing egress networks.
fn explain_egress(h: &RunHandle) {
    let (sys, others, port53_direct) = h.with(|s| {
        let sys = s.resolvers.iter().filter(|r| r.group == "system").find_map(|r| {
            let egress = s.probes.get(&r.id)?.egress.first()?.clone();
            Some((r.ipv4.first().or(r.ipv6.first()).cloned().unwrap_or_default(), egress))
        });
        // (provider, label, first egress) for every other resolver.
        let others: Vec<(String, String, Option<String>)> = other_operators(&s.resolvers)
            .map(|r| (r.provider.clone(), r.label(), s.probes.get(&r.id).and_then(|p| p.egress.first().cloned())))
            .collect();
        (sys, others, s.env.pointer("/interception/verdict").and_then(Value::as_str) == Some("direct"))
    });
    let Some((sys_ip, egress)) = sys else { return };
    let hits: Vec<(String, String)> =
        others.into_iter().filter(|(_, _, e)| e.as_deref() == Some(egress.as_str())).map(|(p, l, _)| (p, l)).collect();
    let providers: BTreeSet<&str> = hits.iter().map(|(p, _)| p.as_str()).collect();
    let names = list(&hits.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>());
    match providers.len() {
        0 => infer_forwarder(h),
        1 => h.note(
            "info",
            format!(
                "Your system resolver ({sys_ip}) appears to forward to {}: lookups through either one reached authoritative servers from the same {0} address, {egress}.",
                providers.first().unwrap()
            ),
            None,
        ),
        _ if port53_direct => h.note(
            "info",
            format!(
                "Lookups through {names} reached authoritative servers from {egress}, the same address as your system resolver ({sys_ip}). The port-53 check found DNS reaching resolvers directly, so they probably share an upstream or infrastructure."
            ),
            None,
        ),
        _ => h.note(
            "warn",
            format!(
                "Lookups sent to {names} reached authoritative servers from {egress}, the same address as your system resolver ({sys_ip}). Separate operators don't share a recursive address, so something on this network is probably redirecting port 53 to one resolver: these results may describe that resolver, not the services named."
            ),
            None,
        ),
    }
}

/// Resolvers other than the system resolver. An entry for the system resolver's own
/// address (a custom "Pi-hole" entry, say) is the same server, not another operator.
fn other_operators(resolvers: &[Resolver]) -> impl Iterator<Item = &Resolver> {
    let system: Vec<&String> = resolvers.iter().filter(|r| r.group == "system").flat_map(|r| r.ipv4.iter().chain(&r.ipv6)).collect();
    resolvers.iter().filter(move |r| r.group != "system" && !r.ipv4.iter().chain(&r.ipv6).any(|ip| system.contains(&ip)))
}

/// "A", "A and B", "A, B and C".
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// When egress addresses differ (IPv4 vs IPv6, several nodes), the egress network can
/// still show where the system resolver forwards: one public operator egressing from
/// the same AS (and not the client's own ISP) is a strong hint.
fn infer_forwarder(h: &RunHandle) {
    let (sys_asn, sys_net, client_asn, others) = h.with(|s| {
        let asn = |id: &str| s.intel.get(id).and_then(|v| v.pointer("/egress/asn")).and_then(Value::as_u64);
        let system = s.resolvers.iter().find(|r| r.group == "system");
        let sys = system.and_then(|r| asn(&r.id));
        // "AS2495 (KANREN - Kansas Research and Education Network)" when the holder is known.
        let holder = system.and_then(|r| s.intel.get(&r.id)?.pointer("/egress/holder")?.as_str().map(str::to_owned));
        let net = sys.map(|a| holder.map_or_else(|| format!("AS{a}"), |h| format!("AS{a} ({h})")));
        let client = s.env.pointer("/vantage/network/asn").and_then(Value::as_u64);
        let others: Vec<(String, Option<u64>)> = other_operators(&s.resolvers).map(|r| (r.provider.clone(), asn(&r.id))).collect();
        (sys, net, client, others)
    });
    let (Some(sys), Some(net)) = (sys_asn, sys_net) else { return };
    if Some(sys) == client_asn {
        h.note(
            "info",
            format!("Your system resolver reaches authoritative servers from your own ISP's network, {net}, so it is probably your ISP's resolver or recurses on its own."),
            None,
        );
        return;
    }
    let providers: BTreeSet<String> = others.into_iter().filter(|(_, a)| *a == Some(sys)).map(|(p, _)| p).collect();
    if providers.len() == 1 {
        h.note(
            "info",
            format!(
                "Your system resolver appears to forward to {}: lookups through either one reach authoritative servers from the same network, {net}.",
                providers.first().unwrap()
            ),
            None,
        );
    }
}

/// What each test counts for in the overall latency. Cached answers are most of what people get;
/// uncached lookups (cache misses) are the delays they notice; TLD lookups show how quickly a resolver gets through the
/// root and TLD servers when it has to, which is a real share of a slow lookup. The UI mirrors this.
pub const TEST_WEIGHTS: [(&str, f64); 3] = [("cached", 0.40), ("uncached", 0.35), ("tld", 0.25)];

/// How far a test's typical latency is pulled from its median toward its 95th percentile: consistency
/// counts, so a steady resolver beats an erratic one with the same median, and one lucky fast answer
/// counts for nothing. The UI mirrors this.
pub const TAIL_WEIGHT: f64 = 0.5;

/// How much of a test's standard deviation is added on top. The 95th percentile ignores the worst one answer in
/// twenty, so two resolvers can share a tail yet differ in how far their stragglers go; the deviation sees that.
/// The UI mirrors this.
pub const JITTER_WEIGHT: f64 = 0.25;

/// One test's typical latency: the median, halfway to the 95th percentile, plus a quarter of the deviation.
fn typical(s: &stats::Summary) -> Option<f64> {
    s.p50.is_finite().then(|| {
        let tail = if s.p95.is_finite() { (s.p95 - s.p50).max(0.0) } else { 0.0 };
        let jitter = if s.sd.is_finite() { s.sd } else { 0.0 };
        s.p50 + TAIL_WEIGHT * tail + JITTER_WEIGHT * jitter
    })
}

/// The speed score the fastest pick ranks by, in ms: each test's typical latency, weighted by [`TEST_WEIGHTS`], plus
/// what its timeouts cost on average ([`TIMEOUT_COST_MS`] for each one, spread over every query sent).
pub fn composite(t: &TargetInfo) -> Option<f64> {
    score(&t.stats)
}

fn score(stats: &BTreeMap<String, stats::Summary>) -> Option<f64> {
    let (mut sum, mut w) = (0.0, 0.0);
    for (k, wt) in TEST_WEIGHTS {
        if let Some(v) = stats.get(k).and_then(typical) {
            sum += v * wt;
            w += wt;
        }
    }
    (w > 0.0).then(|| sum / w + timeout_cost_of(stats))
}

/// The average wait timeouts add to each lookup: the share of queries that timed out, times [`TIMEOUT_COST_MS`].
fn timeout_cost_of(stats: &BTreeMap<String, stats::Summary>) -> f64 {
    let (to, sent) = stats.values().fold((0u32, 0u32), |(a, n), s| (a + s.timeouts, n + s.sent));
    if sent == 0 { 0.0 } else { TIMEOUT_COST_MS * to as f64 / sent as f64 }
}

pub fn success(t: &TargetInfo) -> f64 {
    let (ok, sent) = t.stats.values().fold((0u32, 0u32), |(o, n), s| (o + s.ok, n + s.sent));
    if sent == 0 { 0.0 } else { ok as f64 / sent as f64 }
}

/// A target's CDN penalty (its resolver's median extra round trip to CDN edges), if mapped.
pub fn cdn_penalty(s: &RunState, t: &TargetInfo) -> Option<f64> {
    s.cdn.as_ref()?.resolvers.get(&t.resolver_id)?.penalty_ms
}

/// A local forwarder (dnsmasq, Pi-hole, the gateway's DNS proxy) among the resolvers this computer is set
/// to use. It is measured and ranked as a reference line but never named the best pick: it isn't an
/// alternative to choose, its cache answers from your own network, and what you choose is what it forwards to.
/// An ISP's resolver is no forwarder of this kind and competes like any other.
pub fn is_baseline(s: &RunState, t: &TargetInfo) -> bool {
    s.resolvers.iter().find(|r| r.id == t.resolver_id).is_some_and(|r| r.forwarder.is_some())
}

/// One target in ranking order.
pub struct Ranked<'a> {
    pub target: &'a TargetInfo,
    pub composite: Option<f64>,
    pub success: f64,
}

impl Ranked<'_> {
    /// Reliable, not intercepted, and with a speed score.
    fn eligible(&self) -> bool {
        self.composite.is_some() && !self.target.intercepted && self.success >= RELIABLE
    }
}

/// Targets in speed-score order: reliable, unintercepted targets with a result first. CDN edges have a pick of
/// their own and don't move this order.
pub fn ranked(s: &RunState) -> Vec<Ranked<'_>> {
    let mut v: Vec<Ranked> = s.targets.iter().map(|t| Ranked { target: t, composite: composite(t), success: success(t) }).collect();
    let tier = |r: &Ranked| match (r.composite, r.target.intercepted, r.success >= RELIABLE) {
        (None, _, _) => 3,
        (_, true, _) => 2,
        (_, _, false) => 1,
        _ => 0,
    };
    let key = |r: &Ranked| r.composite.unwrap_or(f64::MAX);
    v.sort_by(|a, b| tier(a).cmp(&tier(b)).then(key(a).total_cmp(&key(b))));
    v
}

/// Whether `behind` trails `ahead` by less than this run can tell apart: both speed scores redrawn 200 times from
/// their own queries (timeouts and errors drawn as such), and the 95% interval of the difference reaches zero.
/// Mirrors `tiedWith` in the UI step for step, seed included, so the run list and the verdict agree.
pub fn tied(s: &RunState, ahead: &TargetInfo, behind: &TargetInfo) -> bool {
    // Per test: answered round trips (ms, sorted), timeouts, queries counted.
    type Draws = Vec<(&'static str, Vec<f64>, u32, u32)>;
    let collect = |t: &TargetInfo| -> Draws {
        TestKind::ALL
            .iter()
            .map(|k| {
                let (mut v, mut to, mut sent) = (Vec::new(), 0, 0);
                for x in s.samples.iter().filter(|x| x.t as usize == t.idx && x.k == *k && x.o != Outcome::FromCache) {
                    sent += 1;
                    match x.o {
                        Outcome::Ok => v.push(x.us as f64 / 1000.0),
                        Outcome::Timeout => to += 1,
                        _ => {}
                    }
                }
                v.sort_by(|a, b| a.total_cmp(b));
                (k.key(), v, to, sent)
            })
            .collect()
    };
    let (a, b) = (collect(ahead), collect(behind));
    let mut rand = Mulberry32(fnv1a(&format!("{}:{}:{}", s.id, ahead.idx, behind.idx)));
    let mut redraw = |d: &Draws| {
        let mut st = BTreeMap::new();
        for (k, v, to, sent) in d {
            let (mut out, mut timeouts) = (Vec::new(), 0);
            for _ in 0..*sent {
                let x = (rand.next() * *sent as f64).floor() as usize;
                if x < v.len() {
                    out.push(v[x]);
                } else if x < v.len() + *to as usize {
                    timeouts += 1;
                }
            }
            if *sent > 0 {
                st.insert(k.to_string(), stats::summarize(&mut out, *sent, timeouts, 0));
            }
        }
        score(&st)
    };
    let mut diffs = Vec::new();
    for _ in 0..200 {
        let (x, y) = (redraw(&a), redraw(&b));
        if let (Some(x), Some(y)) = (x, y) {
            diffs.push(y - x);
        }
    }
    if diffs.len() < 100 {
        return false;
    }
    diffs.sort_by(|a, b| a.total_cmp(b));
    stats::quantile(&diffs, 0.025) <= 0.0
}

/// The UI's seeded generator (mulberry32), so the same run always gives the same answer in both.
struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

/// FNV-1a over UTF-16 code units, as the UI's `hash`.
fn fnv1a(s: &str) -> u32 {
    s.encode_utf16().fold(2_166_136_261u32, |h, c| (h ^ c as u32).wrapping_mul(16_777_619))
}

/// Small summary stored alongside each run for the history list and the CLI.
pub fn summary(s: &RunState) -> Value {
    // The fastest pick, and the next resolver when this run can't tell the two apart.
    let label = |t: &TargetInfo| format!("{} {}", t.provider, t.variant).trim().to_string();
    let picks: Vec<Ranked> = ranked(s).into_iter().filter(|r| r.eligible() && !is_baseline(s, r.target)).collect();
    let best = picks.first().map(|r| {
        let next = picks.iter().find(|n| n.target.provider != r.target.provider);
        // Within 0.05 ms is level before any redrawing, as the verdict has it.
        let level = |n: &Ranked| matches!((n.composite, r.composite), (Some(a), Some(b)) if a - b < 0.05);
        let tied_with = next.filter(|n| level(n) || tied(s, r.target, n.target)).map(|n| label(n.target));
        json!({
            "resolver_id": r.target.resolver_id, "label": label(r.target),
            "transport": r.target.transport, "composite_ms": r.composite,
            "cdn_penalty_ms": cdn_penalty(s, r.target), "success": r.success, "tied_with": tied_with,
        })
    });
    json!({
        "best": best,
        "resolvers": s.resolvers.len(),
        "targets": s.targets.len(),
        "queries": s.samples.len(),
        "tests": s.config.tests,
        "transports": s.config.transports,
        "vantage": s.env.pointer("/vantage/network/holder"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encrypted_only_run_never_opens_plain_dns() {
        let r = Resolver::custom("192.0.2.53".parse().unwrap(), None, None, None);
        let t = Duration::from_millis(200);
        assert!(open_for_run(&r, &[], t, false, false).is_none());
        assert_eq!(open_for_run(&r, &[], t, false, true).map(|(_, via)| via), Some("UDP"));
    }

    #[test]
    fn ecs_coverage() {
        let me: std::net::Ipv4Addr = "203.0.113.124".parse().unwrap();
        assert_eq!(ecs_contains("203.0.113.0/24", Some(me)), Some(true));
        assert_eq!(ecs_contains("223.165.96.0/24", Some(me)), Some(false));
        assert_eq!(ecs_contains("203.0.113.0/0", Some(me)), None);
        assert_eq!(ecs_contains("203.0.113.0/24", None), None);
    }

    #[test]
    fn system_aliases_are_not_other_operators() {
        let pihole: std::net::IpAddr = "192.168.1.53".parse().unwrap();
        let mut all = crate::catalog::system_entries(&[pihole]);
        all.push(Resolver::custom(pihole, Some("Pi Hole".into()), None, None));
        all.extend(crate::catalog::builtin().into_iter().filter(|r| r.id == "quad9"));
        let ids: Vec<&str> = other_operators(&all).map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["quad9"]);
        assert_eq!(list(&["A".into(), "B".into(), "C".into()]), "A, B and C");
        assert_eq!(list(&["A".into()]), "A");
    }

    #[test]
    fn consistency_and_cached_answers_count_in_the_composite() {
        let r = crate::catalog::builtin().into_iter().find(|r| r.id == "cloudflare").unwrap();
        let mk = |p50: f64, p95: f64| {
            let mut t = TargetInfo::new(&r, Transport::Udp, false, r.primary(false).unwrap());
            for k in ["cached", "uncached", "tld"] {
                t.stats.insert(k.into(), stats::Summary { p50, p95, ..Default::default() });
            }
            t
        };
        assert!((TEST_WEIGHTS.iter().map(|(_, w)| w).sum::<f64>() - 1.0).abs() < 1e-9);
        // Same median: the one with the long tail ranks behind the steady one.
        let (steady, erratic) = (composite(&mk(15.0, 16.0)).unwrap(), composite(&mk(15.0, 40.0)).unwrap());
        assert!((steady - 15.5).abs() < 1e-9 && (erratic - 27.5).abs() < 1e-9, "{steady} {erratic}");
        // Two resolvers with the same median and 95th percentile: the one with the wider spread ranks behind.
        let spread = |sd: f64| {
            let mut t = mk(15.0, 21.0);
            for v in t.stats.values_mut() {
                v.sd = sd;
            }
            composite(&t).unwrap()
        };
        assert!((spread(4.0) - 19.0).abs() < 1e-9 && spread(8.0) > spread(4.0));
        // A slightly slower median can win on steadiness.
        assert!(composite(&mk(16.0, 17.0)).unwrap() < erratic);
        // Cached answers weigh most, then uncached, then TLD.
        let mut t = mk(10.0, 10.0);
        t.stats.insert("cached".into(), stats::Summary { p50: 20.0, p95: 20.0, ..Default::default() });
        let cached_slow = composite(&t).unwrap();
        let mut u = mk(10.0, 10.0);
        u.stats.insert("uncached".into(), stats::Summary { p50: 20.0, p95: 20.0, ..Default::default() });
        let uncached_slow = composite(&u).unwrap();
        let mut tld = mk(10.0, 10.0);
        tld.stats.insert("tld".into(), stats::Summary { p50: 20.0, p95: 20.0, ..Default::default() });
        let tld_slow = composite(&tld).unwrap();
        assert!(cached_slow > uncached_slow && uncached_slow > tld_slow, "{cached_slow} {uncached_slow} {tld_slow}");
        assert!((cached_slow - 14.0).abs() < 1e-9 && (tld_slow - 12.5).abs() < 1e-9);
        // A test with no answers is left out rather than counted as zero.
        let mut none = mk(12.0, 12.0);
        none.stats.insert("tld".into(), stats::Summary { p50: f64::NAN, p95: f64::NAN, ..Default::default() });
        assert!((composite(&none).unwrap() - 12.0).abs() < 1e-9);
    }

    #[test]
    fn timeouts_cost_a_retry_in_the_composite() {
        let r = crate::catalog::builtin().into_iter().find(|r| r.id == "cloudflare").unwrap();
        let mut t = TargetInfo::new(&r, Transport::Udp, false, r.primary(false).unwrap());
        for k in ["cached", "uncached", "tld"] {
            t.stats.insert(k.into(), stats::Summary { sent: 100, p50: 10.0, p95: 10.0, ..Default::default() });
        }
        assert!((composite(&t).unwrap() - 10.0).abs() < 1e-9);
        // 3 timeouts in 300 queries: 1% of lookups wait a second more, 10 ms on average.
        t.stats.get_mut("uncached").unwrap().timeouts = 3;
        assert!((composite(&t).unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(timeout_cost_of(&TargetInfo::new(&r, Transport::Udp, false, r.primary(false).unwrap()).stats), 0.0);
    }

    #[test]
    fn answers_from_a_cache_are_left_out_of_uncached() {
        let zones: Vec<String> = vec!["a.test".into(), "b.test".into()];
        let smp = |t: u16, i: u32, d: Option<u32>| Sample { t, k: TestKind::Uncached, i, us: 9_000, o: Outcome::Ok, at: 0, h: 0, d };
        let mut v = vec![
            // Target 0: zone a (even rounds) at 300, then one answer at 120; zone b (odd rounds) at 60 throughout.
            smp(0, 0, Some(300)),
            smp(0, 2, Some(299)),
            smp(0, 4, Some(120)),
            smp(0, 1, Some(60)),
            smp(0, 3, Some(60)),
            // Target 1 caps TTLs at 100: always the same, never marked.
            smp(1, 0, Some(100)),
            smp(1, 2, Some(100)),
            // A negative answer has no TTL and is left alone.
            Sample { o: Outcome::Nxdomain, ..smp(1, 4, None) },
        ];
        let (changed, flagged) = mark_cached_answers(&mut v, &zones);
        assert_eq!(changed.len(), 1);
        assert_eq!(flagged, vec![(0, 5, 1)]);
        assert_eq!(v[2].o, Outcome::FromCache);
        assert!(v.iter().enumerate().all(|(n, x)| n == 2 || x.o != Outcome::FromCache));
    }

    /// A finished run with the given resolvers (one UDP target each) and samples `(target, test, round, outcome, at ms)`.
    fn run_with(ids: &[&str], samples: &[(u16, TestKind, u32, Outcome, u32)]) -> RunState {
        let resolvers: Vec<Resolver> = ids.iter().map(|id| crate::catalog::builtin().into_iter().find(|r| r.id == *id).unwrap()).collect();
        let targets = resolvers
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mut t = TargetInfo::new(r, Transport::Udp, false, r.primary(false).unwrap());
                t.idx = i;
                t
            })
            .collect();
        RunState {
            id: "t".into(),
            started_at: 0,
            finished_at: None,
            status: "done".into(),
            phase: "done".into(),
            done: 0,
            total: 0,
            config: RunConfig::with_resolvers(vec![]),
            env: Value::Null,
            resolvers,
            targets,
            samples: samples.iter().map(|&(t, k, i, o, at)| Sample { t, k, i, us: 15_000, o, at, h: 0, d: None }).collect(),
            warm: vec![],
            probes: BTreeMap::new(),
            intel: BTreeMap::new(),
            notes: vec![],
            cdn: None,
            link: None,
            paths: BTreeMap::new(),
            rates: BTreeMap::new(),
            timings: vec![],
            seq: 0,
        }
    }

    #[test]
    fn the_peak_rate_is_the_busiest_second_per_resolver() {
        // Google: 50 queries in the first second (two targets of one resolver would add up) and a trickle after.
        let mut v: Vec<_> = (0..50).map(|i| (0u16, TestKind::Cached, i, Outcome::Ok, i * 19)).collect();
        v.extend((0..5).map(|i| (0u16, TestKind::Cached, 50 + i, Outcome::Ok, 3000 + i * 900)));
        v.extend((0..4).map(|i| (1u16, TestKind::Cached, i, Outcome::Ok, i * 1500)));
        let s = run_with(&["google", "quad9"], &v);
        let r = peak_rates(&s);
        assert_eq!((r["google"], r["quad9"]), (50.0, 1.0));
        // A window is one second: samples exactly 1000 ms apart are not in it together.
        let s = run_with(&["google"], &[(0, TestKind::Cached, 0, Outcome::Ok, 0), (0, TestKind::Cached, 1, Outcome::Ok, 1000)]);
        assert_eq!(peak_rates(&s)["google"], 1.0);
    }

    #[test]
    fn answering_then_stopping_is_a_stall() {
        let ok = |n: u32| (0..n).map(|i| (0u16, TestKind::Uncached, i, Outcome::Ok, i * 20)).collect::<Vec<_>>();
        let then = |mut v: Vec<(u16, TestKind, u32, Outcome, u32)>, o: Outcome, n: u32| {
            let from = v.len() as u32;
            v.extend((0..n).map(|i| (0u16, TestKind::Uncached, from + i, o, (from + i) * 20)));
            v
        };
        let stall = |v: Vec<(u16, TestKind, u32, Outcome, u32)>| stalls(&run_with(&["google"], &v));
        let a = stall(then(ok(12), Outcome::Timeout, 6));
        assert_eq!((a.len(), a[0].answered, a[0].stopped, a[0].how), (1, 12, 6, "stopped answering"));
        assert_eq!(stall(then(ok(12), Outcome::Refused, 7))[0].how, "began refusing queries");
        assert_eq!(stall(then(ok(12), Outcome::Servfail, 5))[0].how, "began returning SERVFAIL");
        // Not stalls: a short tail, too little before it, or a head that was already lossy.
        assert!(stall(then(ok(12), Outcome::Timeout, 4)).is_empty());
        assert!(stall(then(ok(4), Outcome::Timeout, 8)).is_empty());
        let mut lossy = then(ok(6), Outcome::Timeout, 3);
        lossy.extend((9..12).map(|i| (0u16, TestKind::Uncached, i, Outcome::Ok, i * 20)));
        assert!(stall(then(lossy, Outcome::Timeout, 6)).is_empty());
        // An NXDOMAIN or a slow answer is not a refusal to answer.
        assert!(stall(then(ok(12), Outcome::Nxdomain, 8)).is_empty());
    }

    #[test]
    fn published_limits_are_set_beside_what_was_sent() {
        let mut v: Vec<_> = (0..12).map(|i| (0u16, TestKind::Uncached, i, Outcome::Ok, i * 20)).collect();
        v.extend((12..18).map(|i| (0u16, TestKind::Uncached, i, Outcome::Timeout, i * 20)));
        // Google publishes 1,000 per second; Recursa sent ~18: the network is the likelier cause, and it says so.
        let mut s = run_with(&["google", "quad9"], &v);
        s.rates = peak_rates(&s);
        let sel = s.resolvers.clone();
        let n = load_notes(&s, &sel);
        assert_eq!(n.len(), 1, "{n:?}");
        assert!(
            n[0].0.contains("1000 queries per second") && n[0].0.contains("at most 18") && n[0].0.contains("network is the likelier"),
            "{}",
            n[0].0
        );
        // Within half of a published rate: a warning about the run itself.
        s.rates.insert("google".into(), 600.0);
        let n = load_notes(&s, &sel);
        assert!(n.iter().any(|(t, id)| t.contains("within reach of the 1000") && id.as_deref() == Some("google")), "{n:?}");
        // A resolver that publishes nothing is not accused of anything, and says so when it stalls.
        let mut s = run_with(&["quad9"], &v);
        s.rates = peak_rates(&s);
        let sel = s.resolvers.clone();
        assert!(load_notes(&s, &sel)[0].0.contains("publishes no limit"));
    }

    #[test]
    fn a_local_system_resolver_is_a_baseline_not_a_pick() {
        let mut v = vec![10.0; 20];
        let base = stats::summarize(&mut v, 20, 0, 0);
        let mut local = crate::catalog::system_entries(&["192.168.1.53".parse().unwrap()]).remove(0);
        local.forwarder = Some("Pi-hole (dnsmasq-pi-hole-v2.93)".into());
        let isp = crate::catalog::system_entries(&["203.0.113.9".parse().unwrap()]).remove(0);
        let cf = crate::catalog::builtin().into_iter().find(|r| r.id == "cloudflare").unwrap();
        let target = |r: &Resolver, p50: f64, idx: usize| {
            let mut t = TargetInfo::new(r, Transport::Udp, false, r.primary(false).unwrap());
            t.idx = idx;
            t.state = "ready".into();
            for k in ["cached", "uncached", "tld"] {
                t.stats.insert(k.into(), stats::Summary { p50, p95: p50, ..base.clone() });
            }
            t
        };
        let mut isp = isp;
        isp.id = "system-1".into();
        let s = RunState {
            id: "t".into(),
            started_at: 0,
            finished_at: None,
            status: "done".into(),
            phase: "done".into(),
            done: 0,
            total: 0,
            config: RunConfig::with_resolvers(vec![]),
            env: Value::Null,
            targets: vec![target(&local, 6.0, 0), target(&isp, 12.0, 1), target(&cf, 15.0, 2)],
            resolvers: vec![local, isp, cf],
            samples: vec![],
            warm: vec![],
            probes: BTreeMap::new(),
            intel: BTreeMap::new(),
            notes: vec![],
            cdn: None,
            link: None,
            paths: BTreeMap::new(),
            rates: BTreeMap::new(),
            timings: vec![],
            seq: 0,
        };
        // The router or Pi-hole is the fastest, and stays in the ranking, but isn't the best pick;
        // an ISP's resolver is an alternative like any other, even at a private address.
        assert_eq!(ranked(&s)[0].target.resolver_id, "system-0");
        assert!(is_baseline(&s, &s.targets[0]) && !is_baseline(&s, &s.targets[1]) && !is_baseline(&s, &s.targets[2]));
        assert_eq!(summary(&s)["best"]["resolver_id"], "system-1");
    }

    #[test]
    fn cdn_edges_dont_move_the_speed_order() {
        // One resolver answers fastest while another maps CDNs better. Each is a pick of its own.
        let mut v = vec![10.0; 20];
        let base = stats::summarize(&mut v, 20, 0, 0);
        let target = |id: &str, p50: f64, idx: usize| {
            let r = crate::catalog::builtin().into_iter().find(|r| r.id == id).unwrap();
            let mut t = TargetInfo::new(&r, Transport::Udp, false, r.primary(false).unwrap());
            t.idx = idx;
            t.state = "ready".into();
            for k in ["cached", "uncached", "tld"] {
                t.stats.insert(k.into(), stats::Summary { p50, ..base.clone() });
            }
            t
        };
        let mut s = RunState {
            id: "t".into(),
            started_at: 0,
            finished_at: None,
            status: "done".into(),
            phase: "done".into(),
            done: 0,
            total: 0,
            config: RunConfig::with_resolvers(vec![]),
            env: Value::Null,
            resolvers: vec![],
            targets: vec![target("controld", 15.9, 0), target("cloudflare", 21.2, 1)],
            samples: vec![],
            warm: vec![],
            probes: BTreeMap::new(),
            intel: BTreeMap::new(),
            notes: vec![],
            cdn: None,
            link: None,
            paths: BTreeMap::new(),
            rates: BTreeMap::new(),
            timings: vec![],
            seq: 0,
        };
        assert_eq!(ranked(&s)[0].target.resolver_id, "controld");
        let mut cdn = cdn::CdnReport::default();
        for (id, p) in [("controld", 2.95), ("cloudflare", 0.07)] {
            cdn.resolvers.insert(id.into(), cdn::ResolverCdn { penalty_ms: Some(p), ..Default::default() });
        }
        s.cdn = Some(cdn);
        assert_eq!(ranked(&s)[0].target.resolver_id, "controld");
        assert_eq!(summary(&s)["best"]["resolver_id"], "controld");
        assert!((summary(&s)["best"]["cdn_penalty_ms"].as_f64().unwrap() - 2.95).abs() < 1e-9);
    }

    #[test]
    fn close_scores_are_tied_and_clear_gaps_are_not() {
        let cf = crate::catalog::builtin().into_iter().find(|r| r.id == "cloudflare").unwrap();
        let target = |idx: usize| {
            let mut t = TargetInfo::new(&cf, Transport::Udp, false, cf.primary(false).unwrap());
            t.idx = idx;
            t
        };
        // Round trips that wander by a few ms around a centre, the same pattern for each target.
        let wobble = [0.0, 3.1, -2.4, 5.6, -1.2, 0.8, 7.9, -3.3, 2.2, -0.6];
        let samples = |t: u16, centre: f64| -> Vec<Sample> {
            (0..60u32)
                .map(|i| Sample {
                    t,
                    k: TestKind::Cached,
                    i,
                    us: ((centre + wobble[i as usize % 10]) * 1000.0) as u32,
                    o: Outcome::Ok,
                    at: i,
                    h: 0,
                    d: None,
                })
                .collect()
        };
        let mut s = RunState {
            id: "t".into(),
            started_at: 0,
            finished_at: None,
            status: "done".into(),
            phase: "done".into(),
            done: 0,
            total: 0,
            config: RunConfig::with_resolvers(vec![]),
            env: Value::Null,
            resolvers: vec![],
            targets: vec![target(0), target(1)],
            samples: [samples(0, 20.0), samples(1, 20.4)].concat(),
            warm: vec![],
            probes: BTreeMap::new(),
            intel: BTreeMap::new(),
            notes: vec![],
            cdn: None,
            link: None,
            paths: BTreeMap::new(),
            rates: BTreeMap::new(),
            timings: vec![],
            seq: 0,
        };
        assert!(tied(&s, &s.targets[0], &s.targets[1]));
        s.samples = [samples(0, 20.0), samples(1, 30.0)].concat();
        assert!(!tied(&s, &s.targets[0], &s.targets[1]));
        // Nothing to redraw is no tie either.
        s.samples.clear();
        assert!(!tied(&s, &s.targets[0], &s.targets[1]));
    }

    #[test]
    fn classify_outcomes() {
        let reply = |rcode: u8, an: u16| {
            let mut b = vec![0u8; 12];
            b[2] = 0x81;
            b[3] = 0x80 | rcode;
            b[6..8].copy_from_slice(&an.to_be_bytes());
            Ok(net::Reply { bytes: b, rtt_ns: 12_345_000, user_rtt_ns: 12_400_000, kernel_stamp: true, ttl: Some(57) })
        };
        // NXDOMAIN for ub1.com: with the .com SOA it came from the registry; with another SOA, or none, it was made up.
        let nx = |soa_owner: &[u8]| {
            let mut b = vec![0, 1, 0x81, 0x83, 0, 1, 0, 0, 0, 1, 0, 0, 3, b'u', b'b', b'1', 3, b'c', b'o', b'm', 0, 0, 1, 0, 1];
            b.extend_from_slice(soa_owner);
            b.extend_from_slice(&[0, 6, 0, 1, 0, 0, 3, 0x84, 0, 22, 0, 0]);
            b.extend_from_slice(&[0; 20]);
            Ok(net::Reply { bytes: b, rtt_ns: 12_345_000, user_rtt_ns: 12_400_000, kernel_stamp: true, ttl: None })
        };
        assert_eq!(classify(TestKind::Tld, &nx(&[0xC0, 16])), (Outcome::Ok, 12_345, None));
        assert_eq!(classify(TestKind::Tld, &nx(&[0])).0, Outcome::Blocked);
        assert_eq!(classify(TestKind::Tld, &reply(3, 0)).0, Outcome::Blocked);
        assert_eq!(classify(TestKind::Uncached, &reply(3, 0)).0, Outcome::Nxdomain);
        assert_eq!(classify(TestKind::Tld, &reply(0, 1)).0, Outcome::Hijack);
        assert_eq!(classify(TestKind::Uncached, &reply(0, 2)).0, Outcome::Ok);
        assert_eq!(classify(TestKind::Uncached, &reply(2, 0)).0, Outcome::Servfail);
        assert_eq!(classify(TestKind::Cached, &Err(XErr::Timeout)), (Outcome::Timeout, 0, None));
        // A real answer: the first record's TTL is kept, and a filter's block address is no answer.
        let a = |ip: [u8; 4], ttl: u32| {
            let mut b = vec![0, 1, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0, 1, b'a', 0, 0, 1, 0, 1, 0xC0, 12, 0, 1, 0, 1];
            b.extend_from_slice(&ttl.to_be_bytes());
            b.extend_from_slice(&[0, 4]);
            b.extend_from_slice(&ip);
            Ok(net::Reply { bytes: b, rtt_ns: 9_000_000, user_rtt_ns: 9_000_000, kernel_stamp: true, ttl: None })
        };
        assert_eq!(classify(TestKind::Uncached, &a([185, 199, 108, 153], 3600)), (Outcome::Ok, 9_000, Some(3600)));
        assert_eq!(classify(TestKind::Uncached, &a([0, 0, 0, 0], 10)).0, Outcome::Blocked);
        assert_eq!(classify(TestKind::Cached, &a([127, 0, 0, 1], 10)).0, Outcome::Blocked);
    }

    #[test]
    fn only_opt_out_nsec3_keeps_a_tld() {
        let rec = |rtype: u16, flags: u8| dns::Record {
            name: "abc.com".into(),
            rtype,
            ttl: 900,
            data: dns::RData::Other(vec![1, flags, 0, 0, 0]),
        };
        let msg = |authority: Vec<dns::Record>| dns::Message {
            flags: 0,
            rcode: dns::RCODE_NXDOMAIN,
            qname: "ubx.com".into(),
            answers: vec![],
            authority,
            opt: None,
        };
        assert_eq!(denial(&msg(vec![rec(dns::TYPE_NSEC3, 1), rec(dns::TYPE_NSEC3, 1)]), "com"), Some(Denial::OptOut));
        assert_eq!(denial(&msg(vec![rec(dns::TYPE_NSEC3, 1), rec(dns::TYPE_NSEC3, 0)]), "com"), Some(Denial::Reusable));
        assert_eq!(denial(&msg(vec![rec(dns::TYPE_NSEC, 0)]), "com"), Some(Denial::Reusable));
        assert_eq!(denial(&msg(vec![rec(dns::TYPE_NSEC, 0)]), "org"), None);
        assert_eq!(denial(&msg(vec![]), "com"), None);
    }

    #[test]
    fn blocked_cached_names_are_named_per_resolver() {
        let b = Outcome::Blocked;
        let s = run_with(
            &["cloudflare", "google"],
            &[(0, TestKind::Cached, 1, b, 0), (0, TestKind::Cached, 9, b, 0), (0, TestKind::Cached, 3, b, 0), (1, TestKind::Tld, 0, b, 0)],
        );
        let cached: Vec<String> = ["a.example", "ads.example", "c.example", "track.example"].iter().map(|n| n.to_string()).collect();
        let notes = blocked_notes(&s, &cached);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].0, "cloudflare");
        assert!(notes[0].1.contains("blocked 2 of the Cached test's names (ads.example and track.example)"), "{}", notes[0].1);
        assert!(notes[0].1.contains("isn't ranked"));
        assert!(notes[1].1.contains("blocked 1 of the random names the TLD test asks"), "{}", notes[1].1);
    }

    /// Answers (or times out) after `delay`, recording each send instant.
    struct PaceStub {
        id: usize,
        delay: Duration,
        timeout: bool,
        sends: Arc<Mutex<Vec<(usize, Instant)>>>,
    }

    impl Exchanger for PaceStub {
        fn exchange(&mut self, query: &[u8], _timeout: Duration) -> Result<net::Reply, XErr> {
            self.sends.lock().unwrap().push((self.id, Instant::now()));
            std::thread::sleep(self.delay);
            if self.timeout {
                return Err(XErr::Timeout);
            }
            let mut end = 12;
            while query[end] != 0 {
                end += 1 + query[end] as usize;
            }
            end += 5;
            let mut r = query[..end].to_vec();
            r[2] = 0x81;
            r[3] = 0x80;
            r[6..12].copy_from_slice(&[0, 1, 0, 0, 0, 0]);
            r.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 1, 0x2C, 0, 4, 192, 0, 2, 1]);
            Ok(net::Reply { bytes: r, rtt_ns: 1_000_000, user_rtt_ns: 1_000_000, kernel_stamp: false, ttl: None })
        }
    }

    type PacedRun = (Vec<Sample>, Vec<usize>, Vec<(usize, Instant)>, Duration);

    /// Runs `measure_paced` for each stub `(delay, always_timeout)`; returns samples, dropped targets,
    /// send records and wall time.
    fn run_paced(stubs: &[(Duration, bool)], samples: u32, gap: Duration, cancel: &AtomicBool) -> PacedRun {
        let sends = Arc::new(Mutex::new(Vec::new()));
        let mut xs: Vec<Box<dyn Exchanger>> = stubs
            .iter()
            .enumerate()
            .map(|(id, &(delay, timeout))| Box::new(PaceStub { id, delay, timeout, sends: sends.clone() }) as Box<dyn Exchanger>)
            .collect();
        let pacer = Pacer::new(gap);
        let pacer = &pacer;
        let fresh = Fresh::defaults();
        let fresh = &fresh;
        let (tx, rx) = mpsc::channel();
        let t0 = Instant::now();
        let dropped: Vec<usize> = std::thread::scope(|sc| {
            let workers: Vec<_> = xs
                .iter_mut()
                .enumerate()
                .map(|(ti, x)| {
                    let tx = tx.clone();
                    sc.spawn(move || {
                        measure_paced(ti, x.as_mut(), pacer, TestKind::Cached, samples, &[], fresh, Duration::from_secs(1), t0, cancel, tx)
                    })
                })
                .collect();
            drop(tx);
            workers.into_iter().filter_map(|w| w.join().unwrap()).collect()
        });
        let wall = t0.elapsed();
        let out: Vec<Sample> = rx.try_iter().collect();
        let sends = sends.lock().unwrap().clone();
        (out, dropped, sends, wall)
    }

    #[test]
    fn paced_every_target_gets_all_its_samples() {
        let stubs = [(Duration::from_millis(1), false); 3];
        let (out, dropped, _, _) = run_paced(&stubs, 6, Duration::from_millis(2), &AtomicBool::new(false));
        assert!(dropped.is_empty());
        assert_eq!(out.len(), 18);
        for t in 0..3u16 {
            let mut is: Vec<u32> = out.iter().filter(|s| s.t == t).map(|s| s.i).collect();
            is.sort();
            assert_eq!(is, (0..6).collect::<Vec<_>>());
            assert!(out.iter().filter(|s| s.t == t).all(|s| s.o == Outcome::Ok));
        }
    }

    #[test]
    fn paced_sends_are_a_gap_apart_globally() {
        let gap = Duration::from_millis(4);
        let stubs = [(Duration::from_millis(1), false); 4];
        let (_, _, sends, _) = run_paced(&stubs, 10, gap, &AtomicBool::new(false));
        let mut at: Vec<Instant> = sends.iter().map(|s| s.1).collect();
        assert_eq!(at.len(), 40);
        at.sort();
        // Stamps are taken after a thread wakes, so wake-up jitter (worse under a parallel test run) can put one
        // pair closer than the gap; the reserved slots are exact, so the whole span cannot be shorter than 39 gaps.
        let span = *at.last().unwrap() - at[0];
        assert!(span + Duration::from_millis(5) >= gap * 39, "span {span:?}");
    }

    #[test]
    fn paced_slow_target_does_not_hold_the_others() {
        let slow = Duration::from_millis(150);
        let stubs =
            [(slow, false), (Duration::from_millis(1), false), (Duration::from_millis(1), false), (Duration::from_millis(1), false)];
        let (out, _, _, wall) = run_paced(&stubs, 5, Duration::from_millis(4), &AtomicBool::new(false));
        // Ideally the fast targets are done within a few tens of ms, but CI runners stretch the pacer's
        // short sleeps (one Windows run finished at 153 ms), so the bound is 400 ms. It still proves the
        // point: serialised behind the slow target they would finish after its fourth turn, past 600 ms.
        for t in 1..4u16 {
            let last = out.iter().filter(|s| s.t == t).map(|s| s.at).max().unwrap();
            assert!(last < 400, "target {t} finished at {last} ms");
        }
        // Serial across targets would take 20 slow-or-fast turns; CI runners overshoot the stub's sleeps by
        // a few hundred ms, so the bound is loose and the check above carries the test.
        assert!(wall < slow * 10, "wall {wall:?}");
    }

    #[test]
    fn paced_target_is_dropped_after_consecutive_timeouts() {
        let stubs = [(Duration::from_millis(1), true), (Duration::from_millis(1), false), (Duration::from_millis(1), false)];
        let (out, dropped, _, _) = run_paced(&stubs, 10, Duration::from_millis(2), &AtomicBool::new(false));
        assert_eq!(dropped, vec![0]);
        assert_eq!(out.iter().filter(|s| s.t == 0).count(), DROP_AFTER as usize);
        assert!(out.iter().filter(|s| s.t == 0).all(|s| s.o == Outcome::Timeout));
        assert_eq!(out.iter().filter(|s| s.t == 1).count(), 10);
        assert_eq!(out.iter().filter(|s| s.t == 2).count(), 10);
    }

    #[test]
    fn paced_cancel_stops_promptly() {
        let cancel = AtomicBool::new(false);
        let sends = Arc::new(Mutex::new(Vec::new()));
        let mut xs: Vec<PaceStub> =
            (0..3).map(|id| PaceStub { id, delay: Duration::from_millis(1), timeout: false, sends: sends.clone() }).collect();
        let (pacer, fresh) = (Pacer::new(Duration::from_millis(2)), Fresh::defaults());
        let (tx, rx) = mpsc::channel();
        let t0 = Instant::now();
        let (pacer, fresh, cancel) = (&pacer, &fresh, &cancel);
        let count = std::thread::scope(|sc| {
            for (ti, x) in xs.iter_mut().enumerate() {
                let tx = tx.clone();
                sc.spawn(move || measure_paced(ti, x, pacer, TestKind::Cached, 1000, &[], fresh, Duration::from_secs(1), t0, cancel, tx));
            }
            drop(tx);
            rx.recv().unwrap();
            cancel.store(true, Ordering::Relaxed);
            1 + rx.iter().count()
        });
        // Each thread may finish the query it had in flight, and no more.
        assert!(count <= 1 + 3, "{count} samples after the first");
    }
}
