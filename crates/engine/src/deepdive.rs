//! Deep Dive: a short, fixed, read-only look at one resolver.
//!
//! About twenty ordinary queries, spaced like the benchmark's, give the shape of one resolver's
//! round trips, a guess at what its cache saves, a guess at whether it serves a popular answer from
//! cache, and the details of one response. It is a diagnostic, not a load test: the query count is
//! fixed and nothing here can be made to send more.
//!
//! Only the round trips and the response fields are measured. The cache findings are inferred from
//! them and are labelled that way. How many upstream lookups the resolver made is not observable
//! from here, so none is reported.

use crate::bench::{ACCURATE_GAP, on_local_network};
use crate::catalog::Resolver;
use crate::dns::{self, QuerySpec};
use crate::endpoints;
use crate::intel::{FeedHost, ThreatFeeds};
use crate::net::{self, Endpoint, Exchanger, Transport, XErr};
use crate::probes::{self, FilterCategory, rand_label};
use crate::stats::{self, Summary};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Hard cap on queries for the whole report.
pub const MAX_QUERIES: u32 = 24;
/// Identical repeats of the popular name for the latency distribution.
const REPEATS: u32 = 12;
/// Unique-label names that force a cache miss (one per wildcard zone).
const COLD: usize = 3;
/// How long to wait before the last query, so a counting-down TTL has moved by whole seconds.
const TTL_WAIT: Duration = Duration::from_millis(2000);
/// Stop after this many failures in a row.
const GIVE_UP: u32 = 3;

#[derive(Clone, Debug, Serialize)]
pub struct DeepDive {
    /// UDP, TCP, DoT or DoH: whichever connection the dive used.
    pub transport: String,
    /// The popular name asked for the latency, warm and TTL findings.
    pub name: String,
    /// Queries sent in all.
    pub queries: u32,
    pub duration_ms: f64,
    pub latency: Latency,
    pub cache: CacheCost,
    pub ttl: TtlProbe,
    /// The first answered response, field by field.
    pub response: Option<ResponseInfo>,
    /// Every query sent, in order.
    pub timeline: Vec<Step>,
    /// Why the dive stopped early, if it did.
    pub aborted: Option<String>,
}

/// Which part of the dive a query belongs to.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// The popular name, first and second ask.
    Popular,
    /// A unique label that has to be recursed.
    Random,
    /// The popular name again, for the latency distribution.
    Repeat,
    /// The popular name after the wait, for the TTL.
    Last,
}

/// One query sent: its round trip, or `None` when nothing usable came back.
#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub kind: StepKind,
    pub ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Latency {
    /// Round trips of the identical repeats, in the order sent (ms). Answered queries only.
    pub samples_ms: Vec<f64>,
    /// min, p50, p95, max; `iqr` is the jitter measure.
    pub stats: Summary,
    /// At least one reply carried no kernel receive stamp, so these are timed in user space.
    pub user_space_timed: bool,
}

/// Cold against warm, as an inference.
#[derive(Clone, Debug, Serialize)]
pub struct CacheCost {
    /// Median of the unique-label queries (each must be recursed), ms.
    pub cold_ms: Option<f64>,
    pub cold_samples_ms: Vec<f64>,
    /// The popular name's second ask, ms.
    pub warm_ms: Option<f64>,
    /// `cold_ms - warm_ms`: approximately what recursing costs over a cache hit. Inferred.
    pub recursion_cost_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Residency {
    /// The TTL fell in step with the clock.
    CountingDown,
    /// The TTL did not move although seconds passed.
    Unchanged,
    /// The TTL went up: the answer was fetched again, or another server answered.
    Refreshed,
    /// It fell, but not in step with the clock.
    Irregular,
    /// Too little time passed, or no answer carried a TTL.
    Undetermined,
}

/// Cache residency from the answer TTL, as an inference.
#[derive(Clone, Debug, Serialize)]
pub struct TtlProbe {
    /// TTL of the first answer record at the first ask.
    pub first: Option<u32>,
    /// TTL at the last ask.
    pub last: Option<u32>,
    /// Seconds between those two asks.
    pub elapsed_s: f64,
    pub suggests: Residency,
}

#[derive(Clone, Debug, Serialize)]
pub struct Flags {
    pub qr: bool,
    pub aa: bool,
    pub tc: bool,
    pub rd: bool,
    pub ra: bool,
    pub ad: bool,
    pub cd: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EdnsInfo {
    pub udp_size: u16,
    pub version: u8,
    pub dnssec_ok: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct EdeInfo {
    pub code: u16,
    pub name: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResponseInfo {
    pub rcode: u16,
    pub rcode_name: String,
    pub flags: Flags,
    /// Record counts from the message header.
    pub answers: u16,
    pub authority: u16,
    pub additional: u16,
    pub size_bytes: usize,
    pub edns: Option<EdnsInfo>,
    pub ede: Vec<EdeInfo>,
    pub nsid: Option<String>,
}

struct Asked {
    rtt_ms: f64,
    kernel_stamp: bool,
    bytes: Vec<u8>,
    msg: dns::Message,
    at: Instant,
}

struct Session<'a> {
    ex: &'a mut dyn Exchanger,
    timeout: Duration,
    cancel: &'a AtomicBool,
    queries: u32,
    failures: u32,
    aborted: Option<String>,
    timeline: Vec<Step>,
}

impl Session<'_> {
    /// One query, then the benchmark's pause. `None` on a failure or once the dive has given up.
    fn ask(&mut self, name: &str, kind: StepKind) -> Option<Asked> {
        if self.cancel.load(Ordering::Relaxed) {
            self.aborted.get_or_insert_with(|| "The dive was cancelled.".into());
        }
        if self.aborted.is_some() || self.queries >= MAX_QUERIES {
            return None;
        }
        self.queries += 1;
        // DO and NSID on every query, so cold and warm differ only in whether the name is cached.
        let spec = QuerySpec::new(name, dns::TYPE_A).with_edns(|e| {
            e.dnssec_ok = true;
            e.nsid = true;
        });
        let res =
            dns::encode_query(dns::new_id(), &spec).map_err(|e| XErr::Io(e.to_string())).and_then(|q| self.ex.exchange(&q, self.timeout));
        let out = match res {
            Ok(r) => match dns::parse(&r.bytes) {
                Ok(msg) => {
                    Some(Asked { rtt_ms: r.rtt_ns as f64 / 1e6, kernel_stamp: r.kernel_stamp, bytes: r.bytes, msg, at: Instant::now() })
                }
                Err(_) => None,
            },
            Err(_) => None,
        };
        self.timeline.push(Step { kind, ms: out.as_ref().map(|a| a.rtt_ms) });
        if out.is_some() {
            self.failures = 0;
        } else {
            self.failures += 1;
            if self.failures >= GIVE_UP {
                self.aborted = Some(format!("The resolver didn't answer {GIVE_UP} queries in a row, so the dive stopped."));
            }
        }
        std::thread::sleep(ACCURATE_GAP);
        out
    }
}

/// Run the dive over an open connection, stopping early once `cancel` is set. `transport` is only a label for the report.
pub fn run(ex: &mut dyn Exchanger, transport: &str, timeout: Duration, cancel: &AtomicBool) -> DeepDive {
    run_with(ex, transport, timeout, TTL_WAIT, cancel)
}

fn run_with(ex: &mut dyn Exchanger, transport: &str, timeout: Duration, ttl_wait: Duration, cancel: &AtomicBool) -> DeepDive {
    let started = Instant::now();
    let name = crate::bench::CACHED_NAMES[0];
    let mut s = Session { ex, timeout, cancel, queries: 0, failures: 0, aborted: None, timeline: Vec::new() };
    let mut kernel_missing = false;

    // The popular name, twice: the first may fetch it, the second is the warm sample.
    let prime = s.ask(name, StepKind::Popular);
    let hit = s.ask(name, StepKind::Popular);

    // Unique labels under wildcard zones: each one is a miss the resolver has to recurse.
    let mut cold = Vec::new();
    for zone in crate::bench::UNCACHED_ZONES.iter().take(COLD) {
        if let Some(a) = s.ask(&format!("dd{}.{zone}", rand_label(12)), StepKind::Random)
            && matches!(a.msg.rcode, dns::RCODE_NOERROR | dns::RCODE_NXDOMAIN)
        {
            kernel_missing |= !a.kernel_stamp;
            cold.push(a.rtt_ms);
        }
    }

    let mut series = Vec::new();
    for _ in 0..REPEATS {
        if let Some(a) = s.ask(name, StepKind::Repeat) {
            series.push(a);
        }
    }
    // In short slices, so a cancel is heard.
    let wake = Instant::now() + ttl_wait;
    while !cancel.load(Ordering::Relaxed) && Instant::now() < wake {
        std::thread::sleep(Duration::from_millis(50).min(wake.saturating_duration_since(Instant::now())));
    }
    let last = s.ask(name, StepKind::Last);

    // Latency: the repeats that came back NOERROR. The rest count as errors, whatever went wrong.
    let mut ok_ms = Vec::new();
    for a in &series {
        kernel_missing |= !a.kernel_stamp;
        if a.msg.rcode == dns::RCODE_NOERROR {
            ok_ms.push(a.rtt_ms);
        }
    }
    let samples_ms = ok_ms.clone();
    let latency_stats = stats::summarize(&mut ok_ms, REPEATS, 0, REPEATS - samples_ms.len() as u32);

    let warm_ms = hit.as_ref().filter(|a| a.msg.rcode == dns::RCODE_NOERROR).map(|a| a.rtt_ms);
    if let Some(a) = hit.as_ref().or(prime.as_ref()) {
        kernel_missing |= !a.kernel_stamp;
    }
    let mut sorted_cold = cold.clone();
    sorted_cold.sort_by(|a, b| a.total_cmp(b));
    let cold_ms = (!sorted_cold.is_empty()).then(|| stats::quantile(&sorted_cold, 0.5));
    let recursion_cost_ms = cold_ms.zip(warm_ms).map(|(c, w)| c - w);

    // TTL: the first answer against the last, with the time between them.
    let first = prime.as_ref().or(hit.as_ref());
    let end = last.as_ref().or(series.last());
    let ttl_of = |a: Option<&Asked>| a.and_then(|a| a.msg.answers.first()).map(|r| r.ttl);
    let (t_first, t_last) = (ttl_of(first), ttl_of(end));
    let elapsed_s = match (first, end) {
        (Some(f), Some(l)) => l.at.saturating_duration_since(f.at).as_secs_f64(),
        _ => 0.0,
    };
    let suggests = match (t_first, t_last) {
        (Some(f), Some(l)) => infer_residency(f, l, elapsed_s),
        _ => Residency::Undetermined,
    };

    let response = prime.as_ref().or(hit.as_ref()).or(series.first()).map(|a| response_info(&a.msg, &a.bytes));

    DeepDive {
        transport: transport.to_string(),
        name: name.to_string(),
        queries: s.queries,
        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
        latency: Latency { samples_ms, stats: latency_stats, user_space_timed: kernel_missing },
        cache: CacheCost { cold_ms, cold_samples_ms: cold, warm_ms, recursion_cost_ms },
        ttl: TtlProbe { first: t_first, last: t_last, elapsed_s, suggests },
        response,
        timeline: s.timeline,
        aborted: s.aborted,
    }
}

/// Whether a TTL's movement over `elapsed_s` looks like a cache counting down.
fn infer_residency(first: u32, last: u32, elapsed_s: f64) -> Residency {
    if elapsed_s < 1.0 {
        return Residency::Undetermined;
    }
    let drop = first as i64 - last as i64;
    match drop {
        d if d < 0 => Residency::Refreshed,
        0 => Residency::Unchanged,
        d if (d as f64 - elapsed_s).abs() <= 2.0 => Residency::CountingDown,
        _ => Residency::Irregular,
    }
}

fn response_info(m: &dns::Message, bytes: &[u8]) -> ResponseInfo {
    let count = |i: usize| bytes.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0);
    let f = |bit: u16| m.flags & bit != 0;
    ResponseInfo {
        rcode: m.rcode,
        rcode_name: dns::rcode_name(m.rcode).to_string(),
        flags: Flags { qr: f(0x8000), aa: m.aa(), tc: f(0x0200), rd: f(0x0100), ra: f(0x0080), ad: m.ad(), cd: f(0x0010) },
        answers: count(6),
        authority: count(8),
        additional: count(10),
        size_bytes: bytes.len(),
        edns: m.opt.as_ref().map(|o| EdnsInfo { udp_size: o.udp_size, version: o.version, dnssec_ok: o.dnssec_ok }),
        ede: ede_infos(m),
        nsid: m.nsid(),
    }
}

fn ede_infos(m: &dns::Message) -> Vec<EdeInfo> {
    m.ede().into_iter().map(|(code, text)| EdeInfo { code, name: dns::ede_name(code).to_string(), text }).collect()
}

/// Feed hosts sampled per category, and how many of them that still resolve get tested.
const LIVE_SAMPLE: usize = 60;
const LIVE_WANT: usize = 20;
/// How long a confirmed sample is reused, so resolvers checked one after another face the same hosts.
pub const LIVE_SAMPLE_TTL_MS: i64 = 30 * 60_000;

/// Feed hosts an unfiltered resolver confirmed are live. Shared by every live check for a while, so their block
/// rates compare like for like.
#[derive(Clone, Debug, Serialize)]
pub struct LiveSample {
    /// When the hosts were confirmed (Unix ms).
    pub taken_at: i64,
    /// "malware" (URLhaus) and "phishing" (OpenPhish).
    pub categories: BTreeMap<String, SampleCategory>,
    /// Why a category has fewer hosts than asked for, or none.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SampleCategory {
    pub source: String,
    /// When the feed was downloaded (Unix ms).
    pub fetched_at: Option<i64>,
    /// Feed hosts drawn at random; `hosts` are the ones of those that still resolve.
    pub sampled: usize,
    pub hosts: Vec<FeedHost>,
}

/// The live-feed blocking check: hosts from current malware and phishing feeds that still resolve through an
/// unfiltered resolver, looked up through this one. Opt-in from the Deep Dive page, because security tools on the
/// network may flag those lookups.
#[derive(Clone, Debug, Serialize)]
pub struct LiveBlocking {
    /// DoT or DoH, or UDP/TCP when plain DNS was allowed.
    pub transport: String,
    /// The sample's `taken_at`: checks with the same value tested the same hosts.
    pub sample_taken_at: i64,
    pub categories: BTreeMap<String, LiveCategory>,
    pub notes: Vec<String>,
    pub duration_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LiveCategory {
    #[serde(flatten)]
    pub result: FilterCategory,
    pub fetched_at: Option<i64>,
    pub sampled: usize,
    /// When the feed first listed each tested host, for feeds that date entries.
    pub listed_at: BTreeMap<String, i64>,
}

/// Whether live threat names may go to `r` over plain DNS: when the user allowed it, or to a system resolver on
/// this network (a Pi-hole, the router), whose blocking is what the user most needs checked.
fn live_over_plain(r: &Resolver, allow_plain: bool) -> bool {
    allow_plain || (r.group == "system" && r.primary(false).or_else(|| r.primary(true)).is_some_and(|ip| on_local_network(&ip)))
}

/// Draw hosts from each feed and keep those an unfiltered resolver still resolves. The baseline is encrypted: a
/// blocking box on the path would otherwise make live hosts look dead.
fn live_sample(feeds: ThreatFeeds, timeout: Duration, cancel: &AtomicBool) -> Result<LiveSample, String> {
    let mut base = [("8.8.8.8", "dns.google"), ("1.1.1.1", "one.one.one.one")]
        .iter()
        .find_map(|(ip, host)| {
            let ep = Endpoint {
                transport: Transport::Dot,
                addr: SocketAddr::new(ip.parse().unwrap(), 853),
                host: Some(host.to_string()),
                url: None,
            };
            net::open(&ep, timeout).ok()
        })
        .ok_or("Couldn't reach an unfiltered resolver over DoT (8.8.8.8 or 1.1.1.1) to confirm which feed hosts are live.")?;
    let mut sample = LiveSample { taken_at: crate::bench::now_ms(), categories: BTreeMap::new(), notes: Vec::new() };
    for (cat, feed, source) in [("malware", feeds.malware, "URLhaus"), ("phishing", feeds.phishing, "OpenPhish")] {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut list = feed.hosts;
        if list.is_empty() {
            sample.notes.push(format!("Couldn't download the {source} feed, so {cat} wasn't tested."));
            continue;
        }
        fastrand::shuffle(&mut list);
        list.truncate(LIVE_SAMPLE);
        let names: Vec<String> = list.iter().map(|h| h.host.clone()).collect();
        let live = probes::validate_live(base.as_mut(), &names, LIVE_WANT, timeout);
        if live.len() < 3 {
            sample.notes.push(format!(
                "Only {} of {} {source} hosts sampled still resolve, so the {cat} result is less certain.",
                live.len(),
                list.len()
            ));
        }
        let hosts = live.iter().filter_map(|d| list.iter().find(|h| &h.host == d).cloned()).collect();
        sample
            .categories
            .insert(cat.to_string(), SampleCategory { source: source.into(), fetched_at: feed.fetched_at, sampled: list.len(), hosts });
    }
    Ok(sample)
}

/// Run the live-feed check against `r`, with `prior` as the sample if it is still fresh, else a new one drawn
/// from `feeds`. Returns the sample used, to keep for the next check. Errors are messages for the user.
pub fn live_blocking(
    r: &Resolver,
    prior: Option<LiveSample>,
    feeds: ThreatFeeds,
    allow_plain: bool,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<(LiveBlocking, LiveSample), String> {
    let started = Instant::now();
    // Live names travel inside an encrypted connection unless plain DNS is allowed for this resolver.
    let (mut ex, transport) = endpoints::open_encrypted(r, &[], timeout, true)
        .or_else(|| live_over_plain(r, allow_plain).then(|| endpoints::open_best(r, &[], timeout, false)).flatten())
        .ok_or_else(|| {
            if live_over_plain(r, allow_plain) {
                format!("Couldn't open a connection to {}.", r.label())
            } else {
                format!("{} has no DoT or DoH endpoint here. Allow plain DNS to test it over that.", r.label())
            }
        })?;
    let quick = timeout.min(Duration::from_millis(1500));
    let sample = match prior {
        Some(s) => s,
        None => live_sample(feeds, quick, cancel)?,
    };
    let mut categories = BTreeMap::new();
    for (cat, sc) in &sample.categories {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let names: Vec<String> = sc.hosts.iter().map(|h| h.host.clone()).collect();
        let source = format!("{}: {} live of {} sampled", sc.source, names.len(), sc.sampled);
        let result = probes::test_category(ex.as_mut(), &names, &source, quick);
        let listed_at = sc.hosts.iter().filter_map(|h| Some((h.host.clone(), h.listed_at?))).collect();
        categories.insert(cat.clone(), LiveCategory { result, fetched_at: sc.fetched_at, sampled: sc.sampled, listed_at });
    }
    let out = LiveBlocking {
        transport: transport.to_string(),
        sample_taken_at: sample.taken_at,
        categories,
        notes: sample.notes.clone(),
        duration_ms: started.elapsed().as_secs_f64() * 1000.0,
    };
    Ok((out, sample))
}

/// Large signed answer for the truncation check: the root's DNSKEY set with signatures, about 1.1 KB, more while a key
/// rollover double-signs it. Whether it should fit a buffer is judged against its size over TCP.
const BIG_NAME: &str = ".";
/// Unsigned zone whose wildcard answers carry a three-day TTL: a fresh label shows the longest the resolver keeps an
/// answer, and a counting-down TTL shows which asks it answered from cache.
const LONG_TTL_ZONE: &str = "localtest.me";
/// A 20-second TXT record, to show whether the resolver raises short TTLs.
const SHORT_TTL_NAME: &str = "whoami.ds.akahelp.net";
/// Unsigned zone with no wildcard and a two-hour negative TTL, for the negative-caching check.
const NEG_ZONE: &str = "akahelp.net";
/// Known DNSSEC cases: name, what it shows, and whether a validating resolver should accept it. `None` means either
/// answer is correct (an algorithm a resolver may not support is treated as unsigned).
const DNSSEC_CASES: &[(&str, &str, Option<bool>)] = &[
    ("valid.extended-dns-errors.com", "RSA/SHA-256, correctly signed", Some(true)),
    ("cloudflare.com", "ECDSA P-256, correctly signed", Some(true)),
    ("ed25519.extended-dns-errors.com", "Ed25519, correctly signed", Some(true)),
    ("ed448.extended-dns-errors.com", "Ed448, correctly signed", None),
    ("rrsig-exp-all.extended-dns-errors.com", "Expired signatures", Some(false)),
    ("ds-bad-tag.extended-dns-errors.com", "DS record matches no key", Some(false)),
];
/// Made-up names and rounds for the cache count. Rounds are at least a second apart so a cached TTL has counted down.
const CACHE_NAMES: usize = 4;
const CACHE_ROUNDS: usize = 6;
const ROUND_GAP: Duration = Duration::from_millis(1100);
/// Queries per transport for the cost check, after the connection is open.
const COST_ASKS: u32 = 10;

/// The Deep Dive's further checks: truncation and TCP fallback, TTL limits, negative caching, more DNSSEC cases,
/// how many caches answer, and what each transport costs.
#[derive(Clone, Debug, Serialize)]
pub struct DeepChecks {
    /// The connection used for the TTL, negative-caching, DNSSEC and cache checks.
    pub transport: Option<String>,
    pub truncation: Vec<SizeStep>,
    pub ttl: Vec<TtlLimit>,
    pub negative: Option<NegativeCache>,
    pub dnssec: Vec<DnssecCase>,
    pub caches: Option<CacheCount>,
    pub transports: Vec<TransportCost>,
    pub notes: Vec<String>,
    pub duration_ms: f64,
}

/// One ask for the large signed answer.
#[derive(Clone, Debug, Serialize)]
pub struct SizeStep {
    pub transport: String,
    /// EDNS buffer size the query offered; `None` over TCP, where it doesn't apply.
    pub buffer: Option<u16>,
    pub answered: bool,
    pub size_bytes: Option<usize>,
    pub tc: bool,
    pub answers: usize,
    pub ad: bool,
    pub ms: Option<f64>,
}

/// One record's TTL from the zone's own server and from the resolver.
#[derive(Clone, Debug, Serialize)]
pub struct TtlLimit {
    /// "long" or "short".
    pub kind: String,
    pub name: String,
    pub qtype: String,
    pub authoritative: Option<u32>,
    pub resolver: Option<u32>,
}

/// How long the resolver keeps a "doesn't exist" answer, asked twice `gap_s` apart.
#[derive(Clone, Debug, Serialize)]
pub struct NegativeCache {
    pub name: String,
    pub rcode: Option<String>,
    /// The zone's negative TTL: the lower of its SOA's TTL and minimum.
    pub authoritative: Option<u32>,
    pub first: Option<u32>,
    pub later: Option<u32>,
    /// Round trips of the two asks: a fast second one came from cache even if its TTL didn't count down.
    pub first_ms: Option<f64>,
    pub later_ms: Option<f64>,
    pub gap_s: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DnssecCase {
    pub name: String,
    pub label: String,
    /// Whether a validating resolver should accept the answer; `None` when either is correct.
    pub expect_valid: Option<bool>,
    pub rcode: Option<String>,
    pub ad: bool,
    pub ede: Vec<EdeInfo>,
}

/// Made-up names asked in rounds. An answer with the full TTL was fetched upstream; a lower one came from a cache.
#[derive(Clone, Debug, Serialize)]
pub struct CacheCount {
    pub rows: Vec<CacheRow>,
    /// Most upstream fetches seen for one name: at least this many caches answer for the address.
    pub caches: Option<u32>,
    pub rounds: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct CacheRow {
    pub name: String,
    pub asks: Vec<CacheAsk>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CacheAsk {
    pub ttl: Option<u32>,
    pub ms: Option<f64>,
    pub fetched: bool,
}

/// Connection setup and per-query time over one transport.
#[derive(Clone, Debug, Serialize)]
pub struct TransportCost {
    pub transport: String,
    /// TCP connect, plus the TLS handshake for DoT and DoH. `None` for UDP, which has no setup.
    pub setup_ms: Option<f64>,
    /// The first query on the new connection.
    pub first_ms: Option<f64>,
    /// Median of the later queries on the same connection.
    pub reused_ms: Option<f64>,
    pub answered: u32,
    pub asked: u32,
    pub error: Option<String>,
}

/// One query over `ex`, with its round trip in milliseconds and its size.
fn ask_once(ex: &mut dyn Exchanger, spec: &QuerySpec, timeout: Duration) -> Option<(f64, usize, dns::Message)> {
    let q = dns::encode_query(dns::new_id(), spec).ok()?;
    let r = ex.exchange(&q, timeout).ok()?;
    let m = dns::parse(&r.bytes).ok()?;
    Some((r.rtt_ns as f64 / 1e6, r.bytes.len(), m))
}

/// Ask for the root DNSKEY set with DO over UDP at 512 and 1232 bytes, then over TCP.
fn truncation(r: &Resolver, timeout: Duration, notes: &mut Vec<String>) -> Vec<SizeStep> {
    let mut steps = Vec::new();
    if !r.udp {
        notes.push("The resolver has no plain DNS address, so truncation and TCP fallback weren't checked.".into());
        return steps;
    }
    for (t, buffer) in [(Transport::Udp, Some(512)), (Transport::Udp, Some(1232)), (Transport::Tcp, None)] {
        let spec = QuerySpec::new(BIG_NAME, dns::TYPE_DNSKEY).with_edns(|e| {
            e.dnssec_ok = true;
            e.udp_size = buffer.unwrap_or(1232);
        });
        let got = endpoints::plain(r, t, false)
            .or_else(|| endpoints::plain(r, t, true))
            .and_then(|ep| net::open(&ep, timeout).ok())
            .and_then(|mut ex| ask_once(ex.as_mut(), &spec, timeout));
        let mut step =
            SizeStep { transport: t.label().into(), buffer, answered: false, size_bytes: None, tc: false, answers: 0, ad: false, ms: None };
        if let Some((ms, size, m)) = got {
            step =
                SizeStep { answered: true, size_bytes: Some(size), tc: m.tc(), answers: m.answers.len(), ad: m.ad(), ms: Some(ms), ..step };
        }
        steps.push(step);
        std::thread::sleep(ACCURATE_GAP);
    }
    steps
}

/// The lowest TTL among answers of `qtype`.
fn answer_ttl(m: &dns::Message, qtype: u16) -> Option<u32> {
    m.answers.iter().filter(|a| a.rtype == qtype).map(|a| a.ttl).min()
}

/// The negative TTL a response carries: its SOA's TTL, capped by the SOA minimum (RFC 2308).
fn negative_ttl(m: &dns::Message) -> Option<u32> {
    m.authority.iter().find_map(|a| match &a.data {
        dns::RData::Soa { minimum, .. } => Some(a.ttl.min(*minimum)),
        _ => None,
    })
}

/// `spec` asked without recursion of its zone's own server, at the first NS address `ex` finds. The zone is the owner
/// of the SOA that comes back for the name, so a delegated subzone is followed. `None` unless the answer is
/// authoritative (a box on the path that answers port 53 itself isn't).
fn ask_authority(ex: &mut dyn Exchanger, mut spec: QuerySpec, timeout: Duration) -> Option<dns::Message> {
    let (_, _, soa) = ask_once(ex, &QuerySpec::new(spec.name.as_str(), dns::TYPE_SOA), timeout)?;
    let zone = soa.answers.iter().chain(&soa.authority).find(|a| a.rtype == dns::TYPE_SOA)?.name.clone();
    let (_, _, ns) = ask_once(ex, &QuerySpec::new(zone, dns::TYPE_NS), timeout)?;
    let host = ns.answers.iter().find_map(|a| match &a.data {
        dns::RData::Name(n) if a.rtype == dns::TYPE_NS => Some(n.clone()),
        _ => None,
    })?;
    let (_, _, a) = ask_once(ex, &QuerySpec::new(host, dns::TYPE_A), timeout)?;
    // The resolver under test picks this address: never one on this computer or its network.
    let ip = a
        .ipv4_answers()
        .into_iter()
        .find(|ip| !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast() && !crate::catalog::is_private(&(*ip).into()))?;
    let ep = Endpoint { transport: Transport::Udp, addr: SocketAddr::new(ip.into(), 53), host: None, url: None };
    let mut auth = net::open(&ep, timeout).ok()?;
    spec.rd = false;
    let (_, _, m) = ask_once(auth.as_mut(), &spec, timeout)?;
    m.aa().then_some(m)
}

/// Compare the resolver's TTL with the authoritative one for a fresh three-day name and a 20-second name.
fn ttl_limits(ex: &mut dyn Exchanger, timeout: Duration, cancel: &AtomicBool, notes: &mut Vec<String>) -> Vec<TtlLimit> {
    let long_name = format!("rc{}.{LONG_TTL_ZONE}", rand_label(12));
    let mut out = Vec::new();
    for (kind, name, qtype, qname) in [("long", long_name.as_str(), dns::TYPE_A, "A"), ("short", SHORT_TTL_NAME, dns::TYPE_TXT, "TXT")] {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let resolver = ask_once(ex, &QuerySpec::new(name, qtype), timeout).and_then(|(_, _, m)| answer_ttl(&m, qtype));
        std::thread::sleep(ACCURATE_GAP);
        let authoritative = ask_authority(ex, QuerySpec::new(name, qtype), timeout).and_then(|m| answer_ttl(&m, qtype));
        if authoritative.is_none() {
            notes.push(format!("Couldn't get {name}'s TTL from its own server, so the {kind} TTL can't be compared."));
        }
        out.push(TtlLimit { kind: kind.into(), name: name.into(), qtype: qname.into(), authoritative, resolver });
    }
    out
}

/// Ask the resolver each DNSSEC case with DO set.
fn dnssec_cases(ex: &mut dyn Exchanger, timeout: Duration, cancel: &AtomicBool) -> Vec<DnssecCase> {
    let mut out = Vec::new();
    for &(name, label, expect_valid) in DNSSEC_CASES {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let spec = QuerySpec::new(name, dns::TYPE_A).with_edns(|e| e.dnssec_ok = true);
        let got = ask_once(ex, &spec, timeout);
        out.push(DnssecCase {
            name: name.into(),
            label: label.into(),
            expect_valid,
            rcode: got.as_ref().map(|(_, _, m)| dns::rcode_name(m.rcode).into()),
            ad: got.as_ref().is_some_and(|(_, _, m)| m.ad()),
            ede: got.as_ref().map(|(_, _, m)| ede_infos(m)).unwrap_or_default(),
        });
        std::thread::sleep(ACCURATE_GAP);
    }
    out
}

/// Ask made-up names in rounds and count, per name, the asks that came back with the full TTL: each one was a
/// fetch from upstream, so a name fetched more than once was answered by more than one cache.
fn cache_count(ex: &mut dyn Exchanger, timeout: Duration, cancel: &AtomicBool) -> CacheCount {
    let mut rows: Vec<CacheRow> =
        (0..CACHE_NAMES).map(|_| CacheRow { name: format!("rc{}.{LONG_TTL_ZONE}", rand_label(12)), asks: Vec::new() }).collect();
    for _ in 0..CACHE_ROUNDS {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let round = Instant::now();
        for row in &mut rows {
            let got = ask_once(ex, &QuerySpec::new(row.name.as_str(), dns::TYPE_A), timeout);
            row.asks.push(CacheAsk {
                ttl: got.as_ref().and_then(|(_, _, m)| answer_ttl(m, dns::TYPE_A)),
                ms: got.map(|g| g.0),
                fetched: false,
            });
            std::thread::sleep(ACCURATE_GAP);
        }
        std::thread::sleep(ROUND_GAP.saturating_sub(round.elapsed()));
    }
    let mut counts_down = false;
    for row in &mut rows {
        let Some(full) = row.asks.iter().filter_map(|a| a.ttl).max() else { continue };
        counts_down |= row.asks.iter().any(|a| a.ttl.is_some_and(|t| t < full));
        for a in &mut row.asks {
            a.fetched = a.ttl == Some(full);
        }
    }
    // A TTL that never counts down can't tell a fetch from a cached answer.
    let caches = counts_down.then(|| rows.iter().map(|r| r.asks.iter().filter(|a| a.fetched).count() as u32).max()).flatten();
    if !counts_down {
        rows.iter_mut().flat_map(|r| r.asks.iter_mut()).for_each(|a| a.fetched = false);
    }
    CacheCount { rows, caches, rounds: CACHE_ROUNDS }
}

/// Open UDP, TCP, DoT and DoH where the resolver offers them, then time `COST_ASKS` queries for a popular name on that connection.
fn transport_costs(r: &Resolver, timeout: Duration, cancel: &AtomicBool) -> Vec<TransportCost> {
    let name = crate::bench::CACHED_NAMES[0];
    let mut out = Vec::new();
    for t in [Transport::Udp, Transport::Tcp, Transport::Dot, Transport::Doh] {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let ep = match t {
            Transport::Udp | Transport::Tcp if r.udp => endpoints::plain(r, t, false).or_else(|| endpoints::plain(r, t, true)),
            Transport::Dot | Transport::Doh => endpoints::encrypted(r, t, false, timeout, true).map(|(e, _)| e),
            _ => None,
        };
        let Some(ep) = ep else { continue };
        let mut cost = TransportCost {
            transport: t.label().into(),
            setup_ms: None,
            first_ms: None,
            reused_ms: None,
            answered: 0,
            asked: 0,
            error: None,
        };
        match net::open(&ep, timeout) {
            Err(e) => cost.error = Some(e.to_string()),
            Ok(mut ex) => {
                cost.setup_ms = ex.setup_ns().map(|n| n as f64 / 1e6);
                let mut later = Vec::new();
                for i in 0..COST_ASKS {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    cost.asked += 1;
                    if let Some((ms, _, _)) = ask_once(ex.as_mut(), &QuerySpec::new(name, dns::TYPE_A), timeout) {
                        cost.answered += 1;
                        if i == 0 {
                            cost.first_ms = Some(ms);
                        } else {
                            later.push(ms);
                        }
                    }
                    std::thread::sleep(ACCURATE_GAP);
                }
                later.sort_by(f64::total_cmp);
                cost.reused_ms = (!later.is_empty()).then(|| stats::quantile(&later, 0.5));
            }
        }
        out.push(cost);
    }
    out
}

/// Run the further checks against `r`, stopping early once `cancel` is set. About 90 queries in all, most of them
/// the cache count and the transport timings.
pub fn deep_checks(r: &Resolver, timeout: Duration, cancel: &AtomicBool) -> Result<DeepChecks, String> {
    let started = Instant::now();
    let mut notes = Vec::new();
    let truncation = truncation(r, timeout, &mut notes);
    let mut out = DeepChecks {
        transport: None,
        truncation,
        ttl: Vec::new(),
        negative: None,
        dnssec: Vec::new(),
        caches: None,
        transports: Vec::new(),
        notes: Vec::new(),
        duration_ms: 0.0,
    };
    match endpoints::open_best(r, &[], timeout, false) {
        Some((mut ex, transport)) => {
            out.transport = Some(transport.into());
            let ex = ex.as_mut();
            // A "doesn't exist" name asked now and again after the other checks, to see its TTL count down.
            let nx = format!("rc{}.{NEG_ZONE}", rand_label(12));
            let first = ask_once(ex, &QuerySpec::new(nx.as_str(), dns::TYPE_A), timeout);
            let asked = Instant::now();
            std::thread::sleep(ACCURATE_GAP);
            out.ttl = ttl_limits(ex, timeout, cancel, &mut notes);
            out.dnssec = dnssec_cases(ex, timeout, cancel);
            out.caches = (!cancel.load(Ordering::Relaxed)).then(|| cache_count(ex, timeout, cancel));
            let later = ask_once(ex, &QuerySpec::new(nx.as_str(), dns::TYPE_A), timeout);
            let gap_s = asked.elapsed().as_secs_f64();
            let authoritative = ask_authority(ex, QuerySpec::new(nx.as_str(), dns::TYPE_A), timeout)
                .filter(|m| m.rcode == 3)
                .and_then(|m| negative_ttl(&m));
            out.negative = Some(NegativeCache {
                rcode: first.as_ref().map(|(_, _, m)| dns::rcode_name(m.rcode).into()),
                first: first.as_ref().and_then(|(_, _, m)| negative_ttl(m)),
                later: later.as_ref().and_then(|(_, _, m)| negative_ttl(m)),
                first_ms: first.as_ref().map(|g| g.0),
                later_ms: later.as_ref().map(|g| g.0),
                name: nx,
                authoritative,
                gap_s,
            });
        }
        None => notes
            .push(format!("Couldn't open a connection to {}, so TTLs, negative caching, DNSSEC and caches weren't checked.", r.label())),
    }
    out.transports = transport_costs(r, timeout, cancel);
    if out.truncation.iter().all(|s| !s.answered) && out.transport.is_none() && out.transports.iter().all(|t| t.answered == 0) {
        return Err(format!("Couldn't reach {}.", r.label()));
    }
    out.notes = notes;
    out.duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Reply;

    /// Answers every query itself: slower for unique-label names, a fixed TTL, and a
    /// scripted OPT with NSID and an EDE.
    struct Stub {
        calls: u32,
        ttl: u32,
        kernel: bool,
        dead: bool,
    }

    impl Exchanger for Stub {
        fn exchange(&mut self, query: &[u8], _timeout: Duration) -> Result<Reply, XErr> {
            self.calls += 1;
            if self.dead {
                return Err(XErr::Timeout);
            }
            // The question runs from byte 12 to the root label plus type and class.
            let mut end = 12;
            while query[end] != 0 {
                end += 1 + query[end] as usize;
            }
            end += 5;
            let cold = query[13] == b'd' && query[14] == b'd';
            let mut r = query[..end].to_vec();
            r[2] = 0x81; // QR RD
            r[3] = 0x80; // RA, NOERROR
            r[6..12].copy_from_slice(&[0, 1, 0, 0, 0, 1]);
            r.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1]);
            r.extend_from_slice(&self.ttl.to_be_bytes());
            r.extend_from_slice(&[0, 4, 192, 0, 2, 1]);
            // OPT: 1232 bytes, DO, NSID "stub-1" and EDE 3.
            r.extend_from_slice(&[0, 0, 41, 0x04, 0xD0, 0, 0, 0x80, 0, 0, 16]);
            r.extend_from_slice(&[0, 3, 0, 6]);
            r.extend_from_slice(b"stub-1");
            r.extend_from_slice(&[0, 15, 0, 2, 0, 3]);
            let ms = if cold { 80_000_000 } else { 20_000_000 };
            Ok(Reply { bytes: r, rtt_ns: ms, user_rtt_ns: ms, kernel_stamp: self.kernel, ttl: None })
        }
    }

    fn stub() -> Stub {
        Stub { calls: 0, ttl: 300, kernel: true, dead: false }
    }

    #[test]
    fn a_dive_is_fixed_small_and_reads_the_response() {
        let mut ex = stub();
        let d = run_with(&mut ex, "UDP", Duration::from_millis(100), Duration::ZERO, &AtomicBool::new(false));
        assert_eq!(d.queries, 1 + 1 + COLD as u32 + REPEATS + 1);
        assert_eq!(ex.calls, d.queries);
        assert!(d.queries <= MAX_QUERIES);
        assert_eq!(d.timeline.len(), d.queries as usize);
        let kinds: Vec<_> = d.timeline.iter().map(|t| t.kind).collect();
        assert_eq!(&kinds[..3], &[StepKind::Popular, StepKind::Popular, StepKind::Random]);
        assert_eq!(kinds.last(), Some(&StepKind::Last));
        assert!(d.timeline.iter().all(|t| t.ms.is_some()));
        assert_eq!(d.latency.samples_ms.len(), REPEATS as usize);
        assert_eq!(d.latency.stats.min, 20.0);
        assert_eq!(d.latency.stats.p50, 20.0);
        assert!(!d.latency.user_space_timed);
        assert_eq!(d.cache.cold_ms, Some(80.0));
        assert_eq!(d.cache.warm_ms, Some(20.0));
        assert_eq!(d.cache.recursion_cost_ms, Some(60.0));
        assert_eq!(d.ttl.suggests, Residency::Undetermined, "no time passed, so no verdict");
        let r = d.response.expect("a response");
        assert_eq!(r.rcode_name, "NOERROR");
        assert!(r.flags.qr && r.flags.rd && r.flags.ra && !r.flags.aa && !r.flags.ad);
        assert_eq!((r.answers, r.authority, r.additional), (1, 0, 1));
        let e = r.edns.expect("edns");
        assert_eq!((e.udp_size, e.version, e.dnssec_ok), (1232, 0, true));
        assert_eq!(r.nsid.as_deref(), Some("stub-1"));
        assert_eq!((r.ede[0].code, r.ede[0].name.as_str()), (3, "Stale Answer"));
        assert!(d.aborted.is_none());
    }

    #[test]
    fn a_reply_without_a_kernel_stamp_marks_the_timing_as_user_space() {
        let mut ex = Stub { kernel: false, ..stub() };
        let d = run_with(&mut ex, "UDP", Duration::from_millis(100), Duration::ZERO, &AtomicBool::new(false));
        assert!(d.latency.user_space_timed);
    }

    #[test]
    fn a_silent_resolver_stops_the_dive_after_three_failures() {
        let mut ex = Stub { dead: true, ..stub() };
        let d = run_with(&mut ex, "UDP", Duration::from_millis(100), Duration::ZERO, &AtomicBool::new(false));
        assert_eq!(ex.calls, GIVE_UP);
        assert_eq!(d.queries, GIVE_UP);
        assert!(d.timeline.len() == GIVE_UP as usize && d.timeline.iter().all(|t| t.ms.is_none()));
        assert!(d.aborted.is_some());
        assert!(d.response.is_none() && d.cache.cold_ms.is_none());
    }

    #[test]
    fn ttl_movement_is_read_against_the_clock() {
        assert_eq!(infer_residency(300, 297, 3.0), Residency::CountingDown);
        assert_eq!(infer_residency(300, 300, 3.0), Residency::Unchanged);
        assert_eq!(infer_residency(200, 300, 3.0), Residency::Refreshed);
        assert_eq!(infer_residency(300, 200, 3.0), Residency::Irregular);
        assert_eq!(infer_residency(300, 299, 0.4), Residency::Undetermined);
    }

    #[test]
    fn live_names_reach_only_a_local_system_resolver_over_plain_dns() {
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        let pihole = crate::catalog::system_entries(&[ip("192.168.1.53")]).remove(0);
        let isp = crate::catalog::system_entries(&[ip("68.94.156.1")]).remove(0);
        let custom = Resolver::custom(ip("10.0.0.53"), None, None, None);
        assert!(live_over_plain(&pihole, false));
        assert!(!live_over_plain(&isp, false) && !live_over_plain(&custom, false));
        assert!(live_over_plain(&isp, true) && live_over_plain(&custom, true));
        for stub in ["100.100.100.100", "100.64.0.1", "127.0.0.53"] {
            let sys = crate::catalog::system_entries(&[ip(stub)]).remove(0);
            assert!(!live_over_plain(&sys, false), "{stub}");
        }
    }
}
