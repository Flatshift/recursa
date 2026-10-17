//! One validation run with Routinator's library: the five RIR trust anchors, RRDP only, into the
//! cache directory. Blocking, and run only in the short-lived child process (see [`super::child_entry`]).

use super::table::Vrp;
use routinator::config::Config;
use routinator::engine::{CaCert, Engine, ProcessPubPoint, ProcessRun};
use routinator::error::Failed;
use routinator::payload::ValidationReport;
use routinator::rpki::repository::aspa::AsProviderAttestation;
use routinator::rpki::repository::cert::{Cert, ResourceCert};
use routinator::rpki::repository::roa::RouteOriginAttestation;
use routinator::rpki::repository::tal::{Tal, TalUri};
use routinator::rpki::repository::x509::{Time, Validity};
use routinator::rpki::uri;
use routinator::slurm::LocalExceptions;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

/// What a finished run looked like. `rsync_only_*` are the publication points (and the hosts they sit on)
/// that are published over rsync only, which an RRDP-only validator cannot read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Stats {
    pub seconds: f64,
    pub rrdp_repos: u32,
    pub rrdp_repos_ok: u32,
    pub valid_points: u32,
    pub rsync_only_repos: u32,
    pub rsync_only_points: u32,
}

pub(super) struct Outcome {
    pub vrps: Vec<Vrp>,
    pub stats: Stats,
}

pub(super) enum Failure {
    /// The child was ended on purpose (the setting went off, or a measured phase began); nothing is wrong.
    Stopped,
    Failed(String),
}

/// The library's version, from its own default user agent ("Routinator/0.15.2").
pub(super) fn engine_version() -> String {
    let ua = Config::default_with_paths("".into(), "".into()).rrdp_user_agent;
    ua.strip_prefix("Routinator/").unwrap_or(&ua).to_string()
}

fn config(cache: &Path) -> Config {
    let mut c = Config::default_with_paths(cache.join("routinator.conf"), cache.join("validator"));
    c.disable_rsync = true;
    // Most of a run is waiting on the network, and a few dozen small repositories are dead or filtered.
    c.validation_threads = 8;
    c.rrdp_connect_timeout = Some(Duration::from_secs(10));
    // ARIN's snapshot is several hundred MB; the library's default of ten minutes for one request is too tight on a slow link.
    c.rrdp_timeout = Some(Duration::from_secs(30 * 60));
    c.rrdp_user_agent = concat!("Recursa-DNS-Benchmark/", env!("CARGO_PKG_VERSION")).into();
    c
}

/// `points` counts the publication points met so far, for the progress line.
pub(super) fn run(cache: &Path, points: &AtomicU64) -> Result<Outcome, Failure> {
    install_logger();
    let started = Instant::now();
    let config = config(cache);
    let fail = |what: &str| Failure::Failed(last_error().map_or_else(|| what.to_string(), |e| format!("{what} ({e})")));
    let mut engine = Engine::new(&config, true).map_err(|_| fail("Couldn't open the route-origin cache"))?;
    engine.ignite().map_err(|_| fail("Couldn't start the route-origin validator"))?;
    let report = ValidationReport::new(&config);
    let rsync_only = Mutex::new(HashSet::new());
    let mut metrics = {
        let mut run = engine
            .start(Guard { report: &report, points, rsync_only: &rsync_only }, false)
            .map_err(|_| fail("Couldn't start a validation run"))?;
        let done = run.process();
        done.map_err(|_| fail("Validation failed"))?;
        run.cleanup().map_err(|_| fail("Couldn't tidy the route-origin cache"))?;
        run.done()
    };
    let snapshot = report.into_snapshot(&LocalExceptions::empty(), &mut metrics);
    let vrps: Vec<Vrp> = snapshot
        .origins()
        .map(|(o, _)| Vrp {
            addr: o.prefix.addr(),
            len: o.prefix.prefix_len(),
            max_len: o.prefix.resolved_max_len(),
            asn: o.asn.into_u32(),
        })
        .collect();
    drop(snapshot);
    drop(engine);

    let mut stats = Stats {
        seconds: started.elapsed().as_secs_f64(),
        rrdp_repos: metrics.rrdp.len() as u32,
        rrdp_repos_ok: metrics.rrdp.iter().filter(|r| r.notify_status.is_success() || r.notify_status.is_not_modified()).count() as u32,
        valid_points: metrics.publication.valid_points,
        ..Stats::default()
    };
    let rsync_only = rsync_only.into_inner().unwrap_or_default();
    stats.rsync_only_points = rsync_only.len() as u32;
    stats.rsync_only_repos =
        rsync_only.iter().filter_map(|u| u.strip_prefix("rsync://")?.split('/').next()).collect::<HashSet<_>>().len() as u32;
    // Many small repositories are dead or filtered, which is normal. A table built from nothing, or
    // without one of the five RIRs, would call real routes "not found".
    if vrps.is_empty() {
        return Err(fail("The RIRs' repositories could not be reached"));
    }
    if let Some(t) = metrics.tals.iter().find(|t| t.publication.valid_points == 0) {
        return Err(fail(&format!("The {} repository could not be read", t.name().to_uppercase())));
    }
    Ok(Outcome { vrps, stats })
}

// The library reports problems through the `log` crate. Keep the last error for the status line.

static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

struct Keep;

impl log::Log for Keep {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Error
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            *LAST_ERROR.lock().unwrap() = Some(r.args().to_string().chars().take(240).collect());
        }
    }
    fn flush(&self) {}
}

fn install_logger() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if log::set_logger(&Keep).is_ok() {
            log::set_max_level(log::LevelFilter::Error);
        }
    });
    *LAST_ERROR.lock().unwrap() = None;
}

fn last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap().clone()
}

// Routinator's own collector of valid objects, wrapped to count publication points and to note
// the ones published over rsync only.

#[derive(Clone, Copy)]
struct Guard<'a> {
    report: &'a ValidationReport,
    points: &'a AtomicU64,
    /// Repositories of the CAs that name no RRDP notification file, as the run meets them.
    rsync_only: &'a Mutex<HashSet<String>>,
}

type Inner<'a> = <&'a ValidationReport as ProcessRun>::PubPoint;

struct Point<'a> {
    inner: Inner<'a>,
    points: &'a AtomicU64,
    rsync_only: &'a Mutex<HashSet<String>>,
}

impl<'a> ProcessRun for Guard<'a> {
    type PubPoint = Point<'a>;

    fn process_ta(&self, tal: &Tal, uri: &TalUri, cert: &CaCert, tal_index: usize) -> Result<Option<Self::PubPoint>, Failed> {
        let inner = ProcessRun::process_ta(&self.report, tal, uri, cert, tal_index)?;
        Ok(inner.map(|inner| Point { inner, points: self.points, rsync_only: self.rsync_only }))
    }
}

impl ProcessPubPoint for Point<'_> {
    fn repository_index(&mut self, repository_index: usize) {
        self.inner.repository_index(repository_index)
    }

    fn point_validity(&mut self, manifest_ee: Validity, stale: Time) {
        self.inner.point_validity(manifest_ee, stale)
    }

    fn want(&self, uri: &uri::Rsync) -> Result<bool, Failed> {
        self.inner.want(uri)
    }

    fn process_ca(&mut self, uri: &uri::Rsync, cert: &CaCert) -> Result<Option<Self>, Failed> {
        self.points.fetch_add(1, Ordering::Relaxed);
        if cert.rpki_notify().is_none() {
            self.rsync_only.lock().unwrap().insert(cert.ca_repository().to_string());
        }
        let (points, rsync_only) = (self.points, self.rsync_only);
        Ok(self.inner.process_ca(uri, cert)?.map(|inner| Point { inner, points, rsync_only }))
    }

    fn process_router_cert(&mut self, uri: &uri::Rsync, cert: Cert, ca_cert: &CaCert) -> Result<(), Failed> {
        self.inner.process_router_cert(uri, cert, ca_cert)
    }

    fn process_roa(&mut self, uri: &uri::Rsync, cert: ResourceCert, route: RouteOriginAttestation) -> Result<(), Failed> {
        self.inner.process_roa(uri, cert, route)
    }

    fn process_aspa(&mut self, uri: &uri::Rsync, cert: ResourceCert, aspa: AsProviderAttestation) -> Result<(), Failed> {
        self.inner.process_aspa(uri, cert, aspa)
    }

    fn restart(&mut self) -> Result<(), Failed> {
        self.inner.restart()
    }

    fn commit(self) {
        self.inner.commit()
    }

    fn cancel(self, cert: &CaCert) {
        self.inner.cancel(cert)
    }
}
