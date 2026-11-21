//! Route-origin validation (on by default): is the resolver's prefix announced by an AS that a ROA authorises?
//!
//! Recursa does not validate anything itself. It downloads two tables that public validators have already built
//! from the RIRs' repositories, the rpki-client console's and RIPE NCC's ([`feeds::DEFS`]), keeps each as a compact
//! table ([`table`]) in memory and in `<data dir>/rpki/feed-<key>.bin`, and answers with a verdict (RFC 6811) only
//! when both tables give the same one. A table is used only while its build time is under 24 hours old.
//!
//! Network calls (nothing else is contacted): one request to each of the two hosts per sync, gzip-compressed (about
//! 5 MB each). The hosts see this computer's address, not which resolvers are checked. The JSON is read while it
//! streams in and never stored.
//!
//! A sync starts when the app opens with a table missing or too old to use ([`Rpki::prefetch`]), on the first lookup
//! or Settings status that needs it, when the check is turned on, on "Sync now", and when a table was last fetched
//! over 12 hours ago and a lookup needs it. It never starts while a measured phase runs (`measuring`), a running one
//! is ended if one begins, and lookups never wait for it: until the tables are ready the ROA cell says so ("Not
//! checked" with the reason). The one wait is [`Rpki::wait_for_tables`], which a run makes once before its
//! network-facts lookups so a first run is not left at "Syncing…".
//!
//! The setting is `rpki_check` (absent means on). Builds that ran their own validator left files in the same folder,
//! which are deleted the first time this module is used ([`purge_old`]).

mod feeds;
mod table;

use crate::app::{ApiError, AppState};
use crate::bench::now_ms;
use crate::store::Store;
use feeds::{DEFS, Failure, MAX_AGE_MS, Reply};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use table::Table;

/// Data fetched longer ago than this is fetched again when a lookup needs it.
const REFRESH_MS: i64 = 12 * 3_600_000;
/// After a failed sync, lookups leave the network alone for this long. "Sync now" ignores it.
const RETRY_MS: i64 = 15 * 60_000;
/// How long a run waits for a first download.
const WAIT: Duration = Duration::from_secs(60);
/// At most this many covering ROAs are listed in a result.
const MAX_ROAS: usize = 12;
const SETTING: &str = "rpki_check";
/// The `Last-Modified` of each accepted download, by source key.
const FEED_STATE: &str = "rpki_feed_state";
/// What a result names as its validator.
const VALIDATOR: &str = "rpki-client console and RIPE NCC";
/// What builds with their own validator left in the data folder, beside `validator/`.
const OLD_FILES: [&str; 3] = ["vrps.bin", "vrps.bin.tmp", "routinator.conf"];

/// One request for a source, given the `Last-Modified` of the table held.
type Fetch<'a> = dyn Fn(&feeds::Def, Option<&str>) -> Result<Reply, Failure> + 'a;

pub struct Rpki {
    dir: PathBuf,
    store: Arc<Store>,
    measuring: Arc<AtomicBool>,
    /// Bumped when the setting goes off: a sync that began earlier is ended.
    epoch: AtomicU64,
    state: Mutex<State>,
    /// Held for reading while a sync runs; Settings › Data's clears take it for writing (see [`crate::intel::Intel`]).
    fetches: Arc<tokio::sync::RwLock<()>>,
    /// The fewest entries a download may have.
    floor: usize,
    /// The old validator's files have been looked for.
    purged: AtomicBool,
}

/// One source's table and what is known about it.
#[derive(Default)]
struct Source {
    table: Option<Arc<Table>>,
    /// When the table was built, ms since the epoch.
    built: i64,
    /// When it was last fetched (or confirmed unchanged), ms since the epoch.
    fetched: i64,
    /// Why the last fetch failed, and when; cleared by the next success.
    error: Option<(String, i64)>,
    /// The `Last-Modified` of the download the table came from.
    last_modified: Option<String>,
}

impl Source {
    /// Whether the table may be used: it is there and was built under 24 hours ago.
    fn usable(&self, now: i64) -> bool {
        self.table.is_some() && now - self.built < MAX_AGE_MS
    }
}

#[derive(Default)]
struct State {
    /// The table files have been looked for.
    loaded: bool,
    /// In the order of [`DEFS`].
    sources: [Source; 2],
    syncing_since: Option<i64>,
    disk_bytes: u64,
}

impl Rpki {
    /// `dir` is `<data dir>/rpki`. Nothing is read or created until it is needed.
    pub(crate) fn new(dir: PathBuf, store: Arc<Store>, measuring: Arc<AtomicBool>, fetches: Arc<tokio::sync::RwLock<()>>) -> Arc<Self> {
        Self::with_floor(dir, store, measuring, fetches, feeds::MIN_ENTRIES)
    }

    fn with_floor(
        dir: PathBuf,
        store: Arc<Store>,
        measuring: Arc<AtomicBool>,
        fetches: Arc<tokio::sync::RwLock<()>>,
        floor: usize,
    ) -> Arc<Self> {
        Arc::new(Rpki {
            dir,
            store,
            measuring,
            epoch: AtomicU64::new(0),
            state: Mutex::default(),
            fetches,
            floor,
            purged: AtomicBool::new(false),
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.store.get::<bool>(SETTING).unwrap_or(true)
    }

    /// Whether a sync is running in this process.
    pub(crate) fn syncing(&self) -> bool {
        self.state.lock().unwrap().syncing_since.is_some()
    }

    /// The first time anything uses the module: deletes what the built-in validator of older builds left, on a thread
    /// of its own, whether the check is on or off.
    fn touch(&self) {
        if self.purged.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.dir.join("validator").exists() || OLD_FILES.iter().any(|f| self.dir.join(f).exists()) {
            let dir = self.dir.clone();
            let _ = std::thread::Builder::new().name("recursa-rpki-purge".into()).spawn(move || purge_old(&dir));
        }
    }

    /// Reads the saved tables the first time anything asks. The files are read and parsed without the lock held.
    fn load(&self) {
        if self.state.lock().unwrap().loaded {
            return;
        }
        // A download cut off mid-write (the process ended) leaves its temporary file.
        for d in &DEFS {
            let _ = std::fs::remove_file(self.dir.join(format!("{}.tmp", d.file())));
        }
        let saved = DEFS.map(|d| {
            let path = self.dir.join(d.file());
            let (table, built) = Table::from_bytes(&std::fs::read(&path).ok()?)?;
            let fetched = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let fetched =
                fetched.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
            Some((table, built, fetched))
        });
        let marks: BTreeMap<String, String> = self.store.get(FEED_STATE).unwrap_or_default();
        let disk = self.disk();
        let mut s = self.state.lock().unwrap();
        if s.loaded {
            return;
        }
        s.loaded = true;
        s.disk_bytes = disk;
        for ((src, got), def) in s.sources.iter_mut().zip(saved).zip(&DEFS) {
            if let Some((table, built, fetched)) = got {
                *src = Source { table: Some(Arc::new(table)), built, fetched, error: None, last_modified: marks.get(def.key).cloned() };
            }
        }
    }

    /// For a run, before its network-facts lookups: if a table is missing or too old to use, starts a sync (as a lookup
    /// would) and waits for it to end, up to 60 seconds. Returns at once when the check is off, both tables are usable,
    /// a measured phase blocks syncing or a failed sync is still in its retry pause. Blocks the calling thread.
    pub(crate) fn wait_for_tables(self: &Arc<Self>) {
        self.wait_up_to(WAIT);
    }

    /// At app start: starts the sync now if a table is missing or too old to use, so the first run or lookup finds the
    /// tables there. Does not wait for it, but reads the saved tables on the calling thread.
    pub(crate) fn prefetch(self: &Arc<Self>) {
        self.wait_up_to(Duration::ZERO);
    }

    fn wait_up_to(self: &Arc<Self>, bound: Duration) {
        self.touch();
        if !self.enabled() {
            return;
        }
        self.load();
        if !self.sync_if_unusable() {
            return;
        }
        let end = Instant::now() + bound;
        while self.syncing() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// With the tables loaded: if one is missing or too old to use, starts a sync unless a failed one is still in its
    /// retry pause. Returns whether the tables are unusable and a sync is running.
    fn sync_if_unusable(self: &Arc<Self>) -> bool {
        let now = now_ms();
        let (usable, syncing, retry_at) = {
            let s = self.state.lock().unwrap();
            let retry_at = s.sources.iter().filter_map(|src| src.error.as_ref().map(|(_, at)| at + RETRY_MS)).max();
            (s.sources.iter().all(|src| src.usable(now)), s.syncing_since.is_some(), retry_at)
        };
        !usable && (syncing || (retry_at.is_none_or(|t| now >= t) && self.start_sync()))
    }

    /// The bytes of the saved tables.
    fn disk(&self) -> u64 {
        DEFS.iter().filter_map(|d| std::fs::metadata(self.dir.join(d.file())).ok()).map(|m| m.len()).sum()
    }

    /// Turns the check on or off. Off deletes the cache; on starts the first sync.
    pub(crate) fn set_enabled(self: &Arc<Self>, on: bool) -> Value {
        self.touch();
        self.store.put(SETTING, &on);
        if on {
            self.start_sync();
        } else {
            self.turn_off();
            self.store.put(FEED_STATE, &json!({}));
        }
        self.status()
    }

    /// Forget the tables and delete the cache, leaving the setting as it is: Clear All Data has already dropped it.
    pub(crate) fn turn_off(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let idle = {
            let mut s = self.state.lock().unwrap();
            s.loaded = true;
            s.sources = Default::default();
            s.disk_bytes = 0;
            s.syncing_since.is_none()
        };
        // A sync in progress is ended within a moment and deletes the cache as it ends.
        if idle {
            self.delete_cache();
        }
    }

    /// Removes what this module wrote, and the folder if nothing else is in it.
    fn delete_cache(&self) {
        for d in &DEFS {
            let file = d.file();
            let _ = std::fs::remove_file(self.dir.join(&file));
            let _ = std::fs::remove_file(self.dir.join(format!("{file}.tmp")));
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
        // A clear is emptying the data folder: nothing starts until it is done.
        let Ok(fetching) = self.fetches.clone().try_read_owned() else {
            return false;
        };
        {
            let mut s = self.state.lock().unwrap();
            if s.syncing_since.is_some() {
                return true;
            }
            s.syncing_since = Some(now_ms());
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("recursa-rpki".into()).spawn(move || {
            let _fetching = fetching;
            me.sync_thread()
        });
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
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let http = feeds::Http::new();
                self.run_sync(&stop, &|def, since| match &http {
                    Ok(h) => h.fetch(def, since, &stop),
                    Err(e) => Err(Failure::Failed(e.clone())),
                })
            }));
            let disk = self.disk();
            let off = !self.enabled();
            // Stopped by an off that has since been turned on again: start over.
            let again = matches!(ran, Ok(false)) && !off && !self.measuring.load(Ordering::SeqCst);
            let mut s = self.state.lock().unwrap();
            s.loaded = true;
            s.disk_bytes = disk;
            if again {
                continue;
            }
            s.syncing_since = None;
            if off {
                s.sources = Default::default();
                drop(s);
                self.delete_cache();
                self.store.put(FEED_STATE, &json!({}));
                return;
            }
            if ran.is_err() {
                let now = now_ms();
                for src in &mut s.sources {
                    src.error = Some(("The download stopped unexpectedly".into(), now));
                }
                return;
            }
            let marks: BTreeMap<&str, &String> = DEFS
                .iter()
                .zip(&s.sources)
                .filter(|(_, src)| src.table.is_some())
                .filter_map(|(d, src)| Some((d.key, src.last_modified.as_ref()?)))
                .collect();
            let marks = json!(marks);
            drop(s);
            self.store.put(FEED_STATE, &marks);
            return;
        }
    }

    /// One fetch of each source, whatever `fetch` returns for `(source, Last-Modified of the table held)`. One failing
    /// does not discard the other's result. Returns false if `stop` ended it early.
    fn run_sync(&self, stop: &dyn Fn() -> bool, fetch: &Fetch<'_>) -> bool {
        // What is saved is read first, so the downloads can be conditional and nothing read later replaces them.
        self.load();
        for (i, def) in DEFS.iter().enumerate() {
            if stop() {
                return false;
            }
            let (since, previous, held) = {
                let s = self.state.lock().unwrap();
                let src = &s.sources[i];
                // Only a table that is held can stand in for an answer of "not modified".
                (
                    src.table.as_ref().and(src.last_modified.clone()),
                    src.table.as_ref().map(|t| t.len()),
                    src.table.as_ref().map(|_| src.built),
                )
            };
            let got = match fetch(def, since.as_deref()) {
                Ok(Reply::NotModified) if held.is_none() => Err("The server sent no table".to_string()),
                Ok(Reply::NotModified) => Ok(None),
                Ok(Reply::Body { bytes, gzip, last_modified }) => {
                    feeds::parse(def, &bytes, gzip).and_then(|p| feeds::accept(p, previous, now_ms(), self.floor)).and_then(
                        |(table, built)| {
                            // A source's servers can be a build apart: an older one never replaces the table held.
                            if held.is_some_and(|h| built < h) {
                                return Ok(None);
                            }
                            self.save(&def.file(), &table, built).map(|()| Some((table, built, last_modified)))
                        },
                    )
                }
                Err(Failure::Stopped) => return false,
                Err(Failure::Failed(e)) => Err(e),
            };
            if stop() {
                return false;
            }
            let now = now_ms();
            let mut s = self.state.lock().unwrap();
            let src = &mut s.sources[i];
            match got {
                Ok(Some((table, built, last_modified))) => {
                    *src = Source { table: Some(Arc::new(table)), built, fetched: now, error: None, last_modified };
                }
                Ok(None) => {
                    src.fetched = now;
                    src.error = None;
                    // The file's age is what a restart takes for the time of the last fetch.
                    let path = self.dir.join(def.file());
                    let _ = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
                }
                Err(e) => src.error = Some((e, now)),
            }
        }
        true
    }

    /// Writes a table beside its file and renames it over, so a crash never leaves half a table.
    fn save(&self, file: &str, table: &Table, built: i64) -> Result<(), String> {
        let tmp = self.dir.join(format!("{file}.tmp"));
        std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(&tmp, table.to_bytes(built)))
            .and_then(|()| std::fs::rename(&tmp, self.dir.join(file)))
            .map_err(|e| format!("Couldn't save the table: {e}"))
    }

    /// State for the Settings page: off | syncing | ready | error, and the numbers behind it. Like a lookup, it starts
    /// the download when a table is missing or too old to use.
    pub(crate) fn status(self: &Arc<Self>) -> Value {
        self.touch();
        let enabled = self.enabled();
        if enabled {
            self.load();
            self.sync_if_unusable();
        }
        let now = now_ms();
        let s = self.state.lock().unwrap();
        let errors: Vec<String> =
            DEFS.iter().zip(&s.sources).filter_map(|(d, src)| src.error.as_ref().map(|(e, _)| format!("{}: {e}", d.name))).collect();
        let ready = s.sources.iter().all(|src| src.usable(now));
        let state = if !enabled {
            "off"
        } else if s.syncing_since.is_some() {
            "syncing"
        } else if ready {
            "ready"
        } else if !errors.is_empty() {
            "error"
        } else {
            // On, nothing synced yet and nothing running: waiting for a measurement to end.
            "syncing"
        };
        let primary = &s.sources[0];
        let sources: Vec<Value> = DEFS
            .iter()
            .zip(&s.sources)
            .map(|(d, src)| {
                json!({
                    "key": d.key,
                    "name": d.name,
                    "engine": d.engine,
                    "build_time": src.table.as_ref().map(|_| src.built),
                    "vrps": src.table.as_ref().map_or(0, |t| t.len()),
                    "fetched": src.table.as_ref().map(|_| src.fetched),
                    "error": src.error.as_ref().map(|(e, _)| e),
                })
            })
            .collect();
        json!({
            "enabled": enabled,
            "state": state,
            "waiting": enabled && s.syncing_since.is_none() && !ready && errors.is_empty(),
            "last_sync": primary.table.as_ref().map(|_| primary.fetched),
            "vrps": primary.table.as_ref().map_or(0, |t| t.len()),
            "disk_bytes": s.disk_bytes,
            "syncing_since": s.syncing_since,
            "error": (!errors.is_empty()).then(|| errors.join("; ")),
            "validator": VALIDATOR,
            "sources": sources,
        })
    }

    /// The `rpki` member of an address's intel: the RFC 6811 state of `prefix` announced by AS`asn`,
    /// in the shape the UI reads. Never waits for a sync and never fails.
    pub(crate) fn lookup(self: &Arc<Self>, asn: u64, prefix: &str) -> Value {
        self.touch();
        if !self.enabled() {
            return not_checked("off", "Off. It can be turned on in Settings.");
        }
        self.load();
        let now = now_ms();
        let (usable, syncing, retry_at, due) = {
            let s = self.state.lock().unwrap();
            let usable = s.sources.each_ref().map(|src| src.table.clone().filter(|_| src.usable(now)).map(|t| (t, src.built)));
            let retry_at = s.sources.iter().filter_map(|src| src.error.as_ref().map(|(_, at)| at + RETRY_MS)).max();
            let due = s.sources.iter().any(|src| src.table.is_none() || now - src.fetched > REFRESH_MS);
            (usable, s.syncing_since.is_some(), retry_at, due)
        };
        let syncing = syncing || (due && retry_at.is_none_or(|t| now >= t) && self.start_sync());
        if let [Some((a, a_built)), Some((b, b_built))] = &usable {
            return verdict((a.as_ref(), *a_built), (b.as_ref(), *b_built), asn, prefix);
        }
        if syncing {
            return not_checked("syncing", "Syncing route-origin data…");
        }
        let s = self.state.lock().unwrap();
        let why: Vec<String> = DEFS
            .iter()
            .zip(&s.sources)
            .filter(|(_, src)| !src.usable(now))
            .filter_map(|(d, src)| match (&src.error, &src.table) {
                (Some((e, _)), _) => Some(format!("{}: {e}", d.name)),
                (None, Some(_)) => Some(format!("{}: its data is over 24 hours old", d.name)),
                (None, None) => None,
            })
            .collect();
        let blocked = self.measuring.load(Ordering::SeqCst) || crate::lock::busy();
        let has_error = s.sources.iter().any(|src| !src.usable(now) && src.error.is_some());
        if why.is_empty() || (blocked && !has_error) {
            not_checked("waiting", "Route-origin data syncs once the measurement ends.")
        } else {
            not_checked("error", &format!("Couldn’t get route-origin data. {}", why.join("; ")))
        }
    }
}

/// Deletes what the built-in validator of older builds left under `<data dir>/rpki/`.
fn purge_old(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.join("validator"));
    for f in OLD_FILES {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

/// The `rpki` JSON when there is no verdict. `status` is `not_checked`; `state` says why in a word.
fn not_checked(state: &str, reason: &str) -> Value {
    json!({ "status": "not_checked", "state": state, "reason": reason, "roas": [], "validator": null })
}

/// A state in words, for the reason two sources disagree.
fn say(status: &str) -> &str {
    match status {
        "valid" => "valid",
        "invalid_asn" => "invalid (another origin AS)",
        "invalid_length" => "invalid (longer than the maximum length)",
        _ => "not found",
    }
}

/// RFC 6811 for `prefix` (CIDR text) announced by AS`asn` against both tables (each with its build time): a verdict
/// if they agree, else `disagree`. A verdict carries the covering ROAs (origin, prefix, max length and validity each)
/// of the primary table, the validators and the older build time.
fn verdict(primary: (&Table, i64), other: (&Table, i64), asn: u64, prefix: &str) -> Value {
    let route = prefix.split_once('/').and_then(|(a, l)| Some((a.parse::<IpAddr>().ok()?, l.parse::<u8>().ok()?)));
    let (Some((addr, len)), Ok(origin)) = (route, u32::try_from(asn)) else {
        return not_checked("error", "The route’s prefix or origin AS could not be read.");
    };
    let v = primary.0.validate(addr, len, origin);
    let w = other.0.validate(addr, len, origin);
    if v.status != w.status {
        let reason = format!("The two sources disagree: {} says {}, {} says {}.", DEFS[0].name, say(v.status), DEFS[1].name, say(w.status));
        return not_checked("disagree", &reason);
    }
    let roas: Vec<Value> = v
        .covering
        .iter()
        .take(MAX_ROAS)
        .map(|(vrp, m)| json!({ "origin": vrp.asn.to_string(), "prefix": format!("{}/{}", vrp.addr, vrp.len), "max_length": vrp.max_len, "validity": m.as_str() }))
        .collect();
    json!({ "status": v.status, "state": "ready", "roas": roas, "validator": VALIDATOR, "vrp_time": primary.1.min(other.1) })
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
    use feeds::tests::{ENTRIES, document};
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

    const HOUR: i64 = 3_600_000;

    fn table_of(entries: &[(&str, u8, u32)]) -> Table {
        Table::new(entries.iter().map(|(p, m, a)| vrp(p, *m, *a)))
    }

    /// A check over `dir`, with the setting as given (`None`: never written), 3 as the entry floor and no network:
    /// with `measuring` set no sync ever starts.
    fn rpki_in(dir: &Path, setting: Option<bool>, measuring: bool) -> Arc<Rpki> {
        let store = Arc::new(Store::open(dir.join("test.db")).unwrap());
        if let Some(on) = setting {
            store.put(SETTING, &on);
        }
        Rpki::with_floor(dir.join("rpki"), store, Arc::new(AtomicBool::new(measuring)), Arc::default(), 3)
    }

    fn save_feed(dir: &Path, source: usize, entries: &[(&str, u8, u32)], built: i64) {
        std::fs::create_dir_all(dir.join("rpki")).unwrap();
        std::fs::write(dir.join("rpki").join(DEFS[source].file()), table_of(entries).to_bytes(built)).unwrap();
    }

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!("recursa-rpki-test-{:x}", fastrand::u64(..)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn state_of(v: &Value) -> (Option<&str>, Option<&str>) {
        (v["status"].as_str(), v["state"].as_str())
    }

    #[test]
    fn the_result_keeps_the_shape_the_ui_reads() {
        let (a, b) = (sample(), sample());
        let v = verdict((&a, 1234), (&b, 5678), 13335, "1.1.1.0/24");
        assert_eq!(v["status"], "valid");
        assert_eq!(v["state"], "ready");
        assert_eq!(v["roas"][0], json!({ "origin": "13335", "prefix": "1.1.1.0/24", "max_length": 24, "validity": "valid" }));
        assert_eq!(v["validator"], "rpki-client console and RIPE NCC");
        assert_eq!(v["vrp_time"], 1234);
        let none = verdict((&a, 1), (&b, 1), 15169, "8.8.8.0/24");
        assert_eq!((none["status"].as_str(), none["roas"].as_array().map(Vec::len)), (Some("unknown"), Some(0)));
        assert_eq!(verdict((&a, 1), (&b, 1), 13335, "garbage")["status"], "not_checked");
    }

    #[test]
    fn both_sources_agreeing_gives_a_verdict_and_disagreeing_gives_none() {
        let dir = scratch();
        let now = now_ms();
        save_feed(&dir, 0, &ENTRIES, now - 2 * HOUR);
        save_feed(&dir, 1, &ENTRIES, now - HOUR);
        let r = rpki_in(&dir, None, true);
        let v = r.lookup(64496, "192.0.2.0/24");
        assert_eq!(state_of(&v), (Some("valid"), Some("ready")));
        assert_eq!(v["vrp_time"], now - 2 * HOUR);
        assert_eq!(v["validator"], VALIDATOR);
        assert_eq!(r.lookup(64999, "192.0.2.0/24")["status"], "invalid_asn");

        // RIPE NCC's table holds another origin for the prefix.
        let other = [("192.0.2.0/24", 24, 64497), ENTRIES[1], ENTRIES[2]];
        save_feed(&dir, 1, &other, now - HOUR);
        let r = rpki_in(&dir, None, true);
        let v = r.lookup(64496, "192.0.2.0/24");
        assert_eq!(state_of(&v), (Some("not_checked"), Some("disagree")));
        assert_eq!((v["roas"].as_array().map(Vec::len), v["validator"].is_null()), (Some(0), true));
        let reason = v["reason"].as_str().unwrap();
        assert!(reason.contains("rpki-client console says valid") && reason.contains("RIPE NCC says invalid"), "{reason}");
        // A route both leave uncovered still agrees.
        assert_eq!(r.lookup(64496, "203.0.113.0/24")["status"], "unknown");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_source_that_is_stale_or_absent_gives_no_verdict() {
        let dir = scratch();
        let now = now_ms();
        save_feed(&dir, 0, &ENTRIES, now - HOUR);
        save_feed(&dir, 1, &ENTRIES, now - 25 * HOUR);
        let r = rpki_in(&dir, None, true);
        // Over 24 hours old, no error recorded, and a measurement keeps the sync from starting.
        let v = r.lookup(64496, "192.0.2.0/24");
        assert_eq!(state_of(&v), (Some("not_checked"), Some("waiting")));
        r.state.lock().unwrap().sources[1].error = Some(("The server answered 503".into(), now));
        let v = r.lookup(64496, "192.0.2.0/24");
        assert_eq!(v["state"], "error");
        assert!(v["reason"].as_str().unwrap().contains("RIPE NCC: The server answered 503"));
        assert_eq!(r.status()["state"], "error");

        // One source with no table at all.
        let dir = scratch();
        save_feed(&dir, 1, &ENTRIES, now - HOUR);
        let r = rpki_in(&dir, None, true);
        r.state.lock().unwrap().sources[0].error = Some(("boom".into(), now));
        let v = r.lookup(64496, "192.0.2.0/24");
        assert_eq!(v["state"], "error");
        assert!(v["reason"].as_str().unwrap().contains("rpki-client console: boom"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn off_syncing_waiting_and_ready_return_the_right_cells() {
        let dir = scratch();
        let off = rpki_in(&scratch(), Some(false), false);
        let v = off.lookup(64496, "192.0.2.0/24");
        assert_eq!(state_of(&v), (Some("not_checked"), Some("off")));
        assert_eq!(off.status()["state"], "off");

        // On, nothing synced, a measurement running (so no sync starts, and no network is used).
        let wait = rpki_in(&dir, None, true);
        assert_eq!(state_of(&wait.lookup(64496, "192.0.2.0/24")), (Some("not_checked"), Some("waiting")));
        assert_eq!(wait.status()["waiting"], true);

        // A sync in progress: the cell says so and lookups do not block.
        wait.state.lock().unwrap().syncing_since = Some(now_ms());
        let v = wait.lookup(64496, "192.0.2.0/24");
        assert_eq!((v["status"].as_str(), v["reason"].as_str()), (Some("not_checked"), Some("Syncing route-origin data…")));
        assert_eq!(wait.status()["state"], "syncing");

        // Ready, from saved tables that are fresh.
        let now = now_ms();
        save_feed(&dir, 0, &ENTRIES, now - HOUR);
        save_feed(&dir, 1, &ENTRIES, now - HOUR);
        let ready = rpki_in(&dir, None, true);
        assert_eq!(state_of(&ready.lookup(64496, "192.0.2.0/24")), (Some("valid"), Some("ready")));
        let st = ready.status();
        assert_eq!((st["state"].as_str(), st["vrps"].as_u64(), st["validator"].as_str()), (Some("ready"), Some(3), Some(VALIDATOR)));
        assert!(st["last_sync"].as_i64().unwrap() > 0 && st["disk_bytes"].as_u64().unwrap() > 0);
        assert!(st.get("points").is_none() && st.get("stats").is_none());
        let src = &st["sources"][1];
        assert_eq!(
            (src["key"].as_str(), src["name"].as_str(), src["engine"].as_str()),
            (Some("ripe"), Some("RIPE NCC"), Some("Routinator"))
        );
        assert_eq!((src["vrps"].as_u64(), src["build_time"].as_i64(), src["error"].is_null()), (Some(3), Some(now - HOUR), true));
        assert!(src["fetched"].as_i64().unwrap() > 0);
        assert_eq!(st["sources"][0]["key"], "console");

        // Off deletes the tables.
        ready.set_enabled(false);
        assert!(!dir.join("rpki").exists());
        assert_eq!(ready.lookup(64496, "192.0.2.0/24")["state"], "off");
        assert_eq!(ready.status()["sources"][0]["vrps"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_setting_is_on_unless_rpki_check_is_false() {
        assert!(rpki_in(&scratch(), None, true).enabled());
        assert!(rpki_in(&scratch(), Some(true), true).enabled());
        assert!(!rpki_in(&scratch(), Some(false), true).enabled());
        // The old key is never read, whatever it holds.
        for old in [true, false] {
            let r = rpki_in(&scratch(), None, true);
            r.store.put("rpki_local", &old);
            assert!(r.enabled());
        }
    }

    #[test]
    fn the_old_validators_files_are_deleted_and_the_tables_are_not() {
        for setting in [Some(true), Some(false), None] {
            let dir = scratch();
            let old = dir.join("rpki");
            std::fs::create_dir_all(old.join("validator").join("rrdp")).unwrap();
            for f in ["vrps.bin", "vrps.bin.tmp", "routinator.conf"] {
                std::fs::write(old.join(f), "x").unwrap();
            }
            save_feed(&dir, 0, &ENTRIES, now_ms());
            let r = rpki_in(&dir, setting, true);
            r.store.put("rpki_local", &true);
            r.touch();
            for _ in 0..200 {
                if !old.join("validator").exists() && !old.join("vrps.bin").exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            let mut left: Vec<_> =
                std::fs::read_dir(&old).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            left.sort();
            assert_eq!(left, ["feed-console.bin"], "{setting:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The reply a source gives for `entries` built an hour ago; its `Last-Modified` is `lm`.
    fn body(def: &feeds::Def, entries: &[(&str, u8, u32)], lm: &str) -> Reply {
        let built = (now_ms() / 1000 - 3600).to_string();
        Reply::Body { bytes: document(def, entries, Some(&built)), gzip: true, last_modified: Some(lm.into()) }
    }

    #[test]
    fn an_older_build_does_not_replace_the_table_held() {
        let dir = scratch();
        let held = now_ms() - 60_000;
        save_feed(&dir, 0, &ENTRIES, held);
        save_feed(&dir, 1, &ENTRIES, held);
        let r = rpki_in(&dir, None, true);
        // Both servers answer with a build an hour old, one entry different.
        let other = [ENTRIES[0], ("203.0.113.0/24", 24, 64511), ENTRIES[2]];
        assert!(r.run_sync(&|| false, &|def, _| Ok(body(def, &other, "older"))));
        for d in &DEFS {
            let saved = std::fs::read(dir.join("rpki").join(d.file())).unwrap();
            assert_eq!(Table::from_bytes(&saved).unwrap().1, held);
        }
        let st = r.status();
        assert!(st["sources"][0]["error"].is_null() && st["sources"][1]["error"].is_null());
        assert_eq!(state_of(&r.lookup(64496, "192.0.2.0/24")), (Some("valid"), Some("ready")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sync_keeps_each_table_that_passes_and_leaves_the_rest() {
        let dir = scratch();
        let r = rpki_in(&dir, None, true);
        let stop = || false;
        assert!(r.run_sync(&stop, &|def, since| {
            assert!(since.is_none());
            Ok(body(def, &ENTRIES, "first"))
        }));
        let st = r.status();
        assert_eq!((st["sources"][0]["vrps"].as_u64(), st["sources"][1]["vrps"].as_u64()), (Some(3), Some(3)));
        assert!(dir.join("rpki").join("feed-console.bin").exists() && dir.join("rpki").join("feed-ripe.bin").exists());
        assert!(!dir.join("rpki").join("feed-ripe.bin.tmp").exists());
        assert_eq!(state_of(&r.lookup(64496, "192.0.2.0/24")), (Some("valid"), Some("ready")));

        // Second sync: the console is unchanged, RIPE NCC fails; both tables stay, the failure is recorded.
        assert!(r.run_sync(&stop, &|def, since| match def.key {
            "console" => {
                assert_eq!(since, Some("first"));
                Ok(Reply::NotModified)
            }
            _ => Err(Failure::Failed("The server answered 503".into())),
        }));
        let st = r.status();
        assert_eq!(st["sources"][1]["error"], "The server answered 503");
        assert_eq!((st["sources"][0]["vrps"].as_u64(), st["sources"][1]["vrps"].as_u64()), (Some(3), Some(3)));
        assert!(st["sources"][0]["error"].is_null());
        assert_eq!(state_of(&r.lookup(64496, "192.0.2.0/24")), (Some("valid"), Some("ready")));

        // Third: a download that fails a gate (an address family missing) is refused and the old table stays.
        assert!(r.run_sync(&stop, &|def, _| Ok(body(def, &ENTRIES[..2], "third"))));
        let st = r.status();
        assert!(st["sources"][0]["error"].as_str().unwrap().contains("address family"));
        assert_eq!(st["sources"][0]["vrps"], 3);
        let saved = std::fs::read(dir.join("rpki").join("feed-console.bin")).unwrap();
        assert_eq!(Table::from_bytes(&saved).unwrap().0.len(), 3);

        // A stop ends the sync before it fetches anything.
        assert!(!r.run_sync(&|| true, &|_, _| panic!("fetched after a stop")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turning_the_check_off_deletes_the_tables_and_the_saved_times() {
        let dir = scratch();
        let now = now_ms();
        save_feed(&dir, 0, &ENTRIES, now - HOUR);
        save_feed(&dir, 1, &ENTRIES, now - HOUR);
        let r = rpki_in(&dir, None, true);
        r.store.put(FEED_STATE, &json!({ "console": "x" }));
        r.load();
        assert_eq!(r.state.lock().unwrap().sources[0].last_modified.as_deref(), Some("x"));
        let epoch = r.epoch.load(Ordering::SeqCst);
        let st = r.set_enabled(false);
        assert_eq!(st["state"], "off");
        assert!(r.epoch.load(Ordering::SeqCst) > epoch);
        assert!(!dir.join("rpki").exists());
        assert_eq!(r.store.get::<BTreeMap<String, String>>(FEED_STATE), Some(BTreeMap::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn waiting_for_the_tables_is_brief_and_bounded() {
        let ms = Duration::from_millis;
        let took = |r: &Arc<Rpki>, bound| {
            let t = Instant::now();
            r.wait_up_to(bound);
            t.elapsed()
        };
        // Off: at once, with nothing started.
        let off = rpki_in(&scratch(), Some(false), false);
        assert!(took(&off, ms(2000)) < ms(1000) && !off.syncing());
        // A measured phase blocks syncing: at once.
        let busy = rpki_in(&scratch(), None, true);
        assert!(took(&busy, ms(2000)) < ms(1000) && !busy.syncing());
        // Both tables usable: at once, even with a sync running.
        let dir = scratch();
        let now = now_ms();
        save_feed(&dir, 0, &ENTRIES, now - HOUR);
        save_feed(&dir, 1, &ENTRIES, now - HOUR);
        let ready = rpki_in(&dir, None, false);
        ready.state.lock().unwrap().syncing_since = Some(now);
        assert!(took(&ready, ms(2000)) < ms(1000));
        // A failed sync in its retry pause: at once.
        let paused = rpki_in(&scratch(), None, false);
        paused.state.lock().unwrap().sources[0].error = Some(("boom".into(), now));
        assert!(took(&paused, ms(2000)) < ms(1000) && !paused.syncing());
        // A sync that never ends: the wait ends at the bound.
        let stuck = rpki_in(&scratch(), None, false);
        stuck.state.lock().unwrap().syncing_since = Some(now);
        let t = took(&stuck, ms(300));
        assert!(t >= ms(300) && t < ms(1500), "{t:?}");
        // A stray temporary file is removed when the tables are read.
        let dir = scratch();
        save_feed(&dir, 0, &ENTRIES, now);
        std::fs::write(dir.join("rpki").join("feed-ripe.bin.tmp"), "x").unwrap();
        rpki_in(&dir, None, true).load();
        assert!(!dir.join("rpki").join("feed-ripe.bin.tmp").exists() && dir.join("rpki").join("feed-console.bin").exists());
    }
}
