//! Local persistence: runs, monitoring samples, custom resolvers, settings and the
//! intel cache. One SQLite file in the OS application-data directory.

use crate::monitor::{HOUR_MS, HourRollup, Outage, OutageTracker, RollupBuilder, RollupRow};
use rusqlite::types::Type;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;

const PUT_HOUR: &str = "INSERT OR REPLACE INTO monitor_hour(hour, resolver_id, n, ok, cycles, up, first_ts, last_ts, lat)
     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)";
const PUT_OUTAGE: &str = "INSERT OR REPLACE INTO monitor_outage(resolver_id, started, ended, cycles, recovered) VALUES(?1,?2,?3,?4,?5)";

fn put_hour(st: &mut rusqlite::CachedStatement<'_>, r: &HourRollup) -> rusqlite::Result<usize> {
    st.execute(params![r.hour, r.resolver_id, r.n, r.ok, r.cycles, r.up, r.first, r.last, r.latency_blob()])
}

fn put_outage(st: &mut rusqlite::CachedStatement<'_>, id: &str, o: &Outage) -> rusqlite::Result<usize> {
    st.execute(params![id, o.start, o.end, o.cycles, o.recovered])
}

/// A text column without copying it.
fn text<'a>(r: &'a Row<'_>, i: usize) -> rusqlite::Result<&'a str> {
    r.get_ref(i)?.as_str().map_err(|e| rusqlite::Error::FromSqlConversionFailure(i, Type::Text, e.into()))
}

/// One raw monitoring sample: (time, resolver, µs if answered, answered).
fn sample<'a>(r: &'a Row<'_>) -> rusqlite::Result<(i64, &'a str, Option<u32>, bool)> {
    Ok((r.get(0)?, text(r, 1)?, r.get(2)?, r.get::<_, i32>(3)? != 0))
}

pub struct Store {
    conn: Mutex<Connection>,
    /// A read-only connection for long reads (the monitoring history), so they never hold up
    /// writes on `conn`: in WAL mode a reader and a writer don't block each other. `None` if
    /// it couldn't be opened; long reads then share `conn`.
    reader: Option<Mutex<Connection>>,
    pub path: PathBuf,
}

#[derive(Serialize)]
pub struct RunRow {
    pub id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub summary: serde_json::Value,
    /// Kept by hand: skipped by Clear all runs and by automatic cleanup.
    pub saved: bool,
}

#[derive(Serialize, Clone)]
pub struct MonitorRow {
    pub ts: i64,
    pub resolver_id: String,
    pub test: String,
    pub us: Option<u32>,
    pub ok: bool,
}

pub fn default_path() -> PathBuf {
    // Local, not roaming, on Windows: the RPKI cache beside the db runs to gigabytes.
    // The same directory as data_dir() on macOS and Linux.
    let base = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
    base.join("Recursa").join("recursa.db")
}

impl Store {
    pub fn open(path: PathBuf) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS runs(
               id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, finished_at INTEGER,
               status TEXT NOT NULL, summary TEXT NOT NULL, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS monitor(
               ts INTEGER NOT NULL, resolver_id TEXT NOT NULL, test TEXT NOT NULL,
               us INTEGER, ok INTEGER NOT NULL);
             CREATE INDEX IF NOT EXISTS monitor_ts ON monitor(ts);
             CREATE INDEX IF NOT EXISTS runs_started ON runs(started_at);
             CREATE TABLE IF NOT EXISTS cache(key TEXT PRIMARY KEY, fetched_at INTEGER NOT NULL, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS custom(id TEXT PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS monitor_hour(
               hour INTEGER NOT NULL, resolver_id TEXT NOT NULL,
               n INTEGER NOT NULL, ok INTEGER NOT NULL, cycles INTEGER NOT NULL, up INTEGER NOT NULL,
               first_ts INTEGER NOT NULL, last_ts INTEGER NOT NULL, lat BLOB NOT NULL,
               PRIMARY KEY(hour, resolver_id)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS monitor_outage(
               resolver_id TEXT NOT NULL, started INTEGER NOT NULL, ended INTEGER NOT NULL,
               cycles INTEGER NOT NULL, recovered INTEGER,
               PRIMARY KEY(resolver_id, started)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS ntp_sample(
               ts INTEGER NOT NULL, server TEXT NOT NULL, offset_ms REAL, bound_ms REAL, delay_ms REAL,
               stratum INTEGER, ok INTEGER NOT NULL, kind TEXT, sel TEXT);
             CREATE INDEX IF NOT EXISTS ntp_sample_ts ON ntp_sample(ts);
             CREATE TABLE IF NOT EXISTS ntp_clock(
               ts INTEGER PRIMARY KEY, median_ms REAL NOT NULL, bound_ms REAL NOT NULL, voters INTEGER NOT NULL,
               independent INTEGER NOT NULL);",
        )?;
        let mut conn = conn;
        // Older databases: runs had no `saved` flag.
        let has_saved: bool =
            conn.prepare("PRAGMA table_info(runs)")?.query_map([], |r| r.get::<_, String>(1))?.filter_map(Result::ok).any(|c| c == "saved");
        if !has_saved {
            conn.execute_batch("ALTER TABLE runs ADD COLUMN saved INTEGER NOT NULL DEFAULT 0")?;
        }
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1
            && let Err(e) = build_rollups(&mut conn)
        {
            // History then fills in from new cycles; the next launch tries again.
            eprintln!("Couldn't build the monitoring rollups: {e}");
        }
        let reader =
            Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok().map(Mutex::new);
        Ok(Store { conn: Mutex::new(conn), reader, path })
    }

    pub fn cache_get(&self, key: &str, max_age_ms: i64) -> Option<(serde_json::Value, i64)> {
        let (v, at) = self.cache_get_any(key)?;
        (crate::bench::now_ms() - at <= max_age_ms).then_some((v, at))
    }

    pub fn cache_get_any(&self, key: &str) -> Option<(serde_json::Value, i64)> {
        let c = self.conn.lock().unwrap();
        let row: Option<(String, i64)> = c
            .query_row("SELECT body, fetched_at FROM cache WHERE key=?1", params![key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .ok()
            .flatten();
        row.and_then(|(b, at)| serde_json::from_str(&b).ok().map(|v| (v, at)))
    }

    pub fn cache_put(&self, key: &str, v: &serde_json::Value) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute(
            "INSERT INTO cache(key, fetched_at, body) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET fetched_at=excluded.fetched_at, body=excluded.body",
            params![key, crate::bench::now_ms(), v.to_string()],
        );
    }

    pub fn save_run(&self, id: &str, started_at: i64, finished_at: Option<i64>, status: &str, summary: &serde_json::Value, body: &str) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute(
            "INSERT INTO runs(id, started_at, finished_at, status, summary, body) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO UPDATE SET started_at=excluded.started_at, finished_at=excluded.finished_at,
               status=excluded.status, summary=excluded.summary, body=excluded.body",
            params![id, started_at, finished_at, status, summary.to_string(), body],
        );
    }

    /// Mark a run as saved (kept by hand) or not. False if there is no such run.
    pub fn set_run_saved(&self, id: &str, saved: bool) -> bool {
        let c = self.conn.lock().unwrap();
        c.execute("UPDATE runs SET saved=?2 WHERE id=?1", params![id, saved as i32]).map(|n| n > 0).unwrap_or(false)
    }

    /// Delete every run that isn't saved. Returns how many went.
    pub fn clear_runs(&self) -> usize {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM runs WHERE saved=0", []).unwrap_or(0)
    }

    /// Automatic cleanup of unsaved runs: keep the newest `keep_last` of them, and only those
    /// started at or after `since`; `None` switches a rule off. Saved runs are never counted or
    /// removed. Returns how many went.
    pub fn cleanup_runs(&self, keep_last: Option<u32>, since: Option<i64>) -> usize {
        let c = self.conn.lock().unwrap();
        let mut n = 0;
        if let Some(keep) = keep_last {
            n += c
                .execute(
                    "DELETE FROM runs WHERE saved=0 AND id NOT IN
                       (SELECT id FROM runs WHERE saved=0 ORDER BY started_at DESC, id DESC LIMIT ?1)",
                    params![keep],
                )
                .unwrap_or(0);
        }
        if let Some(since) = since {
            n += c.execute("DELETE FROM runs WHERE saved=0 AND started_at < ?1", params![since]).unwrap_or(0);
        }
        n
    }

    pub fn list_runs(&self, limit: u32) -> Vec<RunRow> {
        let c = self.conn.lock().unwrap();
        let Ok(mut st) =
            c.prepare("SELECT id, started_at, finished_at, status, summary, saved FROM runs ORDER BY started_at DESC LIMIT ?1")
        else {
            return vec![];
        };
        st.query_map(params![limit], |r| {
            let s: String = r.get(4)?;
            Ok(RunRow {
                id: r.get(0)?,
                started_at: r.get(1)?,
                finished_at: r.get(2)?,
                status: r.get(3)?,
                summary: serde_json::from_str(&s).unwrap_or(serde_json::Value::Null),
                saved: r.get::<_, i32>(5)? != 0,
            })
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    pub fn get_run(&self, id: &str) -> Option<String> {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT body FROM runs WHERE id=?1", params![id], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn delete_run(&self, id: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM runs WHERE id=?1", params![id]).map(|n| n > 0).unwrap_or(false)
    }

    pub fn custom_list(&self) -> Vec<serde_json::Value> {
        let c = self.conn.lock().unwrap();
        let Ok(mut st) = c.prepare("SELECT body FROM custom ORDER BY rowid") else { return vec![] };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|it| it.filter_map(Result::ok).filter_map(|s| serde_json::from_str(&s).ok()).collect())
            .unwrap_or_default()
    }

    pub fn custom_put(&self, id: &str, v: &serde_json::Value) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("INSERT OR REPLACE INTO custom(id, body) VALUES(?1, ?2)", params![id, v.to_string()]);
    }

    pub fn custom_delete(&self, id: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM custom WHERE id=?1", params![id]).map(|n| n > 0).unwrap_or(false)
    }

    pub fn setting(&self, key: &str) -> Option<serde_json::Value> {
        let c = self.conn.lock().unwrap();
        let s: Option<String> = c.query_row("SELECT body FROM settings WHERE key=?1", params![key], |r| r.get(0)).optional().ok().flatten();
        s.and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn set_setting(&self, key: &str, v: &serde_json::Value) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("INSERT OR REPLACE INTO settings(key, body) VALUES(?1, ?2)", params![key, v.to_string()]);
    }

    /// Record one monitoring cycle: the raw samples, the rollup of each hour they fall in
    /// (rebuilt from that hour's samples), and outages opened, extended or ended. One
    /// transaction, so the three never disagree.
    pub fn monitor_record(&self, rows: &[MonitorRow]) {
        let mut c = self.conn.lock().unwrap();
        let Ok(tx) = c.transaction() else { return };
        match record(&tx, rows) {
            Ok(()) => {
                let _ = tx.commit();
            }
            Err(e) => eprintln!("Couldn't record a monitoring cycle: {e}"),
        }
    }

    /// Hand the hourly rollups from `since` on to `f`, in hour order, on the read-only
    /// connection.
    pub fn monitor_rollups(&self, since: i64, mut f: impl FnMut(RollupRow<'_>)) {
        let c = self.reader.as_ref().unwrap_or(&self.conn).lock().unwrap();
        // Primary-key order: no sort.
        let Ok(mut st) = c.prepare(
            "SELECT hour, resolver_id, n, ok, cycles, up, first_ts, last_ts, lat FROM monitor_hour WHERE hour >= ?1 ORDER BY hour, resolver_id",
        ) else {
            return;
        };
        let Ok(mut rows) = st.query(params![since]) else { return };
        while let Ok(Some(r)) = rows.next() {
            let row = (|| {
                Ok::<_, rusqlite::Error>(RollupRow {
                    hour: r.get(0)?,
                    resolver_id: text(r, 1)?,
                    n: r.get(2)?,
                    ok: r.get(3)?,
                    cycles: r.get(4)?,
                    up: r.get(5)?,
                    first: r.get(6)?,
                    last: r.get(7)?,
                    latencies: r.get_ref(8)?.as_blob().map_err(|e| rusqlite::Error::FromSqlConversionFailure(8, Type::Blob, e.into()))?,
                })
            })();
            if let Ok(row) = row {
                f(row);
            }
        }
    }

    /// Resolvers that have any monitoring history kept, i.e. a record of their own from this network.
    pub fn monitor_recorded(&self) -> Vec<String> {
        let c = self.reader.as_ref().unwrap_or(&self.conn).lock().unwrap();
        let Ok(mut q) = c.prepare("SELECT DISTINCT resolver_id FROM monitor_hour ORDER BY resolver_id") else { return Vec::new() };
        q.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// When the newest monitoring sample was recorded, by this process or any other.
    pub fn monitor_latest(&self) -> Option<i64> {
        let c = self.reader.as_ref().unwrap_or(&self.conn).lock().unwrap();
        c.query_row("SELECT MAX(ts) FROM monitor", [], |r| r.get(0)).ok().flatten()
    }

    /// Outages with a failed cycle at or after `since`: (resolver, outage).
    pub fn monitor_outages(&self, since: i64) -> Vec<(String, Outage)> {
        let c = self.reader.as_ref().unwrap_or(&self.conn).lock().unwrap();
        let Ok(mut st) = c.prepare(
            "SELECT resolver_id, started, ended, cycles, recovered FROM monitor_outage WHERE ended >= ?1 ORDER BY resolver_id, started",
        ) else {
            return vec![];
        };
        st.query_map(params![since], |r| {
            Ok((r.get(0)?, Outage { start: r.get(1)?, end: r.get(2)?, cycles: r.get(3)?, recovered: r.get(4)? }))
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn monitor_insert_raw_for_test(&self, rows: &[MonitorRow]) {
        let c = self.conn.lock().unwrap();
        for r in rows {
            c.execute(
                "INSERT INTO monitor(ts, resolver_id, test, us, ok) VALUES(?1,?2,?3,?4,?5)",
                params![r.ts, r.resolver_id, r.test, r.us, r.ok as i32],
            )
            .unwrap();
        }
        c.execute_batch("DELETE FROM monitor_hour; DELETE FROM monitor_outage; PRAGMA user_version = 0;").unwrap();
    }

    /// Drop raw samples before `raw_before`, and rollups and outages that ended before `before`.
    pub fn monitor_prune(&self, raw_before: i64, before: i64) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("DELETE FROM monitor WHERE ts < ?1", params![raw_before]);
        let _ = c.execute("DELETE FROM monitor_hour WHERE hour <= ?1", params![before - HOUR_MS]);
        let _ = c.execute("DELETE FROM monitor_outage WHERE ended < ?1", params![before]);
    }

    pub fn monitor_clear(&self) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute_batch("DELETE FROM monitor; DELETE FROM monitor_hour; DELETE FROM monitor_outage;");
    }
}

impl Store {
    /// One NTP monitoring cycle: each server's sample, and the clock verdict when the cycle reached one.
    pub fn ntp_record(&self, rows: &[NtpRow], clock: Option<&NtpClockRow>) {
        let mut c = self.conn.lock().unwrap();
        let Ok(tx) = c.transaction() else { return };
        let r = (|| {
            let mut ins = tx.prepare_cached(
                "INSERT INTO ntp_sample(ts, server, offset_ms, bound_ms, delay_ms, stratum, ok, kind, sel) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            )?;
            for r in rows {
                ins.execute(params![r.ts, r.server, r.offset_ms, r.bound_ms, r.delay_ms, r.stratum, r.ok as i32, r.kind, r.sel])?;
            }
            if let Some(k) = clock {
                tx.execute(
                    "INSERT OR REPLACE INTO ntp_clock(ts, median_ms, bound_ms, voters, independent) VALUES(?1,?2,?3,?4,?5)",
                    params![k.ts, k.median_ms, k.bound_ms, k.voters, k.independent],
                )?;
            }
            Ok::<_, rusqlite::Error>(())
        })();
        match r {
            Ok(()) => {
                let _ = tx.commit();
            }
            Err(e) => eprintln!("Couldn't record an NTP monitoring cycle: {e}"),
        }
    }

    /// Samples and clock verdicts from `since` on, oldest first.
    pub fn ntp_history(&self, since: i64) -> (Vec<NtpRow>, Vec<NtpClockRow>) {
        let c = self.reader.as_ref().unwrap_or(&self.conn).lock().unwrap();
        let rows = c
            .prepare("SELECT ts, server, offset_ms, bound_ms, delay_ms, stratum, ok, kind, sel FROM ntp_sample WHERE ts >= ?1 ORDER BY ts, server")
            .and_then(|mut q| {
                q.query_map(params![since], |r| {
                    Ok(NtpRow {
                        ts: r.get(0)?,
                        server: r.get(1)?,
                        offset_ms: r.get(2)?,
                        bound_ms: r.get(3)?,
                        delay_ms: r.get(4)?,
                        stratum: r.get(5)?,
                        ok: r.get::<_, i32>(6)? != 0,
                        kind: r.get(7)?,
                        sel: r.get(8)?,
                    })
                })
                .map(|it| it.filter_map(Result::ok).collect::<Vec<_>>())
            })
            .unwrap_or_default();
        let clock = c
            .prepare("SELECT ts, median_ms, bound_ms, voters, independent FROM ntp_clock WHERE ts >= ?1 ORDER BY ts")
            .and_then(|mut q| {
                q.query_map(params![since], |r| {
                    Ok(NtpClockRow { ts: r.get(0)?, median_ms: r.get(1)?, bound_ms: r.get(2)?, voters: r.get(3)?, independent: r.get(4)? })
                })
                .map(|it| it.filter_map(Result::ok).collect::<Vec<_>>())
            })
            .unwrap_or_default();
        (rows, clock)
    }

    /// When the newest NTP monitoring sample was recorded, by this process or any other.
    pub fn ntp_latest(&self) -> Option<i64> {
        let c = self.reader.as_ref().unwrap_or(&self.conn).lock().unwrap();
        c.query_row("SELECT MAX(ts) FROM ntp_sample", [], |r| r.get(0)).ok().flatten()
    }

    pub fn ntp_prune(&self, before: i64) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("DELETE FROM ntp_sample WHERE ts < ?1", params![before]);
        let _ = c.execute("DELETE FROM ntp_clock WHERE ts < ?1", params![before]);
    }

    pub fn ntp_clear(&self) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute_batch("DELETE FROM ntp_sample; DELETE FROM ntp_clock;");
    }
}

/// One server's answer (or failure) in one NTP monitoring cycle.
#[derive(Clone, Debug, PartialEq)]
pub struct NtpRow {
    pub ts: i64,
    pub server: String,
    pub offset_ms: Option<f64>,
    pub bound_ms: Option<f64>,
    pub delay_ms: Option<f64>,
    pub stratum: Option<u8>,
    pub ok: bool,
    /// Why there was no answer: a [`crate::ntp::ServerResult::error_kind`] name.
    pub kind: Option<String>,
    /// `truechimer` or `falseticker`, for servers that voted.
    pub sel: Option<String>,
}

/// What a cycle's consensus said about this computer's clock.
#[derive(Clone, Debug, PartialEq)]
pub struct NtpClockRow {
    pub ts: i64,
    pub median_ms: f64,
    pub bound_ms: f64,
    pub voters: u32,
    pub independent: u32,
}

/// See [`Store::monitor_record`].
fn record(tx: &Transaction<'_>, rows: &[MonitorRow]) -> rusqlite::Result<()> {
    let mut insert = tx.prepare_cached("INSERT INTO monitor(ts, resolver_id, test, us, ok) VALUES(?1,?2,?3,?4,?5)")?;
    for r in rows {
        insert.execute(params![r.ts, r.resolver_id, r.test, r.us, r.ok as i32])?;
    }
    // Each hour these samples fall in (normally just the current one), rebuilt whole.
    let hours: BTreeSet<i64> = rows.iter().map(|r| r.ts - r.ts.rem_euclid(HOUR_MS)).collect();
    let mut read = tx.prepare_cached("SELECT ts, resolver_id, us, ok FROM monitor WHERE ts >= ?1 AND ts < ?2 ORDER BY ts")?;
    let mut put = tx.prepare_cached(PUT_HOUR)?;
    for h in hours {
        let mut done = Vec::new();
        let mut b = RollupBuilder::new(|r| done.push(r), |_, _, _| {});
        let mut q = read.query(params![h, h + HOUR_MS])?;
        while let Some(r) = q.next()? {
            let (ts, id, us, ok) = sample(r)?;
            b.push(ts, id, us, ok);
        }
        b.finish();
        for r in &done {
            put_hour(&mut put, r)?;
        }
    }
    // Outages: each resolver's cycles in these samples, oldest first, continue its open one.
    let mut cycles: BTreeMap<(&str, i64), bool> = BTreeMap::new();
    for r in rows {
        *cycles.entry((r.resolver_id.as_str(), r.ts)).or_default() |= r.ok;
    }
    let mut open = tx.prepare_cached(
        "SELECT started, ended, cycles FROM monitor_outage WHERE resolver_id = ?1 AND recovered IS NULL ORDER BY started DESC LIMIT 1",
    )?;
    let mut put = tx.prepare_cached(PUT_OUTAGE)?;
    let mut current: Option<(&str, OutageTracker)> = None;
    for ((id, ts), answered) in cycles {
        if current.as_ref().is_none_or(|(c, _)| *c != id) {
            if let Some((prev, t)) = current.take()
                && let Some(o) = t.current()
            {
                put_outage(&mut put, prev, &o)?;
            }
            let open = open.query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
            current = Some((id, OutageTracker { open }));
        }
        let (_, t) = current.as_mut().expect("set above");
        if let Some(ended) = t.cycle(ts, answered) {
            put_outage(&mut put, id, &ended)?;
        }
    }
    if let Some((id, t)) = current
        && let Some(o) = t.current()
    {
        put_outage(&mut put, id, &o)?;
    }
    Ok(())
}

/// Schema version 1: hourly rollups and outages from the raw samples already recorded, in one
/// streaming pass.
fn build_rollups(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch("DELETE FROM monitor_hour; DELETE FROM monitor_outage;")?;
    {
        let failed: RefCell<Option<rusqlite::Error>> = RefCell::new(None);
        let mut hours = tx.prepare_cached(PUT_HOUR)?;
        let mut outages = tx.prepare_cached(PUT_OUTAGE)?;
        let outages = RefCell::new(&mut outages);
        let mut trackers: Vec<(String, OutageTracker)> = Vec::new();
        let mut b = RollupBuilder::new(
            |r| {
                if let Err(e) = put_hour(&mut hours, &r) {
                    failed.borrow_mut().get_or_insert(e);
                }
            },
            |id, ts, answered| {
                let i = trackers.iter().position(|(t, _)| t == id).unwrap_or_else(|| {
                    trackers.push((id.to_owned(), OutageTracker::default()));
                    trackers.len() - 1
                });
                if let Some(o) = trackers[i].1.cycle(ts, answered)
                    && let Err(e) = put_outage(&mut outages.borrow_mut(), id, &o)
                {
                    failed.borrow_mut().get_or_insert(e);
                }
            },
        );
        let mut read = tx.prepare("SELECT ts, resolver_id, us, ok FROM monitor ORDER BY ts")?;
        let mut rows = read.query([])?;
        while let Some(r) = rows.next()? {
            let (ts, id, us, ok) = sample(r)?;
            b.push(ts, id, us, ok);
        }
        b.finish();
        for (id, t) in &trackers {
            if let Some(o) = t.current() {
                put_outage(&mut outages.borrow_mut(), id, &o)?;
            }
        }
        if let Some(e) = failed.into_inner() {
            return Err(e);
        }
    }
    tx.pragma_update(None, "user_version", 1)?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("recursa-store-{:x}.db", fastrand::u64(..)))
    }

    fn remove(path: &std::path::Path) {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    fn ids(store: &Store) -> Vec<String> {
        store.list_runs(100).into_iter().map(|r| r.id).collect()
    }

    #[test]
    fn saved_runs_survive_clear_and_both_cleanup_rules() {
        let path = temp_path();
        let store = Store::open(path.clone()).unwrap();
        for (i, id) in ["r1", "r2", "r3", "r4", "r5"].iter().enumerate() {
            store.save_run(id, 1_000 * (i as i64 + 1), None, "done", &serde_json::json!({}), "{}");
        }
        assert!(store.set_run_saved("r2", true));
        assert!(!store.set_run_saved("nope", true));
        assert!(store.list_runs(100).iter().find(|r| r.id == "r2").unwrap().saved);
        // Keep the last two unsaved runs: r2 is saved and doesn't count.
        assert_eq!(store.cleanup_runs(Some(2), None), 2);
        assert_eq!(ids(&store), ["r5", "r4", "r2"]);
        // Keep runs started at or after 5000: r4 goes, r2 is saved and stays.
        assert_eq!(store.cleanup_runs(None, Some(5_000)), 1);
        assert_eq!(ids(&store), ["r5", "r2"]);
        // Both rules off: nothing goes.
        assert_eq!(store.cleanup_runs(None, None), 0);
        // Clear all takes every unsaved run; unsaving r2 then lets it go.
        assert_eq!(store.clear_runs(), 1);
        assert_eq!(ids(&store), ["r2"]);
        assert!(store.set_run_saved("r2", false));
        assert_eq!(store.clear_runs(), 1);
        assert!(ids(&store).is_empty());
        remove(&path);
    }

    #[test]
    fn a_saved_mark_outlives_the_run_being_rewritten() {
        let path = temp_path();
        let store = Store::open(path.clone()).unwrap();
        store.save_run("r1", 1, None, "running", &serde_json::json!({}), "{}");
        assert!(store.set_run_saved("r1", true));
        store.save_run("r1", 1, Some(2), "done", &serde_json::json!({}), "{}");
        let rows = store.list_runs(1);
        assert!(rows[0].saved && rows[0].status == "done");
        remove(&path);
    }

    #[test]
    fn older_databases_get_the_saved_column() {
        let path = temp_path();
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE runs(id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, finished_at INTEGER,
                   status TEXT NOT NULL, summary TEXT NOT NULL, body TEXT NOT NULL);
                 INSERT INTO runs VALUES('old', 1, 2, 'done', '{}', '{}');",
            )
            .unwrap();
        }
        let store = Store::open(path.clone()).unwrap();
        let rows = store.list_runs(10);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].saved);
        assert!(store.set_run_saved("old", true));
        drop(store);
        // Opening again leaves the column and the mark alone.
        let store = Store::open(path.clone()).unwrap();
        assert!(store.list_runs(10)[0].saved);
        remove(&path);
    }
}
