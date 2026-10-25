//! Route-origin validation on this computer (opt-in): is the resolver's prefix announced by an AS
//! that a ROA authorises? The answer comes from RPKI data Recursa fetches and checks itself.
//!
//! The user turns it on in Settings. Recursa then runs NLnet Labs' Routinator (as a library, the
//! `routinator` crate) once: the five RIR trust anchors, RRDP only, into `<data dir>/rpki/`. The
//! validated ROA payloads (VRPs) are kept as a compact table ([`table`]) in memory and in
//! `<data dir>/rpki/vrps.bin`. Lookups follow RFC 6811.
//! No VRP data is bundled; every byte is fetched by this computer from the RIRs' own repositories.
//!
//! A sync runs in a short-lived child process: the app starts its own executable with `--rpki-sync
//! <dir>` ([`child_entry`], called first thing in `main`). The validator needs about 1 GB while it
//! runs, which a process cannot give back, so the child holds it and exits. It prints progress lines
//! on stdout, writes `vrps.bin` (temp file, then rename) and prints `done`; the parent loads the table.
//! The parent ends the child if the setting goes off or a measured phase starts. If the parent itself
//! ends, the pipe on the child's stdin closes and the child exits, so none is left behind.
//!
//! Network calls (nothing else is contacted): the RRDP notification, snapshot and delta files that
//! the trust anchor locators (as shipped in the `routinator` crate, ARIN's included) lead to.
//! rsync is off: publication points reachable only over rsync are not read (see [`sync::Stats`]).
//!
//! A sync starts when the setting is turned on, and when the table is older than 12 hours and a
//! lookup needs it. It never starts while a measured phase runs (`measuring`), a running one is
//! ended if one begins, and lookups never wait for it: until the table is
//! ready the ROA cell says so ("Not checked" with the reason).
//!
//! # Removing the feature
//!
//! Delete this directory (`crates/engine/src/rpki/`), then remove these one-liners (each is gated
//! by `rpki-local` or sits next to a gated one):
//!
//! * `Cargo.toml` (workspace): the `routinator` line.
//! * `crates/engine/Cargo.toml`: the `routinator` and `log` dependencies, `rpki-local` in `[features]` and in `default`.
//! * `crates/cli/Cargo.toml`, `crates/desktop/Cargo.toml`: the `rpki-local` lines in `[features]` and in `default`.
//! * `crates/engine/src/lib.rs`: `#[cfg(all(feature = "rpki-local", not(target_os = "ios")))] pub mod rpki;`
//! * Everything above is also gated `not(target_os = "ios")` (no child processes there), and `routinator`/`log` sit under
//!   `[target.'cfg(not(target_os = "ios"))'.dependencies]` in `crates/engine/Cargo.toml`.
//! * `crates/cli/src/main.rs`, `crates/desktop/src/main.rs`: the `recursa_engine::rpki::child_entry();` line at the top of `main`.
//! * `crates/engine/src/intel.rs`: the `rpki` field of `Intel`, its line in `Intel::new`, and the
//!   `rpki()` method (its `not(feature)` arm is what a build without the module returns).
//! * `crates/engine/src/app.rs`: the `["rpki", ..]` arm of `AppState::call` and the `rpki_local` line in `AppState::env`.
//! * `web/src/routes/Settings.svelte`: the "Route origins" section and its script.
//! * `web/src/lib/types.ts`: the `rpki_local` member of `Env`, `RpkiStatus`; `web/src/lib/cells.ts` (`roaCell`)
//!   and `web/src/components/Inspector.svelte` keep working: they read the same JSON, with `not_checked`.
//! * `web/src/routes/Method.svelte` (the RPKI line), `README.md`, `CHANGELOG.md`, `web/src/legal/third-party-notices.txt`
//!   (regenerate with `tools/notices.py`).
//!
//! The setting `rpki_local` lives in the same SQLite file as the rest and can simply be dropped.

mod sync;
mod table;

use crate::app::{ApiError, AppState};
use crate::bench::now_ms;
use crate::store::Store;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use table::Table;

/// Data older than this is refreshed when a lookup needs it.
const REFRESH_MS: i64 = 12 * 3_600_000;
/// After a failed sync, lookups leave the network alone for this long. "Sync now" ignores it.
const RETRY_MS: i64 = 15 * 60_000;
/// At most this many covering ROAs are listed in a result.
const MAX_ROAS: usize = 12;
const SETTING: &str = "rpki_local";
const TABLE_FILE: &str = "vrps.bin";
const VALIDATOR_DIR: &str = "validator";
/// The longest one sync may take before its child is ended.
const SYNC_DEADLINE: Duration = Duration::from_secs(20 * 60);
/// The hidden first argument that makes a Recursa binary run one sync and exit.
const CHILD_ARG: &str = "--rpki-sync";

pub struct Rpki {
    dir: PathBuf,
    store: Arc<Store>,
    measuring: Arc<AtomicBool>,
    /// Bumped when the setting goes off: a sync that began earlier is ended.
    epoch: AtomicU64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The table file has been looked for.
    loaded: bool,
    table: Option<Arc<Table>>,
    /// When the table was synced, ms since the epoch.
    time: i64,
    syncing_since: Option<i64>,
    /// Publication points the running sync has met, for the progress line.
    points: u64,
    /// Why the last sync failed, and when; cleared by the next success.
    error: Option<(String, i64)>,
    stats: Option<sync::Stats>,
    disk_bytes: u64,
}

impl Rpki {
    /// `dir` is `<data dir>/rpki`. Nothing is read or created until it is needed.
    pub(crate) fn new(dir: PathBuf, store: Arc<Store>, measuring: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Rpki { dir, store, measuring, epoch: AtomicU64::new(0), state: Mutex::default() })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.store.setting(SETTING).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// Whether a sync is running in this process.
    pub(crate) fn syncing(&self) -> bool {
        self.state.lock().unwrap().syncing_since.is_some()
    }

    fn validator() -> String {
        format!("Recursa (local, routinator {})", sync::engine_version())
    }

    /// Reads the saved table the first time anything asks. The file is read and parsed without the lock held.
    fn load(&self) {
        if self.state.lock().unwrap().loaded {
            return;
        }
        let saved = std::fs::read(self.dir.join(TABLE_FILE)).ok().and_then(|b| Table::from_bytes(&b)).map(|t| (t, dir_size(&self.dir)));
        let mut s = self.state.lock().unwrap();
        if s.loaded {
            return;
        }
        s.loaded = true;
        if let Some(((t, time), disk)) = saved {
            s.table = Some(Arc::new(t));
            s.time = time;
            s.disk_bytes = disk;
        }
    }

    /// Turns the check on or off. Off deletes the cache; on starts the first sync.
    pub(crate) fn set_enabled(self: &Arc<Self>, on: bool) -> Value {
        self.store.set_setting(SETTING, &json!(on));
        if on {
            self.start_sync();
        } else {
            self.epoch.fetch_add(1, Ordering::SeqCst);
            let idle = {
                let mut s = self.state.lock().unwrap();
                s.loaded = true;
                s.table = None;
                s.error = None;
                s.stats = None;
                s.disk_bytes = 0;
                s.syncing_since.is_none()
            };
            // A sync in progress is ended within a moment and deletes the cache as it ends.
            if idle {
                self.delete_cache();
            }
        }
        self.status()
    }

    /// Removes what this module wrote, and the folder if nothing else is in it.
    fn delete_cache(&self) {
        let _ = std::fs::remove_dir_all(self.dir.join(VALIDATOR_DIR));
        for f in [TABLE_FILE, "vrps.bin.tmp", "routinator.conf"] {
            let _ = std::fs::remove_file(self.dir.join(f));
        }
        let _ = std::fs::remove_dir(&self.dir);
    }

    /// "Sync now": same as a refresh, whatever the age of the data.
    fn sync_now(self: &Arc<Self>) -> Result<Value, ApiError> {
        if !self.enabled() {
            return Err(ApiError::new(409, "Turn on the route-origin check first."));
        }
        if self.measuring.load(Ordering::SeqCst) || crate::lock::busy() {
            return Err(ApiError::new(409, "A measurement is running. Syncing waits until it ends."));
        }
        self.start_sync();
        Ok(self.status())
    }

    /// Starts a sync in the background unless one is running, the setting is off or a measured
    /// phase is under way. Returns whether one is running afterwards.
    fn start_sync(self: &Arc<Self>) -> bool {
        if !self.enabled() || self.measuring.load(Ordering::SeqCst) || crate::lock::busy() {
            return false;
        }
        {
            let mut s = self.state.lock().unwrap();
            if s.syncing_since.is_some() {
                return true;
            }
            s.syncing_since = Some(now_ms());
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("recursa-rpki".into()).spawn(move || me.sync_thread());
        if spawned.is_err() {
            self.state.lock().unwrap().syncing_since = None;
            return false;
        }
        true
    }

    fn sync_thread(&self) {
        loop {
            let epoch = self.epoch.load(Ordering::SeqCst);
            let stop = || self.epoch.load(Ordering::SeqCst) != epoch || self.measuring.load(Ordering::SeqCst);
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_child(&stop)));
            let disk = dir_size(&self.dir);
            let off = !self.enabled();
            // Stopped by an off that has since been turned on again: start over.
            let again = matches!(ran, Ok(Err(sync::Failure::Stopped))) && !off && !self.measuring.load(Ordering::SeqCst);
            let mut s = self.state.lock().unwrap();
            s.loaded = true;
            s.disk_bytes = disk;
            if again {
                continue;
            }
            s.syncing_since = None;
            s.points = 0;
            if off {
                s.table = None;
                s.error = None;
                s.stats = None;
                drop(s);
                self.delete_cache();
                return;
            }
            match ran {
                Ok(Ok((table, time, stats))) => {
                    s.table = Some(Arc::new(table));
                    s.time = time;
                    s.error = None;
                    s.stats = Some(stats);
                }
                Ok(Err(sync::Failure::Stopped)) => {}
                Ok(Err(sync::Failure::Failed(e))) => s.error = Some((e, now_ms())),
                Err(_) => s.error = Some(("The route-origin validator stopped unexpectedly".into(), now_ms())),
            }
            return;
        }
    }

    /// One sync in a child process. Polls `stop` and ends the child when it says so.
    fn run_child(&self, stop: &dyn Fn() -> bool) -> Result<(Table, i64, sync::Stats), sync::Failure> {
        let failed = |what: &str, e: &dyn std::fmt::Display| sync::Failure::Failed(format!("{what}: {e}"));
        std::fs::create_dir_all(&self.dir).map_err(|e| failed(&format!("Couldn't create {}", self.dir.display()), &e))?;
        let exe = std::env::current_exe().map_err(|e| failed("Couldn't find Recursa's own program", &e))?;
        let mut child = Command::new(exe)
            .arg(CHILD_ARG)
            .arg(&self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| failed("Couldn't start the route-origin validator", &e))?;
        // Held until the end of this function; the child exits when it closes (also if this process is killed).
        let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
        let child = Reaper(child);
        let Some(stdout) = stdout else { return Err(sync::Failure::Failed("Couldn't read from the route-origin validator".into())) };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut result = None;
        let deadline = Instant::now() + SYNC_DEADLINE;
        loop {
            if stop() {
                return Err(sync::Failure::Stopped);
            }
            // The child is killed and reaped as `child` drops.
            if Instant::now() >= deadline {
                return Err(sync::Failure::Failed("The route-origin sync took too long and was stopped".into()));
            }
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(line) => match line.split_once(' ') {
                    Some(("points", n)) => {
                        let disk = dir_size(&self.dir);
                        let mut s = self.state.lock().unwrap();
                        s.points = n.parse().unwrap_or(s.points);
                        s.disk_bytes = disk;
                    }
                    Some(("done", json)) => result = Some(serde_json::from_str::<sync::Stats>(json).map_err(|e| e.to_string())),
                    Some(("error", e)) => result = Some(Err(e.to_string())),
                    _ => {}
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        drop((child, stdin));
        match result {
            Some(Ok(stats)) => std::fs::read(self.dir.join(TABLE_FILE))
                .ok()
                .and_then(|b| Table::from_bytes(&b))
                .map(|(t, time)| (t, time, stats))
                .ok_or_else(|| sync::Failure::Failed("Couldn't read the saved table".into())),
            Some(Err(e)) => Err(sync::Failure::Failed(e)),
            None => Err(sync::Failure::Failed("The route-origin validator stopped unexpectedly".into())),
        }
    }

    /// State for the Settings page: off | syncing | ready | error, and the numbers behind it.
    pub(crate) fn status(&self) -> Value {
        let enabled = self.enabled();
        if enabled {
            self.load();
        }
        let s = self.state.lock().unwrap();
        let state = if !enabled {
            "off"
        } else if s.syncing_since.is_some() {
            "syncing"
        } else if s.table.is_some() {
            "ready"
        } else if s.error.is_some() {
            "error"
        } else {
            // On, nothing synced yet and nothing running: waiting for a measurement to end or for first use.
            "syncing"
        };
        json!({
            "enabled": enabled,
            "state": state,
            "waiting": enabled && s.syncing_since.is_none() && s.table.is_none() && s.error.is_none(),
            "last_sync": s.table.as_ref().map(|_| s.time),
            "vrps": s.table.as_ref().map_or(0, |t| t.len()),
            "disk_bytes": s.disk_bytes,
            "syncing_since": s.syncing_since,
            "points": s.points,
            "error": s.error.as_ref().map(|(e, _)| e),
            "validator": Self::validator(),
            "stats": s.stats,
        })
    }

    /// The `rpki` member of an address's intel: the RFC 6811 state of `prefix` announced by AS`asn`,
    /// in the shape the UI reads. Never waits for a sync and never fails.
    pub(crate) fn lookup(self: &Arc<Self>, asn: u64, prefix: &str) -> Value {
        if !self.enabled() {
            return not_checked("off", "Off. It can be turned on in Settings.");
        }
        self.load();
        let (table, time, syncing, retry_at) = {
            let s = self.state.lock().unwrap();
            (s.table.clone(), s.time, s.syncing_since.is_some(), s.error.as_ref().map(|(_, at)| at + RETRY_MS))
        };
        let now = now_ms();
        let due = table.is_none() || now - time > REFRESH_MS;
        let syncing = syncing || (due && retry_at.is_none_or(|t| now >= t) && self.start_sync());
        match table {
            Some(t) => verdict(&t, time, asn, prefix),
            None if syncing => not_checked("syncing", "Syncing route-origin data…"),
            None => {
                let s = self.state.lock().unwrap();
                match &s.error {
                    Some((e, _)) => not_checked("error", &format!("Couldn’t sync route-origin data: {e}")),
                    None => not_checked("waiting", "Route-origin data syncs once the measurement ends."),
                }
            }
        }
    }
}

/// Ends the child process, and reaps it, when dropped.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Call this first thing in `main` of every binary that can run a sync (the desktop app and the
/// CLI), before anything else starts. If this process was started as the sync child it runs the
/// sync and exits; otherwise it returns at once.
pub fn child_entry() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|a| a != CHILD_ARG) {
        return;
    }
    let dir = PathBuf::from(args.next().unwrap_or_default());
    crate::install_crypto_provider();
    // The parent holds our stdin open. When it ends or goes away the pipe closes, and so do we.
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        std::process::exit(1);
    });
    let points = Arc::new(AtomicU64::new(0));
    let p = points.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            say(&format!("points {}", p.load(Ordering::Relaxed)));
        }
    });
    // The validator uses blocking I/O: run it on a thread of its own, outside any async runtime the caller made.
    let result = std::thread::spawn(move || child_run(&dir, &points)).join();
    let code = match result {
        Ok(Ok(stats)) => {
            say(&format!("done {}", serde_json::to_string(&stats).unwrap_or_default()));
            0
        }
        Ok(Err(sync::Failure::Failed(e))) => {
            say(&format!("error {e}"));
            1
        }
        _ => {
            say("error The route-origin validator stopped unexpectedly");
            1
        }
    };
    std::process::exit(code);
}

/// One line on stdout for the parent. A closed pipe is the parent going away: the stdin thread ends us.
fn say(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout().lock(), "{}", line.replace(['\n', '\r'], " "));
}

fn child_run(dir: &Path, points: &AtomicU64) -> Result<sync::Stats, sync::Failure> {
    std::fs::create_dir_all(dir).map_err(|e| sync::Failure::Failed(format!("Couldn't create {}: {e}", dir.display())))?;
    let out = sync::run(dir, points)?;
    let table = Table::new(out.vrps);
    // Write beside the target and rename, so a crash never leaves half a table.
    let tmp = dir.join("vrps.bin.tmp");
    std::fs::write(&tmp, table.to_bytes(now_ms()))
        .and_then(|_| std::fs::rename(&tmp, dir.join(TABLE_FILE)))
        .map_err(|e| sync::Failure::Failed(format!("Couldn't save the table: {e}")))?;
    Ok(out.stats)
}

/// The `rpki` JSON when there is no verdict. `status` is `not_checked`; `state` says why in a word.
fn not_checked(state: &str, reason: &str) -> Value {
    json!({ "status": "not_checked", "state": state, "reason": reason, "roas": [], "validator": null })
}

/// RFC 6811 for `prefix` (CIDR text) announced by AS`asn`: the status, the covering ROAs (origin, prefix, max length and
/// validity each), and the validator and the time of the data it used.
fn verdict(table: &Table, time: i64, asn: u64, prefix: &str) -> Value {
    let route = prefix.split_once('/').and_then(|(a, l)| Some((a.parse::<IpAddr>().ok()?, l.parse::<u8>().ok()?)));
    let (Some((addr, len)), Ok(origin)) = (route, u32::try_from(asn)) else {
        return not_checked("error", "The route’s prefix or origin AS could not be read.");
    };
    let v = table.validate(addr, len, origin);
    let roas: Vec<Value> = v
        .covering
        .iter()
        .take(MAX_ROAS)
        .map(|(vrp, m)| json!({ "origin": vrp.asn.to_string(), "prefix": format!("{}/{}", vrp.addr, vrp.len), "max_length": vrp.max_len, "validity": m.as_str() }))
        .collect();
    json!({ "status": v.status, "state": "ready", "roas": roas, "validator": Rpki::validator(), "vrp_time": time })
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// `GET /api/rpki` (status), `PUT /api/rpki` (`{ "enabled": bool }`), `POST /api/rpki/sync`.
pub async fn call(app: &Arc<AppState>, method: &str, segs: &[&str], body: Value) -> Result<String, ApiError> {
    let rpki = &app.intel.rpki;
    let ok = |v: Value| Ok(v.to_string());
    match (method, segs) {
        ("GET", []) => {
            let r = rpki.clone();
            ok(tokio::task::spawn_blocking(move || r.status()).await.map_err(|e| ApiError::new(500, e.to_string()))?)
        }
        ("PUT", []) => {
            let on = body["enabled"].as_bool().ok_or_else(|| ApiError::new(400, "Invalid request: `enabled` is required."))?;
            let r = rpki.clone();
            ok(tokio::task::spawn_blocking(move || r.set_enabled(on)).await.map_err(|e| ApiError::new(500, e.to_string()))?)
        }
        ("POST", ["sync"]) => ok(rpki.sync_now()?),
        _ => Err(ApiError::new(404, "No such endpoint.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use table::{Match, Vrp};

    fn vrp(prefix: &str, max_len: u8, asn: u32) -> Vrp {
        let (a, l) = prefix.split_once('/').unwrap();
        Vrp { addr: a.parse().unwrap(), len: l.parse().unwrap(), max_len, asn }
    }

    fn check(t: &Table, route: &str, origin: u32) -> &'static str {
        let (a, l) = route.split_once('/').unwrap();
        t.validate(a.parse().unwrap(), l.parse().unwrap(), origin).status
    }

    fn sample() -> Table {
        Table::new([
            vrp("1.1.1.0/24", 24, 13335),
            vrp("10.0.0.0/8", 16, 64500),
            vrp("10.1.0.0/16", 24, 64501),
            vrp("10.1.0.0/16", 16, 64502),
            vrp("2606:4700::/32", 48, 13335),
            vrp("192.0.2.0/24", 24, 0),
        ])
    }

    #[test]
    fn a_matching_origin_within_max_length_is_valid() {
        let t = sample();
        assert_eq!(check(&t, "1.1.1.0/24", 13335), "valid");
        assert_eq!(check(&t, "10.0.0.0/16", 64500), "valid");
    }

    #[test]
    fn another_origin_is_invalid_asn() {
        let t = sample();
        assert_eq!(check(&t, "1.1.1.0/24", 64999), "invalid_asn");
        // AS0 authorises nobody, even AS0 announcing.
        assert_eq!(check(&t, "192.0.2.0/24", 64999), "invalid_asn");
        assert_eq!(check(&t, "192.0.2.0/24", 0), "invalid_asn");
    }

    #[test]
    fn a_route_longer_than_max_length_is_invalid_length() {
        let t = sample();
        assert_eq!(check(&t, "1.1.1.0/25", 13335), "invalid_length");
        assert_eq!(check(&t, "10.0.0.0/17", 64500), "invalid_length");
    }

    #[test]
    fn a_route_no_roa_covers_is_not_found() {
        let t = sample();
        assert_eq!(check(&t, "8.8.8.0/24", 15169), "unknown");
        // Shorter than the ROA's prefix: not covered by it.
        assert_eq!(check(&t, "1.1.0.0/16", 13335), "unknown");
        assert_eq!(check(&t, "2001:db8::/32", 64500), "unknown");
    }

    #[test]
    fn with_several_covering_roas_any_match_makes_it_valid() {
        let t = sample();
        // 10.1.0.0/16 is covered by the /8 (AS64500) and two /16 ROAs.
        let v = t.validate("10.1.0.0".parse().unwrap(), 16, 64502);
        assert_eq!(v.status, "valid");
        assert_eq!(v.covering.len(), 3);
        assert_eq!((v.covering[0].0.asn, v.covering[0].1), (64502, Match::Valid));
        // A /20 from AS64501 fits that ROA's max length of 24 even though the /8's would not allow it.
        assert_eq!(check(&t, "10.1.16.0/20", 64501), "valid");
        // From AS64502 the /20 is too long for its ROA, and nothing else matches the AS.
        assert_eq!(check(&t, "10.1.16.0/20", 64502), "invalid_length");
        assert_eq!(check(&t, "10.1.16.0/20", 64999), "invalid_asn");
    }

    #[test]
    fn ipv6_routes_are_validated_the_same_way() {
        let t = sample();
        assert_eq!(check(&t, "2606:4700::/32", 13335), "valid");
        assert_eq!(check(&t, "2606:4700:4700::/48", 13335), "valid");
        assert_eq!(check(&t, "2606:4700:4700::/64", 13335), "invalid_length");
        assert_eq!(check(&t, "2606:4700::/32", 64500), "invalid_asn");
        // An IPv4 ROA never covers an IPv6 route.
        assert_eq!(check(&t, "::/0", 13335), "unknown");
    }

    #[test]
    fn the_table_survives_the_file_round_trip() {
        let t = sample();
        let (back, time) = Table::from_bytes(&t.to_bytes(42)).unwrap();
        assert_eq!((back.len(), time), (t.len(), 42));
        assert_eq!(check(&back, "2606:4700:4700::/48", 13335), "valid");
        let mut cut = t.to_bytes(42);
        cut.pop();
        assert!(Table::from_bytes(&cut).is_none());
        assert!(Table::from_bytes(b"not a table").is_none());
        // A count far past what the file holds is refused before anything is reserved for it.
        let mut huge = t.to_bytes(42);
        huge[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Table::from_bytes(&huge).is_none());
    }

    #[test]
    fn the_result_keeps_the_shape_the_ui_reads() {
        let v = verdict(&sample(), 1234, 13335, "1.1.1.0/24");
        assert_eq!(v["status"], "valid");
        assert_eq!(v["roas"][0], json!({ "origin": "13335", "prefix": "1.1.1.0/24", "max_length": 24, "validity": "valid" }));
        assert!(v["validator"].as_str().unwrap().starts_with("Recursa (local, routinator "));
        assert_eq!(v["vrp_time"], 1234);
        let none = verdict(&sample(), 1234, 15169, "8.8.8.0/24");
        assert_eq!((none["status"].as_str(), none["roas"].as_array().map(Vec::len)), (Some("unknown"), Some(0)));
        assert_eq!(verdict(&sample(), 1, 13335, "garbage")["status"], "not_checked");
    }

    fn rpki_in(dir: &Path, enabled: bool, measuring: bool) -> Arc<Rpki> {
        let store = Arc::new(Store::open(dir.join("test.db")).unwrap());
        store.set_setting(SETTING, &json!(enabled));
        Rpki::new(dir.join("rpki"), store, Arc::new(AtomicBool::new(measuring)))
    }

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!("recursa-rpki-test-{:x}", fastrand::u64(..)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn off_syncing_and_ready_return_the_right_cells() {
        let dir = scratch();
        // Off: a reason, never a verdict.
        let off = rpki_in(&dir, false, false);
        let v = off.lookup(13335, "1.1.1.0/24");
        assert_eq!((v["status"].as_str(), v["state"].as_str()), (Some("not_checked"), Some("off")));
        assert_eq!(off.status()["state"], "off");

        // On, nothing synced, a measurement running (so no sync starts, and no network is used).
        let wait = rpki_in(&dir, true, true);
        let v = wait.lookup(13335, "1.1.1.0/24");
        assert_eq!((v["status"].as_str(), v["state"].as_str()), (Some("not_checked"), Some("waiting")));
        assert_eq!(wait.status()["waiting"], true);

        // A sync in progress: the cell says so and lookups do not block.
        wait.state.lock().unwrap().syncing_since = Some(now_ms());
        let v = wait.lookup(13335, "1.1.1.0/24");
        assert_eq!((v["status"].as_str(), v["reason"].as_str()), (Some("not_checked"), Some("Syncing route-origin data…")));
        assert_eq!(wait.status()["state"], "syncing");

        // Ready, from a saved table that is fresh.
        std::fs::create_dir_all(dir.join("rpki")).unwrap();
        std::fs::write(dir.join("rpki").join(TABLE_FILE), sample().to_bytes(now_ms())).unwrap();
        let ready = rpki_in(&dir, true, true);
        let v = ready.lookup(13335, "1.1.1.0/24");
        assert_eq!((v["status"].as_str(), v["state"].as_str()), (Some("valid"), Some("ready")));
        let st = ready.status();
        assert_eq!((st["state"].as_str(), st["vrps"].as_u64()), (Some("ready"), Some(6)));
        assert!(st["last_sync"].as_i64().unwrap() > 0 && st["disk_bytes"].as_u64().unwrap() > 0);

        // A failed sync with no table: the reason, and no retry storm.
        let failed = rpki_in(&scratch(), true, true);
        failed.state.lock().unwrap().error = Some(("boom".into(), now_ms()));
        let v = failed.lookup(13335, "1.1.1.0/24");
        assert_eq!(v["state"], "error");
        assert!(v["reason"].as_str().unwrap().contains("boom"));
        assert_eq!(failed.status()["state"], "error");

        // Off deletes the cache.
        ready.set_enabled(false);
        assert!(!dir.join("rpki").exists());
        assert_eq!(ready.lookup(13335, "1.1.1.0/24")["state"], "off");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
