//! Capability probes: what a resolver actually does, verified from this network.
//!
//! Every check records its evidence (the response that decided it), so the UI can
//! show the proof next to the verdict.

use crate::dns::{self, QuerySpec};
use crate::net::{Exchanger, XErr};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Desirable behaviour confirmed.
    Pass,
    /// Undesirable behaviour confirmed.
    Fail,
    /// Partially present.
    Partial,
    /// Neutral fact (identity, a policy choice).
    Info,
    /// The probe could not decide.
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub key: &'static str,
    pub status: Status,
    /// Present / absent for boolean capabilities, independent of whether that is good.
    pub present: Option<bool>,
    pub value: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct FilterSample {
    pub domain: String,
    pub blocked: Option<bool>,
    pub how: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct FilterCategory {
    pub tested: usize,
    pub blocked: usize,
    pub source: String,
    pub samples: Vec<FilterSample>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ProbeReport {
    pub resolver_id: String,
    pub via: String,
    pub checks: Vec<Check>,
    pub egress: Vec<String>,
    pub ecs: Option<String>,
    /// ECS as Akamai's nameservers saw it (whoami.ds.akahelp.net). Resolvers may send
    /// a different subnet, or none, depending on the authoritative server.
    pub ecs_akamai: Option<String>,
    pub nsid: Option<String>,
    pub pop: Option<String>,
    pub filtering: BTreeMap<String, FilterCategory>,
    pub block_style: Option<String>,
    pub duration_ms: f64,
}

/// Filtering test sets, prepared once per run and shared by every resolver.
#[derive(Clone, Debug, Default)]
pub struct FilterSets {
    /// category → (domains, human-readable source)
    pub categories: Vec<(String, Vec<String>, String)>,
}

impl FilterSets {
    pub fn has_threats(&self) -> bool {
        self.categories.iter().any(|(c, _, _)| c == "malware" || c == "phishing")
    }
}

pub const ADS: &[&str] = &[
    "pagead2.googlesyndication.com",
    "ad.doubleclick.net",
    "googleads.g.doubleclick.net",
    "adservice.google.com",
    "www.google-analytics.com",
    "bat.bing.com",
];
pub const ADULT: &[&str] = &["pornhub.com", "xvideos.com", "nudity.testcategory.com"];
pub const MALWARE_VENDOR: &[&str] = &["malware.testcategory.com"];
pub const PHISHING_VENDOR: &[&str] = &["internetbadguys.com"];

struct Ctx<'a> {
    ex: &'a mut dyn Exchanger,
    timeout: Duration,
}

impl Ctx<'_> {
    fn ask(&mut self, spec: &QuerySpec) -> Result<dns::Message, XErr> {
        let id = fastrand::u16(..);
        let q = dns::encode_query(id, spec).map_err(|e| XErr::Io(e.to_string()))?;
        let r = self.ex.exchange(&q, self.timeout)?;
        dns::parse(&r.bytes).map_err(|e| XErr::Io(format!("unparseable response: {e}")))
    }
}

fn ede_text(m: &dns::Message) -> String {
    m.ede()
        .iter()
        .map(
            |(c, t)| if t.is_empty() { format!("EDE {c} {}", dns::ede_name(*c)) } else { format!("EDE {c} {} \"{t}\"", dns::ede_name(*c)) },
        )
        .collect::<Vec<_>>()
        .join(", ")
}

fn summary(m: &dns::Message) -> String {
    let mut s = dns::rcode_name(m.rcode).to_string();
    let a: Vec<String> = m
        .answers
        .iter()
        .filter_map(|r| match &r.data {
            dns::RData::A(a) => Some(a.to_string()),
            dns::RData::Aaaa(a) => Some(a.to_string()),
            dns::RData::Txt(t) => Some(format!("\"{}\"", t.concat())),
            _ => None,
        })
        .take(3)
        .collect();
    if !a.is_empty() {
        s.push_str(&format!(" {}", a.join(" ")));
    }
    if m.ad() {
        s.push_str(" AD=1");
    }
    let e = ede_text(m);
    if !e.is_empty() {
        s.push_str(&format!(", {e}"));
    }
    s
}

pub(crate) fn check(
    key: &'static str,
    status: Status,
    present: Option<bool>,
    value: impl Into<String>,
    evidence: impl Into<String>,
) -> Check {
    Check { key, status, present, value: value.into(), evidence: evidence.into() }
}

fn unknown(key: &'static str, e: &XErr) -> Check {
    check(key, Status::Unknown, None, "Couldn't test", format!("probe {e}"))
}

pub(crate) fn rand_label(n: usize) -> String {
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..n).map(|_| A[fastrand::usize(..A.len())] as char).collect()
}

/// Classify a response to a name that an unfiltered baseline resolves normally.
fn block_signal(m: &dns::Message) -> Option<String> {
    let ede: Vec<u16> = m.ede().iter().map(|(c, _)| *c).filter(|c| matches!(c, 15..=18)).collect();
    if let Some(c) = ede.first() {
        return Some(format!("EDE {c} {}", dns::ede_name(*c)));
    }
    match m.rcode {
        dns::RCODE_NXDOMAIN => return Some("NXDOMAIN".into()),
        dns::RCODE_REFUSED => return Some("REFUSED".into()),
        dns::RCODE_NOERROR => {}
        _ => return None,
    }
    let v4 = m.ipv4_answers();
    if v4.is_empty() && m.ipv6_answers().is_empty() {
        return Some("empty answer".into());
    }
    for a in &v4 {
        if a.is_unspecified() {
            return Some("0.0.0.0".into());
        }
        if a.is_loopback() {
            return Some(format!("{a} (loopback)"));
        }
        let o = a.octets();
        if o[0] == 146 && o[1] == 112 && o[2] == 61 && (104..=110).contains(&o[3]) {
            return Some(format!("{a} (Cisco block page)"));
        }
    }
    None
}

/// Filter lookups get a shorter leash: a filtering resolver answers from policy at once.
fn filter_timeout(timeout: Duration) -> Duration {
    timeout.min(Duration::from_millis(1500))
}

/// Run every probe over `ex`. Live threat names go over `threat_ex` only (an encrypted
/// connection to the same resolver, or plain DNS when the user allowed it); without it
/// those categories are reported as not tested.
pub fn probe(
    resolver_id: &str,
    via: &str,
    ex: &mut dyn Exchanger,
    threat_ex: Option<&mut dyn Exchanger>,
    timeout: Duration,
    sets: &FilterSets,
) -> ProbeReport {
    let started = std::time::Instant::now();
    let mut c = Ctx { ex, timeout };
    let mut rep = ProbeReport { resolver_id: resolver_id.into(), via: via.into(), ..Default::default() };

    // DNSSEC validation: a deliberately broken zone must fail, a signed one should set AD.
    // Random labels keep every probe out of caches, including any transparent DNS proxy
    // on the path, so answers come from the resolver under test.
    let bogus_name = format!("ub{}.dnssec-failed.org", rand_label(10));
    let signed_name = format!("ub{}.ietf.org", rand_label(10));
    let bogus = c.ask(&QuerySpec::new(&bogus_name, dns::TYPE_A).with_edns(|e| e.dnssec_ok = true));
    let signed = c.ask(&QuerySpec::new(&signed_name, dns::TYPE_A).with_edns(|e| e.dnssec_ok = true));
    rep.checks.push(match (&bogus, &signed) {
        // SERVFAIL on the broken zone means validation only if a signed name still resolves:
        // a resolver that can't recurse at all SERVFAILs both.
        (Ok(b), Ok(s)) if b.rcode == dns::RCODE_SERVFAIL && s.rcode != dns::RCODE_SERVFAIL => {
            let ev = format!("{bogus_name} → {}; {signed_name} → {}", summary(b), summary(s));
            if s.ad() {
                check("dnssec", Status::Pass, Some(true), "Validates", ev)
            } else {
                check("dnssec", Status::Pass, Some(true), "Validates (no AD bit)", ev)
            }
        }
        (Ok(b), s) if b.rcode == dns::RCODE_SERVFAIL => check(
            "dnssec",
            Status::Unknown,
            None,
            "Inconclusive",
            format!(
                "{bogus_name} → {}, but the signed {signed_name} → {} too",
                summary(b),
                s.as_ref().map(summary).unwrap_or_else(|e| e.to_string())
            ),
        ),
        (Ok(b), _) if b.rcode == dns::RCODE_NOERROR || b.rcode == dns::RCODE_NXDOMAIN => {
            check("dnssec", Status::Fail, Some(false), "Doesn't validate", format!("{bogus_name} → {} (bogus data returned)", summary(b)))
        }
        (Ok(b), _) => check("dnssec", Status::Unknown, None, "Inconclusive", format!("{bogus_name} → {}", summary(b))),
        (Err(e), _) => unknown("dnssec", e),
    });

    // EDNS(0) presence and advertised payload size.
    match &signed {
        Ok(m) => match &m.opt {
            Some(o) => rep.checks.push(check(
                "edns0",
                Status::Pass,
                Some(true),
                format!("Yes · {} B", o.udp_size),
                format!("OPT in response, UDP payload {} bytes, version {}", o.udp_size, o.version),
            )),
            None => {
                rep.checks.push(check("edns0", Status::Fail, Some(false), "No OPT record", "response carried no EDNS OPT pseudo-record"))
            }
        },
        Err(e) => rep.checks.push(unknown("edns0", e)),
    }

    // EDNS version negotiation (RFC 6891 §6.1.3): EDNS(1) must get BADVERS.
    match c.ask(&QuerySpec::new(format!("ub{}.github.io", rand_label(10)), dns::TYPE_A).with_edns(|e| e.version = 1)) {
        Ok(m) if m.rcode == dns::RCODE_BADVERS => rep.checks.push(check(
            "edns_version",
            Status::Pass,
            Some(true),
            "Rejects unknown versions",
            "EDNS(1) query → BADVERS, as RFC 6891 requires",
        )),
        Ok(m) => rep.checks.push(check(
            "edns_version",
            Status::Fail,
            Some(false),
            "Ignores version",
            format!("EDNS(1) query → {}", dns::rcode_name(m.rcode)),
        )),
        Err(e) => rep.checks.push(unknown("edns_version", &e)),
    }

    // DNS Cookies (RFC 7873) and NSID (RFC 5001) in one exchange.
    let client_cookie: [u8; 8] = std::array::from_fn(|_| fastrand::u8(..));
    match c.ask(&QuerySpec::new(format!("ub{}.github.io", rand_label(10)), dns::TYPE_A).with_edns(|e| {
        e.cookie = Some(client_cookie);
        e.nsid = true;
    })) {
        Ok(m) => {
            match m.option(dns::OPT_COOKIE) {
                Some(v) if v.len() > 8 && v[..8] == client_cookie => rep.checks.push(check(
                    "cookies",
                    Status::Pass,
                    Some(true),
                    "Server cookie",
                    format!("{}-byte server cookie returned", v.len() - 8),
                )),
                _ => rep.checks.push(check("cookies", Status::Info, Some(false), "Not supported", "no server cookie in response")),
            }
            rep.nsid = m.nsid();
            if let Some(n) = &rep.nsid {
                rep.checks.push(check("nsid", Status::Info, Some(true), n.clone(), format!("NSID option: {n}")));
            } else {
                rep.checks.push(check("nsid", Status::Info, Some(false), "Not returned", "no NSID option in response"));
            }
        }
        Err(e) => {
            rep.checks.push(unknown("cookies", &e));
            rep.checks.push(unknown("nsid", &e));
        }
    }

    // Anycast site identity via CHAOS TXT.
    for name in ["id.server", "hostname.bind"] {
        if let Ok(m) = c.ask(&QuerySpec::chaos_txt(name))
            && let Some(t) = m.txt_answers().into_iter().find(|t| !t.is_empty())
        {
            rep.pop = Some(t);
            break;
        }
    }
    if let Some(p) = &rep.pop {
        rep.checks.push(check("pop", Status::Info, Some(true), p.clone(), format!("CH TXT id.server → \"{p}\"")));
    }

    // Egress address and EDNS Client Subnet, as seen by Google's and Akamai's
    // authoritative servers: resolvers can treat the two differently.
    let google = c.ask(&QuerySpec::new("o-o.myaddr.l.google.com", dns::TYPE_TXT));
    if let Ok(m) = &google {
        for t in m.txt_answers() {
            if let Some(rest) = t.strip_prefix("edns0-client-subnet ") {
                rep.ecs = Some(rest.trim().to_string());
            } else if t.parse::<IpAddr>().is_ok() {
                rep.egress.push(t);
            }
        }
    }
    let akamai = c.ask(&QuerySpec::new("whoami.ds.akahelp.net", dns::TYPE_TXT));
    if let Ok(m) = &akamai {
        rep.ecs_akamai = akahelp_ecs(m);
    }
    let prefix = |s: &str| s.rsplit('/').next().unwrap_or("?").to_string();
    match (&google, &akamai) {
        (Err(e), Err(_)) => rep.checks.push(unknown("ecs", e)),
        _ => {
            let seen = |who: &str, v: &Option<String>, ok: bool| match (v, ok) {
                (Some(s), _) => format!("{who}'s nameservers saw ECS {s}"),
                (None, true) => format!("{who}'s nameservers saw no client subnet"),
                (None, false) => format!("{who}'s test didn't answer"),
            };
            let detail = format!("{}; {}", seen("Google", &rep.ecs, google.is_ok()), seen("Akamai", &rep.ecs_akamai, akamai.is_ok()));
            rep.checks.push(match (&rep.ecs, &rep.ecs_akamai) {
                (Some(g), _) => check("ecs", Status::Info, Some(true), format!("Sends /{}", prefix(g)), detail),
                (None, Some(a)) => check("ecs", Status::Info, Some(true), format!("Akamai /{}", prefix(a)), detail),
                (None, None) => check("ecs", Status::Info, Some(false), "Not sent", detail),
            });
        }
    }
    if rep.egress.is_empty()
        && let Ok(m) = c.ask(&QuerySpec::new("whoami.akamai.net", dns::TYPE_A))
    {
        rep.egress.extend(m.ipv4_answers().iter().map(|a| a.to_string()));
    }

    // QNAME minimisation (RFC 9156), via internet.nl's test zone.
    match c.ask(&QuerySpec::new("qnamemintest.internet.nl", dns::TYPE_TXT)) {
        Ok(m) => {
            let txt = m.txt_answers().join(" ");
            rep.checks.push(if txt.contains("HOORAY") {
                check("qmin", Status::Pass, Some(true), "Enabled", "qnamemintest.internet.nl → \"HOORAY - QNAME minimisation is enabled\"")
            } else if txt.contains("NO") {
                check(
                    "qmin",
                    Status::Fail,
                    Some(false),
                    "Not enabled",
                    format!("qnamemintest.internet.nl → \"{}\"", txt.chars().take(80).collect::<String>()),
                )
            } else {
                check("qmin", Status::Unknown, None, "Inconclusive", format!("qnamemintest.internet.nl → {}", summary(&m)))
            });
        }
        Err(e) => rep.checks.push(unknown("qmin", &e)),
    }

    // NXDOMAIN integrity: a name that cannot exist must stay nonexistent.
    let nx = format!("ub{}.com", rand_label(14));
    match c.ask(&QuerySpec::new(&nx, dns::TYPE_A)) {
        Ok(m) if m.rcode == dns::RCODE_NXDOMAIN => {
            rep.checks.push(check("nxdomain", Status::Pass, Some(true), "Honest", format!("{nx} → NXDOMAIN")))
        }
        Ok(m) if !m.ipv4_answers().is_empty() => rep.checks.push(check(
            "nxdomain",
            Status::Fail,
            Some(false),
            "Rewrites NXDOMAIN",
            format!("{nx} → {} (nonexistent name answered: ad or search redirection)", summary(&m)),
        )),
        Ok(m) => rep.checks.push(check("nxdomain", Status::Unknown, None, "Inconclusive", format!("{nx} → {}", summary(&m)))),
        Err(e) => rep.checks.push(unknown("nxdomain", &e)),
    }

    // DNS rebinding protection: public names that point at private addresses.
    let rebind_name = format!("ub{}.localtest.me", rand_label(10));
    match c.ask(&QuerySpec::new(&rebind_name, dns::TYPE_A)) {
        Ok(m) if m.ipv4_answers().contains(&Ipv4Addr::LOCALHOST) => rep.checks.push(check(
            "rebind",
            Status::Info,
            Some(false),
            "Returns private addresses",
            format!("{rebind_name} → 127.0.0.1 returned unfiltered"),
        )),
        // Protection removes the private record (an empty answer) or refuses the name. A
        // SERVFAIL or another error says nothing either way.
        Ok(m) if matches!(m.rcode, dns::RCODE_NOERROR | dns::RCODE_NXDOMAIN | dns::RCODE_REFUSED) => rep.checks.push(check(
            "rebind",
            Status::Info,
            Some(true),
            "Filters private addresses",
            format!("{rebind_name} → {}", summary(&m)),
        )),
        Ok(m) => rep.checks.push(check("rebind", Status::Unknown, None, "Inconclusive", format!("{rebind_name} → {}", summary(&m)))),
        Err(e) => rep.checks.push(unknown("rebind", &e)),
    }

    // Extended DNS Errors: did any failure carry an RFC 8914 code?
    let emits_ede = matches!(&bogus, Ok(b) if !b.ede().is_empty());
    rep.checks.push(if emits_ede {
        check(
            "ede",
            Status::Pass,
            Some(true),
            "Emits EDE",
            format!("{bogus_name} carried {}", bogus.as_ref().map(ede_text).unwrap_or_default()),
        )
    } else if bogus.is_ok() {
        check("ede", Status::Info, Some(false), "None seen", "no Extended DNS Error on a DNSSEC failure")
    } else {
        check("ede", Status::Unknown, None, "Couldn't test", "")
    });

    // Filtering categories. Live malware and phishing names go only over the separate
    // encrypted connection when one is given: in plain DNS, security appliances on the
    // path can see the names and react, up to quarantining this computer's DNS.
    let mut styles: BTreeMap<String, usize> = BTreeMap::new();
    let mut threat = threat_ex.map(|ex| Ctx { ex, timeout: filter_timeout(timeout) });
    for (cat, domains, source) in &sets.categories {
        let is_threat = matches!(cat.as_str(), "malware" | "phishing");
        let key: &'static str = match cat.as_str() {
            "malware" => "filter_malware",
            "phishing" => "filter_phishing",
            "ads" => "filter_ads",
            "adult" => "filter_adult",
            _ => "filter_other",
        };
        if is_threat && threat.is_none() {
            rep.checks.push(check(
                key,
                Status::Unknown,
                None,
                "Not tested",
                "no encrypted endpoint; live threat names go over plain DNS only to your own resolver on the local network, or when you allow it in the test plan",
            ));
            continue;
        }
        let mut fc = FilterCategory { source: source.clone(), ..Default::default() };
        let mut timeouts = 0;
        for d in domains {
            // Repeated timeouts mean the path stopped answering; don't wait out the rest.
            if timeouts >= 3 {
                fc.samples.push(FilterSample { domain: d.clone(), blocked: None, how: "skipped after 3 timeouts in a row".into() });
                continue;
            }
            let q = QuerySpec::new(d.as_str(), dns::TYPE_A);
            let answer = match threat.as_mut().filter(|_| is_threat) {
                Some(t) => t.ask(&q),
                None => c.ask(&q),
            };
            let s = match answer {
                Ok(m) => {
                    timeouts = 0;
                    match block_signal(&m) {
                        Some(how) => {
                            *styles.entry(how.split(" (").next().unwrap_or(&how).to_string()).or_default() += 1;
                            FilterSample { domain: d.clone(), blocked: Some(true), how }
                        }
                        None => FilterSample { domain: d.clone(), blocked: Some(false), how: summary(&m) },
                    }
                }
                Err(e) => {
                    timeouts += (e == XErr::Timeout) as u32;
                    FilterSample { domain: d.clone(), blocked: None, how: e.to_string() }
                }
            };
            if let Some(b) = s.blocked {
                fc.tested += 1;
                fc.blocked += b as usize;
            }
            fc.samples.push(s);
        }
        let (status, present, value) = if fc.tested == 0 {
            (Status::Unknown, None, "Couldn't test".to_string())
        } else {
            let share = fc.blocked as f64 / fc.tested as f64;
            let status = match (is_threat, share) {
                (true, s) if s >= 0.5 => Status::Pass,
                (true, _) if fc.blocked > 0 => Status::Partial,
                (true, _) => Status::Fail,
                _ => Status::Info,
            };
            (status, Some(share >= 0.5), format!("{}/{} blocked", fc.blocked, fc.tested))
        };
        let blocked: Vec<String> =
            fc.samples.iter().filter(|s| s.blocked == Some(true)).map(|s| format!("{} → {}", s.domain, s.how)).take(3).collect();
        rep.checks.push(check(key, status, present, value, format!("{source} · {}", blocked.join("; "))));
        rep.filtering.insert(cat.clone(), fc);
    }
    rep.block_style = styles.into_iter().max_by_key(|(_, n)| *n).map(|(s, _)| s);

    rep.duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    rep
}

/// Keep only names an unfiltered baseline resolves to routable addresses.
pub fn validate_live(baseline: &mut dyn Exchanger, candidates: &[String], want: usize, timeout: Duration) -> Vec<String> {
    let mut c = Ctx { ex: baseline, timeout };
    let mut live = Vec::new();
    for d in candidates {
        if live.len() >= want {
            break;
        }
        if let Ok(m) = c.ask(&QuerySpec::new(d.as_str(), dns::TYPE_A))
            && m.rcode == dns::RCODE_NOERROR
            && block_signal(&m).is_none()
            && !m.ipv4_answers().is_empty()
        {
            live.push(d.clone());
        }
    }
    live
}

/// The ECS that Akamai's whoami saw, from TXT strings like `"ecs" "203.0.113.120/29/24"`
/// (address / source prefix / scope), as "203.0.113.120/29".
fn akahelp_ecs(m: &dns::Message) -> Option<String> {
    m.answers.iter().find_map(|r| match &r.data {
        dns::RData::Txt(parts) if parts.len() >= 2 && parts[0] == "ecs" => {
            let mut it = parts[1].split('/');
            let (addr, len) = (it.next()?, it.next()?);
            addr.parse::<IpAddr>().ok()?;
            len.parse::<u8>().ok()?;
            Some(format!("{addr}/{len}"))
        }
        _ => None,
    })
}
