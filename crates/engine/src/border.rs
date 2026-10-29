//! Background border-link monitoring: for each CDN edge address (see [`crate::edge`]), the last router inside your own
//! network and the first router beyond it are probed with ICMP every few minutes, and the two round trips are kept for
//! weeks. When only the far router slows down, and at the same hours each day, the queue is on the link between them;
//! when both slow down, it is on your side. The pairs come from a trace (Trippy), found again daily.
//!
//! Probes are two per side per cycle, a few seconds at most, through the same machine-wide lock as runs. iOS builds
//! exclude Trippy and get stubs, so there nothing is found or probed.

use crate::app::AppState;
use crate::bench;
use crate::edge::RollCall;
use crate::lock;
use crate::resolve::Resolve;
use crate::stats::{median, quantile};
use crate::trace::{self, Trace, TraceCap};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The shortest time between cycles.
pub(crate) const MIN_INTERVAL_MIN: u32 = 2;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;
/// Hourly buckets a verdict needs, and samples in each.
const VERDICT_HOURS: usize = 24;
// Two, so an hour counts at every interval the page offers (down to one probe every 30 minutes).
const VERDICT_SAMPLES: usize = 2;
/// Cycles in a row the far side may stay silent before the pair is looked for again.
const MAX_MISSES: u32 = 3;
/// Trace identifiers, apart from the ones a run uses (`0x5200` and up): discovery, then two per pair for probes.
const DISCOVER_ID: u16 = 0x5300;
const PROBE_ID: u16 = 0x5400;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BorderMonitorConfig {
    pub enabled: bool,
    pub interval_min: u32,
    pub retention_days: u32,
}

impl Default for BorderMonitorConfig {
    fn default() -> Self {
        BorderMonitorConfig { enabled: false, interval_min: 5, retention_days: 30 }
    }
}

impl BorderMonitorConfig {
    /// Keeps the numbers within what is gentle and sensible.
    pub fn clamped(mut self) -> Self {
        self.interval_min = self.interval_min.clamp(MIN_INTERVAL_MIN, 60);
        self.retention_days = self.retention_days.clamp(1, 365);
        self
    }
}

/// Whether the far router is the target's own network, or one more network in between.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Direct,
    Via,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Direct => "direct",
            Kind::Via => "via",
        }
    }

    pub fn parse(s: &str) -> Kind {
        if s == "direct" { Kind::Direct } else { Kind::Via }
    }
}

/// The two routers probed for one CDN target: `near` is the last hop in your own network, `far` the next to answer.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Pair {
    pub id: i64,
    pub cdn: String,
    pub target: IpAddr,
    pub near: IpAddr,
    pub near_ttl: u8,
    pub far: IpAddr,
    pub far_ttl: u8,
    pub far_asn: Option<u32>,
    pub far_holder: Option<String>,
    pub kind: Kind,
    pub found_at: i64,
    pub retired_at: Option<i64>,
}

/// One cycle's probes of a pair: the lowest round trip on each side, none when it didn't answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub ts: i64,
    pub pair: i64,
    pub near_ms: Option<f64>,
    pub far_ms: Option<f64>,
}

// ---------------------------------------------------------------------------------------------
// Finding the pair
// ---------------------------------------------------------------------------------------------

/// The pair in a trace to `target`: `near` is the last hop in `home_asn` (else the network of the first public hop),
/// `far` the next hop that answered. `None` when no hop maps to the home network or nothing answered beyond it.
fn pair_from(trace: &Trace, cdn: &str, target_asn: Option<u32>, home_asn: Option<u32>, now: i64) -> Option<Pair> {
    let home = home_asn.or_else(|| trace.hops.iter().find(|h| !h.private && h.asn.is_some()).and_then(|h| h.asn))?;
    let near = trace.hops.iter().rev().find(|h| h.asn == Some(home) && !h.addrs.is_empty())?;
    let far = trace.hops.iter().find(|h| h.ttl > near.ttl && !h.addrs.is_empty())?;
    Some(Pair {
        id: 0,
        cdn: cdn.to_string(),
        target: trace.target,
        near: near.addrs[0],
        near_ttl: near.ttl,
        far: far.addrs[0],
        far_ttl: far.ttl,
        far_asn: far.asn,
        far_holder: far.holder.clone(),
        kind: if far.asn.is_some() && far.asn == target_asn { Kind::Direct } else { Kind::Via },
        found_at: now,
        retired_at: None,
    })
}

/// Trace to `target` and find its border pair. Blocks, so call it from `spawn_blocking`. `id` keeps this trace apart
/// from others running at the same time.
fn discover(target: IpAddr, cdn: &str, target_asn: Option<u32>, home_asn: Option<u32>, id: u16, res: &Resolve) -> Option<Pair> {
    let mut t = trace::run_live(target, id, &|_| {}).ok()?;
    trace::enrich(&mut t.hops, res);
    pair_from(&t, cdn, target_asn, home_asn, bench::now_ms())
}

// ---------------------------------------------------------------------------------------------
// Probing
// ---------------------------------------------------------------------------------------------

/// Two ICMP probes toward `target` that expire at `ttl`. The lowest round trip of those answered by `expect`; a reply
/// from another address (the route moved) counts as no answer.
#[cfg(not(target_os = "ios"))]
fn side(target: IpAddr, ttl: u8, expect: IpAddr, id: u16) -> Option<f64> {
    use std::sync::Mutex;
    use trippy_core::{Builder, PrivilegeMode, ProbeStatus, Protocol};

    let mode = if cfg!(target_os = "macos") { PrivilegeMode::Unprivileged } else { PrivilegeMode::Privileged };
    let tracer = Builder::new(target)
        .protocol(Protocol::Icmp)
        .privilege_mode(mode)
        .trace_identifier(id)
        .first_ttl(ttl)
        .max_ttl(ttl)
        .max_rounds(Some(2))
        .grace_duration(Duration::from_millis(100))
        .max_round_duration(Duration::from_millis(1000))
        .min_round_duration(Duration::from_millis(200))
        .build()
        .ok()?;
    let best: Mutex<Option<f64>> = Mutex::new(None);
    tracer
        .run_with(|round| {
            for p in round.probes {
                if let ProbeStatus::Complete(c) = p
                    && c.host == expect
                {
                    let ms = c.received.duration_since(c.sent).unwrap_or_default().as_secs_f64() * 1000.0;
                    let mut b = best.lock().unwrap();
                    *b = Some(b.map_or(ms, |x| x.min(ms)));
                }
            }
        })
        .ok()?;
    best.into_inner().unwrap().map(|ms| (ms * 100.0).round() / 100.0)
}

#[cfg(target_os = "ios")]
fn side(_target: IpAddr, _ttl: u8, _expect: IpAddr, _id: u16) -> Option<f64> {
    None
}

/// One cycle's sample for `pair`: both sides at once, under `id` and `id + 1`. Blocks.
fn probe(pair: &Pair, id: u16, ts: i64) -> Sample {
    let (near_ms, far_ms) = std::thread::scope(|s| {
        let near = s.spawn(|| side(pair.target, pair.near_ttl, pair.near, id));
        let far = side(pair.target, pair.far_ttl, pair.far, id + 1);
        (near.join().ok().flatten(), far)
    });
    Sample { ts, pair: pair.id, near_ms, far_ms }
}

// ---------------------------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------------------------

/// Find the pair for each target of the roll call and make the stored pairs match: a changed pair retires the old one.
/// A target that yields nothing keeps the pair it had. Returns the pairs now in use.
async fn rediscover(app: &AppState, roll: &RollCall, now: i64) -> Vec<Pair> {
    let res = Resolve::new(app.system.read().unwrap().clone());
    let home = roll.you.asn;
    // A network map run under a week old already traced to each edge: use its trace and skip tracing again.
    let mapped = crate::netmap::latest_traces(&app.store, now).unwrap_or_default();
    let mut found: Vec<Pair> = Vec::new();
    let mut jobs = Vec::new();
    for (i, (cdn, ip, asn)) in roll.edges.iter().filter_map(|e| e.ip.map(|ip| (e.cdn, ip, e.asn))).enumerate() {
        let from_map =
            mapped.iter().find(|m| m.target.id == cdn && m.target.addr == ip).and_then(|m| pair_from(&m.trace, cdn, asn, home, now));
        match from_map {
            Some(pair) => found.push(pair),
            None => {
                let res = res.clone();
                jobs.push(tokio::task::spawn_blocking(move || discover(ip, cdn, asn, home, DISCOVER_ID + i as u16, &res)));
            }
        }
    }
    found.extend(futures_util::future::join_all(jobs).await.into_iter().filter_map(|r| r.ok().flatten()));
    let current = app.store.border_pairs(now);
    for new in found {
        match current.iter().find(|p| p.retired_at.is_none() && p.cdn == new.cdn) {
            Some(old) if old.near == new.near && old.far == new.far => {}
            old => {
                if let Some(old) = old {
                    app.store.border_retire(old.id, now);
                }
                app.store.border_insert_pair(&new);
            }
        }
    }
    app.store.border_pairs(now)
}

pub fn spawn(app: Arc<AppState>) {
    tokio::spawn(async move {
        let mut last_cycle = 0i64;
        let mut last_discovery = 0i64;
        let mut misses: HashMap<i64, u32> = HashMap::new();
        let mut owner: Option<lock::Guard> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            // Another process may have switched it on or off.
            app.reload_border_monitor();
            let cfg = app.border_monitor.borrow().clone();
            if !cfg.enabled || trace::available() != TraceCap::Ok {
                owner = None;
                continue;
            }
            let now = bench::now_ms();
            // Seed from what is recorded, so a restart doesn't probe again at once.
            if last_cycle == 0 {
                last_cycle = app.store.border_latest().unwrap_or(0);
            }
            let due = last_cycle + cfg.interval_min.clamp(MIN_INTERVAL_MIN, 60) as i64 * 60_000;
            if app.run_active.load(Ordering::SeqCst) || app.intel.fetching() || now < due {
                continue;
            }
            if owner.is_none() {
                owner = lock::try_lock(&lock::border_monitor_path(&app.store.path));
                if owner.is_none() {
                    continue;
                }
            }
            let Some(_measuring) = lock::try_lock(&lock::measure_path()) else { continue };
            last_cycle = now;
            let mut pairs: Vec<Pair> = app.store.border_pairs(now).into_iter().filter(|p| p.retired_at.is_none()).collect();
            let want = last_discovery == 0
                || now - last_discovery >= DAY_MS
                || misses.values().any(|&m| m >= MAX_MISSES)
                || (pairs.is_empty() && now - last_discovery >= HOUR_MS);
            if want {
                last_discovery = now;
                let roll = app.edges.lock().unwrap().clone();
                let roll = match roll {
                    Some(r) => r,
                    None => app.run_roll_call().await,
                };
                pairs = rediscover(&app, &roll, now).await.into_iter().filter(|p| p.retired_at.is_none()).collect();
                misses.clear();
            }
            let jobs = pairs.iter().enumerate().map(|(i, p)| {
                let p = p.clone();
                tokio::task::spawn_blocking(move || probe(&p, PROBE_ID + 2 * i as u16, now))
            });
            let samples: Vec<Sample> = futures_util::future::join_all(jobs).await.into_iter().filter_map(Result::ok).collect();
            for s in &samples {
                let m = misses.entry(s.pair).or_default();
                *m = if s.far_ms.is_none() { *m + 1 } else { 0 };
            }
            app.store.border_record(&samples);
            app.store.border_prune(now - cfg.retention_days as i64 * DAY_MS);
        }
    });
}

// ---------------------------------------------------------------------------------------------
// What the record says
// ---------------------------------------------------------------------------------------------

/// One pair's samples over one hour: medians, and how many samples there were.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Point {
    /// Start of the hour.
    pub ts: i64,
    pub near_ms: Option<f64>,
    pub far_ms: Option<f64>,
    /// Median of far minus near over the samples where both answered.
    pub delta_ms: Option<f64>,
    pub n: u32,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// Fewer than a day of hours with enough samples.
    Collecting {
        hours: u32,
    },
    Steady,
    /// Only the far side slows down, in these hours of the day (UTC).
    Interconnect {
        adds_ms: f64,
        hours: Vec<u32>,
    },
    /// Both sides slow down together.
    OwnLink {
        adds_ms: f64,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Last {
    pub ts: i64,
    pub near_ms: Option<f64>,
    pub far_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PairHistory {
    pub pair: Pair,
    pub points: Vec<Point>,
    pub verdict: Verdict,
    pub last: Option<Last>,
}

/// Your network, as the last roll call named it.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Home {
    pub asn: Option<u32>,
    pub holder: Option<String>,
}

/// A CDN target with no pair, and why.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Missing {
    pub cdn: String,
    pub target: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct History {
    pub hours: i64,
    pub since: i64,
    pub until: i64,
    pub cycles: u32,
    pub home: Home,
    pub pairs: Vec<PairHistory>,
    pub missing: Vec<Missing>,
}

/// Your network, and the CDNs of the last roll call that have no pair in `pairs`.
pub fn context(roll: Option<&RollCall>, pairs: &[Pair]) -> (Home, Vec<Missing>) {
    let Some(r) = roll else { return (Home::default(), Vec::new()) };
    let missing = r
        .edges
        .iter()
        .filter(|e| !pairs.iter().any(|p| p.retired_at.is_none() && p.cdn == e.cdn))
        .map(|e| Missing {
            cdn: e.cdn.to_string(),
            target: e.ip.map(|i| i.to_string()).unwrap_or_default(),
            reason: if e.ip.is_some() { "border not found" } else { "no address" }.to_string(),
        })
        .collect();
    (Home { asn: r.you.asn, holder: r.you.holder.clone() }, missing)
}

/// Builds what the page shows from the recorded rows in `since..until`.
pub fn history(pairs: &[Pair], samples: &[Sample], home: Home, missing: Vec<Missing>, since: i64, until: i64) -> History {
    let cycles = samples.iter().map(|s| s.ts).collect::<std::collections::BTreeSet<_>>().len() as u32;
    let pairs = pairs
        .iter()
        .map(|p| {
            let own: Vec<&Sample> = samples.iter().filter(|s| s.pair == p.id).collect();
            let (points, verdict) = hourly(&own);
            let last = own.iter().max_by_key(|s| s.ts).map(|s| Last { ts: s.ts, near_ms: s.near_ms, far_ms: s.far_ms });
            PairHistory { pair: p.clone(), points, verdict, last }
        })
        .collect();
    History { hours: (until - since) / HOUR_MS, since, until, cycles, home, pairs, missing }
}

/// A pair's hourly points and its verdict.
fn hourly(samples: &[&Sample]) -> (Vec<Point>, Verdict) {
    let mut hours: BTreeMap<i64, Vec<&Sample>> = BTreeMap::new();
    for s in samples {
        hours.entry(s.ts.div_euclid(HOUR_MS) * HOUR_MS).or_default().push(s);
    }
    let mut points = Vec::new();
    // The hours with enough samples where both sides answered: (hour start, median near, median delta).
    let mut enough: Vec<(i64, f64, f64)> = Vec::new();
    for (ts, g) in hours {
        let deltas: Vec<f64> = g.iter().filter_map(|s| Some(s.far_ms? - s.near_ms?)).collect();
        let p = Point {
            ts,
            near_ms: median(g.iter().filter_map(|s| s.near_ms)),
            far_ms: median(g.iter().filter_map(|s| s.far_ms)),
            delta_ms: median(deltas.iter().copied()),
            n: g.len() as u32,
        };
        if let (true, Some(near), Some(delta)) = (deltas.len() >= VERDICT_SAMPLES, p.near_ms, p.delta_ms) {
            enough.push((ts, near, delta));
        }
        points.push(p);
    }
    (points, verdict(&enough))
}

/// Where the queue is, from the hours with enough samples. The common level is the 10th percentile of the hourly
/// deltas; a peak is a run of at least two hours above it by more than the noise (and 3 ms). If the near side stayed at
/// its own common level through the peak, the far side alone slowed: the link between them. If not, your own link.
fn verdict(hours: &[(i64, f64, f64)]) -> Verdict {
    if hours.len() < VERDICT_HOURS {
        return Verdict::Collecting { hours: hours.len() as u32 };
    }
    let sorted = |v: Vec<f64>| {
        let mut v = v;
        v.sort_by(f64::total_cmp);
        v
    };
    let base_delta = quantile(&sorted(hours.iter().map(|h| h.2).collect()), 0.1);
    let base_near = quantile(&sorted(hours.iter().map(|h| h.1).collect()), 0.1);
    let mid = median(hours.iter().map(|h| h.2)).unwrap_or(base_delta);
    let noise = median(hours.iter().map(|h| (h.2 - mid).abs())).unwrap_or(0.0).max(1.0);
    let limit = base_delta + (2.0 * noise).max(3.0);
    let mut peak: Vec<&(i64, f64, f64)> = Vec::new();
    let mut run: Vec<&(i64, f64, f64)> = Vec::new();
    for h in hours {
        if h.2 > limit && run.last().is_none_or(|l| h.0 - l.0 == HOUR_MS) {
            run.push(h);
            continue;
        }
        if run.len() >= 2 {
            peak.append(&mut run);
        }
        run.clear();
        if h.2 > limit {
            run.push(h);
        }
    }
    if run.len() >= 2 {
        peak.append(&mut run);
    }
    if peak.is_empty() {
        return Verdict::Steady;
    }
    let adds_ms = ((peak.iter().map(|h| h.2).fold(f64::MIN, f64::max) - base_delta) * 10.0).round() / 10.0;
    let near = median(peak.iter().map(|h| h.1)).unwrap_or(base_near);
    if near <= base_near + 2.0 {
        let mut hours: Vec<u32> = peak.iter().map(|h| h.0.div_euclid(HOUR_MS).rem_euclid(24) as u32).collect();
        hours.sort_unstable();
        hours.dedup();
        Verdict::Interconnect { adds_ms, hours }
    } else {
        Verdict::OwnLink { adds_ms }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::Hop;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn hop(ttl: u8, addr: Option<&str>, asn: Option<u32>, holder: &str) -> Hop {
        let addrs: Vec<IpAddr> = addr.map(ip).into_iter().collect();
        Hop {
            ttl,
            private: addrs.first().is_some_and(crate::catalog::is_private),
            addrs,
            asn,
            holder: asn.map(|_| holder.to_string()),
            samples_ms: vec![],
            best_ms: None,
            avg_ms: None,
            last_ms: None,
            sent: 4,
            recv: 4,
        }
    }

    fn trace(hops: Vec<Hop>) -> Trace {
        Trace { target: ip("104.18.0.1"), protocol: "icmp", rounds: 4, reached: true, elapsed_ms: 0.0, hops }
    }

    #[test]
    fn config_is_held_within_bounds() {
        let c = BorderMonitorConfig { enabled: true, interval_min: 1, retention_days: 9999 }.clamped();
        assert_eq!((c.interval_min, c.retention_days), (2, 365));
        let c = BorderMonitorConfig { enabled: true, interval_min: 500, retention_days: 0 }.clamped();
        assert_eq!((c.interval_min, c.retention_days), (60, 1));
        let d = BorderMonitorConfig::default();
        assert_eq!((d.enabled, d.interval_min, d.retention_days), (false, 5, 30));
    }

    #[test]
    fn the_border_is_the_last_home_hop_and_the_next_one() {
        let t = trace(vec![
            hop(1, Some("192.168.1.1"), None, ""),
            hop(2, Some("198.51.100.1"), Some(64500), "HOME"),
            hop(3, Some("198.51.100.9"), Some(64500), "HOME"),
            hop(4, Some("203.0.113.5"), Some(64501), "TRANSIT"),
            hop(5, Some("104.18.0.1"), Some(13335), "CLOUDFLARENET"),
        ]);
        let p = pair_from(&t, "cloudflare", Some(13335), Some(64500), 7).unwrap();
        assert_eq!((p.near, p.near_ttl, p.far, p.far_ttl), (ip("198.51.100.9"), 3, ip("203.0.113.5"), 4));
        assert_eq!((p.kind, p.far_asn, p.far_holder.as_deref(), p.found_at), (Kind::Via, Some(64501), Some("TRANSIT"), 7));
        // The target's own network right after the home network is reached directly.
        let t = trace(vec![hop(1, Some("198.51.100.1"), Some(64500), "HOME"), hop(2, Some("104.18.0.1"), Some(13335), "CF")]);
        assert_eq!(pair_from(&t, "cloudflare", Some(13335), Some(64500), 7).unwrap().kind, Kind::Direct);
    }

    #[test]
    fn no_home_hop_means_no_pair() {
        let t = trace(vec![hop(1, Some("192.168.1.1"), None, ""), hop(2, Some("203.0.113.5"), Some(64501), "TRANSIT")]);
        assert_eq!(pair_from(&t, "cloudflare", None, Some(64500), 7), None);
        // Nothing beyond the home network either.
        let t = trace(vec![hop(1, Some("198.51.100.1"), Some(64500), "HOME")]);
        assert_eq!(pair_from(&t, "cloudflare", None, Some(64500), 7), None);
    }

    #[test]
    fn a_silent_far_hop_is_skipped_to_the_next_answer() {
        let t = trace(vec![
            hop(1, Some("198.51.100.1"), Some(64500), "HOME"),
            hop(2, None, None, ""),
            hop(3, Some("203.0.113.5"), Some(64501), "TRANSIT"),
        ]);
        let p = pair_from(&t, "fastly", None, Some(64500), 7).unwrap();
        assert_eq!((p.near_ttl, p.far_ttl, p.far), (1, 3, ip("203.0.113.5")));
    }

    #[test]
    fn the_home_network_falls_back_to_the_first_public_hop() {
        let t = trace(vec![
            hop(1, Some("192.168.1.1"), None, ""),
            hop(2, Some("198.51.100.1"), Some(64500), "HOME"),
            hop(3, Some("203.0.113.5"), Some(64501), "TRANSIT"),
        ]);
        assert_eq!(pair_from(&t, "fastly", None, None, 7).unwrap().near_ttl, 2);
    }

    fn pair(id: i64) -> Pair {
        Pair {
            id,
            cdn: "cloudflare".into(),
            target: ip("104.18.0.1"),
            near: ip("198.51.100.9"),
            near_ttl: 3,
            far: ip("203.0.113.5"),
            far_ttl: 4,
            far_asn: Some(64501),
            far_holder: None,
            kind: Kind::Via,
            found_at: 0,
            retired_at: None,
        }
    }

    /// `hours` hours from a UTC midnight, 12 samples each; `f(hour, hour of day)` gives the (near, far) of the hour.
    fn series(hours: i64, f: impl Fn(i64) -> (f64, f64)) -> Vec<Sample> {
        let base = 100 * DAY_MS;
        (0..hours)
            .flat_map(|h| {
                let (near, far) = f(h);
                (0..12).map(move |k| Sample { ts: base + h * HOUR_MS + k * 300_000, pair: 1, near_ms: Some(near), far_ms: Some(far) })
            })
            .collect()
    }

    fn verdict_of(samples: &[Sample]) -> Verdict {
        let h = history(&[pair(1)], samples, Home::default(), vec![], 100 * DAY_MS, 103 * DAY_MS);
        h.pairs[0].verdict.clone()
    }

    #[test]
    fn flat_is_steady() {
        let v = verdict_of(&series(48, |h| (5.0 + (h % 3) as f64 * 0.2, 20.0 + (h % 5) as f64 * 0.3)));
        assert_eq!(v, Verdict::Steady);
    }

    #[test]
    fn the_far_side_rising_alone_at_the_same_hours_is_the_interconnect() {
        let evening = |h: i64| (19..=21).contains(&(h % 24));
        let v = verdict_of(&series(48, |h| (5.0, if evening(h) { 70.0 } else { 20.0 })));
        let Verdict::Interconnect { adds_ms, hours } = v else { panic!("{v:?}") };
        assert!((adds_ms - 50.0).abs() < 1.0, "{adds_ms}");
        assert_eq!(hours, [19, 20, 21]);
    }

    #[test]
    fn both_sides_rising_is_your_own_link() {
        let evening = |h: i64| (19..=21).contains(&(h % 24));
        let v = verdict_of(&series(48, |h| if evening(h) { (25.0, 70.0) } else { (5.0, 20.0) }));
        let Verdict::OwnLink { adds_ms } = v else { panic!("{v:?}") };
        assert!((adds_ms - 30.0).abs() < 1.0, "{adds_ms}");
    }

    #[test]
    fn a_single_slow_hour_is_not_a_peak() {
        let v = verdict_of(&series(48, |h| (5.0, if h == 20 { 70.0 } else { 20.0 })));
        assert_eq!(v, Verdict::Steady);
    }

    #[test]
    fn under_a_day_is_still_collecting() {
        assert_eq!(verdict_of(&series(20, |_| (5.0, 20.0))), Verdict::Collecting { hours: 20 });
        // Hours with a single sample don't count.
        let mut thin = series(30, |_| (5.0, 20.0));
        thin.retain(|s| s.ts.div_euclid(HOUR_MS) % 2 == 0 || s.ts % HOUR_MS < 300_000);
        assert_eq!(verdict_of(&thin), Verdict::Collecting { hours: 15 });
    }

    #[test]
    fn points_and_last_follow_the_samples() {
        let mut s = series(2, |_| (5.0, 20.0));
        s[0].far_ms = None;
        let h = history(&[pair(1)], &s, Home::default(), vec![], 100 * DAY_MS, 101 * DAY_MS);
        let p = &h.pairs[0].points[0];
        assert_eq!((p.n, p.near_ms, p.far_ms, p.delta_ms), (12, Some(5.0), Some(20.0), Some(15.0)));
        assert_eq!(h.cycles, 24);
        assert_eq!(h.hours, 24);
        let last = h.pairs[0].last.as_ref().unwrap();
        assert_eq!((last.ts, last.near_ms, last.far_ms), (s[23].ts, Some(5.0), Some(20.0)));
    }

    /// Against the real network: `cargo test -p recursa-engine live_border --release -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_border() {
        crate::install_crypto_provider();
        let db = std::env::temp_dir().join(format!("recursa-border-{:x}.db", fastrand::u64(..)));
        let app = AppState::open(Some(db), false, crate::caps::Caps::BROWSER).unwrap();
        let roll = app.run_roll_call().await;
        let res = Resolve::new(app.system.read().unwrap().clone());
        for (i, e) in roll.edges.iter().enumerate() {
            let Some(ip) = e.ip else { continue };
            let (cdn, asn, home, res) = (e.cdn, e.asn, roll.you.asn, res.clone());
            let found = tokio::task::spawn_blocking(move || discover(ip, cdn, asn, home, DISCOVER_ID + i as u16, &res)).await.unwrap();
            println!("{cdn:<10} {found:?}");
            if let Some(p) = found {
                let s = tokio::task::spawn_blocking(move || probe(&p, PROBE_ID, 0)).await.unwrap();
                println!("           {s:?}");
            }
        }
    }

    #[test]
    fn verdicts_serialize_with_a_kind_tag() {
        let j = |v: &Verdict| serde_json::to_string(v).unwrap();
        assert_eq!(j(&Verdict::Steady), r#"{"kind":"steady"}"#);
        assert_eq!(j(&Verdict::Collecting { hours: 3 }), r#"{"kind":"collecting","hours":3}"#);
        assert_eq!(j(&Verdict::OwnLink { adds_ms: 1.5 }), r#"{"kind":"own_link","adds_ms":1.5}"#);
    }
}
