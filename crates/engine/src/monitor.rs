//! Background monitoring: a light probe of selected resolvers every N minutes while
//! Recursa is running. It pauses during manual runs and builds a local reliability
//! record from this network.

use crate::app::AppState;
use crate::attribution::{Cause, Egress, GapWatch, Reading, Verdict, attribute, operators};
use crate::bench::{self, TestKind};
use crate::catalog::{self, Resolver};
use crate::clock;
use crate::dns::{self, QuerySpec};
use crate::endpoints;
use crate::lock;
use crate::net::Exchanger;
use crate::power;
use crate::store::MonitorRow;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::task::JoinHandle;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MonitorConfig {
    pub enabled: bool,
    pub interval_min: u32,
    pub resolvers: Vec<String>,
    pub retention_days: u32,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        MonitorConfig {
            enabled: false,
            interval_min: 5,
            resolvers: vec!["system-0".into(), "cloudflare".into(), "google".into(), "quad9".into()],
            retention_days: 30,
        }
    }
}

impl MonitorConfig {
    /// Within sane bounds, as [`crate::ntp_monitor::NtpMonitorConfig::clamped`]: a resolver at most once, at most 24 of them.
    pub fn clamped(mut self) -> Self {
        self.interval_min = self.interval_min.clamp(1, 1440);
        self.retention_days = self.retention_days.clamp(1, 365);
        let mut seen = std::collections::HashSet::new();
        self.resolvers.retain(|id| seen.insert(id.clone()));
        self.resolvers.truncate(24);
        self
    }
}

/// The resolvers this configuration monitors. A custom entry folded into another row is monitored as that row.
fn monitored(app: &AppState, cfg: &MonitorConfig) -> Vec<Resolver> {
    let wanted = |r: &Resolver| cfg.resolvers.contains(&r.id) || r.custom_id.iter().chain(&r.aliases).any(|c| cfg.resolvers.contains(c));
    app.resolvers().into_iter().filter(wanted).collect()
}

/// How many independent operators the monitored resolvers make, for the history. While this process has looked up
/// no egress (before its first lookups, or when another process does the monitoring), every operator counts.
pub(crate) fn independent(app: &AppState, cfg: &MonitorConfig) -> usize {
    let resolvers = monitored(app, cfg);
    let egress = app.monitor_egress.read().unwrap();
    let ops = operators(&resolvers, crate::app::builtin(), &egress);
    if app.monitor_egress_asked.load(Ordering::Relaxed) { ops.independent_count() } else { ops.count() }
}

pub fn spawn(app: Arc<AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_cycle = 0i64;
        // Held while this process records monitoring for its database.
        let mut owner: Option<lock::Guard> = None;
        let mut seen = None;
        let mut watch = GapWatch::default();
        // When every monitored resolver's egress was last looked up.
        let mut egress_at = 0i64;
        let mut egress_tried = 0i64;
        let mut egress_missing = false;
        let mut egress_ids: Vec<String> = Vec::new();
        let rt = tokio::runtime::Handle::current();
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            // Another process may have switched it on or off: re-read once something has written to the database.
            if app.store.changed_since(&mut seen) {
                app.reload_monitors();
            }
            let cfg = app.monitor.borrow().clone();
            if !cfg.enabled {
                // Let another Recursa process monitor this database instead; switched on again, start afresh.
                owner = None;
                last_cycle = 0;
                watch = GapWatch::default();
                // Switched on again, every egress is looked up afresh before the first cycle.
                (egress_at, egress_tried) = (0, 0);
                app.monitor_egress_asked.store(false, Ordering::Relaxed);
                continue;
            }
            // Fed on every tick, including those skipped below, so a paused stretch isn't read as a stall.
            let ready = watch.tick(reading());
            let now = bench::now_ms();
            // Due time is derived from the current interval, so shortening it applies at once.
            let due = last_cycle + cfg.interval_min.clamp(1, 1440) as i64 * 60_000;
            // A database download or route-origin sync of ours would skew the probes: wait it out.
            if app.run_active.load(Ordering::SeqCst) || app.downloading() || now < due || !ready {
                continue;
            }
            if owner.is_none() {
                owner = lock::try_lock(&lock::monitor_path(&app.store.path));
                if owner.is_none() {
                    continue;
                }
            }
            // Never measure alongside a run or a cycle, in this process or another.
            let Some(_measuring) = lock::try_lock(&lock::measure_path()) else { continue };
            let resolvers = monitored(&app, &cfg);
            // While port 53 is intercepted, plain DNS would measure the interceptor.
            let avoid_udp = app.interception.read().unwrap()["verdict"] == "intercepted";
            let every = cfg.interval_min.clamp(1, 1440) as i64 * 60_000;
            // Where each resolver's queries come from, looked up under the measure lock just before the cycle.
            let all = now - egress_at >= DAY_MS;
            let after_gap = watch.in_gap();
            let ids: Vec<String> = resolvers.iter().map(|r| r.id.clone()).collect();
            // Not every cycle: a resolver that gave no answer is asked again after five minutes, the rest after an hour.
            let retry = if egress_missing { EGRESS_RETRY_MS } else { HOUR_MS };
            if (all && !egress_missing) || after_gap || ids != egress_ids || now - egress_tried >= retry {
                let (a, rs, h) = (app.clone(), resolvers.clone(), rt.clone());
                let (asked, found) =
                    tokio::task::spawn_blocking(move || refresh_egress(&a, &rs, avoid_udp, all, after_gap, &h)).await.unwrap_or((1, 0));
                // A round that reached nothing (the network still down) isn't the day's round.
                if all && (found > 0 || asked == 0) {
                    egress_at = now;
                }
                egress_missing = found < asked;
                egress_tried = now;
                egress_ids = ids;
                // The lookups can be slow: being late after them is no stall, but a sleep during them still counts.
                if !watch.rebase(reading()) {
                    continue;
                }
            }
            let rs = resolvers.clone();
            let ran = tokio::task::spawn_blocking(move || {
                let rows = cycle(&rs, avoid_udp, every);
                (rows, reading())
            })
            .await;
            let Ok((mut rows, after)) = ran else {
                last_cycle = now;
                watch.rebase(reading());
                continue;
            };
            let answered = rows.iter().any(|r| r.ok);
            let Verdict::Keep(gap) = watch.end_cycle(after, answered) else { continue };
            last_cycle = now;
            label_causes(&mut rows, &resolvers, &app.monitor_egress.read().unwrap());
            app.store.monitor_record(&rows);
            if let Some(gap) = gap {
                let ended = rows.first().map_or_else(bench::now_ms, |r| r.ts);
                let started = bench::now_ms() - (after.total.saturating_sub(gap.since) / 1_000_000) as i64;
                app.store.monitor_gap(started.min(ended), ended, gap.cause);
            }
            let (raw, summaries) = prune_before(now, cfg.retention_days);
            app.store.monitor_prune(raw, summaries);
            watch.rebase(reading());
        }
    })
}

/// The clocks and power stamps now.
fn reading() -> Reading {
    let (awake, total) = clock::awake_total_ns();
    let p = power::watch();
    Reading { awake, total, slept_at: p.slept_at.load(Ordering::Relaxed), woke_at: p.woke_at.load(Ordering::Relaxed) }
}

/// Sets the cause on every sample of each resolver that didn't answer in this cycle.
fn label_causes(rows: &mut [MonitorRow], resolvers: &[Resolver], egress: &HashMap<String, Egress>) {
    let ops = operators(resolvers, crate::app::builtin(), egress);
    let cycle: Vec<(usize, bool, bool)> = resolvers
        .iter()
        .enumerate()
        .map(|(i, r)| (ops.op[i], ops.independent[i], rows.iter().any(|w| w.resolver_id == r.id && w.ok)))
        .collect();
    for (r, cause) in resolvers.iter().zip(attribute(&cycle)) {
        if cause.is_some() {
            rows.iter_mut().filter(|w| w.resolver_id == r.id).for_each(|w| w.cause = cause);
        }
    }
}

/// One `whoami.akamai.net` query through the resolver: the address it reaches authoritative servers from.
fn egress_of(r: &Resolver, avoid_udp: bool) -> Option<IpAddr> {
    let timeout = Duration::from_millis(2000);
    let (mut x, _) = endpoints::open_best(&r.only_server(0), &[], timeout, avoid_udp)?;
    let m = dns::ask_msg(x.as_mut(), &QuerySpec::new("whoami.akamai.net", dns::TYPE_A), timeout).ok()?;
    m.ipv4_answers().first().map(|a| IpAddr::V4(*a))
}

/// Looks up egress for resolvers without an entry or without a known network, or all of them (`all`). After a gap the
/// private-address resolvers' entries are forgotten first, as a network change moves them. A failed lookup keeps the previous
/// value; entries for resolvers no longer monitored are dropped. Returns how many resolvers were asked and how many gave an
/// address.
fn refresh_egress(
    app: &AppState,
    resolvers: &[Resolver],
    avoid_udp: bool,
    all: bool,
    after_gap: bool,
    rt: &tokio::runtime::Handle,
) -> (usize, usize) {
    app.monitor_egress_asked.store(true, Ordering::Relaxed);
    {
        let mut map = app.monitor_egress.write().unwrap();
        map.retain(|id, _| resolvers.iter().any(|r| &r.id == id));
        if after_gap {
            map.retain(|id, _| {
                resolvers.iter().find(|r| &r.id == id).and_then(Resolver::any_ip).is_none_or(|ip| !catalog::is_private(&ip))
            });
        }
    }
    let todo: Vec<&Resolver> = {
        let map = app.monitor_egress.read().unwrap();
        resolvers.iter().filter(|r| all || map.get(&r.id).is_none_or(|e| e.asn.is_none())).collect()
    };
    let found: Vec<Option<IpAddr>> = std::thread::scope(|sc| {
        let hs: Vec<_> = todo.iter().map(|r| sc.spawn(move || egress_of(r, avoid_udp))).collect();
        hs.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    let counts = (todo.len(), found.iter().flatten().count());
    for (r, ip) in todo.iter().zip(found) {
        let Some(addr) = ip else { continue };
        let asn = rt.block_on(app.intel.asn_of(addr))["asn"].as_u64().and_then(|a| u32::try_from(a).ok());
        let mut map = app.monitor_egress.write().unwrap();
        // A failed network lookup doesn't unlearn the network of an unchanged address.
        let asn = asn.or_else(|| map.get(&r.id).filter(|e| e.addr == addr).and_then(|e| e.asn));
        map.insert(r.id.clone(), Egress { addr, asn });
    }
    counts
}

/// One cached, one uncached and one TLD query per resolver address: the primary and the
/// secondary when a resolver lists both. Both servers' samples are recorded under the
/// resolver's one id, so each line on the Reliability page weighs the two together.
pub fn cycle(resolvers: &[Resolver], avoid_udp: bool, every: i64) -> Vec<MonitorRow> {
    let ts = bench::now_ms();
    let timeout = Duration::from_millis(2000);
    let round = (ts / 60_000) as u32;
    std::thread::scope(|sc| {
        let hs: Vec<_> = resolvers.iter().map(|r| sc.spawn(move || probe_one(r, ts, round, timeout, avoid_udp, every))).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

fn probe_one(r: &Resolver, ts: i64, round: u32, timeout: Duration, avoid_udp: bool, every: i64) -> Vec<MonitorRow> {
    let mut rows = Vec::new();
    for i in 0..r.plain_servers().len().max(1) {
        let opened = endpoints::open_best(&r.only_server(i), &[], timeout, avoid_udp);
        // Over DoT or DoH a resolver has one endpoint however many addresses it lists, so a
        // second probe would only repeat the first.
        let plain = opened.as_ref().is_some_and(|(_, how)| matches!(*how, "UDP" | "TCP"));
        let via = opened.as_ref().map(|(_, how)| *how);
        rows.extend(probe_server(r, opened.map(|(x, _)| x), via, ts, round, timeout, every));
        if !plain {
            break;
        }
    }
    rows
}

fn probe_server(
    r: &Resolver,
    mut ex: Option<Box<dyn Exchanger>>,
    via: Option<&str>,
    ts: i64,
    round: u32,
    timeout: Duration,
    every: i64,
) -> Vec<MonitorRow> {
    [TestKind::Cached, TestKind::Uncached, TestKind::Tld]
        .into_iter()
        .map(|test| {
            let result = ex.as_mut().and_then(|x| {
                let q = dns::encode_query(dns::new_id(), &QuerySpec::new(bench::query_name(test, round), dns::TYPE_A)).ok()?;
                Some(bench::classify(test, &x.exchange(&q, timeout)))
            });
            // A resolver that answers with a filter's block page or from its cache is up; only silence and errors are outages.
            let ok = matches!(result, Some((bench::Outcome::Ok | bench::Outcome::Blocked | bench::Outcome::FromCache, _, _)));
            let kind = result.map_or_else(
                || "noconn".to_owned(),
                |(outcome, _, _)| serde_json::to_value(outcome).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
            );
            MonitorRow {
                ts,
                resolver_id: r.id.clone(),
                test: test.key().into(),
                us: result.filter(|_| ok).map(|(_, us, _)| us),
                ok,
                kind: Some(kind),
                via: via.map(str::to_owned),
                cause: None,
                every: Some(every),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// History. Raw samples are kept only briefly; the record lives in hourly rollups (one row
// per resolver and hour, with that hour's answered latencies for exact percentiles) and an
// outage table kept up to date cycle by cycle. A month at 24 resolvers every minute is then
// 17,280 rollup rows instead of 3.1M samples.
// ---------------------------------------------------------------------------------------

pub const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;
const EGRESS_RETRY_MS: i64 = 5 * 60_000;
/// Raw samples are kept this long, which is plenty to rebuild the hour in progress; the
/// rollups and outages keep the user's chosen retention.
const RAW_DAYS: i64 = 7;
/// The availability strip shows at most this many cells: longer windows use cells of
/// several hours (30 days → 4-hour cells), since an hour would be about a pixel wide.
const MAX_CELLS: i64 = 180;

/// The window for `hours` ending now, aligned to whole cells so the totals cover exactly
/// the cells on screen: (start, cell length in ms).
pub fn window(hours: i64, now: i64) -> (i64, i64) {
    let cell_hours = ((hours + MAX_CELLS - 1) / MAX_CELLS).max(1);
    let cell = cell_hours * HOUR_MS;
    let cells = (hours + cell_hours - 1) / cell_hours;
    (now - now.rem_euclid(cell) - (cells - 1) * cell, cell)
}

/// Pruning cutoffs for a retention in days: (raw samples, rollups and outages).
fn prune_before(now: i64, retention_days: u32) -> (i64, i64) {
    let days = retention_days.clamp(1, 365) as i64;
    (now - days.min(RAW_DAYS) * DAY_MS, now - days * DAY_MS)
}

/// One resolver's samples in one hour, as stored in `monitor_hour`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HourRollup {
    pub resolver_id: String,
    pub hour: i64,
    pub n: u32,
    pub ok: u32,
    pub cycles: u32,
    /// Cycles with at least one answer.
    pub up: u32,
    pub first: i64,
    pub last: i64,
    /// Answered latencies in µs, sorted.
    pub latencies: Vec<u32>,
    /// Unanswered cycles that hit several operators at once, and the samples in them.
    pub shared: u32,
    pub shared_n: u32,
    /// Unanswered cycles where some operators answered and several did not.
    pub unclear: u32,
    /// The time the cycles measured (ms): each one's wait since the cycle before it in the hour, or its interval when
    /// it is the hour's first or the wait was far longer; 0 when any cycle's interval is unknown (recorded before it was kept).
    pub covered: i64,
}

impl HourRollup {
    /// The latencies as stored: little-endian u32s.
    pub fn latency_blob(&self) -> Vec<u8> {
        self.latencies.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}

/// A rollup row as read back, borrowing the row's text and blob.
pub struct RollupRow<'a> {
    pub resolver_id: &'a str,
    pub hour: i64,
    pub n: u32,
    pub ok: u32,
    pub cycles: u32,
    pub up: u32,
    pub first: i64,
    pub last: i64,
    pub latencies: &'a [u8],
    pub shared: u32,
    pub shared_n: u32,
    pub unclear: u32,
    pub covered: i64,
}

/// One resolver's cycle: its samples folded together.
#[derive(Clone, Copy, Debug)]
pub struct CycleEnd {
    pub ts: i64,
    /// Any sample answered.
    pub answered: bool,
    pub cause: Option<Cause>,
    /// The interval in force (ms), when it was recorded.
    pub every: Option<i64>,
}

/// Folds raw samples, in time order, into hourly rollups. Each rollup goes to `emit` once its
/// resolver moves on to a later hour (the rest at `finish`), and each cycle to `on_cycle`
/// (resolver, cycle) in time order per resolver, which is what outages are made of.
pub struct RollupBuilder<E: FnMut(HourRollup), C: FnMut(&str, CycleEnd)> {
    open: Vec<OpenHour>,
    last: usize,
    emit: E,
    on_cycle: C,
}

struct OpenHour {
    r: HourRollup,
    /// The cycle being read, and its samples so far.
    cycle: Option<(CycleEnd, u32)>,
    /// A cycle of this hour has no recorded interval.
    unknown: bool,
    /// When the hour's previous cycle ran.
    prev: Option<i64>,
}

impl<E: FnMut(HourRollup), C: FnMut(&str, CycleEnd)> RollupBuilder<E, C> {
    pub fn new(emit: E, on_cycle: C) -> Self {
        RollupBuilder { open: Vec::new(), last: 0, emit, on_cycle }
    }

    pub fn push(&mut self, ts: i64, resolver_id: &str, us: Option<u32>, ok: bool, cause: Option<Cause>, every: Option<i64>) {
        let hour = ts - ts.rem_euclid(HOUR_MS);
        let RollupBuilder { open, last, emit, on_cycle } = self;
        // A handful of resolvers: a list with the previous sample's resolver first beats hashing.
        if open.get(*last).is_none_or(|o| o.r.resolver_id != resolver_id) {
            *last = match open.iter().position(|o| o.r.resolver_id == resolver_id) {
                Some(i) => i,
                None => {
                    let r = HourRollup { resolver_id: resolver_id.to_owned(), hour, first: ts, ..Default::default() };
                    open.push(OpenHour { r, cycle: None, unknown: false, prev: None });
                    open.len() - 1
                }
            };
        }
        let o = &mut open[*last];
        if o.cycle.is_some_and(|(c, _)| c.ts != ts) {
            close_cycle(o, on_cycle);
        }
        if o.r.hour != hour {
            let fresh = HourRollup { resolver_id: o.r.resolver_id.clone(), hour, first: ts, ..Default::default() };
            let mut done = std::mem::replace(&mut o.r, fresh);
            o.prev = None;
            done.latencies.sort_unstable();
            if std::mem::take(&mut o.unknown) {
                done.covered = 0;
            }
            emit(done);
        }
        let (cycle, n) = o.cycle.get_or_insert((CycleEnd { ts, answered: false, cause: None, every: None }, 0));
        cycle.answered |= ok;
        cycle.cause = cycle.cause.or(cause);
        cycle.every = cycle.every.or(every);
        *n += 1;
        o.r.n += 1;
        o.r.last = ts;
        if ok {
            o.r.ok += 1;
            if let Some(us) = us {
                o.r.latencies.push(us);
            }
        }
    }

    pub fn finish(mut self) {
        for mut o in std::mem::take(&mut self.open) {
            close_cycle(&mut o, &mut self.on_cycle);
            o.r.latencies.sort_unstable();
            if o.unknown {
                o.r.covered = 0;
            }
            (self.emit)(o.r);
        }
    }
}

fn close_cycle(o: &mut OpenHour, on_cycle: &mut impl FnMut(&str, CycleEnd)) {
    if let Some((c, n)) = o.cycle.take() {
        o.r.cycles += 1;
        o.r.up += c.answered as u32;
        if !c.answered {
            match c.cause {
                Some(Cause::Shared) => {
                    o.r.shared += 1;
                    o.r.shared_n += n;
                }
                Some(Cause::Unclear) => o.r.unclear += 1,
                _ => {}
            }
        }
        match c.every {
            Some(ms) => o.r.covered += measured(o.prev, c.ts, ms),
            None => o.unknown = true,
        }
        o.prev = Some(c.ts);
        on_cycle(&o.r.resolver_id, c);
    }
}

/// The time a cycle at `ts` measured: its wait since the hour's previous cycle, which runs a little over the interval
/// as each pass takes time; the interval itself when the cycle is the hour's first or the wait was far longer.
fn measured(prev: Option<i64>, ts: i64, every: i64) -> i64 {
    prev.map(|p| ts - p).filter(|wait| *wait <= every + (every / 2).max(30_000)).unwrap_or(every)
}

/// A pause shorter than this never cuts an open outage, whatever the interval.
const CUT_AFTER_MS: i64 = 10 * 60_000;

/// One resolver's run of consecutive unanswered cycles, fed one cycle at a time.
#[derive(Clone, Debug, Default)]
pub struct OutageTracker {
    /// The outage while it is still failing (`recovered` none, `cut` false).
    pub open: Option<Outage>,
    /// The interval of the last failed cycle, when known.
    pub every: Option<i64>,
}

impl OutageTracker {
    /// The next cycle; returns the outage it ends, if it ends one. A cycle that comes more than twice its interval
    /// (or the last failed one's, when that was longer) and more than ten minutes after the last failed one (a sleep,
    /// a quit, a long pause) cuts the open outage: it ends there, not recovered.
    pub fn cycle(&mut self, c: CycleEnd) -> Option<Outage> {
        let mut ended = None;
        if let (Some(o), Some(every)) = (&self.open, c.every)
            && c.ts - o.end > (2 * every.max(self.every.unwrap_or(0))).max(CUT_AFTER_MS)
        {
            ended = self.open.take().map(|o| Outage { cut: true, ..o });
        }
        if c.answered {
            return self.open.take().map(|o| Outage { recovered: Some(c.ts), ..o }).or(ended);
        }
        let o = self.open.get_or_insert(Outage { start: c.ts, end: c.ts, cycles: 0, recovered: None, shared: 0, unclear: 0, cut: false });
        o.end = c.ts;
        o.cycles += 1;
        self.every = c.every;
        match c.cause {
            Some(Cause::Shared) => o.shared += 1,
            Some(Cause::Unclear) => o.unclear += 1,
            _ => {}
        }
        ended
    }

    pub fn current(&self) -> Option<Outage> {
        self.open.clone()
    }
}

/// Uptime, latency and outages per resolver over a window, as the Reliability view shows them.
#[derive(Debug, Serialize)]
pub struct History {
    pub hours: i64,
    pub since: i64,
    /// Length of one strip cell (ms).
    pub cell: i64,
    pub resolvers: Vec<ResolverHistory>,
    /// Stretches in the window with no counted cycle: [started, ended, cause], cause `sleep` or `stall`.
    pub gaps: Vec<(i64, i64, String)>,
    /// How many independent operators are monitored now; below two a failure can't be told apart from a shared one.
    pub independent: usize,
}

#[derive(Debug, Serialize)]
pub struct ResolverHistory {
    pub resolver_id: String,
    pub cycles: u32,
    /// Share of cycles with at least one answer.
    pub uptime: Option<f64>,
    pub query_success: Option<f64>,
    /// Cycles that went unanswered across several operators at once, and those where some operators answered.
    pub shared: u32,
    pub unclear: u32,
    /// Measured time over the window, 0 to 1; none when an hour of it was recorded before intervals were kept.
    pub coverage: Option<f64>,
    /// Answered over the samples outside shared cycles.
    pub query_success_own: Option<f64>,
    /// [low, high]: the share of cycles outside shared ones that were answered, with the unclear ones counted
    /// against the resolver (low) or not (high).
    pub uptime_range: Option<[f64; 2]>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub first: Option<i64>,
    pub last: Option<i64>,
    /// One per cell with samples, oldest first.
    pub buckets: Vec<Cell>,
    /// Outages with a failed cycle in the window, each whole (it may have begun earlier).
    pub outages: Vec<Outage>,
}

/// One strip cell, sent as `[start, samples, answered, p50 ms, p95 ms, histogram, shared samples, measured ms]` to
/// keep a month of cells for two dozen resolvers small. See [`histogram`]. The measured time is none when part of the
/// cell was recorded before intervals were kept.
#[derive(Debug, Serialize)]
pub struct Cell(pub i64, pub u32, pub u32, pub Option<f64>, pub Option<f64>, pub Vec<u32>, pub u32, pub Option<i64>);

/// Latency histogram bins: an eighth of a decade each on a log-ms axis, bin 0 starting at
/// 0.32 ms. The last bin, from 1.78 s, runs to the 2 s timeout.
const HIST_BINS: usize = 31;

fn bin(ms: f64) -> usize {
    ((ms.log10() * 8.0).floor() as i64 + 4).clamp(0, HIST_BINS as i64 - 1) as usize
}

/// Answered latencies (µs) counted per bin, sent as `[first bin, counts…]` with the empty
/// bins at both ends left out; empty when nothing was answered.
fn histogram(us: &[u32]) -> Vec<u32> {
    let mut c = [0u32; HIST_BINS];
    for &u in us {
        c[bin(u as f64 / 1000.0)] += 1;
    }
    let Some(lo) = c.iter().position(|&n| n > 0) else { return Vec::new() };
    let hi = c.iter().rposition(|&n| n > 0).unwrap_or(lo);
    std::iter::once(lo as u32).chain(c[lo..=hi].iter().copied()).collect()
}

/// Consecutive cycles with no answer at all.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Outage {
    pub start: i64,
    pub end: i64,
    pub cycles: u32,
    /// The next cycle that got an answer; none while it is still failing, or when it was cut.
    pub recovered: Option<i64>,
    /// Failed cycles of each cause; the rest were isolated.
    pub shared: u32,
    pub unclear: u32,
    /// Checks stopped (asleep, quit, paused) before it was seen to end.
    pub cut: bool,
}

/// Builds the [`History`] from rollup rows in hour order.
pub struct HistoryBuilder {
    cell: i64,
    by: Vec<(String, Acc)>,
    last: usize,
}

#[derive(Default)]
struct Acc {
    n: u32,
    ok: u32,
    cycles: u32,
    up: u32,
    first: Option<i64>,
    last: Option<i64>,
    latencies: Vec<u32>,
    shared: u32,
    unclear: u32,
    shared_n: u32,
    /// Measured ms, each hour capped at one hour (one interval when that is longer).
    covered: i64,
    /// An hour with cycles has no recorded intervals.
    unknown: bool,
    cell: Option<OpenCell>,
    cells: Vec<Cell>,
}

struct OpenCell {
    t: i64,
    n: u32,
    ok: u32,
    /// Where the cell's latencies begin in `latencies`.
    from: usize,
    shared_n: u32,
    covered: Option<i64>,
}

impl Acc {
    fn close_cell(&mut self) {
        let Some(c) = self.cell.take() else { return };
        // Sorted in place: the window's percentiles don't care about order.
        let lat = &mut self.latencies[c.from..];
        lat.sort_unstable();
        self.cells.push(Cell(c.t, c.n, c.ok, pct(lat, 0.5), pct(lat, 0.95), histogram(lat), c.shared_n, c.covered));
    }
}

impl HistoryBuilder {
    pub fn new(cell: i64) -> Self {
        HistoryBuilder { cell, by: Vec::new(), last: 0 }
    }

    pub fn push(&mut self, r: &RollupRow<'_>) {
        if self.by.get(self.last).is_none_or(|(id, _)| id != r.resolver_id) {
            self.last = match self.by.iter().position(|(id, _)| id == r.resolver_id) {
                Some(i) => i,
                None => {
                    self.by.push((r.resolver_id.to_owned(), Acc::default()));
                    self.by.len() - 1
                }
            };
        }
        let a = &mut self.by[self.last].1;
        let cell = r.hour - r.hour.rem_euclid(self.cell);
        if a.cell.as_ref().is_some_and(|c| c.t != cell) {
            a.close_cell();
        }
        let c = a.cell.get_or_insert(OpenCell { t: cell, n: 0, ok: 0, from: a.latencies.len(), shared_n: 0, covered: Some(0) });
        c.n += r.n;
        c.ok += r.ok;
        c.shared_n += r.shared_n;
        // An hour with cycles and no measured time was recorded before intervals were kept.
        if r.cycles > 0 && r.covered == 0 {
            c.covered = None;
            a.unknown = true;
        } else {
            // Capped at the hour, or at one interval when checks come less often than hourly.
            let hour = r.covered.min(HOUR_MS.max(r.covered / r.cycles.max(1) as i64));
            c.covered = c.covered.map(|v| v + hour);
            a.covered += hour;
        }
        a.latencies.extend(r.latencies.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)));
        a.n += r.n;
        a.ok += r.ok;
        a.cycles += r.cycles;
        a.up += r.up;
        a.shared += r.shared;
        a.unclear += r.unclear;
        a.shared_n += r.shared_n;
        a.first = Some(a.first.map_or(r.first, |f| f.min(r.first)));
        a.last = Some(a.last.map_or(r.last, |l| l.max(r.last)));
    }

    /// `until` ends the window; `outages`: (resolver, outage) for every outage with a failed cycle in the window.
    pub fn finish(
        self,
        hours: i64,
        since: i64,
        until: i64,
        outages: Vec<(String, Outage)>,
        gaps: Vec<(i64, i64, String)>,
        independent: usize,
    ) -> History {
        let mut resolvers: Vec<ResolverHistory> = self
            .by
            .into_iter()
            .map(|(resolver_id, mut a)| {
                a.close_cell();
                a.latencies.sort_unstable();
                let mut mine: Vec<Outage> = outages.iter().filter(|(id, _)| *id == resolver_id).map(|(_, o)| o.clone()).collect();
                mine.sort_by_key(|o| o.start);
                let isolated = a.cycles.saturating_sub(a.up + a.shared + a.unclear) as f64;
                let counted = (a.cycles - a.shared) as f64;
                let own = a.n - a.shared_n;
                ResolverHistory {
                    cycles: a.cycles,
                    uptime: (a.cycles > 0).then(|| a.up as f64 / a.cycles as f64),
                    query_success: (a.n > 0).then(|| a.ok as f64 / a.n as f64),
                    shared: a.shared,
                    unclear: a.unclear,
                    coverage: (!a.unknown)
                        .then(|| (a.covered as f64 / (until - since.max(a.first.unwrap_or(since))).max(1) as f64).min(1.0)),
                    query_success_own: (own > 0).then(|| a.ok as f64 / own as f64),
                    uptime_range: (counted > 0.0).then(|| [1.0 - (isolated + a.unclear as f64) / counted, 1.0 - isolated / counted]),
                    p50: pct(&a.latencies, 0.5),
                    p95: pct(&a.latencies, 0.95),
                    first: a.first,
                    last: a.last,
                    buckets: a.cells,
                    outages: mine,
                    resolver_id,
                }
            })
            .collect();
        resolvers.sort_by(|a, b| a.resolver_id.cmp(&b.resolver_id));
        History { hours, since, cell: self.cell, resolvers, gaps, independent }
    }
}

/// Nearest-rank quantile over sorted µs, in ms.
fn pct(sorted: &[u32], q: f64) -> Option<f64> {
    crate::stats::nearest_rank(sorted, q).map(|us| us as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::GapCause;
    use crate::store::Store;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    /// What the history means, computed the obvious way from raw samples: totals over the
    /// samples in the window, cells by start time, and outages found over the resolver's
    /// whole record, kept when one of their failed cycles is in the window.
    fn reference(rows: &[MonitorRow], hours: i64, since: i64, until: i64, cell: i64) -> Value {
        let pct = |v: &[f64], q: f64| (!v.is_empty()).then(|| v[((v.len() - 1) as f64 * q).round() as usize]);
        let sorted = |mut v: Vec<f64>| {
            v.sort_by(|a, b| a.total_cmp(b));
            v
        };
        let mut by: BTreeMap<&str, Vec<&MonitorRow>> = BTreeMap::new();
        for r in rows {
            by.entry(r.resolver_id.as_str()).or_default().push(r);
        }
        // Each cycle by time: (answered, cause, interval).
        let cycles_of = |rs: &[&MonitorRow]| {
            let mut m: BTreeMap<i64, (bool, Option<Cause>, Option<i64>)> = BTreeMap::new();
            for r in rs {
                let c = m.entry(r.ts).or_insert((false, r.cause, r.every));
                c.0 |= r.ok;
            }
            m
        };
        let out: Vec<Value> = by
            .into_iter()
            .filter_map(|(id, all)| {
                let rs: Vec<&&MonitorRow> = all.iter().filter(|r| r.ts >= since).collect();
                if rs.is_empty() {
                    return None;
                }
                let rs_plain: Vec<&MonitorRow> = rs.iter().map(|r| **r).collect();
                let window_cycles = cycles_of(&rs_plain);
                let shared_cycle = |ts: i64| window_cycles[&ts].1 == Some(Cause::Shared) && !window_cycles[&ts].0;
                // Measured time per hour, capped at the hour or at the mean interval if that is longer.
                let mut hour_sum: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
                let mut prev: Option<i64> = None;
                for (ts, c) in &window_cycles {
                    let hour = ts - ts.rem_euclid(HOUR_MS);
                    let wait = prev.filter(|p| *p >= hour).map(|p| ts - p);
                    let h = hour_sum.entry(hour).or_default();
                    h.0 += c.2.map_or(0, |ev| wait.filter(|w| *w <= ev + (ev / 2).max(30_000)).unwrap_or(ev));
                    h.1 += 1;
                    prev = Some(*ts);
                }
                let hour_cov: BTreeMap<i64, i64> = hour_sum.into_iter().map(|(t, (s, n))| (t, s.min(HOUR_MS.max(s / n)))).collect();
                let first = rs.iter().map(|r| r.ts).min().unwrap();
                let ms = |r: &MonitorRow| r.us.filter(|_| r.ok).map(|u| u as f64 / 1000.0);
                let mut cells: BTreeMap<i64, (u32, u32, Vec<f64>, u32)> = BTreeMap::new();
                for r in &rs {
                    let c = cells.entry(r.ts - r.ts.rem_euclid(cell)).or_default();
                    c.0 += 1;
                    c.1 += r.ok as u32;
                    c.2.extend(ms(r));
                    c.3 += shared_cycle(r.ts) as u32;
                }
                let cells: Vec<Value> = cells
                    .into_iter()
                    .map(|(t, (n, ok, v, shared_n))| {
                        let v = sorted(v);
                        let mut h = [0u32; HIST_BINS];
                        for x in &v {
                            h[((x.log10() * 8.0).floor() as i64 + 4).clamp(0, HIST_BINS as i64 - 1) as usize] += 1;
                        }
                        let hist: Vec<u32> = match (h.iter().position(|&c| c > 0), h.iter().rposition(|&c| c > 0)) {
                            (Some(lo), Some(hi)) => std::iter::once(lo as u32).chain(h[lo..=hi].iter().copied()).collect(),
                            _ => Vec::new(),
                        };
                        let covered: i64 = hour_cov.range(t..t + cell).map(|(_, v)| *v).sum();
                        json!([t, n, ok, pct(&v, 0.5), pct(&v, 0.95), hist, shared_n, covered])
                    })
                    .collect();
                let all_plain: Vec<&MonitorRow> = all.to_vec();
                let mut outages: Vec<Value> = Vec::new();
                // (start, end, cycles, shared, unclear)
                let mut open: Option<(i64, i64, u32, u32, u32)> = None;
                let outage = |(s, e, n, sh, un): (i64, i64, u32, u32, u32), recovered: Option<i64>, cut: bool| {
                    json!({ "start": s, "end": e, "cycles": n, "recovered": recovered, "shared": sh, "unclear": un, "cut": cut })
                };
                for (ts, (ok, cause, every)) in cycles_of(&all_plain) {
                    if let Some(o) = open
                        && every.is_some_and(|ev| ts - o.1 > (2 * ev).max(CUT_AFTER_MS))
                    {
                        outages.push(outage(o, None, true));
                        open = None;
                    }
                    if !ok {
                        let (sh, un) = ((cause == Some(Cause::Shared)) as u32, (cause == Some(Cause::Unclear)) as u32);
                        open = Some(open.map_or((ts, ts, 1, sh, un), |(s, _, n, a, b)| (s, ts, n + 1, a + sh, b + un)));
                    } else if let Some(o) = open.take() {
                        outages.push(outage(o, Some(ts), false));
                    }
                }
                if let Some(o) = open {
                    outages.push(outage(o, None, false));
                }
                outages.retain(|o| o["end"].as_i64() >= Some(since));
                let total = window_cycles.len();
                let up = window_cycles.values().filter(|c| c.0).count();
                let shared = window_cycles.iter().filter(|(ts, _)| shared_cycle(**ts)).count();
                let unclear = window_cycles.values().filter(|c| !c.0 && c.1 == Some(Cause::Unclear)).count();
                let isolated = (total - up - shared - unclear) as f64;
                let counted = (total - shared) as f64;
                let all_ms = sorted(rs.iter().filter_map(|r| ms(r)).collect());
                let q_ok = rs.iter().filter(|r| r.ok).count();
                let shared_n = rs.iter().filter(|r| shared_cycle(r.ts)).count();
                let covered: i64 = hour_cov.values().sum();
                Some(json!({
                    "resolver_id": id,
                    "cycles": total,
                    "uptime": (total > 0).then(|| up as f64 / total as f64),
                    "query_success": Some(q_ok as f64 / rs.len() as f64),
                    "shared": shared,
                    "unclear": unclear,
                    "coverage": Some((covered as f64 / (until - since.max(first)).max(1) as f64).min(1.0)),
                    "query_success_own": (rs.len() > shared_n).then(|| q_ok as f64 / (rs.len() - shared_n) as f64),
                    "uptime_range": (counted > 0.0).then(|| [1.0 - (isolated + unclear as f64) / counted, 1.0 - isolated / counted]),
                    "p50": pct(&all_ms, 0.5),
                    "p95": pct(&all_ms, 0.95),
                    "first": rs.iter().map(|r| r.ts).min(),
                    "last": rs.iter().map(|r| r.ts).max(),
                    "buckets": cells,
                    "outages": outages,
                }))
            })
            .collect();
        json!({ "hours": hours, "since": since, "cell": cell, "resolvers": out, "gaps": [], "independent": 0 })
    }

    const EVERY: i64 = 300_000;

    /// Three days of five-minute cycles: an outage across an hour boundary, one still open
    /// at the end, one before the shorter windows that only ends inside them, a resolver
    /// that joins late, skipped cycles, a resolver whose outage is cut by skipped cycles,
    /// failures of each cause, and resolvers interleaved within a cycle.
    fn sample_cycles() -> Vec<Vec<MonitorRow>> {
        let mut rng = fastrand::Rng::with_seed(7);
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS + 17 * 60_000;
        let total = 3 * 24 * 12;
        (0..total)
            .map(|c| {
                let ts = t0 + c * 300_000;
                let mut cycle = Vec::new();
                for id in ["cloudflare", "google", "system-0", "late", "gappy"] {
                    if id == "late" && c < 400 || id == "google" && c % 97 == 0 || id == "gappy" && (205..207).contains(&c) {
                        continue;
                    }
                    let down = (id == "cloudflare" && (100..112).contains(&c))
                        || (id == "google" && (total - 40..total - 20).contains(&c))
                        || (id == "system-0" && c >= total - 5)
                        || (id == "gappy" && (200..210).contains(&c));
                    let mut mine = Vec::new();
                    for test in ["cached", "uncached", "tld"] {
                        let ok = !down && rng.u32(..1000) > 15;
                        let us = ok.then(|| rng.u32(8_000..60_000)).filter(|_| rng.u32(..500) > 0);
                        let kind = Some(if ok { "ok" } else { "timeout" }.to_owned());
                        mine.push(MonitorRow {
                            ts,
                            resolver_id: id.into(),
                            test: test.into(),
                            us,
                            ok,
                            kind,
                            via: Some("UDP".into()),
                            cause: None,
                            every: Some(EVERY),
                        });
                    }
                    // Causes on unanswered cycles; some are left unset, as in rows from before they were kept.
                    if !mine.iter().any(|r| r.ok) {
                        let cause = match (id, c) {
                            ("cloudflare", 100..104) | ("gappy", 200..202) => Some(Cause::Shared),
                            ("cloudflare", 104..106) | ("google", _) => Some(Cause::Unclear),
                            ("system-0", _) => Some(Cause::Isolated),
                            _ => None,
                        };
                        mine.iter_mut().for_each(|r| r.cause = cause);
                    }
                    cycle.extend(mine);
                }
                rng.shuffle(&mut cycle);
                cycle
            })
            .collect()
    }

    fn from_store(store: &Store, hours: i64, now: i64) -> Value {
        let (since, cell) = window(hours, now);
        let mut b = HistoryBuilder::new(cell);
        store.monitor_rollups(since, |r| b.push(&r));
        serde_json::to_value(b.finish(hours, since, now, store.monitor_outages(since), store.monitor_gaps(since), 0)).unwrap()
    }

    fn temp_store() -> (Store, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("recursa-monitor-{:x}.db", fastrand::u64(..)));
        (Store::open(path.clone()).unwrap(), path)
    }

    fn remove(path: std::path::PathBuf) {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    #[test]
    fn histogram_bins_are_eighths_of_a_decade() {
        // 16 and 16.5 ms share the bin from 10^(9/8); 500 ms is twelve bins up.
        let mut want = vec![13, 2];
        want.extend([0; 11]);
        want.push(1);
        assert_eq!(histogram(&[16_000, 16_500, 500_000]), want);
        assert_eq!(histogram(&[0, 5_000_000]), [&[0, 1][..], &[0; 29], &[1]].concat());
        assert!(histogram(&[]).is_empty());
    }

    #[test]
    fn windows_align_to_cells() {
        let now = 1_791_000_000_000 + 1234;
        let (since, cell) = window(72, now);
        assert_eq!(cell, HOUR_MS);
        assert_eq!(since, now - now % HOUR_MS - 71 * HOUR_MS);
        let (since, cell) = window(720, now);
        assert_eq!(cell, 4 * HOUR_MS);
        assert_eq!((now - now % cell - since) / cell + 1, 180);
        assert_eq!(window(168, now).1, HOUR_MS);
    }

    #[test]
    fn history_from_rollups_matches_the_raw_samples() {
        let cycles = sample_cycles();
        let rows: Vec<MonitorRow> = cycles.iter().flatten().cloned().collect();
        let now = rows.last().unwrap().ts + 60_000;
        let (store, path) = temp_store();
        // Recorded cycle by cycle, as the monitor does: rollups and outages kept up to date.
        for c in &cycles {
            store.monitor_record(c);
        }
        for hours in [1, 3, 5, 24, 72, 720] {
            let (since, cell) = window(hours, now);
            assert_eq!(from_store(&store, hours, now), reference(&rows, hours, since, now, cell), "{hours} h");
        }
        // Raw samples can go: the record lives on in the rollups and outages.
        store.monitor_prune(now, 0);
        assert_eq!(from_store(&store, 72, now), {
            let (since, cell) = window(72, now);
            reference(&rows, 72, since, now, cell)
        });
        let got = from_store(&store, 72, now);
        let r = |id: &str| got["resolvers"].as_array().unwrap().iter().find(|r| r["resolver_id"] == id).unwrap().clone();
        assert_eq!(r("cloudflare")["outages"][0]["cycles"], 12);
        assert!(r("system-0")["outages"].as_array().unwrap().last().unwrap()["recovered"].is_null());
        assert!(r("late")["first"].as_i64() > r("google")["first"].as_i64());
        // Google's outage begins before the 3-hour window and ends inside it: reported whole.
        let (since, _) = window(3, now);
        let short = from_store(&store, 3, now);
        let g = short["resolvers"].as_array().unwrap().iter().find(|r| r["resolver_id"] == "google").unwrap().clone();
        assert!(g["outages"].as_array().unwrap().iter().any(|o| o["cycles"] == 20 && o["start"].as_i64() < Some(since)));
        drop(store);
        remove(path);
    }

    #[test]
    fn both_servers_weigh_into_one_line() {
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS;
        let (store, path) = temp_store();
        // An hour of cycles; the primary always answers and the secondary never does.
        for c in 0..12 {
            let ts = t0 + c * 300_000;
            let rows: Vec<MonitorRow> = [true, false]
                .into_iter()
                .flat_map(|up| {
                    ["cached", "uncached", "tld"].map(|test| MonitorRow {
                        ts,
                        resolver_id: "quad9".into(),
                        test: test.into(),
                        us: up.then_some(9_000),
                        ok: up,
                        kind: None,
                        via: None,
                        cause: None,
                        every: Some(300_000),
                    })
                })
                .collect();
            store.monitor_record(&rows);
        }
        let got = from_store(&store, 3, t0 + HOUR_MS - 1);
        let rs = got["resolvers"].as_array().unwrap();
        assert_eq!(rs.len(), 1, "one line per resolver, not one per server");
        let q = &rs[0];
        // Half of what was asked was answered; a cycle still counts as up, since a server answered.
        assert_eq!(q["query_success"], 0.5);
        assert_eq!(q["uptime"], 1.0);
        assert_eq!(q["buckets"][0][1], 72);
        assert_eq!(q["buckets"][0][2], 36);
        assert!(q["outages"].as_array().unwrap().is_empty());
        assert_eq!(store.monitor_recorded(), ["quad9"]);
        drop(store);
        remove(path);
    }

    #[test]
    fn existing_raw_history_is_migrated() {
        let cycles = sample_cycles();
        let rows: Vec<MonitorRow> = cycles.iter().flatten().cloned().collect();
        let now = rows.last().unwrap().ts + 60_000;
        let (store, path) = temp_store();
        // A database from before rollups: raw samples only, schema version 0.
        store.monitor_insert_raw_for_test(&rows);
        drop(store);
        let store = Store::open(path.clone()).unwrap();
        for hours in [5, 72] {
            let (since, cell) = window(hours, now);
            assert_eq!(from_store(&store, hours, now), reference(&rows, hours, since, now, cell), "{hours} h");
        }
        drop(store);
        remove(path);
    }

    fn cyc(ts: i64, answered: bool, cause: Option<Cause>) -> CycleEnd {
        CycleEnd { ts, answered, cause, every: Some(EVERY) }
    }

    /// One resolver's cycle of three samples, answered or not.
    fn rows_of(id: &str, ts: i64, answered: bool, cause: Option<Cause>) -> Vec<MonitorRow> {
        ["cached", "uncached", "tld"]
            .map(|test| MonitorRow {
                ts,
                resolver_id: id.into(),
                test: test.into(),
                us: answered.then_some(9_000),
                ok: answered,
                kind: Some(if answered { "ok" } else { "timeout" }.into()),
                via: Some("UDP".into()),
                cause: cause.filter(|_| !answered),
                every: Some(EVERY),
            })
            .into()
    }

    #[test]
    fn an_outage_is_cut_by_a_long_wait_not_recovered() {
        let mut t = OutageTracker::default();
        assert!(t.cycle(cyc(0, false, Some(Cause::Shared))).is_none());
        assert!(t.cycle(cyc(EVERY, false, None)).is_none());
        // Answered again 8 hours on: the outage ended unseen, so it is cut and not recovered.
        let o = t.cycle(cyc(EVERY + 8 * HOUR_MS, true, None)).unwrap();
        assert!(o.cut && o.recovered.is_none());
        assert_eq!((o.start, o.end, o.cycles, o.shared, o.unclear), (0, EVERY, 2, 1, 0));
        assert!(t.current().is_none());

        // Failed, gap, failed: the first is cut and the second is a new one, still open.
        let mut t = OutageTracker::default();
        assert!(t.cycle(cyc(0, false, None)).is_none());
        let cut = t.cycle(cyc(8 * HOUR_MS, false, Some(Cause::Unclear))).unwrap();
        assert!(cut.cut && cut.start == 0 && cut.cycles == 1);
        let now = t.current().unwrap();
        assert_eq!((now.start, now.cycles, now.unclear, now.cut, now.recovered), (8 * HOUR_MS, 1, 1, false, None));

        // Twice the interval is still the same outage; a cycle with no recorded interval never cuts.
        let mut t = OutageTracker::default();
        assert!(t.cycle(cyc(0, false, None)).is_none());
        assert!(t.cycle(cyc(2 * EVERY, false, None)).is_none());
        assert!(t.cycle(CycleEnd { every: None, ..cyc(8 * HOUR_MS, false, None) }).is_none());
        assert_eq!(t.current().unwrap().cycles, 3);

        // At a one-minute interval a pause of a few minutes is still the same outage; one over ten minutes cuts it.
        let minute = |ts: i64| CycleEnd { every: Some(60_000), ..cyc(ts, false, None) };
        let mut t = OutageTracker::default();
        assert!(t.cycle(minute(0)).is_none());
        assert!(t.cycle(minute(5 * 60_000)).is_none());
        assert!(t.cycle(minute(16 * 60_000)).is_some_and(|o| o.cut && o.cycles == 2));

        // The interval shortened from an hour to a minute mid-outage: the wait was the old interval, not a pause.
        let mut t = OutageTracker::default();
        assert!(t.cycle(CycleEnd { every: Some(HOUR_MS), ..cyc(0, false, None) }).is_none());
        assert!(t.cycle(minute(40 * 60_000)).is_none());
        assert_eq!(t.current().unwrap().cycles, 2);

        // Causes add up: each failed cycle is shared, unclear or isolated (the rest).
        let mut t = OutageTracker::default();
        for (i, cause) in
            [Some(Cause::Shared), Some(Cause::Shared), Some(Cause::Unclear), None, Some(Cause::Isolated)].into_iter().enumerate()
        {
            assert!(t.cycle(cyc(i as i64 * EVERY, false, cause)).is_none());
        }
        let o = t.cycle(cyc(5 * EVERY, true, None)).unwrap();
        assert_eq!((o.cycles, o.shared, o.unclear, o.cut, o.recovered), (5, 2, 1, false, Some(5 * EVERY)));
    }

    #[test]
    fn a_stored_outage_is_cut_and_the_next_one_starts_fresh() {
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS;
        let (store, path) = temp_store();
        store.monitor_record(&rows_of("alpha", t0, false, Some(Cause::Shared)));
        store.monitor_record(&rows_of("alpha", t0 + EVERY, false, None));
        // Eight hours on, answered: cut, not recovered.
        let t1 = t0 + 8 * HOUR_MS;
        store.monitor_record(&rows_of("alpha", t1, true, None));
        let got = store.monitor_outages(0);
        assert_eq!(got.len(), 1);
        let o = &got[0].1;
        assert!(o.cut && o.recovered.is_none() && o.cycles == 2 && o.shared == 1, "{o:?}");
        // A later failure starts another outage and never extends the cut one.
        store.monitor_record(&rows_of("alpha", t1 + EVERY, false, Some(Cause::Unclear)));
        store.monitor_record(&rows_of("alpha", t1 + 2 * EVERY, false, None));
        let got = store.monitor_outages(0);
        assert_eq!(got.len(), 2);
        assert!(got[0].1.cut && got[0].1.cycles == 2);
        assert!(!got[1].1.cut && got[1].1.recovered.is_none());
        assert_eq!((got[1].1.start, got[1].1.cycles, got[1].1.unclear), (t1 + EVERY, 2, 1));
        // Failed, long gap, failed: two outages.
        store.monitor_record(&rows_of("alpha", t1 + 9 * HOUR_MS, false, None));
        let got = store.monitor_outages(0);
        assert_eq!(got.len(), 3);
        assert!(got[1].1.cut && !got[2].1.cut);
        drop(store);
        remove(path);
    }

    #[test]
    fn shared_samples_and_coverage_survive_an_hour_rebuild() {
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS;
        let (store, path) = temp_store();
        // Each record rebuilds the hour from every raw row in it.
        store.monitor_record(&rows_of("alpha", t0, true, None));
        store.monitor_record(&rows_of("alpha", t0 + EVERY, false, Some(Cause::Shared)));
        store.monitor_record(&rows_of("alpha", t0 + 2 * EVERY, false, Some(Cause::Unclear)));
        store.monitor_record(&rows_of("alpha", t0 + 3 * EVERY, false, None));
        let mut hours = Vec::new();
        store.monitor_rollups(0, |r| hours.push((r.n, r.cycles, r.up, r.shared, r.shared_n, r.unclear, r.covered)));
        assert_eq!(hours, [(12, 4, 1, 1, 3, 1, 4 * EVERY)]);

        let got = from_store(&store, 1, t0 + HOUR_MS - 1);
        let a = &got["resolvers"][0];
        assert_eq!((a["shared"].clone(), a["unclear"].clone()), (json!(1), json!(1)));
        // 3 of 12 samples are in the shared cycle; 3 answered of the other 9.
        assert_eq!(a["query_success"], 0.25);
        assert_eq!(a["query_success_own"], 3.0 / 9.0);
        // 3 counted cycles: one answered, one unclear, one isolated.
        assert_eq!(a["uptime_range"], json!([1.0 - 2.0 / 3.0, 1.0 - 1.0 / 3.0]));
        assert_eq!(a["coverage"], (4 * EVERY) as f64 / (HOUR_MS - 1) as f64);
        assert_eq!(a["buckets"][0][6], 3);
        assert_eq!(a["buckets"][0][7], 4 * EVERY);
        drop(store);
        remove(path);
    }

    #[test]
    fn coverage_counts_the_real_wait_since_the_first_check() {
        // A pass takes time, so cycles run a little over the interval apart; a far longer wait counts as one interval.
        assert_eq!(measured(None, 0, EVERY), EVERY);
        assert_eq!(measured(Some(0), EVERY + 2_000, EVERY), EVERY + 2_000);
        assert_eq!(measured(Some(0), 3 * EVERY, EVERY), EVERY);

        // Monitoring began 20 minutes before the end of a day's window: measured throughout, not 1% of the day.
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS;
        let (store, path) = temp_store();
        for i in 0..4 {
            store.monitor_record(&rows_of("alpha", t0 + i * (EVERY + 2_000), true, None));
        }
        let got = from_store(&store, 24, t0 + 3 * (EVERY + 2_000) + EVERY);
        assert_eq!(got["resolvers"][0]["coverage"], 1.0);
        assert_eq!(got["resolvers"][0]["buckets"][0][7], 4 * EVERY + 6_000);
        drop(store);
        remove(path);
    }

    #[test]
    fn gaps_are_kept_listed_and_pruned() {
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS;
        let (store, path) = temp_store();
        store.monitor_record(&rows_of("alpha", t0, true, None));
        store.monitor_gap(t0 - 2 * HOUR_MS, t0 - HOUR_MS, GapCause::Sleep);
        store.monitor_gap(t0 + 1000, t0 + 2000, GapCause::Stall);
        let got = from_store(&store, 3, t0 + HOUR_MS);
        assert_eq!(got["gaps"], json!([[t0 - 2 * HOUR_MS, t0 - HOUR_MS, "sleep"], [t0 + 1000, t0 + 2000, "stall"]]));
        // A gap that ended before the window isn't listed.
        let (since, _) = window(1, t0 + HOUR_MS - 1);
        assert_eq!(store.monitor_gaps(since), [(t0 + 1000, t0 + 2000, "stall".to_owned())]);
        store.monitor_prune(0, t0);
        assert_eq!(store.monitor_gaps(0).len(), 1);
        store.monitor_clear();
        assert!(store.monitor_gaps(0).is_empty());
        drop(store);
        remove(path);
    }

    #[test]
    fn unknown_coverage_and_empty_denominators_are_null() {
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS;
        let (store, path) = temp_store();
        // An hour whose cycles have no recorded interval, and an hour of only shared cycles.
        let legacy: Vec<MonitorRow> = rows_of("alpha", t0, true, None).into_iter().map(|r| MonitorRow { every: None, ..r }).collect();
        store.monitor_record(&legacy);
        store.monitor_record(&rows_of("beta", t0 + HOUR_MS, false, Some(Cause::Shared)));
        let got = from_store(&store, 3, t0 + 2 * HOUR_MS - 1);
        let by = |id: &str| got["resolvers"].as_array().unwrap().iter().find(|r| r["resolver_id"] == id).unwrap().clone();
        assert!(by("alpha")["coverage"].is_null());
        assert!(by("alpha")["buckets"][0][7].is_null());
        assert!(by("beta")["query_success_own"].is_null() && by("beta")["uptime_range"].is_null());
        assert_eq!(by("beta")["query_success"], 0.0);
        drop(store);
        remove(path);
    }

    #[test]
    fn checks_less_often_than_hourly_still_cover_the_window() {
        let mut b = HistoryBuilder::new(HOUR_MS);
        for h in [0, 2, 4] {
            let t = h * HOUR_MS;
            b.push(&RollupRow {
                resolver_id: "alpha",
                hour: t,
                n: 3,
                ok: 3,
                cycles: 1,
                up: 1,
                first: t,
                last: t,
                latencies: &[],
                shared: 0,
                shared_n: 0,
                unclear: 0,
                covered: 2 * HOUR_MS,
            });
        }
        let got = b.finish(6, 0, 6 * HOUR_MS, Vec::new(), Vec::new(), 2);
        assert_eq!(got.resolvers[0].coverage, Some(1.0));
    }
}
