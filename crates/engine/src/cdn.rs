//! CDN mapping quality: does a resolver send this client to a nearby CDN edge?
//!
//! DNS-steered CDNs choose an edge from the address the query arrives from: the
//! client's subnet when the resolver forwards EDNS Client Subnet and the CDN honors it,
//! otherwise the resolver's own egress. Every resolver resolves several hostnames per CDN
//! (twice, interleaved, to catch rotation). Each CDN's own nameserver is also asked
//! directly from this computer, which shows the edge the CDN picks for the client's own
//! address: what a resolver forwarding a usable ECS should get. The edge addresses are
//! then timed with TCP handshakes (SYN → SYN-ACK = one network round trip) in shuffled
//! order, three passes, keeping the minimum.
//!
//! Scoring is penalty-first: for each hostname, a resolver's penalty is how much slower
//! its edges are than the best any resolver (or the CDN's own answer) found for that same
//! name. A hostname that is slow for everyone therefore adds nothing. A CDN's penalty is the
//! median over its hostnames, and a resolver's headline is the mean over the steered CDNs
//! that every ranked resolver has. Anycast controls are shown but never enter the headline.

use crate::clock;
use crate::dns::{self, QuerySpec, RData};
use crate::net::{self, Endpoint, Exchanger, Transport};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// DNS decides the edge: resolver choice matters.
    Steered,
    /// Anycast: every resolver should get the same edges. Shown, never scored in the headline.
    Control,
}

pub struct Host {
    pub name: &'static str,
    /// The CNAME tail (or the name itself) must end with one of these.
    pub suffixes: &'static [&'static str],
}

pub struct Cdn {
    pub id: &'static str,
    pub name: &'static str,
    pub role: Role,
    pub hosts: &'static [Host],
}

pub const CDNS: &[Cdn] = &[
    Cdn {
        id: "akamai",
        name: "Akamai",
        role: Role::Steered,
        hosts: &[
            Host { name: "www.apple.com", suffixes: &["akamaiedge.net", "akamai.net"] },
            Host { name: "www.microsoft.com", suffixes: &["akamaiedge.net", "akamai.net"] },
            Host { name: "www.oracle.com", suffixes: &["akamaiedge.net", "akamai.net"] },
            Host { name: "www.adobe.com", suffixes: &["akamaiedge.net", "akamai.net"] },
            Host { name: "www.nvidia.com", suffixes: &["akamaiedge.net", "akamai.net"] },
        ],
    },
    Cdn {
        id: "cloudfront",
        name: "Amazon CloudFront",
        role: Role::Steered,
        hosts: &[
            Host { name: "d1.awsstatic.com", suffixes: &["cloudfront.net", "a2z.com", "awsstatic.com"] },
            Host { name: "aws.amazon.com", suffixes: &["cloudfront.net", "a2z.com", "awsstatic.com"] },
            Host { name: "docs.aws.amazon.com", suffixes: &["cloudfront.net", "a2z.com", "awsstatic.com"] },
        ],
    },
    Cdn {
        id: "google",
        name: "Google",
        role: Role::Steered,
        hosts: &[
            Host { name: "www.gstatic.com", suffixes: &["gstatic.com", "googleusercontent.com", "ytimg.com"] },
            Host { name: "lh3.googleusercontent.com", suffixes: &["gstatic.com", "googleusercontent.com", "ytimg.com"] },
            Host { name: "i.ytimg.com", suffixes: &["gstatic.com", "googleusercontent.com", "ytimg.com"] },
        ],
    },
    Cdn {
        id: "meta",
        name: "Meta",
        role: Role::Steered,
        hosts: &[
            Host { name: "star-mini.c10r.facebook.com", suffixes: &["facebook.com", "fbcdn.net", "cdninstagram.com"] },
            Host { name: "scontent.xx.fbcdn.net", suffixes: &["facebook.com", "fbcdn.net", "cdninstagram.com"] },
            Host { name: "scontent.cdninstagram.com", suffixes: &["facebook.com", "fbcdn.net", "cdninstagram.com"] },
        ],
    },
    Cdn {
        id: "fastly",
        name: "Fastly (anycast control)",
        role: Role::Control,
        hosts: &[Host { name: "www.reddit.com", suffixes: &["fastly.net"] }],
    },
    Cdn {
        id: "cloudflare",
        name: "Cloudflare (anycast control)",
        role: Role::Control,
        hosts: &[Host { name: "www.cloudflare.com", suffixes: &["cloudflare.com"] }],
    },
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostStatus {
    #[default]
    Ok,
    /// Timeout or SERVFAIL, twice.
    Failed,
    /// Private, loopback or unspecified addresses only, or NXDOMAIN/REFUSED on both tries.
    Blocked,
    /// No address completed a TCP handshake.
    Unreachable,
    /// Answered by a network that isn't this CDN's.
    OffCdn,
}

/// How a resolver's mapping compares with the CDN's own answer for this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AsYou {
    Same,
    Near,
    Different,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Edge {
    pub ip: String,
    /// Minimum handshake RTT (ms): the best estimate of path latency.
    pub rtt_ms: Option<f64>,
    pub median_ms: Option<f64>,
    pub samples: Vec<f64>,
    /// The edge answered with a reset: still a round trip, so it counts as a sample.
    pub refused: bool,
    pub asn: Option<u64>,
    pub holder: Option<String>,
    /// The edge sits inside the client's own network (an embedded cache).
    pub on_net: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct HostMapping {
    pub ips: Vec<String>,
    /// Last name in the CNAME chain: the name the CDN's own nameservers answer for.
    pub target: Option<String>,
    /// Repeated lookups returned different edges (the CDN rotates its answers).
    pub rotated: bool,
    pub status: HostStatus,
    pub error: Option<String>,
    /// S(r,h): median over the edges of each one's best round trip.
    pub rtt_ms: Option<f64>,
    /// p(r,h) = S(r,h) minus the best any resolver (or the CDN's own answer) found for this name.
    pub penalty_ms: Option<f64>,
    /// The penalty is under the noise floor (or 5% of the best).
    pub same: Option<bool>,
    pub as_you: Option<AsYou>,
}

/// The CDN's own answer for this computer: its authoritative nameserver asked directly,
/// with no resolver in between, so the CDN maps the client's own address.
#[derive(Clone, Debug, Serialize, Default)]
pub struct Reference {
    pub name: String,
    /// Nameserver host and address that answered.
    pub server: String,
    pub ips: Vec<String>,
    pub rtt_ms: Option<f64>,
    pub penalty_ms: Option<f64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct CdnScore {
    /// Median of S over the Ok hostnames.
    pub rtt_ms: Option<f64>,
    /// P(r,c): median of p over the Ok hostnames; needs at least two.
    pub penalty_ms: Option<f64>,
    pub as_you: Option<AsYou>,
    pub worst_host: Option<(String, f64)>,
    pub hosts_used: u8,
    pub hosts_total: u8,
    pub error: Option<String>,
    pub hosts: BTreeMap<String, HostMapping>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Coverage {
    /// Steered CDNs with a penalty, of all steered CDNs.
    pub cdns_used: u8,
    pub cdns_total: u8,
    /// Ok hostnames in steered CDNs, of all of them.
    pub hosts_used: u8,
    pub hosts_total: u8,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ResolverCdn {
    pub mappings: BTreeMap<String, CdnScore>,
    /// The headline H(r): mean penalty over the steered CDNs every ranked resolver has.
    pub penalty_ms: Option<f64>,
    /// 1-based; resolvers within the noise of each other share a tier.
    pub tier: Option<u8>,
    pub worst: Option<(String, f64)>,
    pub coverage: Coverage,
    /// on_net | ecs | resolver_location | unknown
    pub basis: String,
    /// ECS seen by Google's nameservers, and by Akamai's (they can differ).
    pub ecs: Option<String>,
    pub ecs_akamai: Option<String>,
    /// Whether the subnet the resolver sends actually contains the client.
    pub ecs_is_client: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HostInfo {
    pub name: &'static str,
    /// Every resolver (and the CDN's own answer) returned the same edges.
    pub uniform: bool,
    /// A steered name that came back uniform, or an anycast control that didn't.
    pub unexpected: bool,
    /// What the CDN gives this computer's own address, when its nameserver answered.
    pub you: Option<Reference>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CdnInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub role: Role,
    pub hosts: Vec<HostInfo>,
    pub best_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct CdnReport {
    pub version: u8,
    /// δ: how far two handshakes to the same edge differ by chance in this run.
    pub noise_ms: f64,
    pub cdns: Vec<CdnInfo>,
    pub edges: BTreeMap<String, Edge>,
    pub resolvers: BTreeMap<String, ResolverCdn>,
}

/// Resolver id → CDN id → hostname → mapping.
pub type Answers = BTreeMap<String, BTreeMap<String, BTreeMap<String, HostMapping>>>;

fn median(v: &mut [f64]) -> Option<f64> {
    v.sort_by(|a, b| a.total_cmp(b));
    (!v.is_empty()).then(|| crate::stats::quantile(v, 0.5))
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Names of the table in lookup order: hostnames interleaved across CDNs, so no CDN is
/// asked in a burst.
fn interleaved() -> Vec<(&'static Cdn, &'static Host)> {
    let widest = CDNS.iter().map(|c| c.hosts.len()).max().unwrap_or(0);
    (0..widest).flat_map(|i| CDNS.iter().filter_map(move |c| c.hosts.get(i).map(|h| (c, h)))).collect()
}

fn on_cdn(tail: &str, h: &Host) -> bool {
    h.suffixes.iter().any(|s| tail == *s || tail.strip_suffix(s).is_some_and(|rest| rest.ends_with('.')))
}

/// What one hostname returned across the lookups of a run.
#[derive(Default)]
struct Seen {
    sets: Vec<Vec<String>>,
    target: Option<String>,
    blocked: Option<String>,
    failed: Option<String>,
}

impl Seen {
    fn record(&mut self, o: Outcome) {
        match o {
            Outcome::Answer(ips, target) => {
                self.sets.push(ips);
                self.target = self.target.take().or(Some(target));
            }
            Outcome::Blocked(e) => self.blocked = Some(e),
            Outcome::Failed(e) => self.failed = Some(e),
        }
    }

    fn finish(self, h: &Host) -> HostMapping {
        let Seen { sets, target, blocked, failed } = self;
        // Keep the first answer's order (a resolver's preferred edge first), then
        // whatever later lookups added.
        let mut ips: Vec<String> = Vec::new();
        for set in &sets {
            for ip in set {
                if !ips.contains(ip) {
                    ips.push(ip.clone());
                }
            }
        }
        ips.truncate(4);
        if ips.is_empty() {
            return match blocked {
                Some(e) => HostMapping { status: HostStatus::Blocked, error: Some(e), ..Default::default() },
                None => HostMapping {
                    status: HostStatus::Failed,
                    error: Some(failed.unwrap_or_else(|| "no usable address".into())),
                    ..Default::default()
                },
            };
        }
        let rotated = sets.windows(2).any(|w| w[0].iter().collect::<BTreeSet<_>>() != w[1].iter().collect::<BTreeSet<_>>());
        let tail = target.clone().unwrap_or_else(|| h.name.to_string());
        let (status, error) =
            if on_cdn(&tail, h) { (HostStatus::Ok, None) } else { (HostStatus::OffCdn, Some(format!("answered by {tail}, not this CDN"))) };
        HostMapping { ips, target, rotated, status, error, ..Default::default() }
    }
}

/// Resolve every CDN hostname through one resolver, `rounds` times, interleaved.
/// Returns CDN id → hostname → mapping.
pub fn resolve_all(ex: &mut dyn Exchanger, timeout: Duration, rounds: u32) -> BTreeMap<String, BTreeMap<String, HostMapping>> {
    let order = interleaved();
    let mut seen: BTreeMap<&str, Seen> = BTreeMap::new();
    for _ in 0..rounds.max(1) {
        for (_, h) in &order {
            seen.entry(h.name).or_default().record(lookup(ex, h.name, timeout));
        }
    }
    CDNS.iter()
        .map(|c| {
            let hosts = c.hosts.iter().map(|h| (h.name.to_string(), seen.remove(h.name).unwrap_or_default().finish(h))).collect();
            (c.id.to_string(), hosts)
        })
        .collect()
}

enum Outcome {
    /// Up to two usable IPv4 edges and the final name in the CNAME chain.
    Answer(Vec<String>, String),
    Blocked(String),
    Failed(String),
}

/// An address a CDN edge could really have.
fn usable(a: &Ipv4Addr) -> bool {
    !crate::catalog::is_local_or_special_but_cgnat(&IpAddr::V4(*a))
}

/// One A lookup, tried a second time at once if the first didn't answer. The final try counts.
fn lookup(ex: &mut dyn Exchanger, host: &str, timeout: Duration) -> Outcome {
    match lookup_once(ex, host, timeout) {
        Outcome::Answer(ips, target) => Outcome::Answer(ips, target),
        _ => lookup_once(ex, host, timeout),
    }
}

fn lookup_once(ex: &mut dyn Exchanger, host: &str, timeout: Duration) -> Outcome {
    let msg = match dns::ask(ex, &QuerySpec::new(host, dns::TYPE_A), timeout) {
        Ok(a) => a.msg,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    match msg.rcode {
        dns::RCODE_NOERROR => {}
        dns::RCODE_NXDOMAIN | dns::RCODE_REFUSED => return Outcome::Blocked(dns::rcode_name(msg.rcode).into()),
        rc => return Outcome::Failed(dns::rcode_name(rc).into()),
    }
    let mut target = host.to_string();
    let mut ips: Vec<String> = Vec::new();
    let mut any = false;
    for r in &msg.answers {
        if let RData::A(a) = r.data {
            any = true;
            if usable(&a) {
                target = r.name.trim_end_matches('.').to_ascii_lowercase();
                if ips.len() < 2 && !ips.contains(&a.to_string()) {
                    ips.push(a.to_string());
                }
            }
        }
    }
    if !ips.is_empty() {
        Outcome::Answer(ips, target)
    } else if any {
        Outcome::Blocked("only private or unspecified addresses".into())
    } else {
        Outcome::Failed("no usable address".into())
    }
}

/// Ask the authoritative nameserver behind each hostname's target name directly, from this
/// computer. `ex` is any working resolver, used only to find the nameservers. Plain-DNS
/// interception on this network would answer in the CDN's place, so only answers with
/// the Authoritative Answer bit count, and `tcp` avoids UDP 53 altogether.
pub fn references(ex: &mut dyn Exchanger, targets: &BTreeMap<String, String>, timeout: Duration, tcp: bool) -> BTreeMap<String, Reference> {
    targets
        .iter()
        .map(|(host, name)| {
            let r = reference(ex, name, timeout, tcp).unwrap_or_else(|e| Reference {
                name: name.clone(),
                error: Some(e),
                ..Default::default()
            });
            (host.clone(), r)
        })
        .collect()
}

fn reference(ex: &mut dyn Exchanger, name: &str, timeout: Duration, tcp: bool) -> Result<Reference, String> {
    let ask = |ex: &mut dyn Exchanger, spec: QuerySpec| -> Result<dns::Message, String> {
        dns::ask(ex, &spec, timeout).map(|a| a.msg).map_err(|e| e.to_string())
    };
    // The zone that holds `name`: an NS query for it either answers (it's a zone apex)
    // or returns the enclosing zone's SOA in the authority section.
    let probe = ask(ex, QuerySpec::new(name, dns::TYPE_NS))?;
    let zone = if probe.answers.iter().any(|r| r.rtype == dns::TYPE_NS) {
        name.to_string()
    } else {
        probe.authority.iter().find(|r| r.rtype == dns::TYPE_SOA).map(|r| r.name.clone()).ok_or("zone not found")?
    };
    let ns = ask(ex, QuerySpec::new(zone.as_str(), dns::TYPE_NS))?;
    let mut servers: Vec<String> = ns
        .answers
        .iter()
        .filter_map(|r| if let (dns::TYPE_NS, RData::Name(n)) = (r.rtype, &r.data) { Some(n.clone()) } else { None })
        .collect();
    if servers.is_empty() {
        return Err(format!("no nameservers for {zone}"));
    }
    fastrand::shuffle(&mut servers);
    let mut last = String::from("no nameserver answered");
    for host in servers.iter().take(3) {
        // The resolver under test picks this address: never one on this computer or its network.
        let Some(addr) = ask(ex, QuerySpec::new(host.as_str(), dns::TYPE_A))
            .ok()
            .and_then(|m| m.ipv4_answers().into_iter().find(|a| !crate::catalog::is_local_or_special(&IpAddr::V4(*a))))
        else {
            continue;
        };
        let ep = Endpoint {
            transport: if tcp { Transport::Tcp } else { Transport::Udp },
            addr: SocketAddr::new(IpAddr::V4(addr), 53),
            host: None,
            url: None,
        };
        let mut spec = QuerySpec::new(name, dns::TYPE_A);
        spec.rd = false;
        // Ask a few times: CDNs rotate a client between clusters of the same mapping.
        let mut x = match net::open(&ep, timeout) {
            Ok(x) => x,
            Err(e) => {
                last = format!("{host}: {e}");
                continue;
            }
        };
        let mut ips: Vec<String> = Vec::new();
        let mut reply = Err(String::from("no answer"));
        for _ in 0..3 {
            reply = ask(x.as_mut(), spec.clone());
            match &reply {
                Ok(m) if m.rcode == dns::RCODE_NOERROR && m.aa() => {
                    for a in m.ipv4_answers().into_iter().filter(usable) {
                        if ips.len() < 6 && !ips.contains(&a.to_string()) {
                            ips.push(a.to_string());
                        }
                    }
                }
                _ => break,
            }
        }
        match reply {
            Ok(m) if m.rcode == dns::RCODE_NOERROR && m.aa() => {
                if ips.is_empty() {
                    last = format!("{host} returned no address");
                    continue;
                }
                return Ok(Reference { name: name.to_string(), server: format!("{host} ({addr})"), ips, ..Default::default() });
            }
            Ok(m) if m.rcode == dns::RCODE_NOERROR => last = format!("{host} answered without authority (port 53 may be intercepted)"),
            Ok(m) => last = format!("{host}: {}", dns::rcode_name(m.rcode)),
            Err(e) => last = format!("{host}: {e}"),
        }
    }
    Err(last)
}

/// The /24 an IPv4 edge sits in: CDNs answer from a cluster, rotating addresses within it.
fn cluster(ip: &str) -> Option<[u8; 3]> {
    let a: Ipv4Addr = ip.parse().ok()?;
    let o = a.octets();
    Some([o[0], o[1], o[2]])
}

const PROBE_THREADS: usize = 8;
const PASS_ONE_MS: f64 = 1200.0;

enum Probe {
    Sample(f64),
    Refused(f64),
    Failed,
}

/// One TCP handshake: SYN → SYN-ACK, or the reset that answers it.
fn probe(addr: SocketAddr, timeout: Duration) -> Probe {
    let t0 = clock::now_ns();
    let r = TcpStream::connect_timeout(&addr, timeout);
    let ms = ((clock::since(t0) as f64 / 1e6) * 1000.0).round() / 1000.0;
    match r {
        Ok(_) => Probe::Sample(ms),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Probe::Refused(ms),
        Err(_) => Probe::Failed,
    }
}

/// Run `jobs` (index, address, timeout) over a fixed pool of threads, each waiting a
/// random 10–50 ms before it starts a probe so the SYNs don't arrive in a burst.
fn run_pass(jobs: &[(usize, SocketAddr, Duration)]) -> Vec<(usize, Probe)> {
    let next = AtomicUsize::new(0);
    std::thread::scope(|sc| {
        let workers: Vec<_> = (0..PROBE_THREADS.min(jobs.len()))
            .map(|_| {
                sc.spawn(|| {
                    let mut out = Vec::new();
                    while let Some(&(i, addr, timeout)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                        std::thread::sleep(Duration::from_millis(fastrand::u64(10..=50)));
                        out.push((i, probe(addr, timeout)));
                    }
                    out
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    })
}

/// Time TCP handshakes to every edge's HTTPS port: a throwaway warm-up, then three shuffled
/// passes over a pool of threads, keeping every sample. An address that times out in the first
/// pass is dropped from the rest; later passes wait only a few times the best round trip.
pub fn time_edges(ips: &BTreeSet<String>) -> BTreeMap<String, Edge> {
    time_port(ips, 443)
}

fn time_port(ips: &BTreeSet<String>, port: u16) -> BTreeMap<String, Edge> {
    let mut edges: BTreeMap<String, Edge> = ips.iter().map(|ip| (ip.clone(), Edge { ip: ip.clone(), ..Default::default() })).collect();
    let mut order: Vec<(String, SocketAddr)> =
        ips.iter().filter_map(|ip| ip.parse().ok().map(|a: IpAddr| (ip.clone(), SocketAddr::new(a, port)))).collect();
    fastrand::shuffle(&mut order);
    if let Some((_, addr)) = order.first() {
        probe(*addr, Duration::from_millis(PASS_ONE_MS as u64));
    }
    let mut unreachable: BTreeSet<String> = BTreeSet::new();
    for pass in 0..3 {
        fastrand::shuffle(&mut order);
        let live: Vec<&(String, SocketAddr)> = order.iter().filter(|(ip, _)| !unreachable.contains(ip)).collect();
        let jobs: Vec<(usize, SocketAddr, Duration)> = live
            .iter()
            .enumerate()
            .map(|(i, (ip, addr))| {
                let best = edges[ip].samples.iter().copied().reduce(f64::min);
                let ms = match (pass, best) {
                    (0, _) | (_, None) => PASS_ONE_MS,
                    (_, Some(b)) => (4.0 * b).clamp(100.0, PASS_ONE_MS),
                };
                (i, *addr, Duration::from_micros((ms * 1000.0) as u64))
            })
            .collect();
        for (i, result) in run_pass(&jobs) {
            let ip = &live[i].0;
            let e = edges.get_mut(ip).unwrap();
            match result {
                Probe::Sample(ms) => e.samples.push(ms),
                Probe::Refused(ms) => {
                    e.samples.push(ms);
                    e.refused = true;
                }
                Probe::Failed if pass == 0 => {
                    unreachable.insert(ip.clone());
                }
                Probe::Failed => {}
            }
        }
    }
    for e in edges.values_mut() {
        e.rtt_ms = e.samples.iter().copied().reduce(f64::min);
        e.median_ms = median(&mut e.samples.clone());
    }
    edges
}

/// δ: how far apart two handshakes to the same edge land by chance. The 90th percentile
/// over edges of (second-lowest minus lowest sample), kept within 1–5 ms; 1 ms when too
/// few edges have two samples to say.
pub fn noise_floor(edges: &BTreeMap<String, Edge>) -> f64 {
    let mut gaps: Vec<f64> = edges
        .values()
        .filter(|e| e.samples.len() >= 2)
        .map(|e| {
            let mut s = e.samples.clone();
            s.sort_by(|a, b| a.total_cmp(b));
            s[1] - s[0]
        })
        .collect();
    if gaps.len() < 5 {
        return 1.0;
    }
    gaps.sort_by(|a, b| a.total_cmp(b));
    let i = ((gaps.len() as f64 * 0.9).ceil() as usize).clamp(1, gaps.len()) - 1;
    gaps[i].clamp(1.0, 5.0)
}

/// A hostname none of whose addresses completed a handshake can't be scored.
pub fn mark_unreachable(answers: &mut Answers, edges: &BTreeMap<String, Edge>) {
    for m in answers.values_mut().flat_map(|c| c.values_mut().flat_map(|h| h.values_mut())) {
        if m.status == HostStatus::Ok && !m.ips.iter().any(|ip| edges.get(ip).is_some_and(|e| e.rtt_ms.is_some())) {
            m.status = HostStatus::Unreachable;
            m.error = Some("no address completed a handshake".into());
        }
    }
}

/// Median over the edges of each one's best round trip.
fn rtt_of(edges: &BTreeMap<String, Edge>, ips: &[String]) -> Option<f64> {
    let mut rtts: Vec<f64> = ips.iter().filter_map(|ip| edges.get(ip).and_then(|e| e.rtt_ms)).collect();
    median(&mut rtts)
}

/// How one hostname's edges compare with the CDN's own pick for this computer.
fn as_you_of(cdn: &Cdn, m: &HostMapping, r: &Reference, noise: f64) -> Option<AsYou> {
    let (s, sref) = (m.rtt_ms?, r.rtt_ms?);
    let diff = (s - sref).abs();
    if diff <= noise.max(0.04 * sref) {
        return Some(AsYou::Same);
    }
    let mine: BTreeSet<_> = r.ips.iter().filter_map(|ip| cluster(ip)).collect();
    let shares = m.ips.iter().filter_map(|ip| cluster(ip)).any(|c| mine.contains(&c));
    // Akamai, CloudFront and Meta hand out addresses from a cluster and rotate within it;
    // elsewhere a neighbouring cluster proves nothing.
    if matches!(cdn.id, "cloudfront" | "meta" | "akamai") && shares && diff <= 3f64.max(0.10 * sref) {
        return Some(AsYou::Near);
    }
    Some(AsYou::Different)
}

/// Combine per-resolver answers, the CDNs' own answers and edge timings into penalties.
/// `noise` is δ from [`noise_floor`]. No network.
pub fn score(mut answers: Answers, mut refs: BTreeMap<String, Reference>, edges: BTreeMap<String, Edge>, noise: f64) -> CdnReport {
    for r in refs.values_mut() {
        r.rtt_ms = rtt_of(&edges, &r.ips);
    }
    // S(r,h) for hostnames that can be scored, and b(h): the best any resolver or the CDN found.
    let mut best: BTreeMap<String, f64> = BTreeMap::new();
    for m in answers.values_mut().flat_map(|c| c.values_mut().flat_map(|h| h.values_mut())) {
        m.rtt_ms = if m.status == HostStatus::Ok { rtt_of(&edges, &m.ips) } else { None };
    }
    let floors = answers.values().flat_map(|c| c.values().flat_map(|h| h.iter())).filter_map(|(h, m)| m.rtt_ms.map(|s| (h, s)));
    let refd = refs.iter().filter_map(|(h, r)| r.rtt_ms.map(|s| (h, s)));
    for (h, s) in floors.chain(refd) {
        let b = best.entry(h.clone()).or_insert(s);
        *b = b.min(s);
    }
    for cdns in answers.values_mut() {
        for (cid, hosts) in cdns.iter_mut() {
            let Some(cdn) = CDNS.iter().find(|c| c.id == cid) else { continue };
            for (h, m) in hosts.iter_mut() {
                let (Some(s), Some(b)) = (m.rtt_ms, best.get(h)) else { continue };
                let p = round2((s - b).max(0.0));
                m.penalty_ms = Some(p);
                m.same = Some(p < noise.max(0.05 * b));
                m.as_you = refs.get(h).filter(|r| !r.ips.is_empty()).and_then(|r| as_you_of(cdn, m, r, noise));
            }
        }
    }

    let mut out = CdnReport { version: 2, noise_ms: noise, edges, ..Default::default() };
    for (id, cdns) in answers.iter() {
        let mut rc = ResolverCdn { basis: "unknown".into(), ..Default::default() };
        for cdn in CDNS {
            let Some(hosts) = cdns.get(cdn.id) else { continue };
            rc.mappings.insert(cdn.id.to_string(), cdn_score(cdn, hosts));
        }
        out.resolvers.insert(id.clone(), rc);
    }

    // Headline: the mean penalty over the steered CDNs every ranked resolver has.
    let steered: Vec<&Cdn> = CDNS.iter().filter(|c| c.role == Role::Steered).collect();
    let p_of = |rc: &ResolverCdn, c: &Cdn| rc.mappings.get(c.id).and_then(|s| s.penalty_ms);
    let ranked: Vec<&String> =
        out.resolvers.iter().filter(|(_, rc)| steered.iter().filter(|c| p_of(rc, c).is_some()).count() >= 3).map(|(id, _)| id).collect();
    let common: Vec<&Cdn> = steered.iter().copied().filter(|c| ranked.iter().all(|id| p_of(&out.resolvers[*id], c).is_some())).collect();
    let mut heads: Vec<(String, f64)> = Vec::new();
    if !ranked.is_empty() && common.len() >= 3 {
        for id in &ranked {
            let rc = &out.resolvers[*id];
            let mean = common.iter().filter_map(|c| p_of(rc, c)).sum::<f64>() / common.len() as f64;
            heads.push(((*id).clone(), round2(mean)));
        }
    }
    heads.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut tier = 0u8;
    let mut head = f64::NEG_INFINITY;
    for (id, h) in &heads {
        if tier == 0 || *h - head > (2.0 * noise).max(2.0) {
            tier += 1;
            head = *h;
        }
        let rc = out.resolvers.get_mut(id).unwrap();
        rc.penalty_ms = Some(*h);
        rc.tier = Some(tier);
    }
    for rc in out.resolvers.values_mut() {
        let mut cdns_used = 0;
        let (mut hosts_used, mut hosts_total) = (0u8, 0u8);
        for c in &steered {
            hosts_total += c.hosts.len() as u8;
            let Some(s) = rc.mappings.get(c.id) else { continue };
            hosts_used += s.hosts_used;
            if let Some(p) = s.penalty_ms {
                cdns_used += 1;
                if rc.worst.as_ref().is_none_or(|(_, w)| p > *w) {
                    rc.worst = Some((c.id.to_string(), p));
                }
            }
        }
        rc.coverage = Coverage { cdns_used, cdns_total: steered.len() as u8, hosts_used, hosts_total };
    }

    for cdn in CDNS {
        let top = out
            .resolvers
            .iter()
            .filter_map(|(id, rc)| rc.mappings.get(cdn.id).and_then(|s| s.rtt_ms).map(|v| (id.clone(), v)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let hosts = cdn
            .hosts
            .iter()
            .map(|h| {
                // Did every resolver, and the CDN's own answer, return the same edges?
                let mut sets: Vec<BTreeSet<&String>> = answers
                    .values()
                    .filter_map(|c| c.get(cdn.id).and_then(|hs| hs.get(h.name)))
                    .filter(|m| m.status == HostStatus::Ok)
                    .map(|m| m.ips.iter().collect())
                    .collect();
                let mut you = refs.remove(h.name);
                if let Some(y) = you.as_ref().filter(|y| !y.ips.is_empty()) {
                    sets.push(y.ips.iter().collect());
                }
                let uniform = sets.windows(2).all(|w| w[0] == w[1]);
                // Steered names should differ between resolvers, anycast ones shouldn't.
                let unexpected = sets.len() >= 2 && (uniform == (cdn.role == Role::Steered));
                if let (Some(y), Some(b)) = (you.as_mut(), best.get(h.name)) {
                    y.penalty_ms = y.rtt_ms.map(|v| round2((v - b).max(0.0)));
                }
                HostInfo { name: h.name, uniform, unexpected, you }
            })
            .collect();
        out.cdns.push(CdnInfo { id: cdn.id, name: cdn.name, role: cdn.role, hosts, best_ms: top.map(|b| b.1) });
    }
    out
}

/// One resolver's CDN: the median over its Ok hostnames.
fn cdn_score(cdn: &Cdn, hosts: &BTreeMap<String, HostMapping>) -> CdnScore {
    let ok: Vec<(&String, &HostMapping)> = hosts.iter().filter(|(_, m)| m.status == HostStatus::Ok && m.rtt_ms.is_some()).collect();
    let mut s: Vec<f64> = ok.iter().filter_map(|(_, m)| m.rtt_ms).collect();
    let mut p: Vec<f64> = ok.iter().filter_map(|(_, m)| m.penalty_ms).collect();
    let worst_host = ok.iter().filter_map(|(h, m)| m.penalty_ms.map(|p| ((*h).clone(), p))).max_by(|a, b| a.1.total_cmp(&b.1));
    let judged: Vec<AsYou> = ok.iter().filter_map(|(_, m)| m.as_you).collect();
    let share = |f: fn(&AsYou) -> bool| judged.iter().filter(|a| f(a)).count() * 2 >= judged.len();
    let as_you = if judged.is_empty() {
        None
    } else if share(|a| *a == AsYou::Same) {
        Some(AsYou::Same)
    } else if share(|a| *a != AsYou::Different) {
        Some(AsYou::Near)
    } else {
        Some(AsYou::Different)
    };
    let used = ok.len();
    let error = if used >= 2 || (used == 1 && cdn.hosts.len() == 1) {
        None
    } else if used == 0 {
        Some(hosts.values().find_map(|m| m.error.clone()).unwrap_or_else(|| "no usable hostname".into()))
    } else {
        Some(format!("only {used} of {} hostnames usable", cdn.hosts.len()))
    };
    CdnScore {
        rtt_ms: median(&mut s),
        penalty_ms: if used >= 2 { median(&mut p).map(round2) } else { None },
        as_you,
        worst_host,
        hosts_used: used as u8,
        hosts_total: cdn.hosts.len() as u8,
        error,
        hosts: hosts.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{Reply, XErr};

    #[test]
    fn median_sorts_and_averages_the_middle_pair() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [7.0]), Some(7.0));
        assert_eq!(median(&mut [9.0, 1.0, 5.0]), Some(5.0));
        assert_eq!(median(&mut [8.0, 2.0, 6.0, 4.0]), Some(5.0));
    }

    fn cdn_by(id: &str) -> &'static Cdn {
        CDNS.iter().find(|c| c.id == id).unwrap()
    }

    /// The first `n` hostnames of a CDN, all at `rtt` ms.
    fn hs(cdn: &str, n: usize, rtt: f64) -> Vec<(&'static str, f64)> {
        cdn_by(cdn).hosts.iter().take(n).map(|h| (h.name, rtt)).collect()
    }

    fn edge(ip: &str, rtt: f64) -> (String, Edge) {
        (ip.to_string(), Edge { ip: ip.to_string(), rtt_ms: Some(rtt), samples: vec![rtt], ..Default::default() })
    }

    fn ok(ips: &[&str]) -> HostMapping {
        HostMapping { ips: ips.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    fn put(answers: &mut Answers, resolver: &str, host: &str, m: HostMapping) {
        let cdn = CDNS.iter().find(|c| c.hosts.iter().any(|h| h.name == host)).unwrap();
        answers.entry(resolver.into()).or_default().entry(cdn.id.into()).or_default().insert(host.into(), m);
    }

    /// Resolvers with one edge per hostname, each at its own address and round trip.
    fn world(rows: &[(&str, Vec<(&'static str, f64)>)]) -> (Answers, BTreeMap<String, Edge>) {
        let all: Vec<&str> = CDNS.iter().flat_map(|c| c.hosts.iter().map(|h| h.name)).collect();
        let mut answers = Answers::new();
        let mut edges = BTreeMap::new();
        for (ri, (rid, hosts)) in rows.iter().enumerate() {
            for (host, rtt) in hosts {
                let ip = format!("198.51.{ri}.{}", all.iter().position(|h| h == host).unwrap());
                edges.extend([edge(&ip, *rtt)]);
                put(&mut answers, rid, host, ok(&[&ip]));
            }
        }
        (answers, edges)
    }

    fn rep(rows: &[(&str, Vec<(&'static str, f64)>)], noise: f64) -> CdnReport {
        let (a, e) = world(rows);
        score(a, BTreeMap::new(), e, noise)
    }

    /// Per CDN, `n` hostnames at `base + off`: every resolver's P for that CDN is its `off`.
    fn steered(off: f64) -> Vec<(&'static str, f64)> {
        [hs("akamai", 2, 10.0 + off), hs("cloudfront", 2, 20.0 + off), hs("google", 2, 30.0 + off)].concat()
    }

    fn close(a: Option<f64>, b: f64) {
        assert!(a.is_some_and(|a| (a - b).abs() < 1e-9), "{a:?} != {b}");
    }

    #[test]
    fn a_hostname_slow_for_everyone_adds_nothing() {
        let slow = |base: f64| {
            let mut v = [hs("akamai", 5, base), hs("cloudfront", 3, base), hs("google", 3, base)].concat();
            v[4].1 = 80.0; // www.nvidia.com is far from everybody
            v
        };
        let r = rep(&[("a", slow(10.0)), ("b", slow(12.0))], 1.0);
        for id in ["a", "b"] {
            assert_eq!(r.resolvers[id].mappings["akamai"].hosts["www.nvidia.com"].penalty_ms, Some(0.0));
        }
        close(r.resolvers["a"].mappings["akamai"].penalty_ms, 0.0);
        close(r.resolvers["b"].mappings["akamai"].penalty_ms, 2.0);
        close(r.resolvers["b"].penalty_ms, 2.0);
        close(r.resolvers["a"].penalty_ms, 0.0);
    }

    #[test]
    fn one_mismapped_host_in_four_leaves_the_cdn_penalty_alone() {
        let mut b = hs("akamai", 4, 10.0);
        b[2].1 = 40.0;
        let r = rep(&[("a", hs("akamai", 4, 10.0)), ("b", b)], 1.0);
        let s = &r.resolvers["b"].mappings["akamai"];
        close(s.penalty_ms, 0.0);
        assert_eq!(s.worst_host, Some(("www.oracle.com".to_string(), 30.0)));
        assert_eq!((s.hosts_used, s.hosts_total), (4, 5));
    }

    #[test]
    fn a_cdn_needs_two_hostnames_and_a_headline_needs_three_cdns() {
        let one_host = [hs("akamai", 1, 10.0), hs("cloudfront", 3, 10.0), hs("google", 3, 10.0), hs("meta", 3, 10.0)].concat();
        let r = rep(&[("a", one_host)], 1.0);
        let a = &r.resolvers["a"];
        assert_eq!(a.mappings["akamai"].penalty_ms, None);
        assert!(a.mappings["akamai"].error.is_some());
        assert_eq!((a.coverage.cdns_used, a.coverage.cdns_total), (3, 4));
        assert_eq!((a.coverage.hosts_used, a.coverage.hosts_total), (10, 14));
        close(a.penalty_ms, 0.0);

        let two_cdns = [hs("akamai", 5, 10.0), hs("cloudfront", 3, 10.0)].concat();
        let r = rep(&[("a", two_cdns)], 1.0);
        let a = &r.resolvers["a"];
        assert_eq!((a.penalty_ms, a.tier), (None, None));
        assert_eq!((a.coverage.cdns_used, a.coverage.hosts_used), (2, 8));
    }

    #[test]
    fn the_headline_is_the_mean_over_the_cdns_everyone_has() {
        let all = |ak, cf, go, meta| [hs("akamai", 2, ak), hs("cloudfront", 2, cf), hs("google", 2, go), hs("meta", 2, meta)].concat();
        // a is nearest on three CDNs and 10 ms off on Meta; b the other way round.
        let r = rep(&[("a", all(10.0, 20.0, 30.0, 50.0)), ("b", all(12.0, 26.0, 31.0, 40.0))], 1.0);
        close(r.resolvers["a"].penalty_ms, 2.5);
        close(r.resolvers["b"].penalty_ms, 2.25);
        assert_eq!(r.resolvers["a"].worst, Some(("meta".to_string(), 10.0)));

        // b has no Meta answers: Meta leaves the common set, and a's headline with it.
        let three = [hs("akamai", 2, 12.0), hs("cloudfront", 2, 26.0), hs("google", 2, 31.0)].concat();
        let r = rep(&[("a", all(10.0, 20.0, 30.0, 50.0)), ("b", three)], 1.0);
        close(r.resolvers["a"].penalty_ms, 0.0);
        close(r.resolvers["b"].penalty_ms, 3.0);

        // Two common CDNs are too few: nobody is ranked, and coverage still says why.
        let a = [hs("akamai", 2, 10.0), hs("cloudfront", 2, 10.0), hs("google", 2, 10.0)].concat();
        let b = [hs("cloudfront", 2, 10.0), hs("google", 2, 10.0), hs("meta", 2, 10.0)].concat();
        let r = rep(&[("a", a), ("b", b)], 1.0);
        assert!(r.resolvers.values().all(|x| x.penalty_ms.is_none() && x.tier.is_none()));
        assert_eq!(r.resolvers["b"].coverage.cdns_used, 3);
    }

    #[test]
    fn controls_never_enter_the_headline() {
        let with = |off: f64, control: f64| [steered(off), hs("fastly", 1, control), hs("cloudflare", 1, control)].concat();
        let r = rep(&[("a", with(0.0, 5.0)), ("b", with(4.0, 500.0))], 1.0);
        close(r.resolvers["b"].penalty_ms, 4.0);
        close(r.resolvers["a"].penalty_ms, 0.0);
        assert_eq!(r.resolvers["b"].coverage.cdns_total, 4);
        // Shown at the hostname level, never given a CDN penalty (one hostname each).
        let f = &r.resolvers["b"].mappings["fastly"];
        assert_eq!(f.penalty_ms, None);
        assert_eq!(f.error, None);
        close(f.hosts["www.reddit.com"].penalty_ms, 495.0);
        assert_eq!(r.cdns.iter().find(|c| c.id == "fastly").unwrap().role, Role::Control);
    }

    fn gapped(gaps: &[f64]) -> BTreeMap<String, Edge> {
        gaps.iter()
            .enumerate()
            .map(|(i, g)| (format!("192.0.2.{i}"), Edge { samples: vec![10.0 + g, 10.0, 30.0], ..Default::default() }))
            .collect()
    }

    #[test]
    fn the_noise_floor_is_clamped_and_labels_hostnames() {
        assert_eq!(noise_floor(&gapped(&[3.0; 4])), 1.0); // too few edges to say
        assert_eq!(noise_floor(&gapped(&[0.2; 10])), 1.0);
        assert_eq!(noise_floor(&gapped(&[20.0; 10])), 5.0);
        let steps: Vec<f64> = (1..=10).map(|k| 0.5 * k as f64).collect();
        assert!((noise_floor(&gapped(&steps)) - 4.5).abs() < 1e-9);

        let hosts = |rtt| hs("akamai", 2, rtt);
        let r = rep(&[("a", hosts(10.0)), ("b", hosts(11.9)), ("c", hosts(12.1))], 2.0);
        let same = |id: &str| r.resolvers[id].mappings["akamai"].hosts["www.apple.com"].same;
        assert_eq!((same("a"), same("b"), same("c")), (Some(true), Some(true), Some(false)));
        // On a long path 5% of the best is the bigger allowance.
        let r = rep(&[("a", hosts(100.0)), ("b", hosts(104.0)), ("c", hosts(106.0))], 1.0);
        let same = |id: &str| r.resolvers[id].mappings["akamai"].hosts["www.apple.com"].same;
        assert_eq!((same("b"), same("c")), (Some(true), Some(false)));
    }

    #[test]
    fn tiers_group_resolvers_within_twice_the_noise() {
        let tiers = |offs: &[f64], noise: f64| {
            let rows: Vec<(&str, Vec<(&'static str, f64)>)> =
                ["a", "b", "c", "d"].into_iter().zip(offs).map(|(id, o)| (id, steered(*o))).collect();
            let r = rep(&rows, noise);
            ["a", "b", "c", "d"].iter().take(offs.len()).map(|id| r.resolvers[*id].tier).collect::<Vec<_>>()
        };
        // The floor of 2 ms applies when 2δ is smaller.
        assert_eq!(tiers(&[0.0, 1.9, 2.1, 3.0], 1.0), [Some(1), Some(1), Some(2), Some(2)]);
        // With δ = 3 ms the tier is 6 ms wide.
        assert_eq!(tiers(&[0.0, 5.0, 7.0], 3.0), [Some(1), Some(1), Some(2)]);
    }

    #[test]
    fn as_you_is_same_near_or_different() {
        let mut a = Answers::new();
        put(&mut a, "same", "www.apple.com", ok(&["23.222.246.102"]));
        put(&mut a, "near", "www.apple.com", ok(&["23.222.246.77"]));
        put(&mut a, "far", "www.apple.com", ok(&["23.34.126.68"]));
        put(&mut a, "other24", "www.apple.com", ok(&["23.63.157.112"]));
        put(&mut a, "g", "www.gstatic.com", ok(&["142.250.1.9"]));
        let edges = BTreeMap::from([
            edge("23.222.246.101", 28.2),
            edge("23.222.246.102", 28.3),
            edge("23.222.246.77", 31.0),
            edge("23.34.126.68", 14.7),
            edge("23.63.157.112", 31.0),
            edge("142.250.1.1", 20.0),
            edge("142.250.1.9", 22.5),
        ]);
        let refs = BTreeMap::from([
            ("www.apple.com".to_string(), Reference { ips: vec!["23.222.246.101".into()], ..Default::default() }),
            ("www.gstatic.com".to_string(), Reference { ips: vec!["142.250.1.1".into()], ..Default::default() }),
        ]);
        let r = score(a, refs, edges, 1.0);
        let you = |id: &str, cdn: &str, host: &str| r.resolvers[id].mappings[cdn].hosts[host].as_you;
        assert_eq!(you("same", "akamai", "www.apple.com"), Some(AsYou::Same));
        // 2.8 ms off in the client's own /24: more than the allowance for Same, inside 10% for Near.
        assert_eq!(you("near", "akamai", "www.apple.com"), Some(AsYou::Near));
        assert_eq!(you("far", "akamai", "www.apple.com"), Some(AsYou::Different));
        assert_eq!(you("other24", "akamai", "www.apple.com"), Some(AsYou::Different));
        // Google's neighbouring cluster is not evidence of the same mapping.
        assert_eq!(you("g", "google", "www.gstatic.com"), Some(AsYou::Different));
        assert_eq!(r.resolvers["near"].mappings["akamai"].as_you, Some(AsYou::Near));
        let apple = &r.cdns[0].hosts[0];
        assert_eq!(apple.name, "www.apple.com");
        assert!(apple.you.is_some());
    }

    #[test]
    fn no_reference_leaves_as_you_unknown() {
        let r = rep(&[("a", steered(0.0))], 1.0);
        assert!(r.resolvers["a"].mappings.values().all(|s| s.as_you.is_none() && s.hosts.values().all(|m| m.as_you.is_none())));
        assert!(r.cdns.iter().all(|c| c.hosts.iter().all(|h| h.you.is_none())));
    }

    #[test]
    fn steering_audit_flags_the_unexpected() {
        let mut a = Answers::new();
        put(&mut a, "x", "www.apple.com", ok(&["1.1.1.1"]));
        put(&mut a, "y", "www.apple.com", ok(&["1.1.1.1"]));
        put(&mut a, "x", "www.nvidia.com", ok(&["2.2.2.2"]));
        put(&mut a, "y", "www.nvidia.com", ok(&["3.3.3.3"]));
        put(&mut a, "x", "www.reddit.com", ok(&["4.4.4.4"]));
        put(&mut a, "y", "www.reddit.com", ok(&["5.5.5.5"]));
        let r = score(a, BTreeMap::new(), BTreeMap::new(), 1.0);
        let flag = |cdn: &str, host: &str| {
            let h = r.cdns.iter().find(|c| c.id == cdn).unwrap().hosts.iter().find(|h| h.name == host).unwrap();
            (h.uniform, h.unexpected)
        };
        assert_eq!(flag("akamai", "www.apple.com"), (true, true));
        assert_eq!(flag("akamai", "www.nvidia.com"), (false, false));
        assert_eq!(flag("fastly", "www.reddit.com"), (false, true));
    }

    #[test]
    fn a_host_that_never_completed_a_handshake_is_unreachable() {
        let mut a = Answers::new();
        put(&mut a, "x", "www.apple.com", ok(&["1.1.1.1", "1.1.1.2"]));
        put(&mut a, "x", "www.oracle.com", ok(&["2.2.2.2"]));
        let edges = BTreeMap::from([edge("1.1.1.2", 12.0), ("2.2.2.2".to_string(), Edge::default())]);
        mark_unreachable(&mut a, &edges);
        assert_eq!(a["x"]["akamai"]["www.apple.com"].status, HostStatus::Ok);
        assert_eq!(a["x"]["akamai"]["www.oracle.com"].status, HostStatus::Unreachable);
    }

    #[test]
    fn a_tail_must_belong_to_the_cdn() {
        let h = &cdn_by("akamai").hosts[0];
        let seen = |tail: &str| {
            let mut s = Seen::default();
            s.record(Outcome::Answer(vec!["23.1.1.1".into()], tail.into()));
            s.finish(h).status
        };
        assert_eq!(seen("e1.dscb.akamaiedge.net"), HostStatus::Ok);
        assert_eq!(seen("a1.akamai.net"), HostStatus::Ok);
        assert_eq!(seen("www.apple.com.example-cdn.net"), HostStatus::OffCdn);
        assert_eq!(seen("notakamai.net"), HostStatus::OffCdn);
    }

    #[test]
    fn private_answers_are_not_edges() {
        for ip in ["10.1.2.3", "192.168.1.1", "172.16.0.9", "169.254.1.1", "127.0.0.1", "0.0.0.0"] {
            assert!(!usable(&ip.parse().unwrap()), "{ip}");
        }
        assert!(usable(&"23.1.1.1".parse().unwrap()));
    }

    /// Answers every A query with one address on the queried name itself (no CNAME).
    struct Stub {
        ip: [u8; 4],
        fail_first: u32,
        calls: u32,
    }

    impl Exchanger for Stub {
        fn exchange(&mut self, query: &[u8], _timeout: Duration) -> Result<Reply, XErr> {
            self.calls += 1;
            if self.calls <= self.fail_first {
                return Err(XErr::Timeout);
            }
            let mut end = 12;
            while query[end] != 0 {
                end += 1 + query[end] as usize;
            }
            end += 5;
            let mut r = query[..end].to_vec();
            r[2] = 0x81;
            r[3] = 0x80;
            r[6..12].copy_from_slice(&[0, 1, 0, 0, 0, 0]);
            r.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 1, 0x2C, 0, 4]);
            r.extend_from_slice(&self.ip);
            Ok(Reply { bytes: r, rtt_ns: 0, user_rtt_ns: 0, kernel_stamp: false, ttl: None })
        }
    }

    fn hosts_in(m: &BTreeMap<String, BTreeMap<String, HostMapping>>) -> usize {
        m.values().map(|h| h.len()).sum()
    }

    #[test]
    fn resolving_blocks_private_answers_and_retries_once() {
        let mut ex = Stub { ip: [10, 0, 0, 1], fail_first: 0, calls: 0 };
        let m = resolve_all(&mut ex, Duration::from_millis(50), 2);
        let n = hosts_in(&m);
        assert_eq!(n, 16);
        assert!(m.values().flat_map(|h| h.values()).all(|x| x.status == HostStatus::Blocked && x.ips.is_empty()));
        assert_eq!(ex.calls as usize, n * 2 * 2); // two rounds, each retried at once

        // A public address on the queried name: only the CDN whose own name it is passes.
        let mut ex = Stub { ip: [23, 1, 1, 1], fail_first: 1, calls: 0 };
        let m = resolve_all(&mut ex, Duration::from_millis(50), 2);
        assert_eq!(ex.calls as usize, n * 2 + 1); // one timeout, retried
        let status = |cdn: &str, host: &str| m[cdn][host].status;
        assert_eq!(status("akamai", "www.apple.com"), HostStatus::OffCdn);
        assert_eq!(status("cloudflare", "www.cloudflare.com"), HostStatus::Ok);
        assert_eq!(m["cloudflare"]["www.cloudflare.com"].ips, ["23.1.1.1"]);
    }

    #[test]
    fn edges_keep_every_sample() {
        // A listener of the test's own, so the handshake completes everywhere (Windows retries a SYN
        // to a closed port for a second or more before it reports the refusal).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ips = BTreeSet::from(["127.0.0.1".to_string()]);
        let e = &time_port(&ips, port)["127.0.0.1"];
        assert_eq!(e.samples.len(), 3);
        assert!(!e.refused);
        assert!(e.rtt_ms.is_some() && e.median_ms.is_some());
    }
}
