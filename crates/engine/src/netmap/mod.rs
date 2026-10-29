//! The network map: where your traffic goes between your ISP and the networks you reach most.
//!
//! One run traces to a fixed set of targets (the six roll-call CDN edges plus two hosts each, Apple, Microsoft and
//! Netflix with two hosts each, your system resolver, two public resolvers and the NTP pool), over IPv4 and, when
//! this computer has an IPv6 route, over IPv6 as well. Each router's network is named each router's network from CAIDA's prefix-to-AS table
//! held locally, and merges the traces into groups and links. Where traffic leaves your ISP, each trace gets a
//! claim that says how sure the map is (see [`say`]). No hop address is sent to any service: the only lookups are
//! per network number (its registered name) and, when the user turns it on, reverse names for the routers.

pub mod asrel;
pub mod attr;
pub mod pdb;
pub mod rdap;
pub mod rdns;
pub mod say;

use crate::app::AppState;
use crate::bench;
use crate::catalog::is_private;
use crate::cymru;
use crate::dns;
use crate::lock;
use crate::resolve::Resolve;
use crate::trace::{self, Trace, TraceCap};
use attr::{Ctx, Group, Link, Pfx2as};
use pdb::Pdb;
use say::{Claim, Headline};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Traces in flight at once.
const PARALLEL: usize = 6;
/// The gap between one tracer starting and the next. With [`PARALLEL`] this spreads the first probes of a run out
/// instead of sending four trains at once; `run_live`'s own round timing is not changed.
const STAGGER: Duration = Duration::from_millis(200);
/// Trace identifiers for this run's tracers, one each (see `border`: 0x5200 and up belong to runs).
const ID_BASE: u16 = 0x5200;
/// How old the tables may be before they're fetched again.
const REFRESH_MS: i64 = 24 * 3_600_000;
/// How long a stored run's traces may stand in for a fresh discovery of the border pair.
const FRESH_MS: i64 = 7 * 86_400_000;
const NAME_MAX_AGE_MS: i64 = 30 * 86_400_000;
/// The biggest download taken, compressed or not.
const BODY_MAX: usize = 256 << 20;
pub(crate) const RDNS: &str = "netmap.rdns";

enum Via {
    /// The address the roll call connected to for this CDN.
    Edge(&'static str),
    /// Looked up by name over the system resolver.
    Name,
    /// The first system resolver.
    System,
    /// The IPv4 and IPv6 addresses.
    Addr(&'static str, &'static str),
}

struct Spec {
    id: &'static str,
    cdn: Option<&'static str>,
    name: &'static str,
    host: &'static str,
    via: Via,
}

const fn spec(id: &'static str, cdn: Option<&'static str>, name: &'static str, host: &'static str, via: Via) -> Spec {
    Spec { id, cdn, name, host, via }
}

/// What a run traces to, each name resolved once per run. Each is traced over IPv4, and over IPv6 when there is a route.
const TARGETS: &[Spec] = &[
    spec("cloudflare", Some("cloudflare"), "Cloudflare", "speed.cloudflare.com", Via::Edge("cloudflare")),
    spec("cloudflare-apex", Some("cloudflare"), "Cloudflare", "cloudflare.com", Via::Name),
    spec("cloudflare-assets", Some("cloudflare"), "Cloudflare", "cdnjs.cloudflare.com", Via::Name),
    spec("fastly", Some("fastly"), "Fastly", "pypi.org", Via::Edge("fastly")),
    spec("fastly-apex", Some("fastly"), "Fastly", "www.fastly.com", Via::Name),
    spec("fastly-assets", Some("fastly"), "Fastly", "global.ssl.fastly.net", Via::Name),
    spec("cloudfront", Some("cloudfront"), "Amazon CloudFront", "d1.awsstatic.com", Via::Edge("cloudfront")),
    spec("cloudfront-apex", Some("cloudfront"), "Amazon CloudFront", "aws.amazon.com", Via::Name),
    spec("cloudfront-assets", Some("cloudfront"), "Amazon CloudFront", "a0.awsstatic.com", Via::Name),
    spec("google", Some("google"), "Google", "redirector.googlevideo.com", Via::Edge("google")),
    spec("google-apex", Some("google"), "Google", "www.google.com", Via::Name),
    spec("google-assets", Some("google"), "Google", "www.gstatic.com", Via::Name),
    spec("meta", Some("meta"), "Meta", "www.facebook.com", Via::Edge("meta")),
    spec("meta-apex", Some("meta"), "Meta", "facebook.com", Via::Name),
    spec("meta-assets", Some("meta"), "Meta", "static.xx.fbcdn.net", Via::Name),
    spec("akamai", Some("akamai"), "Akamai", "www.microsoft.com", Via::Edge("akamai")),
    spec("akamai-apex", Some("akamai"), "Akamai", "www.akamai.com", Via::Name),
    spec("akamai-assets", Some("akamai"), "Akamai", "www.adobe.com", Via::Name),
    spec("apple", Some("apple"), "Apple", "updates.cdn-apple.com", Via::Name),
    spec("apple-icloud", Some("apple"), "Apple", "gateway.icloud.com", Via::Name),
    spec("apple-assets", Some("apple"), "Apple", "is1-ssl.mzstatic.com", Via::Name),
    spec("microsoft", Some("microsoft"), "Microsoft", "www.office.com", Via::Name),
    spec("microsoft-login", Some("microsoft"), "Microsoft", "login.microsoftonline.com", Via::Name),
    spec("microsoft-outlook", Some("microsoft"), "Microsoft", "outlook.office.com", Via::Name),
    spec("netflix", Some("netflix"), "Netflix", "www.netflix.com", Via::Name),
    spec("netflix-assets", Some("netflix"), "Netflix", "assets.nflxext.com", Via::Name),
    spec("system", None, "Your DNS resolver", "system resolver", Via::System),
    spec("cloudflare-dns", None, "Cloudflare DNS", "1.1.1.1", Via::Addr("1.1.1.1", "2606:4700:4700::1111")),
    spec("google-dns", None, "Google DNS", "8.8.8.8", Via::Addr("8.8.8.8", "2001:4860:4860::8888")),
    spec("ntp-pool", None, "NTP pool", "pool.ntp.org", Via::Name),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    /// The CDN this target belongs to, when it has one.
    pub cdn: Option<String>,
    pub name: String,
    pub host: String,
    pub addr: IpAddr,
    /// 4 or 6: the IP version this target is traced over. The IPv6 target's id is the IPv4 one's plus `-v6`.
    #[serde(default = "four")]
    pub family: u8,
    /// The serving site the roll call found, for an edge target.
    pub site: Option<String>,
}

/// A stored run from before IPv6 has no family: it was IPv4.
fn four() -> u8 {
    4
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapTrace {
    pub target: Target,
    pub trace: Trace,
    /// For an IPv6 trace: whether it crosses the same networks as the IPv4 trace of the same target. `None` for IPv4.
    #[serde(default)]
    pub same_route: Option<bool>,
}

/// A target left out of the run, and why.
#[derive(Clone, Debug, Serialize)]
pub struct Skipped {
    pub target: String,
    pub reason: String,
}

/// Who you are on the map. The coordinates the geo-IP gave are used for city hints and never stored.
#[derive(Clone, Debug, Default, Serialize)]
pub struct You {
    pub asn: Option<u32>,
    pub holder: Option<String>,
    /// Who the number is registered to, when that is another organisation than the name (see `Group::owner`).
    pub owner: Option<String>,
    pub city: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AsrelInfo {
    pub fetched_at: Option<i64>,
    pub rows: usize,
    pub source: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Pfx2asInfo {
    pub fetched_at: Option<i64>,
    pub rows: usize,
    pub source: Option<String>,
    pub rows6: usize,
    pub source6: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PdbInfo {
    pub accepted: bool,
    pub synced_at: Option<i64>,
    pub rows: usize,
}

/// The data the map reads besides the traces, and whether router names are decoded.
/// The registry lookups: allowed or not, and how many blocks are kept.
#[derive(Clone, Debug, Serialize)]
pub struct RdapInfo {
    pub accepted: bool,
    pub blocks: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Datasets {
    pub pfx2as: Pfx2asInfo,
    pub asrel: AsrelInfo,
    pub peeringdb: PdbInfo,
    pub rdns: bool,
    pub rdap: RdapInfo,
}

#[derive(Clone, Debug, Serialize)]
pub struct MapRun {
    pub ts: i64,
    pub elapsed_ms: u64,
    pub you: You,
    pub traces: Vec<MapTrace>,
    /// The run traced over IPv6 too.
    pub ipv6: bool,
    pub groups: Vec<Group>,
    pub edges: Vec<Link>,
    pub claims: Vec<Claim>,
    pub headline: Option<Headline>,
    pub skipped: Vec<Skipped>,
    pub datasets: Datasets,
}

/// How far a run has got, with the map as it stands.
#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    /// The target whose trace finished last.
    pub target: String,
    pub partial: Option<MapRun>,
}

/// What the app keeps between requests.
#[derive(Default)]
pub struct State {
    pub progress: Arc<Mutex<Option<Progress>>>,
    pub cancel: Arc<AtomicBool>,
    /// Why the last run failed, until the next starts.
    pub error: Mutex<Option<String>>,
    /// A run is going, from the request to the end of its post-processing.
    pub running: AtomicBool,
    /// A dataset download is going.
    fetching: AtomicBool,
}

pub fn datasets(store: &crate::store::Store) -> Datasets {
    let (synced_at, rows) = Pdb::stats(store);
    Datasets {
        pfx2as: Pfx2asInfo {
            fetched_at: store.get(attr::FETCHED_AT),
            rows: store.pfx2as_count(),
            source: store.get(attr::SOURCE),
            rows6: store.pfx2as6_count(),
            source6: store.get(attr::SOURCE6),
        },
        asrel: AsrelInfo { fetched_at: store.get(asrel::FETCHED_AT), rows: store.asrel_count(), source: store.get(asrel::SOURCE) },
        peeringdb: PdbInfo { accepted: store.get(pdb::ACCEPTED) == Some(true), synced_at, rows },
        rdns: store.get(RDNS) == Some(true),
        rdap: RdapInfo { accepted: store.get(rdap::ACCEPTED) == Some(true), blocks: rdap::count(store) },
    }
}

/// The stored run as JSON, as it was saved.
pub fn last(store: &crate::store::Store) -> Option<serde_json::Value> {
    serde_json::from_str(&store.netmap_last()?.1).ok()
}

/// The traces of the stored run, when it is under a week old: the border monitor finds its pairs in them rather than
/// tracing again.
pub(crate) fn latest_traces(store: &crate::store::Store, now: i64) -> Option<Vec<MapTrace>> {
    #[derive(Deserialize)]
    struct Stored {
        traces: Vec<MapTrace>,
    }
    let (ts, body) = store.netmap_last()?;
    if now - ts > FRESH_MS {
        return None;
    }
    serde_json::from_str::<Stored>(&body).ok().map(|s| s.traces)
}

// ---------------------------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------------------------

/// A flag held for as long as this lives; the `measuring` flag is set for the trace phase only.
struct Held(Arc<AtomicBool>);

impl Held {
    fn set(flag: &Arc<AtomicBool>) -> Held {
        flag.store(true, Ordering::SeqCst);
        Held(flag.clone())
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn gunzip(body: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(body).take(BODY_MAX as u64).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// GET `url`, refusing while a measurement runs. `gzip` asks for a compressed body and unpacks it; reqwest here has no
/// gzip support of its own, and the PeeringDB listings are large.
async fn get(app: &AppState, url: String, gzip: bool) -> Result<Vec<u8>, String> {
    // A measurement owns the network while it runs. Wait for it rather than give up: a sync that stops halfway leaves
    // the tables short until the next refresh.
    let mut waited = Duration::ZERO;
    while app.measuring.load(Ordering::SeqCst) || lock::busy() {
        if waited >= Duration::from_secs(600) {
            return Err("A measurement is running; the download waits.".into());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        waited += Duration::from_secs(1);
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("recursa/", env!("CARGO_PKG_VERSION"), " (+https://recursa.app)"))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(&url);
    if gzip {
        req = req.header("accept-encoding", "gzip");
    }
    let resp = req.send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{url}: HTTP {}", resp.status().as_u16()));
    }
    let packed = resp.headers().get("content-encoding").and_then(|v| v.to_str().ok()).is_some_and(|v| v.eq_ignore_ascii_case("gzip"));
    let body = resp.bytes().await.map_err(|e| format!("{url}: {e}"))?;
    if body.len() > BODY_MAX {
        return Err(format!("{url}: the download is too big."));
    }
    if gzip && packed { gunzip(&body) } else { Ok(body.to_vec()) }
}

/// Fetch the prefix-to-AS table now.
async fn fetch_pfx2as(app: &Arc<AppState>) -> Result<(), String> {
    attr::refresh(&app.store, |url| {
        let app = app.clone();
        async move { get(&app, url, false).await }
    })
    .await
}

/// Fetch the AS relationships table now. The body is bzip2; `asrel::refresh` unpacks it.
async fn fetch_asrel(app: &Arc<AppState>) -> Result<(), String> {
    asrel::refresh(&app.store, |url| {
        let app = app.clone();
        async move { get(&app, url, false).await }
    })
    .await
}

/// Fetch whatever is stale, on its own time: the prefix table daily, the AS relationships monthly, and PeeringDB's changes once the user allowed it.
/// Waits for a running measurement, and is skipped while another download is going.
pub(crate) fn refresh_in_background(app: &Arc<AppState>) {
    let app = app.clone();
    tokio::spawn(async move {
        if app.netmap.fetching.swap(true, Ordering::SeqCst) {
            return;
        }
        let now = bench::now_ms();
        let stale = |at: Option<i64>| at.is_none_or(|t| now - t > REFRESH_MS);
        if app.store.pfx2as_count() == 0 || app.store.pfx2as6_count() == 0 || stale(app.store.get(attr::FETCHED_AT)) {
            let _ = fetch_pfx2as(&app).await;
        }
        if app.store.asrel_count() == 0 || app.store.get::<i64>(asrel::FETCHED_AT).is_none_or(|t| now - t > asrel::REFRESH_MS) {
            let _ = fetch_asrel(&app).await;
        }
        if app.store.get::<bool>(pdb::ACCEPTED) == Some(true) && stale(Pdb::stats(&app.store).0) {
            let fetch = |url: String| {
                let app = app.clone();
                async move { get(&app, url, true).await }
            };
            if let Err(e) = pdb::sync(&app.store, fetch, pdb::PAUSE).await {
                eprintln!("PeeringDB sync stopped: {e}");
            }
        }
        app.netmap.fetching.store(false, Ordering::SeqCst);
    });
}

// ---------------------------------------------------------------------------------------------
// A run
// ---------------------------------------------------------------------------------------------

/// Whether this computer has a global IPv6 route. Connecting a UDP socket only picks the route and the source address:
/// no packet is sent.
fn ipv6_route() -> bool {
    let Ok(sock) = std::net::UdpSocket::bind("[::]:0") else { return false };
    if sock.connect("[2001:4860:4860::8888]:53").is_err() {
        return false;
    }
    match sock.local_addr() {
        Ok(std::net::SocketAddr::V6(a)) => {
            let ip = a.ip();
            !(ip.is_unspecified() || ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local())
        }
        _ => false,
    }
}

/// The first IPv4 address of `host`.
fn first_a(res: &Resolve, host: &str) -> Result<IpAddr, String> {
    let m = res.query(host, dns::TYPE_A)?;
    m.answers.iter().find_map(|r| if let dns::RData::A(a) = &r.data { Some(IpAddr::V4(*a)) } else { None }).ok_or("No IPv4 address.".into())
}

/// The first IPv6 address of `host`, if it has one.
fn first_aaaa(res: &Resolve, host: &str) -> Option<IpAddr> {
    let m = res.query(host, dns::TYPE_AAAA).ok()?;
    m.answers.iter().find_map(|r| if let dns::RData::Aaaa(a) = &r.data { Some(IpAddr::V6(*a)) } else { None })
}

/// The addresses to trace to, and the targets left out. With `v6`, each destination that has an IPv6 address gets a
/// second target right after its IPv4 one; one without is traced over IPv4 only and not listed as skipped.
fn resolve_targets(res: &Resolve, system: &[IpAddr], roll: &crate::edge::RollCall, v6: bool) -> (Vec<Target>, Vec<Skipped>) {
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    for s in TARGETS {
        let mut site = None;
        let edge = match &s.via {
            Via::Edge(cdn) => roll.edges.iter().find(|e| e.cdn == *cdn),
            _ => None,
        };
        if let Some(e) = edge {
            site = e.site_city.clone().or_else(|| e.site.clone());
        }
        let found: Result<IpAddr, String> = match &s.via {
            Via::Edge(_) => match edge {
                Some(e) => match e.ip {
                    Some(ip @ IpAddr::V4(_)) => Ok(ip),
                    // The roll call connected over IPv6: the IPv4 target is still wanted.
                    Some(_) => first_a(res, s.host),
                    None => Err(e.error.clone().unwrap_or_else(|| "The roll call got no address.".into())),
                },
                None => Err("Not in the roll call.".into()),
            },
            Via::Name => first_a(res, s.host),
            Via::System => system.iter().find(|a| a.is_ipv4()).copied().ok_or_else(|| "No IPv4 system resolver.".to_string()),
            Via::Addr(a, _) => a.parse().map_err(|_| "Bad address.".to_string()),
        };
        let target = |id: String, addr: IpAddr, family: u8| Target {
            id,
            cdn: s.cdn.map(str::to_string),
            name: s.name.into(),
            host: s.host.into(),
            addr,
            family,
            site: site.clone(),
        };
        match found {
            Ok(addr) => targets.push(target(s.id.into(), addr, 4)),
            Err(reason) => skipped.push(Skipped { target: s.id.into(), reason }),
        }
        if v6 {
            let addr6 = match &s.via {
                Via::Edge(_) => match edge.and_then(|e| e.ip) {
                    Some(ip @ IpAddr::V6(_)) => Some(ip),
                    _ => first_aaaa(res, s.host),
                },
                Via::Name => first_aaaa(res, s.host),
                Via::System => system.iter().find(|a| a.is_ipv6()).copied(),
                Via::Addr(_, a) => a.parse().ok(),
            };
            if let Some(addr) = addr6 {
                targets.push(target(format!("{}-v6", s.id), addr, 6));
            }
        }
    }
    (targets, skipped)
}

/// A network's registered name: kept 30 days, asked for only when `online` (one question per network, never per hop).
fn holder_of(app: &AppState, res: &Resolve, asn: u32, online: bool) -> Option<String> {
    let key = format!("asname:{asn}");
    if let Some((v, _)) = app.store.cache_get(&key, NAME_MAX_AGE_MS) {
        return v.as_str().map(str::to_string);
    }
    if !online {
        return None;
    }
    let name = cymru::name_of(res, asn).ok().flatten().map(|n| cymru::holder_name(&n))?;
    app.store.cache_put(&key, &serde_json::json!(name));
    Some(name)
}

/// Name each hop's network, and return the names of the networks of destinations that never answered: those have no
/// router of their own to carry the name.
fn fill_holders(traces: &mut [MapTrace], pfx: &Pfx2as, mut name: impl FnMut(u32) -> Option<String>) -> HashMap<u32, String> {
    let mut seen: HashMap<u32, Option<String>> = HashMap::new();
    for h in traces.iter_mut().flat_map(|t| t.trace.hops.iter_mut()) {
        if let Some(asn) = h.asn {
            h.holder = seen.entry(asn).or_insert_with(|| name(asn)).clone();
        }
    }
    let ends: Vec<u32> = traces.iter().filter(|t| !t.trace.reached).filter_map(|t| pfx.asn_of(t.trace.target)).collect();
    ends.into_iter().filter_map(|a| seen.entry(a).or_insert_with(|| name(a)).clone().map(|n| (a, n))).collect()
}

struct Inputs<'a> {
    app: &'a AppState,
    pfx: &'a Pfx2as,
    pdb: &'a Pdb,
    you: &'a You,
    coords: Option<(f64, f64)>,
    /// The run traces over IPv6 too.
    ipv6: bool,
}

/// Group, merge and make the claims for `traces`.
#[allow(clippy::too_many_arguments)]
fn assemble(
    inp: &Inputs,
    ts: i64,
    elapsed_ms: u64,
    mut traces: Vec<MapTrace>,
    skipped: Vec<Skipped>,
    names: &HashMap<IpAddr, String>,
    registry: &HashMap<IpAddr, rdap::Block>,
    owners: &HashMap<u32, String>,
    holders: &HashMap<u32, String>,
) -> MapRun {
    let cx = Ctx { pfx: inp.pfx, pdb: inp.pdb, home: inp.you.asn, names, you: inp.coords, registry, holders };
    let segs: Vec<Vec<attr::Seg>> = traces.iter().map(|t| attr::group(&t.trace, &cx)).collect();
    // An IPv6 trace that crosses the same networks as the IPv4 trace of its target is listed, not drawn.
    for i in 0..traces.len() {
        if traces[i].target.family != 6 {
            continue;
        }
        let v4 = traces[i].target.id.trim_end_matches("-v6");
        let sibling = traces.iter().position(|t| t.target.family == 4 && t.target.id == v4);
        traces[i].same_route = Some(sibling.is_some_and(|j| attr::route_key(&segs[i]) == attr::route_key(&segs[j])));
    }
    let drawn: Vec<usize> = (0..traces.len()).filter(|&i| traces[i].same_route != Some(true)).collect();
    let paths: Vec<(&str, Vec<attr::Seg>)> = drawn.iter().map(|&i| (traces[i].target.id.as_str(), segs[i].clone())).collect();
    let mut asns: Vec<u32> = traces.iter().flat_map(|t| t.trace.hops.iter().filter_map(|h| h.asn)).collect();
    asns.sort_unstable();
    asns.dedup();
    let rels = asrel::AsRel::for_asns(&inp.app.store, &asns);
    let (mut groups, edges, ids) = attr::merge(&paths, &rels);
    for g in &mut groups {
        g.owner = g.asn.and_then(|a| owners.get(&a)).filter(|o| attr::distinct_owner(g.holder.as_deref(), o)).cloned();
    }
    let month = inp.app.store.get::<String>(asrel::SOURCE).and_then(|s| asrel::month_of(&s));
    let say_paths: Vec<say::Path> = paths
        .iter()
        .zip(&ids)
        .zip(&drawn)
        .filter(|(_, i)| traces[**i].target.family == 4)
        .map(|(((id, segs), g), i)| say::Path { id, name: &traces[*i].target.name, segs, groups: g })
        .collect();
    let (claims, headline) = say::say(&say_paths, &groups, &edges, inp.you.asn, inp.pdb, month.as_deref());
    drop(say_paths);
    drop(paths);
    MapRun {
        ts,
        elapsed_ms,
        you: inp.you.clone(),
        traces,
        ipv6: inp.ipv6,
        groups,
        edges,
        claims,
        headline,
        skipped,
        datasets: datasets(&inp.app.store),
    }
}

/// The router addresses in unknown networks after the last run of your ISP; none before it, and none of yours.
fn unknown_past_isp(segs: &[attr::Seg]) -> Vec<IpAddr> {
    let Some(isp) = segs.iter().rposition(|s| s.role == attr::Role::Isp) else { return Vec::new() };
    segs[isp + 1..].iter().filter(|s| s.role == attr::Role::Unknown).flat_map(|s| s.members.iter().map(|m| m.addr)).collect()
}

/// What the tracers have finished: traces with their target's index, the failures, and how many are done.
#[derive(Default)]
struct Finished {
    traces: Vec<(usize, MapTrace)>,
    failed: Vec<Skipped>,
    count: usize,
}

/// Trace to every target, [`PARALLEL`] at a time. Blocks. Each finished trace updates the progress with the map so far.
fn trace_all(inp: &Inputs, ts: i64, t0: Instant, targets: &[Target], skipped: &mut Vec<Skipped>) -> Vec<MapTrace> {
    let state = &inp.app.netmap;
    let next = AtomicUsize::new(0);
    // When the next tracer may start.
    let gate = Mutex::new(Instant::now());
    let done = Mutex::new(Finished::default());
    let res = Resolve::new(inp.app.system.read().unwrap().clone());
    std::thread::scope(|s| {
        for _ in 0..PARALLEL {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= targets.len() || state.cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    let wait = {
                        let mut g = gate.lock().unwrap();
                        let at = (*g).max(Instant::now());
                        *g = at + STAGGER;
                        at - Instant::now()
                    };
                    std::thread::sleep(wait);
                    let target = &targets[i];
                    let outcome = trace::run_live(target.addr, ID_BASE + i as u16, &|_| {});
                    let mut d = done.lock().unwrap();
                    match outcome {
                        Ok(mut trace) => {
                            attr::attribute(&mut trace, inp.pfx);
                            d.traces.push((i, MapTrace { target: target.clone(), trace, same_route: None }));
                        }
                        Err(reason) => d.failed.push(Skipped { target: target.id.clone(), reason }),
                    }
                    d.count += 1;
                    let mut so_far: Vec<(usize, MapTrace)> = d.traces.clone();
                    so_far.sort_by_key(|(i, _)| *i);
                    let mut traces: Vec<MapTrace> = so_far.into_iter().map(|(_, t)| t).collect();
                    let holders = fill_holders(&mut traces, inp.pfx, |asn| holder_of(inp.app, &res, asn, false));
                    let none = HashMap::new();
                    let partial = assemble(
                        inp,
                        ts,
                        t0.elapsed().as_millis() as u64,
                        traces,
                        d.failed.clone(),
                        &none,
                        &HashMap::new(),
                        &HashMap::new(),
                        &holders,
                    );
                    *state.progress.lock().unwrap() =
                        Some(Progress { done: d.count, total: targets.len(), target: target.id.clone(), partial: Some(partial) });
                }
            });
        }
    });
    let Finished { traces: mut got, failed, .. } = done.into_inner().unwrap();
    skipped.extend(failed);
    got.sort_by_key(|(i, _)| *i);
    got.into_iter().map(|(_, t)| t).collect()
}

/// Run the map. The caller holds the run slot and the machine-wide measure lock (see `AppState::start_netmap`).
pub(crate) async fn run(app: &Arc<AppState>) -> Result<MapRun, String> {
    match trace::available() {
        TraceCap::Ok => {}
        TraceCap::NeedsAdmin => return Err("Traces need administrator rights on this system.".into()),
        TraceCap::Unsupported => return Err("Traces are not available on this system.".into()),
    }
    let (t0, ts) = (Instant::now(), bench::now_ms());
    app.netmap.cancel.store(false, Ordering::SeqCst);
    *app.netmap.progress.lock().unwrap() = None;
    if app.store.pfx2as_count() == 0 || app.store.pfx2as6_count() == 0 {
        fetch_pfx2as(app).await.map_err(|e| format!("The prefix-to-AS table could not be fetched: {e}"))?;
    }
    let roll = app.run_roll_call().await;
    let you = You { asn: roll.you.asn, holder: roll.you.holder.clone(), owner: None, city: roll.you.city.clone() };
    let coords = roll.you.lat.zip(roll.you.lon);
    let a = app.clone();
    let res = Resolve::new(app.system.read().unwrap().clone());
    let (targets, mut skipped, pfx, pdb, ipv6) = tokio::task::spawn_blocking(move || {
        let system = a.system.read().unwrap().clone();
        let ipv6 = ipv6_route();
        let (targets, skipped) = resolve_targets(&Resolve::new(system.clone()), &system, &roll, ipv6);
        // PeeringDB tables already on disk stay unused while the user has it off.
        let pdb = if a.store.get::<bool>(pdb::ACCEPTED) == Some(true) { Pdb::load(&a.store) } else { Pdb::default() };
        (targets, skipped, Arc::new(Pfx2as::load(&a.store)), Arc::new(pdb), ipv6)
    })
    .await
    .map_err(|e| e.to_string())?;
    if pfx.is_empty() {
        return Err("The prefix-to-AS table is empty.".into());
    }
    // The measuring flag covers the trace phase only: the name lookups after it are not timed.
    let measuring = Held::set(&app.measuring);
    let (a, y, p, d) = (app.clone(), you.clone(), pfx.clone(), pdb.clone());
    let (mut traces, skipped_by_trace) = tokio::task::spawn_blocking(move || {
        let inp = Inputs { app: &a, pfx: &p, pdb: &d, you: &y, coords, ipv6 };
        let mut skipped = Vec::new();
        let traces = trace_all(&inp, ts, t0, &targets, &mut skipped);
        (traces, skipped)
    })
    .await
    .map_err(|e| e.to_string())?;
    drop(measuring);
    skipped.extend(skipped_by_trace);
    if app.netmap.cancel.load(Ordering::SeqCst) {
        return Err("Cancelled.".into());
    }
    // Addresses past your ISP that no network announces: ask the registry who holds their block, if allowed.
    let registry = {
        let (names, blocks, holders) = (HashMap::new(), HashMap::new(), HashMap::new());
        let cx = Ctx { pfx: &pfx, pdb: &pdb, home: you.asn, names: &names, you: coords, registry: &blocks, holders: &holders };
        let mut addrs: Vec<IpAddr> = traces.iter().flat_map(|t| unknown_past_isp(&attr::group(&t.trace, &cx))).collect();
        addrs.sort();
        addrs.dedup();
        rdap::lookup(&app.store, &addrs, &app.netmap.cancel, |url| {
            let app = app.clone();
            async move { get(&app, url, false).await }
        })
        .await
    };
    // Who each network's number is registered to, if allowed: the request carries the number only.
    let owners = {
        let mut asns: Vec<u32> = traces.iter().flat_map(|t| t.trace.hops.iter().filter_map(|h| h.asn)).chain(you.asn).collect();
        asns.sort_unstable();
        asns.dedup();
        rdap::owners(&app.store, &asns, &app.netmap.cancel, |url| {
            let app = app.clone();
            async move { get(&app, url, false).await }
        })
        .await
    };
    let (a, y, p, d) = (app.clone(), you, pfx, pdb);
    let run = tokio::task::spawn_blocking(move || {
        let holders = fill_holders(&mut traces, &p, |asn| holder_of(&a, &res, asn, true));
        let mut y = y;
        if let (Some(asn), None) = (y.asn, &y.holder) {
            y.holder = holder_of(&a, &res, asn, true);
        }
        y.owner = y.asn.and_then(|a| owners.get(&a)).filter(|o| attr::distinct_owner(y.holder.as_deref(), o)).cloned();
        let mut names = HashMap::new();
        if a.store.get::<bool>(RDNS) == Some(true) {
            let mut addrs: Vec<IpAddr> = traces
                .iter()
                .flat_map(|t| t.trace.hops.iter().flat_map(|h| h.addrs.iter().copied()))
                .filter(|ip| !is_private(ip))
                .collect();
            addrs.sort();
            addrs.dedup();
            for ip in addrs {
                if a.netmap.cancel.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(name) = crate::edge::ptr(&res, ip) {
                    names.insert(ip, name);
                }
            }
        }
        let inp = Inputs { app: &a, pfx: &p, pdb: &d, you: &y, coords, ipv6 };
        assemble(&inp, ts, t0.elapsed().as_millis() as u64, traces, skipped, &names, &registry, &owners, &holders)
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Ok(body) = serde_json::to_string(&run) {
        app.store.netmap_put(ts, &body);
    }
    refresh_in_background(app);
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(cdn: &'static str, ip: &str) -> crate::edge::EdgeSite {
        crate::edge::EdgeSite {
            cdn,
            name: "Cloudflare",
            host: "speed.cloudflare.com",
            method: "test",
            site: Some("IAD".into()),
            site_raw: None,
            site_city: None,
            site_country: None,
            ip: ip.parse().ok(),
            asn: None,
            holder: None,
            rtt_ms: None,
            floor_ms: None,
            quality: None,
            error: None,
        }
    }

    #[test]
    fn ipv6_targets_follow_their_ipv4_ones_only_when_asked() {
        // No system resolver: names do not resolve, so only the literals and the roll call's own address are left.
        let res = Resolve::new(Vec::new());
        let system: Vec<IpAddr> = vec!["fe80::1".parse().unwrap()];
        let roll = crate::edge::RollCall { edges: vec![edge("cloudflare", "2606:4700::6810:84e5")], ..Default::default() };
        let (v4_only, _) = resolve_targets(&res, &system, &roll, false);
        assert!(v4_only.iter().all(|t| t.family == 4 && !t.id.ends_with("-v6")));
        assert_eq!(v4_only.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["cloudflare-dns", "google-dns"]);

        let (both, skipped) = resolve_targets(&res, &system, &roll, true);
        let ids: Vec<&str> = both.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["cloudflare-v6", "system-v6", "cloudflare-dns", "cloudflare-dns-v6", "google-dns", "google-dns-v6"]);
        let v6 = both.iter().find(|t| t.id == "cloudflare-v6").unwrap();
        assert_eq!(
            (v6.family, v6.addr, v6.cdn.as_deref(), v6.site.as_deref()),
            (6, "2606:4700::6810:84e5".parse().unwrap(), Some("cloudflare"), Some("IAD"))
        );
        // The IPv4 target of an edge the roll call reached over IPv6 is skipped only because no A record came back.
        assert!(skipped.iter().any(|s| s.target == "cloudflare" && s.reason == "no system resolver found"));
        // No AAAA record is no IPv6 target and no skipped row.
        assert!(skipped.iter().all(|s| !s.target.ends_with("-v6")));
    }
}
