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

use crate::bench::ACCURATE_GAP;
use crate::dns::{self, QuerySpec};
use crate::net::{Exchanger, XErr};
use crate::probes::rand_label;
use crate::stats::{self, Summary};
use serde::Serialize;
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
        let res = dns::encode_query(fastrand::u16(..), &spec)
            .map_err(|e| XErr::Io(e.to_string()))
            .and_then(|q| self.ex.exchange(&q, self.timeout));
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
        ede: m.ede().into_iter().map(|(code, text)| EdeInfo { code, name: dns::ede_name(code).to_string(), text }).collect(),
        nsid: m.nsid(),
    }
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
}
