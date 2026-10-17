//! Application state and the API every front end shares.
//!
//! The CLI's HTTP server and the desktop app's IPC bridge both route REST-style calls
//! through [`AppState::call`], so the two transports can't drift apart. Only the live
//! event stream and file export differ per transport, and both build on
//! [`RunHandle::subscribe`] and [`AppState::export`].

use crate::bench::{self, RunConfig, RunHandle};
use crate::calibrate::Calibration;
use crate::catalog::{self, Resolver};
use crate::intel::{self, Intel};
use crate::lock;
use crate::monitor::{self, MonitorConfig};
use crate::ntp_monitor::{self, NtpMonitorConfig};
use crate::store::{self, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::watch;

pub struct AppState {
    pub store: Arc<Store>,
    pub intel: Arc<Intel>,
    pub measuring: Arc<AtomicBool>,
    pub run_active: Arc<AtomicBool>,
    pub runs: Mutex<HashMap<String, Arc<RunHandle>>>,
    pub active_run: Mutex<Option<String>>,
    pub system: Arc<RwLock<Vec<IpAddr>>>,
    /// What each local system resolver turned out to be: a forwarder (dnsmasq, Pi-hole, the gateway's
    /// DNS proxy) with its description, or none. Filled in by `detect_forwarders`.
    pub forwarders: RwLock<HashMap<IpAddr, Option<String>>>,
    pub vantage: RwLock<Value>,
    pub interception: RwLock<Value>,
    pub calibration: RwLock<Option<Calibration>>,
    /// The NTP run in progress, or the last one finished.
    pub ntp: Mutex<Option<NtpRun>>,
    /// Monitoring settings; front ends can `subscribe` to hear about changes.
    pub monitor: watch::Sender<MonitorConfig>,
    pub ntp_monitor: watch::Sender<NtpMonitorConfig>,
    /// Bumped when NTP monitoring is switched off or its record cleared, so a cycle in flight knows to record nothing.
    pub ntp_epoch: AtomicU64,
}

/// A flag set for as long as this lives. Dropping it clears the flag, so a handler dropped mid-await (a closed
/// connection) can't leave `run_active` or `measuring` set for every later run.
struct FlagGuard(Option<Arc<AtomicBool>>);

impl FlagGuard {
    /// Claims `flag` if nobody holds it.
    fn claim(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok().then(|| FlagGuard(Some(flag.clone())))
    }

    fn set(flag: &Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::SeqCst);
        FlagGuard(Some(flag.clone()))
    }

    /// Leaves the flag set: whoever takes over clears it.
    fn hand_off(mut self) {
        self.0 = None;
    }
}

impl Drop for FlagGuard {
    fn drop(&mut self) {
        if let Some(f) = &self.0 {
            f.store(false, Ordering::SeqCst);
        }
    }
}

/// Sets the flag when dropped: work given the flag stops once the one waiting on it is gone.
pub(crate) struct StopOnDrop(pub Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// An API failure: an HTTP status code (kept over IPC too) and a message for people.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub status: u16,
    pub error: String,
}

impl ApiError {
    pub fn new(status: u16, error: impl Into<String>) -> Self {
        ApiError { status, error: error.into() }
    }
}

/// A run serialized for saving to a file.
pub struct Export {
    pub filename: String,
    pub mime: &'static str,
    pub body: String,
}

/// The monitoring setting as saved, if there is one and it parses.
fn stored_monitor(store: &Store) -> Option<MonitorConfig> {
    store.setting("monitor").and_then(|v| serde_json::from_value(v).ok())
}

/// Automatic cleanup of unsaved runs. `None` switches a rule off; saved runs are never counted or removed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Housekeeping {
    /// Keep this many of the newest unsaved runs.
    pub keep_runs: Option<u32>,
    /// Keep unsaved runs started within this many days.
    pub keep_days: Option<u32>,
}

impl Default for Housekeeping {
    fn default() -> Self {
        // A run is a few hundred KB; 50 of them is a season of weekly runs and a few MB of
        // database. 90 days is as far back as a resolver comparison stays current.
        Housekeeping { keep_runs: Some(50), keep_days: Some(90) }
    }
}

impl Housekeeping {
    fn clamped(mut self) -> Self {
        self.keep_runs = self.keep_runs.map(|n| n.clamp(1, 10_000));
        self.keep_days = self.keep_days.map(|d| d.clamp(1, 3_650));
        self
    }
}

/// The housekeeping setting as saved, else the default.
fn stored_housekeeping(store: &Store) -> Housekeeping {
    store
        .setting("housekeeping")
        .and_then(|v| serde_json::from_value::<Housekeeping>(v).ok())
        .map(Housekeeping::clamped)
        .unwrap_or_default()
}

fn stored_ntp_monitor(store: &Store) -> Option<NtpMonitorConfig> {
    store.setting("ntp_monitor").and_then(|v| serde_json::from_value::<NtpMonitorConfig>(v).ok()).map(NtpMonitorConfig::clamped)
}

impl AppState {
    /// Open the database and build the state. `calibrate` measures the timing floor first;
    /// otherwise [`AppState::start_background`] does it.
    pub fn open(db: Option<PathBuf>, calibrate: bool) -> Result<Arc<Self>, String> {
        let path = db.unwrap_or_else(store::default_path);
        let store = Arc::new(Store::open(path.clone()).map_err(|e| format!("{}: {e}", path.display()))?);
        let measuring = Arc::new(AtomicBool::new(false));
        let system = Arc::new(RwLock::new(crate::sysdns::discover()));
        let intel = Arc::new(Intel::new(store.clone(), measuring.clone(), system.clone()));
        let monitor_cfg = stored_monitor(&store).unwrap_or_default();
        let ntp_monitor_cfg = stored_ntp_monitor(&store).unwrap_or_default();
        Ok(Arc::new(AppState {
            store,
            intel,
            measuring,
            run_active: Arc::new(AtomicBool::new(false)),
            runs: Mutex::new(HashMap::new()),
            active_run: Mutex::new(None),
            system,
            forwarders: RwLock::new(HashMap::new()),
            vantage: RwLock::new(Value::Null),
            interception: RwLock::new(Value::Null),
            calibration: RwLock::new(calibrate.then(|| crate::calibrate::run(200).ok()).flatten()),
            ntp: Mutex::new(None),
            monitor: watch::Sender::new(monitor_cfg),
            ntp_monitor: watch::Sender::new(ntp_monitor_cfg),
            ntp_epoch: AtomicU64::new(0),
        }))
    }

    /// Start the background work a long-lived front end needs: calibration if it hasn't
    /// run yet, then vantage and interception detection and the monitoring loop.
    /// Calibration goes first so none of our own traffic disturbs it. Needs a Tokio runtime.
    pub fn start_background(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            if app.calibration.read().unwrap().is_none()
                && let Ok(Ok(c)) = tokio::task::spawn_blocking(|| crate::calibrate::run(200)).await
            {
                *app.calibration.write().unwrap() = Some(c);
            }
            // Old runs go while nothing is measuring, here or in another process; else the
            // next run's end does it.
            if let Some(_measuring) = lock::try_lock(&lock::measure_path()) {
                app.cleanup_runs();
            }
            tokio::spawn(detect_vantage(app.clone()));
            tokio::spawn(detect_interception(app.clone()));
            tokio::spawn({
                let app = app.clone();
                async move { detect_forwarders(&app).await }
            });
            ntp_monitor::spawn(app.clone());
            monitor::spawn(app);
        });
    }

    pub fn resolvers(&self) -> Vec<Resolver> {
        let mut all = catalog::system_entries(&self.system.read().unwrap());
        {
            let known = self.forwarders.read().unwrap();
            for r in all.iter_mut() {
                r.forwarder = r.name.parse::<IpAddr>().ok().and_then(|ip| known.get(&ip).cloned().flatten());
            }
        }
        let mut custom: Vec<Resolver> = self.store.custom_list().into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
        // A custom entry for the system resolver's own address (a named Pi-hole, say) names
        // that row instead of adding a second one: same server, measured once.
        custom.retain(|c| {
            let Some(sys) = all.iter_mut().find(|s| s.name == c.name) else { return true };
            sys.provider = c.provider.clone();
            sys.variant = if sys.id == "system-0" { "system resolver".into() } else { format!("system resolver {}", sys.variant) };
            sys.dot = c.dot.clone();
            sys.doh = c.doh.clone();
            sys.custom_id = Some(c.id.clone());
            false
        });
        // A custom entry for an address the catalog lists (Cogent's two, added by hand before
        // they were listed) is that catalog entry: same servers, so it is measured once. Saved
        // selections keep it through the alias; it is not a default just because it was added by hand.
        let mut builtin = catalog::builtin();
        custom.retain(|c| {
            let Some(b) = builtin.iter_mut().find(|b| b.ipv4.contains(&c.name) || b.ipv6.contains(&c.name)) else { return true };
            b.aliases.push(c.id.clone());
            false
        });
        all.extend(custom);
        // A system resolver that is a catalog entry (1.1.1.1 set on this Mac, say) says so and
        // stays out of Defaults, so that server is measured once, under its catalog name.
        for sys in all.iter_mut().filter(|r| r.group == "system" && r.custom_id.is_none()) {
            if let Some(b) = builtin.iter().find(|b| b.ipv4.contains(&sys.name) || b.ipv6.contains(&sys.name)) {
                sys.variant = format!("{} · {}", sys.variant, b.label());
                sys.notes = Some(format!("Set directly on this computer: this is {}, also listed under {}.", b.label(), b.provider));
                sys.default = false;
            }
        }
        all.extend(builtin);
        all
    }

    pub fn env(&self) -> Value {
        json!({
            "platform": std::env::consts::OS,
            "version": env!("CARGO_PKG_VERSION"),
            "build": crate::BUILD,
            "clock": crate::clock::SOURCE,
            "kernel_rx": crate::calibrate::kernel_stamps_available(),
            "calibration": *self.calibration.read().unwrap(),
            "vantage": *self.vantage.read().unwrap(),
            "interception": *self.interception.read().unwrap(),
            "system": self.system.read().unwrap().iter().map(|i| i.to_string()).collect::<Vec<_>>(),
            "db": self.store.path.display().to_string(),
            "rpki_local": cfg!(all(feature = "rpki-local", not(target_os = "ios"))),
        })
    }

    fn client_v6(&self) -> bool {
        self.vantage.read().unwrap()["ipv6_reachable"].as_bool().unwrap_or(false)
    }

    /// The NTP run in memory, else the last finished one from the store.
    fn ntp_state(&self) -> Option<Value> {
        match &*self.ntp.lock().unwrap() {
            Some(r) => serde_json::to_value(r).ok(),
            None => self.store.setting("ntp_last"),
        }
    }

    fn ntp_ledger(&self) -> crate::ntp::Ledger {
        self.store.setting("ntp_asked").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
    }

    /// Which of these servers may be asked now, and which are being left alone (asked in the last ten minutes, or told us
    /// to stop). Anything that may be asked is noted as asked, so a second run straight after doesn't ask again.
    pub fn ntp_guard(&self, entries: &[crate::ntp::Entry]) -> (Vec<crate::ntp::Entry>, Vec<crate::ntp::Skipped>) {
        let now = bench::now_ms();
        let mut ledger = self.ntp_ledger();
        let (ask, skipped) = crate::ntp::guard(entries, &ledger, now);
        crate::ntp::stamp(&mut ledger, &ask, now);
        self.store.set_setting("ntp_asked", &json!(ledger));
        (ask, skipped)
    }

    /// Remember any server that told a finished run to stop.
    pub fn ntp_finished(&self, results: &[crate::ntp::ServerResult]) {
        let mut ledger = self.ntp_ledger();
        crate::ntp::remember_kisses(&mut ledger, results, bench::now_ms());
        self.store.set_setting("ntp_asked", &json!(ledger));
    }

    /// Start an NTP run: servers from the catalog, asked in parallel, eight seconds apart. Shares the one measurement
    /// slot with benchmarks and monitoring, so nothing else is on the network while it times.
    async fn start_ntp(self: &Arc<Self>, req: NtpRequest) -> Result<Value, ApiError> {
        let catalog = crate::ntp::catalog();
        let mut entries: Vec<crate::ntp::Entry> = Vec::new();
        for id in &req.servers {
            match catalog.iter().find(|e| &e.id == id) {
                Some(e) if !entries.iter().any(|x| x.id == e.id) => entries.push(e.clone()),
                Some(_) => {}
                None => return Err(ApiError::new(400, format!("Unknown time server: {id}."))),
            }
        }
        if req.custom.len() > crate::ntp::MAX_CUSTOM {
            return Err(ApiError::new(400, format!("At most {} servers of your own per run.", crate::ntp::MAX_CUSTOM)));
        }
        for text in &req.custom {
            let e = crate::ntp::custom_entry(text).map_err(|m| ApiError::new(400, m))?;
            if !entries.iter().any(|x| x.id == e.id) {
                entries.push(e);
            }
        }
        if entries.is_empty() {
            return Err(ApiError::new(400, "Pick at least one time server."));
        }
        let (entries, added) = crate::ntp::with_references(entries);
        let samples = req.samples.clamp(2, crate::ntp::MAX_SAMPLES);
        let Some(claim) = FlagGuard::claim(&self.run_active) else {
            return Err(ApiError::new(409, "A run is already in progress."));
        };
        let Some(measure) = lock::wait(&lock::measure_path(), Duration::from_secs(10)).await else {
            return Err(ApiError::new(409, "Another Recursa window or terminal is measuring right now. Try again when it finishes."));
        };
        // Servers asked a few minutes ago, or that said stop, are left alone; if that is all of them, there is nothing to do.
        let (entries, skipped) = self.ntp_guard(&entries);
        if entries.is_empty() {
            drop(measure);
            let wait = skipped.iter().map(|s| s.until).min().unwrap_or(0).saturating_sub(bench::now_ms()).max(0) / 60_000 + 1;
            return Err(ApiError::new(
                409,
                format!(
                    "Every server you picked was asked a few minutes ago, or asked us to stop. To be gentle, wait about {wait} minute{} and try again.",
                    if wait == 1 { "" } else { "s" }
                ),
            ));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let run = NtpRun {
            id: format!("ntp-{:x}", bench::now_ms()),
            status: "running".into(),
            started_at: bench::now_ms(),
            finished_at: None,
            total: entries.len(),
            done: 0,
            samples,
            nts: req.nts,
            results: Vec::new(),
            consensus: None,
            skipped,
            added,
            spacing_s: crate::ntp::MIN_SPACING.as_secs(),
            cancel: cancel.clone(),
        };
        *self.ntp.lock().unwrap() = Some(run);
        let app = self.clone();
        let measuring = FlagGuard::set(&self.measuring);
        std::thread::spawn(move || {
            // Held until the run ends, however it ends.
            let _held = (measuring, claim, measure);
            let plan = crate::ntp::Plan { samples, ..Default::default() };
            let app2 = app.clone();
            let (results, consensus) = crate::ntp::run(&entries, &plan, req.nts, &cancel, &move |r| {
                if let Some(run) = app2.ntp.lock().unwrap().as_mut() {
                    run.results.push(r.clone());
                    run.done += 1;
                }
            });
            let results_for_ledger = results.clone();
            let state = {
                let mut g = app.ntp.lock().unwrap();
                if let Some(run) = g.as_mut() {
                    run.results = results;
                    run.consensus = consensus;
                    run.status = if cancel.load(Ordering::Relaxed) { "cancelled" } else { "done" }.into();
                    run.finished_at = Some(bench::now_ms());
                }
                g.as_ref().and_then(|r| serde_json::to_value(r).ok())
            };
            app.ntp_finished(&results_for_ledger);
            if let Some(v) = state {
                app.store.set_setting("ntp_last", &v);
            }
        });
        self.ntp_state().ok_or_else(|| ApiError::new(500, "The run didn't start."))
    }

    /// The saved lists of Cached-test names, in the order they were saved.
    fn cached_lists(&self) -> Vec<NameList> {
        self.store.setting("cached_lists").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
    }

    /// Check and store the saved lists: a name and a valid list each, no repeated names of lists.
    fn save_cached_lists(&self, lists: Vec<NameList>) -> Result<Vec<NameList>, ApiError> {
        if lists.len() > MAX_LISTS {
            return Err(ApiError::new(400, format!("At most {MAX_LISTS} saved lists.")));
        }
        let mut out: Vec<NameList> = Vec::new();
        for l in lists {
            let name: String = l.name.trim().chars().take(40).collect();
            if name.is_empty() {
                return Err(ApiError::new(400, "A saved list needs a name."));
            }
            if out.iter().any(|o| o.name.eq_ignore_ascii_case(&name)) {
                return Err(ApiError::new(400, format!("There is already a list called “{name}”.")));
            }
            let names = crate::names::validate(l.names).map_err(|e| ApiError::new(400, format!("{name}: {e}")))?;
            out.push(NameList { name, names });
        }
        self.store.set_setting("cached_lists", &json!(out));
        Ok(out)
    }

    /// Start a run: validate it, claim the single run slot and the machine-wide measure
    /// lock, persist the result when it finishes, and keep the live handle for the UI.
    pub async fn launch(self: &Arc<Self>, mut cfg: RunConfig) -> Result<Arc<RunHandle>, ApiError> {
        // The run records which local resolvers are forwarders, so know before it starts (a no-op once known).
        detect_forwarders(self).await;
        let resolvers = self.resolvers();
        // A custom entry folded into another row (the system resolver's, or a catalog entry's) answers to that row's id.
        for id in cfg.resolvers.iter_mut() {
            if let Some(r) = resolvers.iter().find(|r| r.custom_id.as_deref() == Some(id.as_str()) || r.aliases.contains(id)) {
                *id = r.id.clone();
            }
        }
        // Each resolver once, in the order given.
        let mut seen = std::collections::HashSet::new();
        cfg.resolvers.retain(|id| seen.insert(id.clone()));
        if cfg.resolvers.is_empty() {
            return Err(ApiError::new(400, "Pick at least one resolver."));
        }
        if cfg.tests.is_empty() {
            return Err(ApiError::new(400, "Pick at least one test."));
        }
        cfg.cached_names = if cfg.cached_names.is_empty() {
            crate::names::defaults()
        } else {
            crate::names::validate(std::mem::take(&mut cfg.cached_names)).map_err(|e| ApiError::new(400, e))?
        };
        if cfg.transports.is_empty() {
            return Err(ApiError::new(400, "Pick at least one transport."));
        }
        let unknown: Vec<&str> = cfg.resolvers.iter().filter(|id| !resolvers.iter().any(|r| &r.id == *id)).map(String::as_str).collect();
        if !unknown.is_empty() {
            return Err(ApiError::new(400, format!("Unknown resolver: {}.", unknown.join(", "))));
        }
        let Some(claim) = FlagGuard::claim(&self.run_active) else {
            return Err(ApiError::new(409, "A run is already in progress."));
        };
        // Wait out a monitoring cycle (seconds at most); refuse if another process is running a benchmark.
        let Some(measure) = lock::wait(&lock::measure_path(), Duration::from_secs(10)).await else {
            return Err(ApiError::new(409, "Another Recursa window or terminal is measuring right now. Try again when it finishes."));
        };
        let app = self.clone();
        let deps = bench::Deps {
            resolvers,
            intel: self.intel.clone(),
            rt: tokio::runtime::Handle::current(),
            env: self.env(),
            client_v6: self.client_v6(),
            measuring: self.measuring.clone(),
            run_active: self.run_active.clone(),
            on_finish: Box::new(move |s| {
                let body = serde_json::to_string(s).unwrap_or_default();
                app.store.save_run(&s.id, s.started_at, s.finished_at, &s.status, &bench::summary(s), &body);
                *app.active_run.lock().unwrap() = None;
                app.cleanup_runs();
                drop(measure);
            }),
        };
        let h = bench::spawn(cfg, deps);
        // The run's thread clears the flag when it ends.
        claim.hand_off();
        *self.active_run.lock().unwrap() = Some(h.id.clone());
        let mut runs = self.runs.lock().unwrap();
        runs.insert(h.id.clone(), h.clone());
        // Keep only a few runs in memory; the store has the rest. Ids sort by time.
        while runs.len() > 4 {
            let Some(oldest) = runs.keys().filter(|k| **k != h.id).min().cloned() else { break };
            runs.remove(&oldest);
        }
        Ok(h)
    }

    /// The live handle for a run that is still in memory.
    pub fn run_handle(&self, id: &str) -> Option<Arc<RunHandle>> {
        self.runs.lock().unwrap().get(id).cloned()
    }

    /// The time run, as the versioned JSON or the CSV that `recursa ntp --format` prints.
    fn export_ntp(&self, id: &str, csv: bool) -> Option<Export> {
        let v = self.ntp_state().filter(|v| v["id"] == id)?;
        let results: Vec<crate::ntp::ServerResult> = serde_json::from_value(v["results"].clone()).ok()?;
        if csv {
            return Some(Export { filename: format!("recursa-{id}.csv"), mime: "text/csv", body: crate::ntp_report::to_csv(&results) });
        }
        let body = json!({ "schema_version": crate::ntp_report::SCHEMA_VERSION, "results": v["results"], "consensus": v["consensus"] });
        Some(Export { filename: format!("recursa-{id}.json"), mime: "application/json", body: body.to_string() })
    }

    /// The run as JSON: live from memory, else from the store.
    fn run_json(&self, id: &str) -> Option<String> {
        match self.run_handle(id) {
            Some(h) => serde_json::to_string(&*h.state()).ok(),
            None => self.store.get_run(id),
        }
    }

    /// Handle one API call. `path` is the request path with an optional query string,
    /// with or without the `/api` prefix; `body` is the parsed JSON body (null if none).
    /// Returns the response as JSON text.
    pub async fn call(self: &Arc<Self>, method: &str, path: &str, body: Value) -> Result<String, ApiError> {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
        let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segs.first() == Some(&"api") {
            segs.remove(0);
        }
        let ok = |v: Value| Ok(v.to_string());
        match (method, segs.as_slice()) {
            ("GET", ["bootstrap"]) => ok(self.bootstrap()),
            ("GET", ["env"]) => ok(self.env()),
            ("POST", ["calibrate"]) => {
                self.idle()?;
                let c = tokio::task::spawn_blocking(|| crate::calibrate::run(300))
                    .await
                    .map_err(|e| ApiError::new(500, e.to_string()))?
                    .map_err(|e| ApiError::new(500, e.to_string()))?;
                *self.calibration.write().unwrap() = Some(c);
                ok(self.env())
            }
            ("POST", ["interception"]) => {
                self.idle()?;
                detect_interception(self.clone()).await;
                ok(self.env())
            }
            ("GET", ["runs"]) => ok(json!(self.store.list_runs(200))),
            ("POST", ["runs"]) => {
                let h = self.launch(parse(body)?).await?;
                ok(json!({ "id": h.id }))
            }
            ("GET", ["runs", id]) => self.run_json(id).ok_or_else(|| ApiError::new(404, "No such run.")),
            ("DELETE", ["runs"]) => {
                // Every unsaved finished run. The live one has no row yet and stays in memory.
                let active = self.active_run.lock().unwrap().clone();
                self.runs.lock().unwrap().retain(|id, _| Some(id) == active.as_ref());
                ok(json!({ "deleted": self.store.clear_runs(active.as_deref()) }))
            }
            ("DELETE", ["runs", id]) => {
                if self.active_run.lock().unwrap().as_deref() == Some(*id) {
                    return Err(ApiError::new(409, "Cancel the run before deleting it."));
                }
                self.runs.lock().unwrap().remove(*id);
                ok(json!({ "deleted": self.store.delete_run(id) }))
            }
            ("PUT", ["runs", id, "saved"]) => {
                let saved = body["saved"].as_bool().ok_or_else(|| ApiError::new(400, "Say whether the run is saved."))?;
                if !self.store.set_run_saved(id, saved) {
                    return Err(ApiError::new(404, "No such run."));
                }
                ok(json!({ "id": id, "saved": saved }))
            }
            ("GET", ["housekeeping"]) => ok(json!(stored_housekeeping(&self.store))),
            ("PUT", ["housekeeping"]) => {
                // A key left out keeps that rule as it is; null switches it off.
                let obj = body.as_object().ok_or_else(|| ApiError::new(400, "Invalid request: expected an object."))?;
                if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "keep_runs" | "keep_days")) {
                    return Err(ApiError::new(400, format!("Invalid request: unknown field `{k}`.")));
                }
                let mut cfg = stored_housekeeping(&self.store);
                let rule = |key: &str, cur: Option<u32>| obj.get(key).map_or(Ok(cur), |v| parse(v.clone()));
                cfg.keep_runs = rule("keep_runs", cfg.keep_runs)?;
                cfg.keep_days = rule("keep_days", cfg.keep_days)?;
                let cfg = cfg.clamped();
                self.store.set_setting("housekeeping", &serde_json::to_value(&cfg).unwrap());
                ok(json!(cfg))
            }
            ("POST", ["runs", id, "cancel"]) => {
                let h = self.run_handle(id).ok_or_else(|| ApiError::new(404, "No such run."))?;
                if h.state().status != "running" {
                    return Err(ApiError::new(409, "That run has already finished."));
                }
                h.cancel.store(true, Ordering::SeqCst);
                ok(json!({ "cancelling": true }))
            }
            ("GET", ["status"]) => {
                let mut live = self.intel.status(param("force") == Some("1")).await;
                // Judged from where the client is: a report for another region leaves a resolver operational here.
                let here = self.vantage.read().unwrap()["geo"]["country"].as_str().and_then(crate::regions::Client::from_code);
                intel::localize(&mut live["feeds"], here.as_ref());
                ok(json!({ "live": live, "history": intel::curated_incidents(), "measuring": self.measuring.load(Ordering::SeqCst) }))
            }
            // `?live=1`: just what changes while a run is going, without the catalog.
            ("GET", ["ntp"]) if param("live") == Some("1") => {
                ok(json!({ "run": self.ntp_state(), "asked": self.ntp_ledger(), "now": bench::now_ms() }))
            }
            ("GET", ["ntp"]) => ok(json!({
                "catalog": crate::ntp::catalog(),
                "run": self.ntp_state(),
                "spacing_s": crate::ntp::MIN_SPACING.as_secs(),
                "cooldown_s": crate::ntp::COOLDOWN.as_secs(),
                "max_samples": crate::ntp::MAX_SAMPLES,
                "asked": self.ntp_ledger(),
                "now": bench::now_ms(),
            })),
            ("POST", ["ntp", "run"]) => ok(self.start_ntp(parse(body)?).await?),
            ("POST", ["ntp", "cancel"]) => {
                if let Some(r) = self.ntp.lock().unwrap().as_ref().filter(|r| r.status == "running") {
                    r.cancel.store(true, Ordering::SeqCst);
                }
                ok(json!({ "cancelling": true }))
            }
            ("GET", ["cached-lists"]) => {
                ok(json!({ "lists": self.cached_lists(), "default": crate::names::defaults(), "max": crate::names::MAX_NAMES }))
            }
            ("PUT", ["cached-lists"]) => {
                let NameLists { lists } = parse(body)?;
                ok(json!({ "lists": self.save_cached_lists(lists)? }))
            }
            ("POST", ["cached-lists", "parse"]) => {
                let ParseNames { text } = parse(body)?;
                if text.len() > MAX_IMPORT_BYTES {
                    return Err(ApiError::new(413, "That file is too large to read as a list of names (1 MB at most)."));
                }
                let (names, skipped) = crate::names::parse_list(&text);
                let total = names.len();
                ok(
                    json!({ "names": names.into_iter().take(1000).collect::<Vec<_>>(), "total": total, "skipped": skipped.into_iter().take(20).collect::<Vec<_>>() }),
                )
            }
            ("POST", ["resolvers"]) => ok(self.add_resolver(parse(body)?)?),
            ("DELETE", ["resolvers", id]) => ok(json!({ "deleted": self.store.custom_delete(id) })),
            ("POST", ["resolvers", id, "deep-dive"]) => ok(json!(self.deep_dive(id).await?)),
            ("GET", ["monitor"]) => {
                let hours = param("hours").and_then(|h| h.parse::<i64>().ok()).unwrap_or(72).clamp(1, 24 * 90);
                let (since, cell) = monitor::window(hours, bench::now_ms());
                // From the hourly rollups (at most 24 × 2,160 rows), off the async threads.
                let store = self.store.clone();
                let history = tokio::task::spawn_blocking(move || {
                    let mut b = monitor::HistoryBuilder::new(cell);
                    store.monitor_rollups(since, |r| b.push(&r));
                    b.finish(hours, since, store.monitor_outages(since))
                })
                .await
                .map_err(|e| ApiError::new(500, e.to_string()))?;
                #[derive(Serialize)]
                struct Resp {
                    config: MonitorConfig,
                    last: Option<i64>,
                    paused: bool,
                    history: monitor::History,
                }
                let resp = Resp {
                    config: self.monitor.borrow().clone(),
                    // From the database, so it counts cycles any Recursa process recorded.
                    last: self.store.monitor_latest(),
                    paused: self.run_active.load(Ordering::SeqCst),
                    history,
                };
                // Straight to text: no intermediate JSON tree for thousands of hourly cells.
                serde_json::to_string(&resp).map_err(|e| ApiError::new(500, e.to_string()))
            }
            ("GET", ["monitor", "recorded"]) => ok(json!(self.store.monitor_recorded())),
            // Cheap enough to poll: the page refetches the history only when a cycle has run.
            ("GET", ["monitor", "last"]) => ok(json!({ "last": self.store.monitor_latest() })),
            ("PUT", ["monitor"]) => ok(json!(self.set_monitor(parse(body)?))),
            ("DELETE", ["monitor", "history"]) => {
                self.store.monitor_clear();
                ok(json!({ "cleared": true }))
            }
            ("GET", ["ntp", "monitor"]) => {
                let days = param("days").and_then(|h| h.parse::<i64>().ok()).unwrap_or(7).clamp(1, 365);
                let until = bench::now_ms();
                let since = until - days * 86_400_000;
                let store = self.store.clone();
                let history = tokio::task::spawn_blocking(move || {
                    let (rows, clock) = store.ntp_history(since);
                    ntp_monitor::history(&rows, &clock, since, until)
                })
                .await
                .map_err(|e| ApiError::new(500, e.to_string()))?;
                #[derive(Serialize)]
                struct Resp {
                    config: NtpMonitorConfig,
                    last: Option<i64>,
                    paused: bool,
                    history: ntp_monitor::History,
                }
                let resp = Resp {
                    config: self.ntp_monitor.borrow().clone(),
                    last: self.store.ntp_latest(),
                    paused: self.run_active.load(Ordering::SeqCst),
                    history,
                };
                serde_json::to_string(&resp).map_err(|e| ApiError::new(500, e.to_string()))
            }
            ("GET", ["ntp", "monitor", "last"]) => ok(json!({ "last": self.store.ntp_latest() })),
            ("PUT", ["ntp", "monitor"]) => ok(json!(self.set_ntp_monitor(parse(body)?)?)),
            ("DELETE", ["ntp", "monitor", "history"]) => {
                self.ntp_epoch.fetch_add(1, Ordering::SeqCst);
                self.store.ntp_clear();
                ok(json!({ "cleared": true }))
            }
            #[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
            (_, ["rpki", rest @ ..]) => crate::rpki::call(self, method, rest, body).await,
            _ => Err(ApiError::new(404, "No such endpoint.")),
        }
    }

    /// A short, fixed look at one resolver (see [`crate::deepdive`]). Holds the same claim a run does, so it
    /// never overlaps a benchmark or a monitoring cycle.
    async fn deep_dive(&self, id: &str) -> Result<crate::deepdive::DeepDive, ApiError> {
        let r = self
            .resolvers()
            .into_iter()
            .find(|r| r.id == id || r.custom_id.as_deref() == Some(id) || r.aliases.iter().any(|a| a == id))
            .ok_or_else(|| ApiError::new(404, "No such resolver."))?;
        let Some(claim) = FlagGuard::claim(&self.run_active) else {
            return Err(ApiError::new(409, "Wait for the current run to finish."));
        };
        let Some(measure) = lock::wait(&lock::measure_path(), Duration::from_secs(10)).await else {
            return Err(ApiError::new(409, "Another Recursa window or terminal is measuring right now. Try again when it finishes."));
        };
        let label = r.label();
        let held = (FlagGuard::set(&self.measuring), claim, measure);
        // If the request is dropped mid-look (the window closed), the look stops at its next query.
        let cancel = Arc::new(AtomicBool::new(false));
        let _stop = StopOnDrop(cancel.clone());
        let out = tokio::task::spawn_blocking(move || {
            // Held until the look ends, even if the request that asked for it is gone.
            let _held = held;
            let timeout = Duration::from_millis(2000);
            let (mut ex, transport) = crate::endpoints::open_best(&r, &[], timeout, false)?;
            Some(crate::deepdive::run(ex.as_mut(), transport, timeout, &cancel))
        })
        .await;
        out.map_err(|e| ApiError::new(500, e.to_string()))?
            .ok_or_else(|| ApiError::new(502, format!("Couldn't open a connection to {label}.")))
    }

    fn idle(&self) -> Result<(), ApiError> {
        if self.run_active.load(Ordering::SeqCst) {
            return Err(ApiError::new(409, "Wait for the current run to finish."));
        }
        Ok(())
    }

    fn bootstrap(&self) -> Value {
        let resolvers = self.resolvers();
        let defaults = RunConfig::with_resolvers(resolvers.iter().filter(|r| r.default).map(|r| r.id.clone()).collect());
        json!({
            "env": self.env(),
            "resolvers": resolvers,
            "defaults": defaults,
            "monitor": *self.monitor.borrow(),
            "active_run": *self.active_run.lock().unwrap(),
            "runs": self.store.list_runs(200),
            "method": {
                "cached_names": bench::CACHED_NAMES,
                "uncached_zones": bench::UNCACHED_ZONES,
                "cdns": crate::cdn::CDNS
                    .iter()
                    .map(|c| json!({"id": c.id, "name": c.name, "role": c.role, "hosts": c.hosts.iter().map(|h| h.name).collect::<Vec<_>>()}))
                    .collect::<Vec<_>>(),
            },
        })
    }

    fn add_resolver(&self, n: NewResolver) -> Result<Value, ApiError> {
        let ip: IpAddr = n.ip.trim().parse().map_err(|_| ApiError::new(400, "That isn't an IPv4 or IPv6 address."))?;
        // ::ffff:192.0.2.1 is an IPv4 resolver.
        let ip = ip.to_canonical();
        let broadcast = matches!(ip, IpAddr::V4(v) if v.is_broadcast());
        if ip.is_unspecified() || ip.is_multicast() || broadcast {
            return Err(ApiError::new(400, "That's not a server address (it's unspecified, broadcast or multicast)."));
        }
        // A second copy of a catalog entry would be measured twice under two names.
        if let Some(known) = catalog::builtin().into_iter().find(|r| r.ipv4.iter().chain(&r.ipv6).any(|a| *a == ip.to_string())) {
            return Err(ApiError::new(400, format!("That's {}, already in the list.", known.label())));
        }
        let clean = |s: Option<String>| s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
        let doh = clean(n.doh);
        if let Some(u) = &doh {
            let ok = u.parse::<http::Uri>().ok().is_some_and(|u| u.scheme_str() == Some("https") && u.host().is_some());
            if !ok {
                return Err(ApiError::new(400, "The DoH URL must start with https:// and include a host."));
            }
        }
        let dot = clean(n.dot);
        if let Some(h) = &dot
            && rustls::pki_types::ServerName::try_from(h.as_str()).is_err()
        {
            return Err(ApiError::new(400, "The DoT name must be a valid host name."));
        }
        let name = clean(n.name).map(|s| s.chars().take(60).collect());
        let r = Resolver::custom(ip, name, dot, doh);
        self.store.custom_put(&r.id, &serde_json::to_value(&r).unwrap());
        // For the system resolver's own address, hand back the row it now names.
        let row = self.resolvers().into_iter().find(|x| x.id == r.id || x.custom_id.as_deref() == Some(r.id.as_str())).unwrap_or(r);
        Ok(serde_json::to_value(&row).unwrap())
    }

    /// Picks up monitoring settings another process saved to this database. Unreadable ones are ignored.
    pub fn reload_monitors(&self) {
        if let Some(new) = stored_monitor(&self.store) {
            self.monitor.send_if_modified(|c| std::mem::replace(c, new.clone()) != new);
        }
    }

    /// Delete old unsaved runs by the saved housekeeping setting (read fresh, so another
    /// process's Settings apply too). Call it only while nothing is measuring. Returns how many went.
    pub fn cleanup_runs(&self) -> usize {
        let cfg = stored_housekeeping(&self.store);
        let since = cfg.keep_days.map(|d| bench::now_ms() - d as i64 * 86_400_000);
        let n = self.store.cleanup_runs(cfg.keep_runs, since);
        if n > 0 {
            // Recent runs are held in memory too; a deleted one mustn't answer GET /runs/:id.
            let active = self.active_run.lock().unwrap().clone();
            self.runs.lock().unwrap().retain(|id, _| Some(id) == active.as_ref() || self.store.has_run(id));
        }
        n
    }

    /// As [`AppState::reload_monitors`], for the NTP monitor.
    pub fn reload_ntp_monitor(&self) {
        if let Some(new) = stored_ntp_monitor(&self.store) {
            self.ntp_monitor.send_if_modified(|c| std::mem::replace(c, new.clone()) != new);
        }
    }

    /// Save and apply a monitoring configuration, within sane bounds, and tell subscribers.
    pub fn set_monitor(&self, mut cfg: MonitorConfig) -> MonitorConfig {
        cfg.interval_min = cfg.interval_min.clamp(1, 1440);
        cfg.retention_days = cfg.retention_days.clamp(1, 365);
        let mut seen = std::collections::HashSet::new();
        cfg.resolvers.retain(|id| seen.insert(id.clone()));
        cfg.resolvers.truncate(24);
        self.store.set_setting("monitor", &serde_json::to_value(&cfg).unwrap());
        self.monitor.send_replace(cfg.clone());
        cfg
    }

    /// Save and apply an NTP monitoring configuration. Typed hosts must be valid addresses.
    fn set_ntp_monitor(&self, cfg: NtpMonitorConfig) -> Result<NtpMonitorConfig, ApiError> {
        let cfg = cfg.clamped();
        let cat = crate::ntp::catalog();
        for s in &cfg.servers {
            if !cat.iter().any(|e| e.id == *s && !e.nts_only && e.kind != "pool") {
                crate::ntp::custom_entry(s).map_err(|m| ApiError::new(400, m))?;
            }
        }
        if cfg.enabled && cfg.entries().is_empty() {
            return Err(ApiError::new(400, "Pick at least one time server to watch."));
        }
        self.store.set_setting("ntp_monitor", &serde_json::to_value(&cfg).unwrap());
        if !cfg.enabled {
            self.ntp_epoch.fetch_add(1, Ordering::SeqCst);
        }
        self.ntp_monitor.send_replace(cfg.clone());
        Ok(cfg)
    }

    /// A run as CSV (one row per sample) or as the full JSON state.
    pub fn export(&self, id: &str, csv: bool) -> Option<Export> {
        if id.starts_with("ntp-") {
            return self.export_ntp(id, csv);
        }
        let body = self.run_json(id)?;
        if !csv {
            return Some(Export { filename: format!("recursa-{id}.json"), mime: "application/json", body });
        }
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let empty = Vec::new();
        let targets = v["targets"].as_array().unwrap_or(&empty);
        let samples = v["samples"].as_array().unwrap_or(&empty);
        let q = |x: &Value| format!("\"{}\"", x.as_str().unwrap_or("").replace('"', "\"\""));
        let mut out = String::with_capacity(64 + samples.len() * 96);
        out.push_str("resolver_id,provider,variant,transport,family,ip,test,round,outcome,rtt_ms,at_ms\n");
        for s in samples {
            let Some(t) = s["t"].as_u64().and_then(|i| targets.get(i as usize)) else { continue };
            let us = s["us"].as_u64().unwrap_or(0);
            let rtt = if us > 0 { format!("{:.3}", us as f64 / 1000.0) } else { String::new() };
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{}\n",
                q(&t["resolver_id"]),
                q(&t["provider"]),
                q(&t["variant"]),
                t["transport"].as_str().unwrap_or(""),
                t["family"].as_str().unwrap_or(""),
                t["ip"].as_str().unwrap_or(""),
                s["k"].as_str().unwrap_or(""),
                s["i"],
                s["o"].as_str().unwrap_or(""),
                rtt,
                s["at"]
            ));
        }
        Some(Export { filename: format!("recursa-{id}.csv"), mime: "text/csv; charset=utf-8", body: out })
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: Value) -> Result<T, ApiError> {
    serde_json::from_value(body).map_err(|e| ApiError::new(400, format!("Invalid request: {e}.")))
}

/// An NTP run, as the UI sees it.
#[derive(Clone, Serialize)]
pub struct NtpRun {
    pub id: String,
    /// running | done | cancelled
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub total: usize,
    pub done: usize,
    pub samples: u32,
    pub nts: bool,
    pub results: Vec<crate::ntp::ServerResult>,
    pub consensus: Option<crate::ntp::Consensus>,
    /// Servers left out because they were asked a few minutes ago, or told us to stop.
    pub skipped: Vec<crate::ntp::Skipped>,
    /// Reference servers added because every server in the run was one you typed.
    pub added: Vec<String>,
    /// The longest wait between queries to one address.
    pub spacing_s: u64,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct NtpRequest {
    servers: Vec<String>,
    /// Hosts typed by the user: `host`, `host:port`, `[v6]:port`.
    #[serde(default)]
    custom: Vec<String>,
    #[serde(default = "ntp_samples")]
    samples: u32,
    #[serde(default = "yes_")]
    nts: bool,
}

fn ntp_samples() -> u32 {
    4
}
fn yes_() -> bool {
    true
}

/// A saved list of Cached-test names.
#[derive(Clone, Deserialize, Serialize)]
struct NameList {
    name: String,
    names: Vec<String>,
}

#[derive(Deserialize)]
struct NameLists {
    lists: Vec<NameList>,
}

#[derive(Deserialize)]
struct ParseNames {
    text: String,
}

/// Saved lists kept, and the longest text that is read as one.
const MAX_LISTS: usize = 20;
const MAX_IMPORT_BYTES: usize = 1 << 20;

#[derive(Deserialize)]
struct NewResolver {
    ip: String,
    name: Option<String>,
    dot: Option<String>,
    doh: Option<String>,
}

pub async fn detect_vantage(app: Arc<AppState>) {
    let v = app.intel.vantage().await;
    *app.vantage.write().unwrap() = v;
}

/// Identify the local system resolvers that aren't known yet: dnsmasq and Pi-hole by what they say they
/// are, the default gateway as a router's DNS proxy. A second at most per resolver, once per address.
async fn detect_forwarders(app: &AppState) {
    let todo: Vec<IpAddr> = {
        let known = app.forwarders.read().unwrap();
        app.system.read().unwrap().iter().filter(|ip| catalog::is_private(ip) && !known.contains_key(ip)).copied().collect()
    };
    if todo.is_empty() {
        return;
    }
    let found = tokio::task::spawn_blocking(move || {
        let gateway = crate::sysdns::default_gateway();
        todo.into_iter().map(|ip| (ip, crate::forwarder::identify(ip, gateway))).collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    app.forwarders.write().unwrap().extend(found);
}

pub async fn detect_interception(app: Arc<AppState>) {
    if app.run_active.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(v) = tokio::task::spawn_blocking(crate::intercept::check).await {
        *app.interception.write().unwrap() = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Arc<AppState> {
        crate::install_crypto_provider();
        let db = std::env::temp_dir().join(format!("recursa-test-{:x}.db", fastrand::u64(..)));
        AppState::open(Some(db), false).unwrap()
    }

    #[tokio::test]
    async fn custom_entries_for_catalog_addresses_fold_into_the_catalog_entry() {
        let a = app();
        // Made by hand before Cogent was listed.
        for (ip, name) in [("66.28.0.45", "Cogent 1"), ("66.28.0.61", "Cogent 2")] {
            let r = Resolver::custom(ip.parse().unwrap(), Some(name.into()), None, None);
            a.store.custom_put(&r.id, &serde_json::to_value(&r).unwrap());
        }
        let all = a.resolvers();
        assert!(all.iter().all(|r| r.group != "custom"), "the hand-made entries are listed once, as the catalog's");
        let cogent: Vec<_> = all.iter().filter(|r| r.ipv4.iter().any(|x| x == "66.28.0.45" || x == "66.28.0.61")).collect();
        assert_eq!(cogent.len(), 1);
        assert_eq!(cogent[0].id, "cogent");
        assert!(!cogent[0].default, "hand-made entries don't add to the defaults");
        assert_eq!(cogent[0].aliases, ["custom-66-28-0-45", "custom-66-28-0-61"]);
    }

    #[tokio::test]
    async fn a_known_forwarder_is_marked_on_its_system_resolver() {
        let a = app();
        let ip: IpAddr = "192.0.2.53".parse().unwrap();
        *a.system.write().unwrap() = vec![ip, "203.0.113.9".parse().unwrap()];
        a.forwarders.write().unwrap().insert(ip, Some("Pi-hole (dnsmasq-pi-hole-v2.93)".into()));
        let all = a.resolvers();
        assert_eq!(all[0].forwarder.as_deref(), Some("Pi-hole (dnsmasq-pi-hole-v2.93)"));
        assert_eq!(all[1].forwarder, None, "an ISP resolver stays a candidate");
    }

    #[tokio::test]
    async fn cached_lists_are_read_saved_and_checked() {
        let a = app();
        *a.system.write().unwrap() = vec![];
        let get = |s: String| serde_json::from_str::<Value>(&s).unwrap();
        // Reading a pasted list keeps the good names and says what it skipped.
        let r = get(a
            .call("POST", "/api/cached-lists/parse", json!({ "text": "Example.com\nbad\nhttps://news.example.org/x\n" }))
            .await
            .unwrap());
        assert_eq!(r["names"], json!(["example.com", "news.example.org"]));
        assert_eq!(r["skipped"][0]["text"], "bad");
        // Saved lists come back normalized; the defaults and the limit come with them.
        let saved = json!({ "lists": [{ "name": " Home ", "names": ["A.com", "b.org", "a.com."] }] });
        let r = get(a.call("PUT", "/api/cached-lists", saved).await.unwrap());
        assert_eq!(r["lists"][0], json!({ "name": "Home", "names": ["a.com", "b.org"] }));
        let r = get(a.call("GET", "/api/cached-lists", Value::Null).await.unwrap());
        assert_eq!((r["lists"][0]["name"].as_str(), r["max"].as_u64()), (Some("Home"), Some(50)));
        assert_eq!(r["default"].as_array().unwrap().len(), crate::bench::CACHED_NAMES.len());
        for bad in [
            json!({ "lists": [{ "name": "A", "names": ["x.com"] }, { "name": "a", "names": ["y.com"] }] }),
            json!({ "lists": [{ "name": "", "names": ["x.com"] }] }),
            json!({ "lists": [{ "name": "Bad", "names": ["nope"] }] }),
        ] {
            assert_eq!(a.call("PUT", "/api/cached-lists", bad).await.unwrap_err().status, 400);
        }
        // A run with a name that can't be a host name is refused before anything is measured.
        let mut cfg = RunConfig::with_resolvers(vec!["cloudflare".into()]);
        cfg.cached_names = vec!["example.com".into(), "nope".into()];
        let e = a.launch(cfg).await.err().expect("refused");
        assert!(e.status == 400 && e.error.contains("nope"), "{} {}", e.status, e.error);
    }

    #[tokio::test]
    async fn ntp_runs_are_checked_before_anything_is_sent() {
        let a = app();
        let get = |s: String| serde_json::from_str::<Value>(&s).unwrap();
        let r = get(a.call("GET", "/api/ntp", Value::Null).await.unwrap());
        assert!(r["catalog"].as_array().is_some_and(|c| c.len() > 40));
        assert_eq!((r["run"].is_null(), r["spacing_s"].as_u64(), r["cooldown_s"].as_u64()), (true, Some(8), Some(600)));
        let live = get(a.call("GET", "/api/ntp?live=1", Value::Null).await.unwrap());
        assert!(live["catalog"].is_null() && live["run"].is_null() && live["now"].is_u64());
        let nope = a.call("POST", "/api/ntp/run", json!({ "servers": ["google", "no-such-server"] })).await.unwrap_err();
        assert!(nope.status == 400 && nope.error.contains("no-such-server"), "{} {}", nope.status, nope.error);
        assert_eq!(a.call("POST", "/api/ntp/run", json!({ "servers": [] })).await.unwrap_err().status, 400);
        // A host you type is checked too: a bad one is a 400 that says which, and so is a fifth.
        let bad = a.call("POST", "/api/ntp/run", json!({ "servers": [], "custom": ["ntp.lan:99999"] })).await.unwrap_err();
        assert!(bad.status == 400 && bad.error.contains("ntp.lan:99999"), "{} {}", bad.status, bad.error);
        let many: Vec<String> = (0..5).map(|i| format!("h{i}.lan")).collect();
        assert_eq!(a.call("POST", "/api/ntp/run", json!({ "servers": [], "custom": many })).await.unwrap_err().status, 400);
        // Nothing was started, so the measurement slot is free.
        assert!(!a.run_active.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn the_ntp_monitor_is_configured_recorded_and_cleared() {
        let a = app();
        let get = |days: &'static str| {
            let a = a.clone();
            async move {
                serde_json::from_str::<Value>(&a.call("GET", &format!("/api/ntp/monitor?days={days}"), Value::Null).await.unwrap()).unwrap()
            }
        };
        let v = get("7").await;
        assert_eq!((v["config"]["enabled"].as_bool(), v["last"].is_null()), (Some(false), true));
        // The interval is held to what is gentle, and a typed host must be an address.
        let cfg = json!({ "enabled": true, "interval_min": 1, "servers": ["cloudflare", "ntp.lan:1123"], "retention_days": 9999 });
        let saved: Value = serde_json::from_str(&a.call("PUT", "/api/ntp/monitor", cfg).await.unwrap()).unwrap();
        assert_eq!((saved["interval_min"].as_u64(), saved["retention_days"].as_u64()), (Some(15), Some(365)));
        assert_eq!(a.ntp_monitor.borrow().servers.len(), 2);
        let bad = json!({ "enabled": true, "interval_min": 30, "servers": ["not a host!"], "retention_days": 30 });
        assert_eq!(a.call("PUT", "/api/ntp/monitor", bad).await.unwrap_err().status, 400);
        let none = json!({ "enabled": true, "interval_min": 30, "servers": [], "retention_days": 30 });
        assert_eq!(a.call("PUT", "/api/ntp/monitor", none).await.unwrap_err().status, 400);
        // What was recorded comes back summarised, and can be cleared.
        let now = bench::now_ms();
        let row = |ts: i64, off: f64| crate::store::NtpRow {
            ts,
            server: "cloudflare".into(),
            offset_ms: Some(off),
            bound_ms: Some(5.0),
            delay_ms: Some(20.0),
            stratum: Some(3),
            ok: true,
            kind: None,
            sel: Some("truechimer".into()),
        };
        a.store.ntp_record(&[row(now - 7_200_000, 1.0), row(now - 3_600_000, 2.0)], None);
        let v = get("1").await;
        assert_eq!((v["history"]["cycles"].as_u64(), v["history"]["servers"][0]["stats"]["ok"].as_u64()), (Some(2), Some(2)));
        assert_eq!(v["last"].as_i64(), Some(now - 3_600_000));
        a.call("DELETE", "/api/ntp/monitor/history", Value::Null).await.unwrap();
        assert_eq!(get("1").await["history"]["cycles"].as_u64(), Some(0));
    }

    #[tokio::test]
    async fn runs_are_saved_cleared_and_cleaned_up_by_the_housekeeping_setting() {
        let a = app();
        let get = |s: String| serde_json::from_str::<Value>(&s).unwrap();
        let now = bench::now_ms();
        let day = 86_400_000;
        for (i, id) in ["r1", "r2", "r3"].iter().enumerate() {
            a.store.save_run(id, now - (3 - i as i64) * 100 * day, None, "done", &json!({}), "{}");
        }
        a.store.save_run("r4", now, None, "done", &json!({}), "{}");
        // Defaults: 50 runs, 90 days.
        let cfg = get(a.call("GET", "/api/housekeeping", Value::Null).await.unwrap());
        assert_eq!(cfg, json!({ "keep_runs": 50, "keep_days": 90 }));
        assert_eq!(a.call("PUT", "/api/runs/zzz/saved", json!({ "saved": true })).await.unwrap_err().status, 404);
        assert_eq!(a.call("PUT", "/api/runs/r1/saved", json!({})).await.unwrap_err().status, 400);
        let r = get(a.call("PUT", "/api/runs/r1/saved", json!({ "saved": true })).await.unwrap());
        assert_eq!(r, json!({ "id": "r1", "saved": true }));
        let rows = get(a.call("GET", "/api/runs", Value::Null).await.unwrap());
        assert_eq!(rows[3]["saved"], true);
        // The 90-day rule: r2 and r3 are 200 and 100 days old, r1 is saved and 300 days old.
        assert_eq!(a.cleanup_runs(), 2);
        let ids = |rows: Value| rows.as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(ids(get(a.call("GET", "/api/runs", Value::Null).await.unwrap())), ["r4", "r1"]);
        // Days off, keep one run: still nothing but unsaved runs beyond the newest go.
        let saved = get(a.call("PUT", "/api/housekeeping", json!({ "keep_runs": 1, "keep_days": null })).await.unwrap());
        assert_eq!(saved, json!({ "keep_runs": 1, "keep_days": null }));
        a.store.save_run("r5", now + 1, None, "done", &json!({}), "{}");
        assert_eq!(a.cleanup_runs(), 1);
        assert_eq!(ids(get(a.call("GET", "/api/runs", Value::Null).await.unwrap())), ["r5", "r1"]);
        // Out-of-range values are clamped, and a bad body is refused.
        let saved = get(a.call("PUT", "/api/housekeeping", json!({ "keep_runs": 0, "keep_days": 99999 })).await.unwrap());
        assert_eq!(saved, json!({ "keep_runs": 1, "keep_days": 3650 }));
        assert_eq!(a.call("PUT", "/api/housekeeping", json!({ "keep_runs": "many" })).await.unwrap_err().status, 400);
        assert_eq!(a.call("PUT", "/api/housekeeping", json!({ "keep_run": 5 })).await.unwrap_err().status, 400);
        assert_eq!(a.call("PUT", "/api/housekeeping", json!(7)).await.unwrap_err().status, 400);
        // A key left out keeps that rule.
        let saved = get(a.call("PUT", "/api/housekeeping", json!({ "keep_runs": 20 })).await.unwrap());
        assert_eq!(saved, json!({ "keep_runs": 20, "keep_days": 3650 }));
        let saved = get(a.call("PUT", "/api/housekeeping", json!({})).await.unwrap());
        assert_eq!(saved, json!({ "keep_runs": 20, "keep_days": 3650 }));
        // Clear all: the saved run stays, and the count says how many went.
        let r = get(a.call("DELETE", "/api/runs", Value::Null).await.unwrap());
        assert_eq!(r, json!({ "deleted": 1 }));
        assert_eq!(ids(get(a.call("GET", "/api/runs", Value::Null).await.unwrap())), ["r1"]);
    }

    #[tokio::test]
    async fn routes_and_errors() {
        let a = app();
        assert!(!a.resolvers().is_empty());
        // The prefix is optional and the query string is parsed.
        let m: Value = serde_json::from_str(&a.call("GET", "monitor?hours=5", Value::Null).await.unwrap()).unwrap();
        assert!(m["config"].is_object());
        assert_eq!(a.call("GET", "/api/nope", Value::Null).await.unwrap_err().status, 404);
        assert_eq!(a.call("GET", "/apiruns", Value::Null).await.unwrap_err().status, 404);
        assert_eq!(a.call("GET", "/api/runs/zzz", Value::Null).await.unwrap_err().status, 404);
        assert_eq!(a.call("POST", "/api/runs", Value::Null).await.unwrap_err().status, 400);
        assert_eq!(a.call("POST", "/api/runs", json!({"resolvers": []})).await.unwrap_err().status, 400);
        // A custom entry for the system resolver's address names that row; it isn't listed twice.
        *a.system.write().unwrap() = vec!["192.0.2.53".parse().unwrap()];
        let row: Value =
            serde_json::from_str(&a.call("POST", "/api/resolvers", json!({"ip": "192.0.2.53", "name": "Pi-hole"})).await.unwrap()).unwrap();
        assert_eq!((row["id"].as_str(), row["provider"].as_str()), (Some("system-0"), Some("Pi-hole")));
        assert_eq!(row["custom_id"], "custom-192-0-2-53");
        assert_eq!(a.resolvers().iter().filter(|r| r.name == "192.0.2.53").count(), 1);
        a.call("DELETE", "/api/resolvers/custom-192-0-2-53", Value::Null).await.unwrap();
        assert_eq!(a.resolvers()[0].provider, "System resolver");
        // A system resolver that is a catalog entry is labelled and left out of Defaults.
        *a.system.write().unwrap() = vec!["1.1.1.1".parse().unwrap()];
        let sys = a.resolvers().into_iter().next().unwrap();
        assert_eq!((sys.variant.as_str(), sys.default), ("Primary · Cloudflare Standard", false));
        for (ip, why) in
            [("255.255.255.255", "not a server"), ("224.0.0.251", "not a server"), ("::", "not a server"), ("1.1.1.1", "Cloudflare")]
        {
            let e = a.call("POST", "/api/resolvers", json!({ "ip": ip })).await.unwrap_err();
            assert!(e.status == 400 && e.error.contains(why), "{ip}: {}", e.error);
        }
        let mapped: Value =
            serde_json::from_str(&a.call("POST", "/api/resolvers", json!({"ip": "::ffff:192.0.2.9"})).await.unwrap()).unwrap();
        assert_eq!(mapped["ipv4"][0], "192.0.2.9");
        let mut cfg: Value = serde_json::from_str(&a.call("GET", "/api/bootstrap", Value::Null).await.unwrap()).unwrap();
        cfg["defaults"]["tests"] = json!([]);
        assert_eq!(a.call("POST", "/api/runs", cfg["defaults"].clone()).await.unwrap_err().error, "Pick at least one test.");
        let bad = a.call("POST", "/api/resolvers", json!({"ip": "not-an-ip"})).await.unwrap_err();
        assert_eq!((bad.status, bad.error.as_str()), (400, "That isn't an IPv4 or IPv6 address."));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", a.store.path.display()));
        }
    }
}
