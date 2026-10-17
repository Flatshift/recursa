//! Background NTP monitoring: one light query per chosen server every N minutes while Recursa is running, kept for
//! weeks, so that what one run can't show comes through: how steady each server is, whether it drops out, how its
//! delay follows the time of day, and whether this computer's clock wanders.
//!
//! It is as gentle as a run, and then some: one packet to each address of a server per cycle, never more often than
//! every [`MIN_INTERVAL_MIN`] minutes (a run's own cooldown is ten), through the same ledger as runs, so a server that
//! told us to stop is left alone here too.

use crate::app::{AppState, StopOnDrop};
use crate::bench;
use crate::lock;
use crate::ntp::{self, Consensus, Entry, Plan, Selection, ServerResult, quantile, sorted};
use crate::store::{NtpClockRow, NtpRow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The shortest time between cycles.
pub const MIN_INTERVAL_MIN: u32 = 15;
/// Most servers one cycle asks.
pub const MAX_SERVERS: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NtpMonitorConfig {
    pub enabled: bool,
    pub interval_min: u32,
    /// Catalogue ids, or hosts you typed (`host`, `host:port`).
    pub servers: Vec<String>,
    pub retention_days: u32,
}

impl Default for NtpMonitorConfig {
    fn default() -> Self {
        NtpMonitorConfig {
            enabled: false,
            interval_min: 30,
            servers: ntp::catalog().into_iter().filter(|e| e.default && !e.nts_only && e.kind != "pool").take(4).map(|e| e.id).collect(),
            retention_days: 30,
        }
    }
}

impl NtpMonitorConfig {
    /// Keeps the numbers and the list within what is gentle and sensible.
    pub fn clamped(mut self) -> Self {
        self.interval_min = self.interval_min.clamp(MIN_INTERVAL_MIN, 1440);
        self.retention_days = self.retention_days.clamp(1, 365);
        let mut seen = std::collections::HashSet::new();
        self.servers.retain(|s| !s.trim().is_empty() && seen.insert(s.clone()));
        self.servers.truncate(MAX_SERVERS);
        self
    }

    /// The servers to ask: catalogue entries by id, anything else as a host you typed, up to [`ntp::MAX_CUSTOM`] of those.
    pub fn entries(&self) -> Vec<Entry> {
        let cat = ntp::catalog();
        let mut out: Vec<Entry> = Vec::new();
        let mut custom = 0;
        for s in &self.servers {
            let e = match cat.iter().find(|e| e.id == *s && !e.nts_only && e.kind != "pool") {
                Some(e) => e.clone(),
                None => match ntp::custom_entry(s) {
                    Ok(e) if e.kind == "custom" && custom < ntp::MAX_CUSTOM => {
                        custom += 1;
                        e
                    }
                    Ok(e) if e.kind != "custom" && !e.nts_only && e.kind != "pool" => e,
                    _ => continue,
                },
            };
            if !out.iter().any(|x| x.id == e.id) {
                out.push(e);
            }
        }
        out.truncate(MAX_SERVERS);
        out
    }
}

pub fn spawn(app: Arc<AppState>) {
    tokio::spawn(async move {
        let mut last_cycle = 0i64;
        let mut owner: Option<lock::Guard> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            // Another process may have switched it on or off.
            app.reload_ntp_monitor();
            let cfg = app.ntp_monitor.borrow().clone();
            if !cfg.enabled {
                owner = None;
                continue;
            }
            let now = bench::now_ms();
            // Seed from what is recorded, so a restart doesn't ask again at once.
            if last_cycle == 0 {
                last_cycle = app.store.ntp_latest().unwrap_or(0);
            }
            let due = last_cycle + cfg.interval_min.clamp(MIN_INTERVAL_MIN, 1440) as i64 * 60_000;
            if app.run_active.load(Ordering::SeqCst) || app.intel.fetching() || now < due {
                continue;
            }
            if owner.is_none() {
                owner = lock::try_lock(&lock::ntp_monitor_path(&app.store.path));
                if owner.is_none() {
                    continue;
                }
            }
            let Some(_measuring) = lock::try_lock(&lock::measure_path()) else { continue };
            let (entries, _skipped) = app.ntp_guard(&cfg.entries());
            if !entries.is_empty() {
                // Only a cycle that asks someone uses up the interval; servers all resting are tried again soon.
                last_cycle = now;
                // Switching monitoring off, clearing its record or quitting (this task dropped) stops the cycle.
                let cancel = Arc::new(AtomicBool::new(false));
                let _stop = StopOnDrop(cancel.clone());
                let epoch = app.ntp_epoch.load(Ordering::SeqCst);
                let mut work = tokio::task::spawn_blocking({
                    let cancel = cancel.clone();
                    move || cycle(&entries, &cancel)
                });
                let mut poll = tokio::time::interval(Duration::from_millis(500));
                let (results, consensus) = loop {
                    tokio::select! {
                        r = &mut work => break r.unwrap_or_default(),
                        _ = poll.tick() => {
                            if app.ntp_epoch.load(Ordering::SeqCst) != epoch || !app.ntp_monitor.borrow().enabled {
                                cancel.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                };
                app.ntp_finished(&results);
                let stale =
                    cancel.load(Ordering::SeqCst) || app.ntp_epoch.load(Ordering::SeqCst) != epoch || !app.ntp_monitor.borrow().enabled;
                if !stale {
                    let (rows, clock) = rows(bench::now_ms(), &results, consensus.as_ref());
                    app.store.ntp_record(&rows, clock.as_ref());
                }
            }
            app.store.ntp_prune(now - cfg.retention_days as i64 * 86_400_000);
        }
    });
}

/// One cycle: one query per address, no NTS handshake.
fn cycle(entries: &[Entry], cancel: &AtomicBool) -> (Vec<ServerResult>, Option<Consensus>) {
    let plan = Plan { samples: 1, ipv6: false, ..Default::default() };
    ntp::run(entries, &plan, false, cancel, &|_| {})
}

/// A cycle's results as rows to record.
pub fn rows(ts: i64, results: &[ServerResult], consensus: Option<&Consensus>) -> (Vec<NtpRow>, Option<NtpClockRow>) {
    let rows = results
        .iter()
        .filter(|r| !r.nts_only)
        .map(|r| NtpRow {
            ts,
            server: r.id.clone(),
            offset_ms: r.offset_ms,
            bound_ms: r.reach_ms(),
            delay_ms: r.delay_ms,
            stratum: r.stratum,
            ok: r.ok > 0,
            kind: if r.ok > 0 {
                None
            } else {
                r.error_kind.clone().or_else(|| r.kiss.as_ref().map(|_| "kiss".to_string())).or(Some("timeout".into()))
            },
            sel: r.selection.map(|s| match s {
                Selection::Truechimer => "truechimer".to_string(),
                Selection::Falseticker => "falseticker".to_string(),
            }),
        })
        .collect();
    let clock = consensus.map(|c| NtpClockRow {
        ts,
        median_ms: c.median_ms,
        bound_ms: c.bound_ms,
        voters: c.voters as u32,
        independent: c.independent as u32,
    });
    (rows, clock)
}

// ---------------------------------------------------------------------------------------------
// What the record says
// ---------------------------------------------------------------------------------------------

/// A server's samples over one stretch of the window.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Point {
    pub ts: i64,
    /// Median offset of the answers in the stretch, and their range.
    pub offset_ms: Option<f64>,
    pub lo_ms: Option<f64>,
    pub hi_ms: Option<f64>,
    /// The widest error bound (± the path) among them.
    pub bound_ms: Option<f64>,
    pub delay_ms: Option<f64>,
    pub sent: u32,
    pub ok: u32,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub sent: u32,
    pub ok: u32,
    /// The share of cycles answered, 0 to 100.
    pub up_pct: f64,
    /// How often it went from answering to silent or back.
    pub flaps: u32,
    pub offset_median_ms: Option<f64>,
    /// How far the middle half of its offsets spread: how steadily it reads from here.
    pub offset_spread_ms: Option<f64>,
    pub delay_median_ms: Option<f64>,
    pub delay_p95_ms: Option<f64>,
    /// Median delay by hour of the day (UTC, 24 entries), when it has answered in that hour: follows the load on the path.
    pub delay_by_hour: Vec<Option<f64>>,
    /// Cycles in which it was a falseticker, of those it voted in.
    pub falseticks: u32,
    pub voted: u32,
    /// What it said last, and when.
    pub last_ts: Option<i64>,
    pub last_ok: Option<bool>,
    pub last_kind: Option<String>,
    pub stratum: Option<u8>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ServerHistory {
    pub id: String,
    pub stats: Stats,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ClockPoint {
    pub ts: i64,
    pub median_ms: f64,
    pub bound_ms: f64,
    pub independent: u32,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ClockHistory {
    pub points: Vec<ClockPoint>,
    pub cycles: u32,
    /// Median of the cycles' medians, and how far the middle half of them spread.
    pub median_ms: Option<f64>,
    pub spread_ms: Option<f64>,
    /// The slope of those medians over time, in ms per hour, when the record spans at least six hours and eight cycles.
    pub trend_ms_per_hour: Option<f64>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct History {
    pub since: i64,
    pub until: i64,
    pub bucket_ms: i64,
    pub cycles: u32,
    pub servers: Vec<ServerHistory>,
    pub clock: ClockHistory,
}

/// At most this many points per series, whatever the window.
const MAX_POINTS: i64 = 240;

fn median(v: impl Iterator<Item = f64>) -> Option<f64> {
    quantile(&sorted(v), 0.5)
}

/// Builds what the page shows from the recorded rows in `since..until`.
pub fn history(rows: &[NtpRow], clock: &[NtpClockRow], since: i64, until: i64) -> History {
    let span = (until - since).max(1);
    // Stretches are whole minutes, wide enough that a series has at most MAX_POINTS.
    let bucket = (span / MAX_POINTS / 60_000 + 1).max(1) * 60_000;
    let mut by_server: BTreeMap<&str, Vec<&NtpRow>> = BTreeMap::new();
    for r in rows {
        by_server.entry(r.server.as_str()).or_default().push(r);
    }
    let servers = by_server
        .into_iter()
        .map(|(id, rs)| ServerHistory { id: id.to_string(), stats: stats(&rs), points: points(&rs, since, bucket) })
        .collect();
    let cycles = rows.iter().map(|r| r.ts).collect::<std::collections::BTreeSet<_>>().len() as u32;
    History { since, until, bucket_ms: bucket, cycles, servers, clock: clock_history(clock, since, bucket) }
}

fn points(rs: &[&NtpRow], since: i64, bucket: i64) -> Vec<Point> {
    let mut b: BTreeMap<i64, Vec<&NtpRow>> = BTreeMap::new();
    for r in rs {
        b.entry((r.ts - since).max(0) / bucket).or_default().push(r);
    }
    b.into_iter()
        .map(|(k, g)| {
            let off: Vec<f64> = g.iter().filter_map(|r| r.offset_ms).collect();
            Point {
                ts: since + k * bucket + bucket / 2,
                offset_ms: median(off.iter().copied()),
                lo_ms: off.iter().copied().min_by(f64::total_cmp),
                hi_ms: off.iter().copied().max_by(f64::total_cmp),
                bound_ms: g.iter().filter_map(|r| r.bound_ms).max_by(f64::total_cmp),
                delay_ms: median(g.iter().filter_map(|r| r.delay_ms)),
                sent: g.len() as u32,
                ok: g.iter().filter(|r| r.ok).count() as u32,
            }
        })
        .collect()
}

fn stats(rs: &[&NtpRow]) -> Stats {
    let sent = rs.len() as u32;
    let ok = rs.iter().filter(|r| r.ok).count() as u32;
    let offsets = sorted(rs.iter().filter_map(|r| r.offset_ms));
    let delays = sorted(rs.iter().filter_map(|r| r.delay_ms));
    let mut flaps = 0;
    for w in rs.windows(2) {
        if w[0].ok != w[1].ok {
            flaps += 1;
        }
    }
    let mut hours: Vec<Vec<f64>> = vec![Vec::new(); 24];
    for r in rs {
        if let Some(d) = r.delay_ms {
            hours[(r.ts.div_euclid(3_600_000).rem_euclid(24)) as usize].push(d);
        }
    }
    let last = rs.last();
    Stats {
        sent,
        ok,
        up_pct: if sent == 0 { 0.0 } else { ok as f64 * 100.0 / sent as f64 },
        flaps,
        offset_median_ms: quantile(&offsets, 0.5),
        offset_spread_ms: quantile(&offsets, 0.75).zip(quantile(&offsets, 0.25)).map(|(a, b)| a - b),
        delay_median_ms: quantile(&delays, 0.5),
        delay_p95_ms: if delays.len() >= 8 { quantile(&delays, 0.95) } else { None },
        delay_by_hour: hours.into_iter().map(|h| median(h.into_iter())).collect(),
        falseticks: rs.iter().filter(|r| r.sel.as_deref() == Some("falseticker")).count() as u32,
        voted: rs.iter().filter(|r| r.sel.is_some()).count() as u32,
        last_ts: last.map(|r| r.ts),
        last_ok: last.map(|r| r.ok),
        last_kind: last.and_then(|r| r.kind.clone()),
        stratum: rs.iter().rev().find_map(|r| r.stratum),
    }
}

fn clock_history(clock: &[NtpClockRow], since: i64, bucket: i64) -> ClockHistory {
    let mut b: BTreeMap<i64, Vec<&NtpClockRow>> = BTreeMap::new();
    for c in clock {
        b.entry((c.ts - since).max(0) / bucket).or_default().push(c);
    }
    let points = b
        .into_iter()
        .map(|(k, g)| ClockPoint {
            ts: since + k * bucket + bucket / 2,
            median_ms: median(g.iter().map(|c| c.median_ms)).unwrap_or(0.0),
            bound_ms: g.iter().map(|c| c.bound_ms).fold(0.0, f64::max),
            independent: g.iter().map(|c| c.independent).min().unwrap_or(0),
        })
        .collect();
    let meds = sorted(clock.iter().map(|c| c.median_ms));
    let span_h = clock.last().zip(clock.first()).map_or(0.0, |(l, f)| (l.ts - f.ts) as f64 / 3_600_000.0);
    ClockHistory {
        points,
        cycles: clock.len() as u32,
        median_ms: quantile(&meds, 0.5),
        spread_ms: quantile(&meds, 0.75).zip(quantile(&meds, 0.25)).map(|(a, b)| a - b),
        trend_ms_per_hour: if clock.len() >= 8 && span_h >= 6.0 { slope(clock) } else { None },
    }
}

/// Least-squares slope of the medians against time, in ms per hour.
fn slope(clock: &[NtpClockRow]) -> Option<f64> {
    let n = clock.len() as f64;
    let t0 = clock.first()?.ts;
    let xs: Vec<f64> = clock.iter().map(|c| (c.ts - t0) as f64 / 3_600_000.0).collect();
    let (mx, my) = (xs.iter().sum::<f64>() / n, clock.iter().map(|c| c.median_ms).sum::<f64>() / n);
    let var: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    if var <= 0.0 {
        return None;
    }
    Some(xs.iter().zip(clock).map(|(x, c)| (x - mx) * (c.median_ms - my)).sum::<f64>() / var)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;

    fn row(ts: i64, server: &str, offset: Option<f64>, delay: Option<f64>) -> NtpRow {
        NtpRow {
            ts,
            server: server.into(),
            offset_ms: offset,
            bound_ms: delay.map(|d| d / 2.0),
            delay_ms: delay,
            stratum: offset.map(|_| 2),
            ok: offset.is_some(),
            kind: offset.is_none().then(|| "timeout".to_string()),
            sel: None,
        }
    }

    #[test]
    fn the_interval_and_list_are_kept_gentle() {
        let c = NtpMonitorConfig {
            enabled: true,
            interval_min: 1,
            servers: (0..20).map(|i| format!("s{i}")).chain(["a".to_string(), "a".to_string()]).collect(),
            retention_days: 0,
        }
        .clamped();
        assert_eq!((c.interval_min, c.retention_days, c.servers.len()), (MIN_INTERVAL_MIN, 1, MAX_SERVERS));
        let d = NtpMonitorConfig::default();
        assert!(d.servers.len() >= 3 && !d.enabled);
        // Defaults name real catalogue servers, none a pool.
        assert_eq!(d.entries().len(), d.servers.len());
    }

    #[test]
    fn typed_hosts_are_capped_and_pools_are_refused() {
        let cfg = NtpMonitorConfig {
            servers: ["a.lan", "b.lan", "c.lan", "d.lan", "e.lan", "pool"].map(String::from).to_vec(),
            ..NtpMonitorConfig::default()
        };
        let e = cfg.entries();
        assert_eq!(e.iter().filter(|e| e.kind == "custom").count(), ntp::MAX_CUSTOM);
        assert!(e.iter().all(|e| e.kind != "pool"));
    }

    #[test]
    fn a_servers_record_shows_its_uptime_flaps_and_steadiness() {
        // Answers, answers, silent, answers: two flaps, three of four up.
        let rows = vec![
            row(0, "a", Some(1.0), Some(20.0)),
            row(H, "a", Some(1.2), Some(22.0)),
            row(2 * H, "a", None, None),
            row(3 * H, "a", Some(0.8), Some(24.0)),
        ];
        let h = history(&rows, &[], 0, 4 * H);
        let s = &h.servers[0].stats;
        assert_eq!((s.sent, s.ok, s.flaps), (4, 3, 2));
        assert_eq!(s.up_pct, 75.0);
        assert_eq!(s.last_ok, Some(true));
        assert!((s.offset_median_ms.unwrap() - 1.0).abs() < 1e-9);
        assert!((s.delay_median_ms.unwrap() - 22.0).abs() < 1e-9);
        // Three delays are too few for a 95th percentile.
        assert_eq!(s.delay_p95_ms, None);
        assert_eq!((h.cycles, s.delay_by_hour.len()), (4, 24));
        assert_eq!(s.delay_by_hour[1], Some(22.0));
        assert_eq!(s.delay_by_hour[2], None);
    }

    #[test]
    fn a_long_window_is_thinned_to_a_few_hundred_points() {
        let rows: Vec<NtpRow> = (0..30 * 96).map(|i| row(i * 900_000, "a", Some(1.0), Some(10.0))).collect();
        let h = history(&rows, &[], 0, 30 * 24 * H);
        let p = &h.servers[0].points;
        assert!(p.len() as i64 <= MAX_POINTS, "{} points", p.len());
        assert_eq!(p.iter().map(|p| p.sent).sum::<u32>(), 30 * 96);
        // A bucket holds several answers, and keeps their range.
        assert!(p.iter().all(|p| p.lo_ms.is_some() && p.hi_ms.is_some()));
    }

    #[test]
    fn the_clock_trend_needs_a_long_enough_record() {
        let at = |i: i64, m: f64| NtpClockRow { ts: i * H, median_ms: m, bound_ms: 5.0, voters: 5, independent: 4 };
        let short: Vec<_> = (0..4).map(|i| at(i, i as f64)).collect();
        assert_eq!(history(&[], &short, 0, 4 * H).clock.trend_ms_per_hour, None);
        // Two ms later every hour, over a day.
        let day: Vec<_> = (0..24).map(|i| at(i, 2.0 * i as f64)).collect();
        let c = history(&[], &day, 0, 24 * H).clock;
        assert!((c.trend_ms_per_hour.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(c.cycles, 24);
    }

    #[test]
    fn a_cycle_becomes_rows_naming_why_a_server_was_silent() {
        let ok = ServerResult { id: "a".into(), offset_ms: Some(2.0), delay_ms: Some(30.0), ok: 1, sent: 1, ..Default::default() };
        let dead = ServerResult { id: "b".into(), error_kind: Some("dns".into()), sent: 0, ..Default::default() };
        let kissed = ServerResult { id: "c".into(), kiss: Some("RATE".into()), sent: 1, ..Default::default() };
        let nts_only = ServerResult { id: "d".into(), nts_only: true, ..Default::default() };
        let (rows, clock) = rows(7, &[ok, dead, kissed, nts_only], None);
        assert!(clock.is_none());
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].ok, rows[0].kind.as_deref()), (true, None));
        assert_eq!(rows[1].kind.as_deref(), Some("dns"));
        assert_eq!(rows[2].kind.as_deref(), Some("kiss"));
    }
}
