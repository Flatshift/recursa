//! Local persistence: runs, monitoring samples, custom resolvers, settings and the
//! intel cache. One SQLite file in the OS application-data directory.

use crate::border::{Kind, Pair, Sample as BorderSample};
use crate::monitor::{HOUR_MS, HourRollup, Outage, OutageTracker, RollupBuilder, RollupRow};
use rusqlite::types::Type;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, Transaction, params};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

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
    // Never the shared temp directory, where another user could create the folder first. Without a home, the
    // current directory, which the user chose.
    let base = dirs::data_local_dir().unwrap_or_default();
    base.join("Recursa").join("recursa.db")
}

/// Create `dir` and its missing parents, private to this user on Unix: it holds run history and settings. The
/// default data directory is made private even if an older version created it open to others.
pub(crate) fn make_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
        if default_path().parent() == Some(dir)
            && let Ok(m) = std::fs::metadata(dir)
            && m.permissions().mode() & 0o077 != 0
        {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    let _ = std::fs::create_dir_all(dir);
}

impl Store {
    pub fn open(path: PathBuf) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            make_dir(dir);
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
               independent INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS border_pair(
               id INTEGER PRIMARY KEY, cdn TEXT NOT NULL, target TEXT NOT NULL,
               near TEXT NOT NULL, near_ttl INTEGER NOT NULL, far TEXT NOT NULL, far_ttl INTEGER NOT NULL,
               far_asn INTEGER, far_holder TEXT, kind TEXT NOT NULL, found_at INTEGER NOT NULL, retired_at INTEGER);
             CREATE TABLE IF NOT EXISTS border_sample(
               ts INTEGER NOT NULL, pair INTEGER NOT NULL, near_ms REAL, far_ms REAL);
             CREATE INDEX IF NOT EXISTS border_sample_ts ON border_sample(ts);
             CREATE TABLE IF NOT EXISTS pfx2as(
               prefix INTEGER NOT NULL, len INTEGER NOT NULL, asn INTEGER NOT NULL, moas INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS pfx2as6(
               prefix BLOB NOT NULL, len INTEGER NOT NULL, asn INTEGER NOT NULL, moas INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS asrel(
               a INTEGER NOT NULL, b INTEGER NOT NULL, rel INTEGER NOT NULL, PRIMARY KEY (a, b));
             CREATE INDEX IF NOT EXISTS asrel_b ON asrel(b);
             CREATE TABLE IF NOT EXISTS netmap_run(
               id INTEGER PRIMARY KEY CHECK (id = 1), ts INTEGER NOT NULL, body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS pdb_sync(\"table\" TEXT PRIMARY KEY, synced_at INTEGER NOT NULL, since TEXT);
             CREATE TABLE IF NOT EXISTS pdb_ix(
               id INTEGER PRIMARY KEY, name TEXT, name_long TEXT, city TEXT, country TEXT, updated TEXT);
             CREATE TABLE IF NOT EXISTS pdb_ixpfx(id INTEGER PRIMARY KEY, ix_id INTEGER, prefix TEXT, updated TEXT);
             CREATE TABLE IF NOT EXISTS pdb_netixlan(
               id INTEGER PRIMARY KEY, ix_id INTEGER, asn INTEGER, ipaddr4 TEXT, ipaddr6 TEXT, speed INTEGER, updated TEXT);
             CREATE TABLE IF NOT EXISTS pdb_fac(
               id INTEGER PRIMARY KEY, name TEXT, address1 TEXT, city TEXT, country TEXT, updated TEXT);
             CREATE TABLE IF NOT EXISTS pdb_netfac(id INTEGER PRIMARY KEY, fac_id INTEGER, asn INTEGER, updated TEXT);
             CREATE TABLE IF NOT EXISTS rdap_block(
               start BLOB PRIMARY KEY, end BLOB NOT NULL, body TEXT NOT NULL, fetched_at INTEGER NOT NULL);",
        )?;
        let mut conn = conn;
        // Older databases: runs had no `saved` flag.
        let has_saved: bool =
            conn.prepare("PRAGMA table_info(runs)")?.query_map([], |r| r.get::<_, String>(1))?.filter_map(Result::ok).any(|c| c == "saved");
        if !has_saved {
            // A database with runs from before the flag starts with cleanup off: nobody has had
            // the chance to save anything yet. A new database gets the defaults. Seeded before
            // the column lands, so a crash in between can't leave the defaults to run.
            let has_runs: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM runs)", [], |r| r.get(0))?;
            if has_runs {
                conn.execute(
                    "INSERT OR IGNORE INTO settings(key, body) VALUES('housekeeping', ?1)",
                    params![r#"{"keep_runs":null,"keep_days":null}"#],
                )?;
            }
            // Another process may add the column between the check and this.
            if let Err(e) = conn.execute_batch("ALTER TABLE runs ADD COLUMN saved INTEGER NOT NULL DEFAULT 0")
                && !e.to_string().contains("duplicate column")
            {
                return Err(e);
            }
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

    /// Delete every run that isn't saved, except `keep` (the live run, whose row may land
    /// meanwhile). Returns how many went.
    pub fn clear_runs(&self, keep: Option<&str>) -> usize {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM runs WHERE saved=0 AND (?1 IS NULL OR id != ?1)", params![keep]).unwrap_or(0)
    }

    pub fn has_run(&self, id: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT 1 FROM runs WHERE id=?1", params![id], |_| Ok(())).optional().ok().flatten().is_some()
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

    fn setting(&self, key: &str) -> Option<serde_json::Value> {
        let c = self.conn.lock().unwrap();
        let s: Option<String> = c.query_row("SELECT body FROM settings WHERE key=?1", params![key], |r| r.get(0)).optional().ok().flatten();
        s.and_then(|s| serde_json::from_str(&s).ok())
    }

    fn set_setting(&self, key: &str, v: &serde_json::Value) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("INSERT OR REPLACE INTO settings(key, body) VALUES(?1, ?2)", params![key, v.to_string()]);
    }

    /// The setting at `key` as a `T`; `None` when there is none or it doesn't parse.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.setting(key).and_then(|v| serde_json::from_value(v).ok())
    }

    /// Save `v` as the setting at `key`.
    pub fn put<T: Serialize>(&self, key: &str, v: &T) {
        if let Ok(v) = serde_json::to_value(v) {
            self.set_setting(key, &v);
        }
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

    /// The connection for a long read: the read-only one, else `conn`.
    fn rd(&self) -> MutexGuard<'_, Connection> {
        self.reader.as_ref().unwrap_or(&self.conn).lock().unwrap()
    }

    /// Hand the hourly rollups from `since` on to `f`, in hour order, on the read-only
    /// connection.
    pub fn monitor_rollups(&self, since: i64, mut f: impl FnMut(RollupRow<'_>)) {
        let c = self.rd();
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
        let c = self.rd();
        let Ok(mut q) = c.prepare("SELECT DISTINCT resolver_id FROM monitor_hour ORDER BY resolver_id") else { return Vec::new() };
        q.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// When the newest monitoring sample was recorded, by this process or any other.
    pub fn monitor_latest(&self) -> Option<i64> {
        let c = self.rd();
        c.query_row("SELECT MAX(ts) FROM monitor", [], |r| r.get(0)).ok().flatten()
    }

    /// Outages with a failed cycle at or after `since`: (resolver, outage).
    pub fn monitor_outages(&self, since: i64) -> Vec<(String, Outage)> {
        let c = self.rd();
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
        let c = self.rd();
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
        let c = self.rd();
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

impl Store {
    /// Store a new pair, returning its id.
    pub fn border_insert_pair(&self, p: &Pair) -> i64 {
        let c = self.conn.lock().unwrap();
        let r = c.execute(
            "INSERT INTO border_pair(cdn, target, near, near_ttl, far, far_ttl, far_asn, far_holder, kind, found_at, retired_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                p.cdn,
                p.target.to_string(),
                p.near.to_string(),
                p.near_ttl,
                p.far.to_string(),
                p.far_ttl,
                p.far_asn,
                p.far_holder,
                p.kind.as_str(),
                p.found_at,
                p.retired_at
            ],
        );
        match r {
            Ok(_) => c.last_insert_rowid(),
            Err(e) => {
                eprintln!("Couldn't record a border pair: {e}");
                0
            }
        }
    }

    /// End a pair: its series stops here, and a new pair starts its own.
    pub fn border_retire(&self, id: i64, at: i64) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("UPDATE border_pair SET retired_at = ?2 WHERE id = ?1 AND retired_at IS NULL", params![id, at]);
    }

    /// The pairs in use, and those retired at or after `since`, oldest first.
    pub fn border_pairs(&self, since: i64) -> Vec<Pair> {
        let c = self.rd();
        c.prepare(
            "SELECT id, cdn, target, near, near_ttl, far, far_ttl, far_asn, far_holder, kind, found_at, retired_at
             FROM border_pair WHERE retired_at IS NULL OR retired_at >= ?1 ORDER BY id",
        )
        .and_then(|mut q| {
            q.query_map(params![since], |r| {
                let ip = |i: usize| r.get::<_, String>(i).map(|s| s.parse::<std::net::IpAddr>().ok());
                Ok(Some(Pair {
                    id: r.get(0)?,
                    cdn: r.get(1)?,
                    target: ip(2)?.ok_or(rusqlite::Error::InvalidQuery)?,
                    near: ip(3)?.ok_or(rusqlite::Error::InvalidQuery)?,
                    near_ttl: r.get(4)?,
                    far: ip(5)?.ok_or(rusqlite::Error::InvalidQuery)?,
                    far_ttl: r.get(6)?,
                    far_asn: r.get(7)?,
                    far_holder: r.get(8)?,
                    kind: Kind::parse(&r.get::<_, String>(9)?),
                    found_at: r.get(10)?,
                    retired_at: r.get(11)?,
                }))
            })
            .map(|it| it.filter_map(Result::ok).flatten().collect::<Vec<_>>())
        })
        .unwrap_or_default()
    }

    pub fn border_record(&self, rows: &[BorderSample]) {
        let mut c = self.conn.lock().unwrap();
        let Ok(tx) = c.transaction() else { return };
        let r = (|| {
            let mut ins = tx.prepare_cached("INSERT INTO border_sample(ts, pair, near_ms, far_ms) VALUES(?1,?2,?3,?4)")?;
            for s in rows {
                ins.execute(params![s.ts, s.pair, s.near_ms, s.far_ms])?;
            }
            Ok::<_, rusqlite::Error>(())
        })();
        match r {
            Ok(()) => {
                let _ = tx.commit();
            }
            Err(e) => eprintln!("Couldn't record a border monitoring cycle: {e}"),
        }
    }

    /// Samples from `since` on, oldest first.
    pub fn border_samples(&self, since: i64) -> Vec<BorderSample> {
        let c = self.rd();
        c.prepare("SELECT ts, pair, near_ms, far_ms FROM border_sample WHERE ts >= ?1 ORDER BY ts, pair")
            .and_then(|mut q| {
                q.query_map(params![since], |r| Ok(BorderSample { ts: r.get(0)?, pair: r.get(1)?, near_ms: r.get(2)?, far_ms: r.get(3)? }))
                    .map(|it| it.filter_map(Result::ok).collect::<Vec<_>>())
            })
            .unwrap_or_default()
    }

    /// When the newest border sample was recorded, by this process or any other.
    pub fn border_latest(&self) -> Option<i64> {
        let c = self.rd();
        c.query_row("SELECT MAX(ts) FROM border_sample", [], |r| r.get(0)).ok().flatten()
    }

    /// Drop samples before `before`, and pairs retired before it.
    pub fn border_prune(&self, before: i64) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("DELETE FROM border_sample WHERE ts < ?1", params![before]);
        let _ = c.execute("DELETE FROM border_pair WHERE retired_at IS NOT NULL AND retired_at < ?1", params![before]);
    }

    pub fn border_clear(&self) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute_batch("DELETE FROM border_sample; DELETE FROM border_pair;");
    }
}

impl Store {
    /// Run `f` on the database connection. For the network map's datasets, which keep their own SQL beside the code
    /// that reads them.
    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        f(&mut self.conn.lock().unwrap())
    }

    /// Keep `body` as the one stored network map run.
    pub fn netmap_put(&self, ts: i64, body: &str) {
        let c = self.conn.lock().unwrap();
        let _ = c.execute("INSERT OR REPLACE INTO netmap_run(id, ts, body) VALUES(1, ?1, ?2)", params![ts, body]);
    }

    /// The stored network map run: when it was made, and its JSON.
    pub fn netmap_last(&self) -> Option<(i64, String)> {
        let c = self.rd();
        c.query_row("SELECT ts, body FROM netmap_run WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten()
    }

    /// Replace the whole prefix-to-network table with `rows` (prefix as a number, length, origin, whether several
    /// networks announce it), in one transaction: a failure leaves the old table.
    pub fn pfx2as_replace(&self, rows: impl Iterator<Item = (u32, u8, u32, bool)>) -> rusqlite::Result<usize> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM pfx2as", [])?;
        let mut n = 0;
        {
            let mut st = tx.prepare("INSERT INTO pfx2as(prefix, len, asn, moas) VALUES(?1,?2,?3,?4)")?;
            for (prefix, len, asn, moas) in rows {
                st.execute(params![prefix, len, asn, moas])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Hand every prefix-to-network row to `f`, on the read-only connection.
    pub fn pfx2as_each(&self, mut f: impl FnMut(u32, u8, u32, bool)) {
        let c = self.rd();
        let Ok(mut st) = c.prepare("SELECT prefix, len, asn, moas FROM pfx2as") else { return };
        let Ok(rows) = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))) else { return };
        for (p, l, a, m) in rows.flatten() {
            f(p, l, a, m);
        }
    }

    pub fn pfx2as_count(&self) -> usize {
        let c = self.rd();
        c.query_row("SELECT COUNT(*) FROM pfx2as", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }

    /// [`Store::pfx2as_replace`] for the IPv6 table: the prefix is the masked network as a number, stored as its 16
    /// big-endian bytes.
    pub fn pfx2as6_replace(&self, rows: impl Iterator<Item = (u128, u8, u32, bool)>) -> rusqlite::Result<usize> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM pfx2as6", [])?;
        let mut n = 0;
        {
            let mut st = tx.prepare("INSERT INTO pfx2as6(prefix, len, asn, moas) VALUES(?1,?2,?3,?4)")?;
            for (prefix, len, asn, moas) in rows {
                st.execute(params![&prefix.to_be_bytes()[..], len, asn, moas])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Hand every IPv6 prefix-to-network row to `f`, on the read-only connection.
    pub fn pfx2as6_each(&self, mut f: impl FnMut(u128, u8, u32, bool)) {
        let c = self.rd();
        let Ok(mut st) = c.prepare("SELECT prefix, len, asn, moas FROM pfx2as6") else { return };
        let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get(1)?, r.get(2)?, r.get(3)?))) else { return };
        for (p, l, a, m) in rows.flatten() {
            if let Ok(b) = <[u8; 16]>::try_from(p.as_slice()) {
                f(u128::from_be_bytes(b), l, a, m);
            }
        }
    }

    pub fn pfx2as6_count(&self) -> usize {
        let c = self.rd();
        c.query_row("SELECT COUNT(*) FROM pfx2as6", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }

    /// Replace the whole AS-relationship table with `rows` (a, b, rel: -1 = a provides for b, 0 = peers), in one
    /// transaction: a failure leaves the old table.
    pub fn asrel_replace(&self, rows: &[(u32, u32, i8)]) -> rusqlite::Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM asrel", [])?;
        {
            let mut st = tx.prepare("INSERT OR REPLACE INTO asrel(a, b, rel) VALUES(?1,?2,?3)")?;
            for (a, b, rel) in rows {
                st.execute(params![a, b, rel])?;
            }
        }
        tx.commit()
    }

    pub fn asrel_count(&self) -> usize {
        let c = self.rd();
        c.query_row("SELECT COUNT(*) FROM asrel", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }

    /// Every stored pair with at least one side in `asns`, as stored.
    pub fn asrel_touching(&self, asns: &[u32]) -> Vec<(u32, u32, i8)> {
        let mut out = Vec::new();
        let c = self.rd();
        // Chunked to stay under SQLite's variable limit.
        for chunk in asns.chunks(400) {
            let marks = vec!["?"; chunk.len()].join(",");
            let Ok(mut st) = c.prepare(&format!("SELECT a, b, rel FROM asrel WHERE a IN ({marks}) OR b IN ({marks})")) else { continue };
            let args = chunk.iter().chain(chunk.iter());
            let Ok(rows) = st.query_map(rusqlite::params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))) else { continue };
            out.extend(rows.flatten());
        }
        out.sort_unstable();
        out.dedup();
        out
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
    fn border_pairs_retire_and_samples_prune() {
        let path = temp_path();
        let store = Store::open(path.clone()).unwrap();
        let pair = |far: &str| Pair {
            id: 0,
            cdn: "fastly".into(),
            target: "151.101.1.1".parse().unwrap(),
            near: "198.51.100.9".parse().unwrap(),
            near_ttl: 3,
            far: far.parse().unwrap(),
            far_ttl: 4,
            far_asn: Some(64501),
            far_holder: Some("TRANSIT".into()),
            kind: Kind::Via,
            found_at: 1_000,
            retired_at: None,
        };
        let a = store.border_insert_pair(&pair("203.0.113.5"));
        store.border_retire(a, 2_000);
        // Retiring again leaves the first time.
        store.border_retire(a, 9_000);
        let b = store.border_insert_pair(&pair("203.0.113.6"));
        assert!(a > 0 && b > a);
        let all = store.border_pairs(0);
        assert_eq!(all.iter().map(|p| (p.id, p.retired_at)).collect::<Vec<_>>(), [(a, Some(2_000)), (b, None)]);
        assert_eq!((all[0].far.to_string(), all[0].kind, all[0].far_holder.as_deref()), ("203.0.113.5".into(), Kind::Via, Some("TRANSIT")));
        // Only the pair in use, past the retirement.
        assert_eq!(store.border_pairs(3_000).iter().map(|p| p.id).collect::<Vec<_>>(), [b]);

        let s = |ts: i64, pair: i64| BorderSample { ts, pair, near_ms: Some(5.0), far_ms: None };
        store.border_record(&[s(1_000, a), s(2_000, a), s(3_000, b)]);
        assert_eq!((store.border_latest(), store.border_samples(2_000).len()), (Some(3_000), 2));
        store.border_prune(2_500);
        assert_eq!(store.border_samples(0), [s(3_000, b)]);
        // The retired pair goes with its samples; the one in use stays.
        assert_eq!(store.border_pairs(0).iter().map(|p| p.id).collect::<Vec<_>>(), [b]);
        store.border_clear();
        assert!(store.border_pairs(0).is_empty() && store.border_latest().is_none());
        drop(store);
        remove(&path);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_database_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let top = std::env::temp_dir().join(format!("recursa-dir-{:x}", fastrand::u64(..)));
        let path = top.join("Recursa").join("recursa.db");
        drop(Store::open(path.clone()).unwrap());
        for dir in [&top, &top.join("Recursa")] {
            assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700, "{}", dir.display());
        }
        let _ = std::fs::remove_dir_all(&top);
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
        // Clear all takes every unsaved run except the one named; unsaving r2 then lets it go.
        assert_eq!(store.clear_runs(Some("r5")), 0);
        assert_eq!(store.clear_runs(None), 1);
        assert_eq!(ids(&store), ["r2"]);
        assert!(store.has_run("r2") && !store.has_run("r5"));
        assert!(store.set_run_saved("r2", false));
        assert_eq!(store.clear_runs(None), 1);
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
        // Its runs predate saving, so cleanup starts switched off.
        assert_eq!(store.setting("housekeeping"), Some(serde_json::json!({ "keep_runs": null, "keep_days": null })));
        drop(store);
        // Opening again leaves the column and the mark alone.
        let store = Store::open(path.clone()).unwrap();
        assert!(store.list_runs(10)[0].saved);
        remove(&path);
    }

    #[test]
    fn an_empty_or_new_database_keeps_the_default_cleanup() {
        let path = temp_path();
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE runs(id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, finished_at INTEGER,
                   status TEXT NOT NULL, summary TEXT NOT NULL, body TEXT NOT NULL);",
            )
            .unwrap();
        }
        assert_eq!(Store::open(path.clone()).unwrap().setting("housekeeping"), None);
        remove(&path);
        let path = temp_path();
        assert_eq!(Store::open(path.clone()).unwrap().setting("housekeeping"), None);
        remove(&path);
    }

    #[test]
    fn netmap_tables_replace_wholesale_and_keep_one_run() {
        let path = temp_path();
        let store = Store::open(path.clone()).unwrap();
        // The prefix table is swapped in one go: the second load leaves none of the first.
        assert_eq!(store.pfx2as_replace([(0x0800_0000, 8, 1, false), (0x0808_0800, 24, 15169, true)].into_iter()).unwrap(), 2);
        assert_eq!(store.pfx2as_replace([(0x0102_0300, 24, 7, false)].into_iter()).unwrap(), 1);
        let mut rows = Vec::new();
        store.pfx2as_each(|p, l, a, m| rows.push((p, l, a, m)));
        assert_eq!((store.pfx2as_count(), rows), (1, vec![(0x0102_0300, 24, 7, false)]));
        // The IPv6 table is the same, with 16-byte prefixes.
        let net6 = 0x2001_0db8_u128 << 96;
        assert_eq!(store.pfx2as6_replace([(net6, 32, 1, false), (net6 | (1 << 80), 48, 2, true)].into_iter()).unwrap(), 2);
        assert_eq!(store.pfx2as6_replace([(net6 | (1 << 80), 48, 7, true)].into_iter()).unwrap(), 1);
        let mut rows6 = Vec::new();
        store.pfx2as6_each(|p, l, a, m| rows6.push((p, l, a, m)));
        assert_eq!((store.pfx2as6_count(), rows6), (1, vec![(net6 | (1 << 80), 48, 7, true)]));
        // The relationship table is swapped whole too, and read by either side.
        store.asrel_replace(&[(9, 9, 0)]).unwrap();
        store.asrel_replace(&[(1299, 46887, -1), (2906, 46887, 0), (6461, 20055, -1)]).unwrap();
        assert_eq!(store.asrel_count(), 3);
        assert_eq!(store.asrel_touching(&[46887]), [(1299, 46887, -1), (2906, 46887, 0)]);
        assert_eq!(store.asrel_touching(&[1299, 20055]), [(1299, 46887, -1), (6461, 20055, -1)]);
        assert!(store.asrel_touching(&[7]).is_empty() && store.asrel_touching(&[]).is_empty());
        // Only the last run is kept.
        assert_eq!(store.netmap_last(), None);
        store.netmap_put(10, "{\"a\":1}");
        store.netmap_put(20, "{\"a\":2}");
        assert_eq!(store.netmap_last(), Some((20, "{\"a\":2}".to_string())));
        // The PeeringDB tables exist and start empty.
        for t in ["ix", "ixpfx", "netixlan", "fac", "netfac"] {
            let n: i64 = store.with_conn(|c| c.query_row(&format!("SELECT COUNT(*) FROM pdb_{t}"), [], |r| r.get(0)).unwrap());
            assert_eq!(n, 0, "{t}");
        }
        drop(store);
        remove(&path);
    }
}
