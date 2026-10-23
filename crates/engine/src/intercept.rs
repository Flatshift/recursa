//! Quick port-53 interception check, independent of a full run.
//!
//! For a few anycast resolvers, compare the true network round trip (TCP handshake to
//! the DoT port on the same address) with how fast a cached plain-DNS answer arrives.
//! An answer that beats the handshake by a wide margin can't have come from the
//! resolver: something on the local network answered it.

use crate::dns::{self, QuerySpec};
use crate::net::{self, Endpoint, Exchanger, Transport, UdpX};
use crate::{bench, clock};
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

const PROBES: &[(&str, &str)] = &[("1.1.1.1", "Cloudflare"), ("8.8.8.8", "Google"), ("9.9.9.9", "Quad9")];

/// Best of three TCP handshakes to the DoT port, or HTTPS where 853 is blocked.
fn handshake_ms(ip: &str) -> Option<f64> {
    [853, 443].into_iter().find_map(|port| {
        let addr: SocketAddr = format!("{ip}:{port}").parse().ok()?;
        (0..3)
            .filter_map(|_| {
                let t = clock::now_ns();
                TcpStream::connect_timeout(&addr, Duration::from_millis(1500)).ok()?;
                Some(clock::now_ns().saturating_sub(t) as f64 / 1e6)
            })
            .reduce(f64::min)
    })
}

/// Fastest cached plain-DNS answer (ms) and the fewest router hops any answer crossed.
fn udp_probe(ip: &str) -> (Option<f64>, Option<u8>) {
    let Some(mut x) = format!("{ip}:53").parse().ok().and_then(|a| UdpX::connect(a).ok()) else { return (None, None) };
    let (mut best, mut hops): (Option<f64>, Option<u8>) = (None, None);
    for i in 0..4 {
        let Ok(q) = dns::encode_query(dns::new_id(), &QuerySpec::new("www.google.com", dns::TYPE_A)) else { continue };
        let Ok(r) = x.exchange(&q, Duration::from_millis(1500)) else { continue };
        if let Some(h) = r.ttl.map(net::hops) {
            hops = Some(hops.map_or(h, |x| x.min(h)));
        }
        // The first query only warms caches along the path.
        if i > 0 {
            let ms = r.rtt_ns as f64 / 1e6;
            best = Some(best.map_or(ms, |b| b.min(ms)));
        }
    }
    (best, hops)
}

/// What a box says about itself when asked the CHAOS questions `version.bind` and `id.server` at an
/// address that isn't its own: a real resolver gives a site code for `id.server` and nothing for
/// `version.bind`; dnsmasq, which is what most home routers and Pi-hole run, gives its version.
fn chaos(ip: &str, tcp: bool) -> (Option<String>, Option<String>) {
    let Ok(addr) = format!("{ip}:53").parse::<SocketAddr>() else { return (None, None) };
    let ask = |name: &str| -> Option<String> {
        let ep = Endpoint { transport: if tcp { Transport::Tcp } else { Transport::Udp }, addr, host: None, url: None };
        let mut x = net::open(&ep, Duration::from_millis(1000)).ok()?;
        let q = dns::encode_query(dns::new_id(), &QuerySpec::chaos_txt(name)).ok()?;
        let r = x.exchange(&q, Duration::from_millis(1000)).ok()?;
        dns::parse(&r.bytes).ok()?.txt_answers().into_iter().map(|t| crate::forwarder::clean(&t)).find(|t| !t.is_empty())
    };
    (ask("version.bind"), ask("id.server"))
}

/// Fastest of three queries for names nobody has asked before (random labels under .com, which can't
/// exist): what the real path costs when no cache can hold the answer.
fn fresh_name_ms(ip: &str) -> Option<f64> {
    let mut x = format!("{ip}:53").parse().ok().and_then(|a| UdpX::connect(a).ok())?;
    (0..3)
        .filter_map(|_| {
            let q =
                dns::encode_query(dns::new_id(), &QuerySpec::new(format!("ub{}.com", crate::probes::rand_label(14)), dns::TYPE_A)).ok()?;
            x.exchange(&q, Duration::from_millis(1500)).ok().map(|r| r.rtt_ns as f64 / 1e6)
        })
        .reduce(f64::min)
}

/// Best of three TCP handshakes to port 53.
fn handshake53_ms(ip: &str) -> Option<f64> {
    let addr: SocketAddr = format!("{ip}:53").parse().ok()?;
    (0..3)
        .filter_map(|_| {
            let t = clock::now_ns();
            TcpStream::connect_timeout(&addr, Duration::from_millis(1000)).ok()?;
            Some(clock::now_ns().saturating_sub(t) as f64 / 1e6)
        })
        .reduce(f64::min)
}

/// A device this computer uses for DNS, and what it says its version is.
struct Local {
    what: String,
    ip: IpAddr,
    version: Option<String>,
}

/// The resolvers this computer is set to use and its default gateway, asked for `version.bind`, except `asked`, an
/// address whose version was just read.
fn locals(asked: (&str, &Option<String>)) -> Vec<Local> {
    let gateway = crate::sysdns::default_gateway();
    let mut ips = crate::sysdns::discover();
    ips.extend(gateway);
    let mut seen = Vec::new();
    ips.into_iter()
        .filter(|ip| {
            crate::catalog::is_private(ip) && !seen.contains(ip) && {
                seen.push(*ip);
                true
            }
        })
        .map(|ip| {
            let version = if ip.to_string() == asked.0 { asked.1.clone() } else { crate::forwarder::version_bind(ip) };
            let what = match (version.as_deref().and_then(crate::forwarder::describe), gateway == Some(ip)) {
                (Some(d), true) => format!("{d}, your default gateway"),
                (Some(d), false) => d,
                (None, true) => "your default gateway".to_string(),
                (None, false) => "a resolver this computer uses".to_string(),
            };
            Local { what, ip, version }
        })
        .collect()
}

/// Reads the box that answered: who it says it is, whether that matches a device you know, and whether
/// TCP is intercepted too. `hops` is how many routers the replies crossed; `tcp` is "intercepted", "direct"
/// or "closed" for TCP port 53. These are tells, not proof.
fn attribute(
    version: Option<&str>,
    id: Option<&str>,
    hops: Option<u64>,
    tcp: Option<&str>,
    cache: Option<(f64, f64)>,
    locals: &[Local],
) -> (String, String) {
    let who = match version {
        Some(v) => {
            let same: Vec<&Local> = locals.iter().filter(|l| l.version.as_deref() == Some(v)).collect();
            if !same.is_empty() {
                let names: Vec<String> = same.iter().map(|l| format!("{} at {}", l.what, l.ip)).collect();
                let more = if same.len() > 1 { " They answer the same, so they can't be told apart this way." } else { "" };
                ("match", format!("Asked for its version, the box that answered said “{v}”, the same as {}.{more}", names.join(" and ")))
            } else if v.to_ascii_lowercase().contains("dnsmasq") {
                ("elsewhere", format!("Asked for its version, the box that answered said “{v}”: dnsmasq, but not the version of any resolver this computer uses, so another device on your network (an access point, a second router, a Pi-hole that isn't your DNS server)."))
            } else {
                ("other", format!("Asked for its version, the answer was “{v}”. A real resolver can give that itself, so it doesn't identify the box."))
            }
        }
        None if id.is_some() => ("passes", "It answers id.server but not version.bind, so it may pass CHAOS questions on to the real resolver; this can't identify it.".into()),
        None => ("silent", "It answers ordinary lookups but ignores version.bind and id.server, which dnsmasq (what home routers and Pi-hole run) would answer. That fits an access point feature or a firewall appliance better.".into()),
    };
    let wh = match hops {
        Some(0) => " Its replies crossed no router, so a device on your own link answered: an access point, or your gateway itself.",
        Some(1) => {
            " Its replies crossed one router, so a box behind your gateway answered, for example a Pi-hole the gateway redirects to."
        }
        _ => "",
    };
    // A name it has seen answered far faster than one it hasn't: the box keeps a cache of its own.
    let ca = match cache {
        Some((seen, fresh)) if fresh >= 4.0 && seen < fresh * 0.5 => format!(
            " A name it had already seen came back in {seen:.1} ms and one it hadn't took {fresh:.1} ms: it answers from a cache of its own, so cached lookups here measure that cache and not the resolver."
        ),
        _ => String::new(),
    };
    let tc = match tcp {
        Some("intercepted") => " TCP port 53 is answered locally too, which fits a rule that covers both protocols.",
        Some("direct") => {
            " TCP port 53 reaches the real resolver, so only UDP is being intercepted, which a rule written for both protocols would not do."
        }
        Some("closed") => " TCP port 53 doesn't answer at all (blocked or filtered).",
        _ => "",
    };
    (who.0.to_string(), format!("{}{ca}{wh}{tc}", who.1))
}

pub fn check() -> Value {
    let results: Vec<Value> = PROBES
        .iter()
        .map(|(ip, name)| {
            let rtt = handshake_ms(ip);
            let (udp, hops) = udp_probe(ip);
            // Either signal is enough: an answer faster than the path allows, or one that
            // crossed no router although the resolver is several networks away.
            let too_fast = matches!((rtt, udp), (Some(r), Some(u)) if r >= 4.0 && u < r * 0.5);
            let on_lan = hops.is_some_and(|h| h <= 1);
            json!({ "ip": ip, "name": name, "path_ms": rtt, "udp_ms": udp, "hops": hops, "intercepted": too_fast || on_lan, "on_lan": on_lan })
        })
        .collect();
    let decided: Vec<&Value> =
        results.iter().filter(|r| r["udp_ms"].is_number() && (r["path_ms"].is_number() || r["hops"].is_number())).collect();
    let hits = decided.iter().filter(|r| r["intercepted"] == true).count();
    let verdict = if decided.is_empty() {
        "unknown"
    } else if hits * 2 > decided.len() {
        "intercepted"
    } else if hits > 0 {
        "partial"
    } else {
        "direct"
    };
    // Who answered: asked of the first address that was answered locally.
    let fingerprint = results.iter().find(|r| r["intercepted"] == true).map(|r| {
        let ip = r["ip"].as_str().unwrap_or_default();
        let (version, id) = chaos(ip, false);
        let (tcp_version, tcp_id) = chaos(ip, true);
        let t53 = handshake53_ms(ip);
        let tcp = match (t53, r["path_ms"].as_f64()) {
            (None, _) => "closed",
            (Some(t), Some(p)) if p >= 4.0 && t < p * 0.5 => "intercepted",
            (Some(_), Some(_)) => "direct",
            _ => "unknown",
        };
        let fresh = fresh_name_ms(ip);
        let cache = r["udp_ms"].as_f64().zip(fresh);
        let (kind, summary) = attribute(version.as_deref(), id.as_deref(), r["hops"].as_u64(), Some(tcp), cache, &locals((ip, &version)));
        json!({
            "ip": ip, "name": r["name"], "kind": kind, "summary": summary,
            "udp": { "version": version, "id": id }, "tcp": { "version": tcp_version, "id": tcp_id, "port53": tcp },
        })
    });
    json!({ "verdict": verdict, "results": results, "fingerprint": fingerprint, "link": crate::sysdns::link(), "checked_at": bench::now_ms() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(what: &str, ip: &str, version: &str) -> Local {
        Local { what: what.into(), ip: ip.parse().unwrap(), version: Some(version.into()) }
    }

    #[test]
    fn a_dnsmasq_answer_is_matched_to_the_device_that_says_the_same() {
        let locals = [
            local("Pi-hole (dnsmasq-pi-hole-v2.93)", "192.168.1.53", "dnsmasq-pi-hole-v2.93"),
            local("dnsmasq (dnsmasq-UNKNOWN), your default gateway", "192.168.1.1", "dnsmasq-UNKNOWN"),
        ];
        let (kind, s) = attribute(Some("dnsmasq-pi-hole-v2.93"), None, Some(1), Some("intercepted"), None, &locals);
        assert_eq!(kind, "match");
        assert!(s.contains("Pi-hole") && s.contains("192.168.1.53") && !s.contains("192.168.1.1"), "{s}");
        assert!(s.contains("one router") && s.contains("both protocols"), "{s}");
        let (_, s) = attribute(Some("dnsmasq-UNKNOWN"), None, Some(0), Some("direct"), None, &locals);
        assert!(s.contains("default gateway") && s.contains("no router") && s.contains("only UDP"), "{s}");
    }

    /// The network half: ask a real dnsmasq (`RECURSA_TEST_DNSMASQ=<ip>`) over UDP and over TCP.
    #[test]
    #[ignore = "needs a dnsmasq to ask"]
    fn chaos_questions_reach_a_real_dnsmasq() {
        let Ok(ip) = std::env::var("RECURSA_TEST_DNSMASQ") else { return };
        for tcp in [false, true] {
            let (version, _) = chaos(&ip, tcp);
            assert!(version.as_deref().is_some_and(|v| v.contains("dnsmasq")), "tcp={tcp}: {version:?}");
        }
        assert!(handshake53_ms(&ip).is_some());
    }

    #[test]
    fn a_box_with_its_own_cache_is_said_to_have_one() {
        let (_, s) = attribute(None, Some("iad10"), Some(0), Some("direct"), Some((2.4, 21.0)), &[]);
        assert!(s.contains("2.4 ms") && s.contains("21.0 ms") && s.contains("cache of its own"), "{s}");
        // Seen and unseen names alike taking the path's time: nothing to say.
        assert!(!attribute(None, None, None, None, Some((15.0, 16.0)), &[]).1.contains("cache of its own"));
        assert!(!attribute(None, None, None, None, Some((1.0, 2.0)), &[]).1.contains("cache of its own"));
    }

    #[test]
    fn unknown_answers_are_said_to_be_unknown() {
        let locals = [local("dnsmasq (dnsmasq-UNKNOWN)", "192.168.1.1", "dnsmasq-UNKNOWN")];
        let (k, s) = attribute(Some("dnsmasq-2.90"), None, Some(0), None, None, &locals);
        assert_eq!(k, "elsewhere");
        assert!(s.contains("another device"), "{s}");
        // Nothing that dnsmasq would answer: probably not dnsmasq.
        let (k, s) = attribute(None, None, Some(0), Some("closed"), None, &locals);
        assert_eq!(k, "silent");
        assert!(s.contains("access point") && s.contains("doesn't answer at all"), "{s}");
        assert_eq!(attribute(None, Some("iad05"), None, None, None, &locals).0, "passes");
        // Two devices with the same string can't be told apart.
        let twins = [local("a", "10.0.0.1", "dnsmasq-UNKNOWN"), local("b", "10.0.0.2", "dnsmasq-UNKNOWN")];
        assert!(attribute(Some("dnsmasq-UNKNOWN"), None, None, None, None, &twins).1.contains("can't be told apart"));
    }
}
