//! Background monitoring: a light probe of selected resolvers every N minutes while
//! Recursa is running. It pauses during manual runs and builds a local reliability
//! record from this network.

use crate::app::AppState;
use crate::bench::{self, TestKind};
use crate::catalog::Resolver;
use crate::dns::{self, QuerySpec};
use crate::endpoints;
use crate::lock;
use crate::net::Exchanger;
use crate::store::MonitorRow;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

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

pub fn spawn(app: Arc<AppState>) {
    tokio::spawn(async move {
        let mut last_cycle = 0i64;
        // Held while this process records monitoring for its database.
        let mut owner: Option<lock::Guard> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            // Another process may have switched it on or off.
            app.reload_monitors();
            let cfg = app.monitor.borrow().clone();
            if !cfg.enabled {
                // Let another Recursa process monitor this database instead.
                owner = None;
                continue;
            }
            let now = bench::now_ms();
            // Due time is derived from the current interval, so shortening it applies at once.
            let due = last_cycle + cfg.interval_min.clamp(1, 1440) as i64 * 60_000;
            // A database download or route-origin sync of ours would skew the probes: wait it out.
            if app.run_active.load(Ordering::SeqCst) || app.intel.fetching() || now < due {
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
            last_cycle = now;
            // A custom entry folded into another row is monitored as that row.
            let wanted =
                |r: &Resolver| cfg.resolvers.contains(&r.id) || r.custom_id.iter().chain(&r.aliases).any(|c| cfg.resolvers.contains(c));
            let resolvers: Vec<Resolver> = app.resolvers().into_iter().filter(wanted).collect();
            // While port 53 is intercepted, plain DNS would measure the interceptor.
            let avoid_udp = app.interception.read().unwrap()["verdict"] == "intercepted";
            let rows = tokio::task::spawn_blocking(move || cycle(&resolvers, avoid_udp)).await.unwrap_or_default();
            app.store.monitor_record(&rows);
            let (raw, summaries) = prune_before(now, cfg.retention_days);
            app.store.monitor_prune(raw, summaries);
        }
    });
}

/// One cached, one uncached and one TLD query per resolver address: the primary and the
/// secondary when a resolver lists both. Both servers' samples are recorded under the
/// resolver's one id, so each line on the Reliability page weighs the two together.
pub fn cycle(resolvers: &[Resolver], avoid_udp: bool) -> Vec<MonitorRow> {
    let ts = bench::now_ms();
    let timeout = Duration::from_millis(2000);
    let round = (ts / 60_000) as u32;
    std::thread::scope(|sc| {
        let hs: Vec<_> = resolvers.iter().map(|r| sc.spawn(move || probe_one(r, ts, round, timeout, avoid_udp))).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

fn probe_one(r: &Resolver, ts: i64, round: u32, timeout: Duration, avoid_udp: bool) -> Vec<MonitorRow> {
    let mut rows = Vec::new();
    for i in 0..r.plain_servers().len().max(1) {
        let opened = endpoints::open_best(&r.only_server(i), &[], timeout, avoid_udp);
        // Over DoT or DoH a resolver has one endpoint however many addresses it lists, so a
        // second probe would only repeat the first.
        let plain = opened.as_ref().is_some_and(|(_, how)| matches!(*how, "UDP" | "TCP"));
        rows.extend(probe_server(r, opened.map(|(x, _)| x), ts, round, timeout));
        if !plain {
            break;
        }
    }
    rows
}

fn probe_server(r: &Resolver, mut ex: Option<Box<dyn Exchanger>>, ts: i64, round: u32, timeout: Duration) -> Vec<MonitorRow> {
    [TestKind::Cached, TestKind::Uncached, TestKind::Tld]
        .into_iter()
        .map(|test| {
            let result = ex.as_mut().and_then(|x| {
                let q = dns::encode_query(dns::new_id(), &QuerySpec::new(bench::query_name(test, round), dns::TYPE_A)).ok()?;
                Some(bench::classify(test, &x.exchange(&q, timeout)))
            });
            // A resolver that answers with a filter's block page or from its cache is up; only silence and errors are outages.
            let ok = matches!(result, Some((bench::Outcome::Ok | bench::Outcome::Blocked | bench::Outcome::FromCache, _, _)));
            MonitorRow { ts, resolver_id: r.id.clone(), test: test.key().into(), us: result.filter(|_| ok).map(|(_, us, _)| us), ok }
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
}

/// Folds raw samples, in time order, into hourly rollups. Each rollup goes to `emit` once its
/// resolver moves on to a later hour (the rest at `finish`), and each cycle to `on_cycle`
/// (resolver, time, answered) in time order per resolver, which is what outages are made of.
pub struct RollupBuilder<E: FnMut(HourRollup), C: FnMut(&str, i64, bool)> {
    open: Vec<OpenHour>,
    last: usize,
    emit: E,
    on_cycle: C,
}

struct OpenHour {
    r: HourRollup,
    /// (time, answered) of the cycle being read.
    cycle: Option<(i64, bool)>,
}

impl<E: FnMut(HourRollup), C: FnMut(&str, i64, bool)> RollupBuilder<E, C> {
    pub fn new(emit: E, on_cycle: C) -> Self {
        RollupBuilder { open: Vec::new(), last: 0, emit, on_cycle }
    }

    pub fn push(&mut self, ts: i64, resolver_id: &str, us: Option<u32>, ok: bool) {
        let hour = ts - ts.rem_euclid(HOUR_MS);
        let RollupBuilder { open, last, emit, on_cycle } = self;
        // A handful of resolvers: a list with the previous sample's resolver first beats hashing.
        if open.get(*last).is_none_or(|o| o.r.resolver_id != resolver_id) {
            *last = match open.iter().position(|o| o.r.resolver_id == resolver_id) {
                Some(i) => i,
                None => {
                    let r = HourRollup { resolver_id: resolver_id.to_owned(), hour, first: ts, ..Default::default() };
                    open.push(OpenHour { r, cycle: None });
                    open.len() - 1
                }
            };
        }
        let o = &mut open[*last];
        if o.cycle.is_some_and(|(t, _)| t != ts) {
            close_cycle(o, on_cycle);
        }
        if o.r.hour != hour {
            let fresh = HourRollup { resolver_id: o.r.resolver_id.clone(), hour, first: ts, ..Default::default() };
            let mut done = std::mem::replace(&mut o.r, fresh);
            done.latencies.sort_unstable();
            emit(done);
        }
        let cycle = o.cycle.get_or_insert((ts, false));
        cycle.1 |= ok;
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
            (self.emit)(o.r);
        }
    }
}

fn close_cycle(o: &mut OpenHour, on_cycle: &mut impl FnMut(&str, i64, bool)) {
    if let Some((ts, answered)) = o.cycle.take() {
        o.r.cycles += 1;
        o.r.up += answered as u32;
        on_cycle(&o.r.resolver_id, ts, answered);
    }
}

/// One resolver's run of consecutive unanswered cycles, fed one cycle at a time.
#[derive(Clone, Copy, Debug, Default)]
pub struct OutageTracker {
    /// (first failed cycle, last failed cycle, cycles) while it is still failing.
    pub open: Option<(i64, i64, u32)>,
}

impl OutageTracker {
    /// The next cycle; returns the outage it ends, if it ends one.
    pub fn cycle(&mut self, ts: i64, answered: bool) -> Option<Outage> {
        if answered {
            return self.open.take().map(|(start, end, cycles)| Outage { start, end, cycles, recovered: Some(ts) });
        }
        self.open = Some(match self.open {
            Some((start, _, n)) => (start, ts, n + 1),
            None => (ts, ts, 1),
        });
        None
    }

    pub fn current(&self) -> Option<Outage> {
        self.open.map(|(start, end, cycles)| Outage { start, end, cycles, recovered: None })
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
}

#[derive(Debug, Serialize)]
pub struct ResolverHistory {
    pub resolver_id: String,
    pub cycles: u32,
    /// Share of cycles with at least one answer.
    pub uptime: Option<f64>,
    pub query_success: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub first: Option<i64>,
    pub last: Option<i64>,
    /// One per cell with samples, oldest first.
    pub buckets: Vec<Cell>,
    /// Outages with a failed cycle in the window, each whole (it may have begun earlier).
    pub outages: Vec<Outage>,
}

/// One strip cell, sent as `[start, samples, answered, p50 ms, p95 ms, histogram]` to keep a
/// month of cells for two dozen resolvers small. See [`histogram`].
#[derive(Debug, Serialize)]
pub struct Cell(pub i64, pub u32, pub u32, pub Option<f64>, pub Option<f64>, pub Vec<u32>);

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
    /// The next cycle that got an answer; none while it is still failing.
    pub recovered: Option<i64>,
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
    /// (cell start, samples, answered, where the cell's latencies begin in `latencies`)
    cell: Option<(i64, u32, u32, usize)>,
    cells: Vec<Cell>,
}

impl Acc {
    fn close_cell(&mut self) {
        let Some((t, n, ok, from)) = self.cell.take() else { return };
        // Sorted in place: the window's percentiles don't care about order.
        let lat = &mut self.latencies[from..];
        lat.sort_unstable();
        self.cells.push(Cell(t, n, ok, pct(lat, 0.5), pct(lat, 0.95), histogram(lat)));
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
        if a.cell.as_ref().is_some_and(|c| c.0 != cell) {
            a.close_cell();
        }
        let c = a.cell.get_or_insert((cell, 0, 0, a.latencies.len()));
        c.1 += r.n;
        c.2 += r.ok;
        a.latencies.extend(r.latencies.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)));
        a.n += r.n;
        a.ok += r.ok;
        a.cycles += r.cycles;
        a.up += r.up;
        a.first = Some(a.first.map_or(r.first, |f| f.min(r.first)));
        a.last = Some(a.last.map_or(r.last, |l| l.max(r.last)));
    }

    /// `outages`: (resolver, outage) for every outage with a failed cycle in the window.
    pub fn finish(self, hours: i64, since: i64, outages: Vec<(String, Outage)>) -> History {
        let mut resolvers: Vec<ResolverHistory> = self
            .by
            .into_iter()
            .map(|(resolver_id, mut a)| {
                a.close_cell();
                a.latencies.sort_unstable();
                let mut mine: Vec<Outage> = outages.iter().filter(|(id, _)| *id == resolver_id).map(|(_, o)| o.clone()).collect();
                mine.sort_by_key(|o| o.start);
                ResolverHistory {
                    cycles: a.cycles,
                    uptime: (a.cycles > 0).then(|| a.up as f64 / a.cycles as f64),
                    query_success: (a.n > 0).then(|| a.ok as f64 / a.n as f64),
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
        History { hours, since, cell: self.cell, resolvers }
    }
}

/// Nearest-rank quantile over sorted µs, in ms.
fn pct(sorted: &[u32], q: f64) -> Option<f64> {
    crate::stats::nearest_rank(sorted, q).map(|us| us as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    /// What the history means, computed the obvious way from raw samples: totals over the
    /// samples in the window, cells by start time, and outages found over the resolver's
    /// whole record, kept when one of their failed cycles is in the window.
    fn reference(rows: &[MonitorRow], hours: i64, since: i64, cell: i64) -> Value {
        let pct = |v: &[f64], q: f64| (!v.is_empty()).then(|| v[((v.len() - 1) as f64 * q).round() as usize]);
        let sorted = |mut v: Vec<f64>| {
            v.sort_by(|a, b| a.total_cmp(b));
            v
        };
        let mut by: BTreeMap<&str, Vec<&MonitorRow>> = BTreeMap::new();
        for r in rows {
            by.entry(r.resolver_id.as_str()).or_default().push(r);
        }
        let out: Vec<Value> = by
            .into_iter()
            .filter_map(|(id, all)| {
                let rs: Vec<&&MonitorRow> = all.iter().filter(|r| r.ts >= since).collect();
                if rs.is_empty() {
                    return None;
                }
                let ms = |r: &MonitorRow| r.us.filter(|_| r.ok).map(|u| u as f64 / 1000.0);
                let mut cells: BTreeMap<i64, (u32, u32, Vec<f64>)> = BTreeMap::new();
                for r in &rs {
                    let c = cells.entry(r.ts - r.ts.rem_euclid(cell)).or_default();
                    c.0 += 1;
                    c.1 += r.ok as u32;
                    c.2.extend(ms(r));
                }
                let cells: Vec<Value> = cells
                    .into_iter()
                    .map(|(t, (n, ok, v))| {
                        let v = sorted(v);
                        let mut h = [0u32; HIST_BINS];
                        for x in &v {
                            h[((x.log10() * 8.0).floor() as i64 + 4).clamp(0, HIST_BINS as i64 - 1) as usize] += 1;
                        }
                        let hist: Vec<u32> = match (h.iter().position(|&c| c > 0), h.iter().rposition(|&c| c > 0)) {
                            (Some(lo), Some(hi)) => std::iter::once(lo as u32).chain(h[lo..=hi].iter().copied()).collect(),
                            _ => Vec::new(),
                        };
                        json!([t, n, ok, pct(&v, 0.5), pct(&v, 0.95), hist])
                    })
                    .collect();
                let cycles_of = |rs: &[&MonitorRow]| {
                    let mut m: BTreeMap<i64, bool> = BTreeMap::new();
                    for r in rs {
                        *m.entry(r.ts).or_default() |= r.ok;
                    }
                    m
                };
                let window_cycles = cycles_of(&rs.iter().map(|r| **r).collect::<Vec<_>>());
                let mut outages: Vec<Value> = Vec::new();
                let mut open: Option<(i64, i64, u32)> = None;
                for (ts, ok) in cycles_of(&all) {
                    if !ok {
                        open = Some(open.map_or((ts, ts, 1), |(s, _, n)| (s, ts, n + 1)));
                    } else if let Some((s, e, n)) = open.take() {
                        outages.push(json!({ "start": s, "end": e, "cycles": n, "recovered": ts }));
                    }
                }
                if let Some((s, e, n)) = open {
                    outages.push(json!({ "start": s, "end": e, "cycles": n, "recovered": null }));
                }
                outages.retain(|o| o["end"].as_i64() >= Some(since));
                let total = window_cycles.len();
                let up = window_cycles.values().filter(|ok| **ok).count();
                let all_ms = sorted(rs.iter().filter_map(|r| ms(r)).collect());
                let q_ok = rs.iter().filter(|r| r.ok).count();
                Some(json!({
                    "resolver_id": id,
                    "cycles": total,
                    "uptime": (total > 0).then(|| up as f64 / total as f64),
                    "query_success": Some(q_ok as f64 / rs.len() as f64),
                    "p50": pct(&all_ms, 0.5),
                    "p95": pct(&all_ms, 0.95),
                    "first": rs.iter().map(|r| r.ts).min(),
                    "last": rs.iter().map(|r| r.ts).max(),
                    "buckets": cells,
                    "outages": outages,
                }))
            })
            .collect();
        json!({ "hours": hours, "since": since, "cell": cell, "resolvers": out })
    }

    /// Three days of five-minute cycles: an outage across an hour boundary, one still open
    /// at the end, one before the shorter windows that only ends inside them, a resolver
    /// that joins late, skipped cycles, and resolvers interleaved within a cycle.
    fn sample_cycles() -> Vec<Vec<MonitorRow>> {
        let mut rng = fastrand::Rng::with_seed(7);
        let t0 = 1_791_000_000_000 - 1_791_000_000_000 % HOUR_MS + 17 * 60_000;
        let total = 3 * 24 * 12;
        (0..total)
            .map(|c| {
                let ts = t0 + c * 300_000;
                let mut cycle = Vec::new();
                for id in ["cloudflare", "google", "system-0", "late"] {
                    if id == "late" && c < 400 || id == "google" && c % 97 == 0 {
                        continue;
                    }
                    let down = (id == "cloudflare" && (100..112).contains(&c))
                        || (id == "google" && (total - 40..total - 20).contains(&c))
                        || (id == "system-0" && c >= total - 5);
                    for test in ["cached", "uncached", "tld"] {
                        let ok = !down && rng.u32(..1000) > 15;
                        let us = ok.then(|| rng.u32(8_000..60_000)).filter(|_| rng.u32(..500) > 0);
                        cycle.push(MonitorRow { ts, resolver_id: id.into(), test: test.into(), us, ok });
                    }
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
        serde_json::to_value(b.finish(hours, since, store.monitor_outages(since))).unwrap()
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
            assert_eq!(from_store(&store, hours, now), reference(&rows, hours, since, cell), "{hours} h");
        }
        // Raw samples can go: the record lives on in the rollups and outages.
        store.monitor_prune(now, 0);
        assert_eq!(from_store(&store, 72, now), {
            let (since, cell) = window(72, now);
            reference(&rows, 72, since, cell)
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
            assert_eq!(from_store(&store, hours, now), reference(&rows, hours, since, cell), "{hours} h");
        }
        drop(store);
        remove(path);
    }
}
