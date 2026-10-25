//! Where to reach a resolver over each transport.
//!
//! DoT and DoH hosts are resolved without trusting the system resolver, which on many
//! networks deliberately sinkholes encrypted-DNS hostnames. The resolver itself is asked
//! first, then well-known public resolvers. Sinkhole answers are rejected, and the
//! resolver's own address is the last resort.

use crate::catalog::{self, Resolver};
use crate::dns::{self, QuerySpec};
use crate::net::{self, Endpoint, Exchanger, Transport};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(600);

/// Address for a DoT/DoH host, plus a sentence on how it was found. Successful lookups
/// are cached for ten minutes so monitoring cycles don't repeat them. Without `plain` (an
/// encrypted-only run) the lookup goes over DoT only.
pub fn bootstrap(host: &str, want_v6: bool, r: &Resolver, timeout: Duration, plain: bool) -> (Option<IpAddr>, String) {
    type Cache = Mutex<HashMap<(String, bool), (IpAddr, String, Instant)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (host.to_ascii_lowercase(), want_v6);
    if let Some((ip, how, at)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key)
        && at.elapsed() < CACHE_TTL
    {
        return (Some(*ip), how.clone());
    }

    let qtype = if want_v6 { dns::TYPE_AAAA } else { dns::TYPE_A };
    let short = timeout.min(Duration::from_millis(1500));
    for (ep, via) in lookup_servers(r, plain) {
        let Ok(mut x) = net::open(&ep, short) else { continue };
        let Ok(dns::Answer { msg: m, .. }) = dns::ask(x.as_mut(), &QuerySpec::new(host, qtype), short) else { continue };
        // CGNAT stays allowed: some resolvers live there (Tailscale's 100.100.100.100).
        let found = if want_v6 {
            m.ipv6_answers().into_iter().map(IpAddr::V6).find(|ip| !crate::catalog::is_local_or_special_but_cgnat(ip))
        } else {
            m.ipv4_answers().into_iter().map(IpAddr::V4).find(|ip| !crate::catalog::is_local_or_special_but_cgnat(ip))
        };
        if let Some(ip) = found {
            let how = format!("{host} → {ip} via {via}");
            cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (ip, how.clone(), Instant::now()));
            return (Some(ip), how);
        }
    }
    (r.primary(want_v6), format!("{host}: couldn't resolve; using the resolver's own address"))
}

/// Who `bootstrap` asks: the resolver itself, then public resolvers, over plain UDP; or
/// only public resolvers over DoT when the run sends no plain DNS.
fn lookup_servers(r: &Resolver, plain: bool) -> Vec<(Endpoint, String)> {
    if !plain {
        return [("1.1.1.1", "one.one.one.one"), ("9.9.9.9", "dns.quad9.net"), ("8.8.8.8", "dns.google")]
            .into_iter()
            .map(|(ip, host)| {
                let ep = Endpoint {
                    transport: Transport::Dot,
                    addr: SocketAddr::new(ip.parse().unwrap(), 853),
                    host: Some(host.into()),
                    url: None,
                };
                (ep, format!("{host} (DoT)"))
            })
            .collect();
    }
    let own = r.any_ip().filter(|_| r.udp).map(|ip| (ip, format!("{} itself", r.label())));
    let public = ["1.1.1.1", "9.9.9.9", "8.8.8.8"].into_iter().map(|s| (s.parse::<IpAddr>().unwrap(), s.to_string()));
    own.into_iter().chain(public).map(|(ip, via)| (Endpoint::new(Transport::Udp, SocketAddr::new(ip, 53)), via)).collect()
}

/// Plain DNS (UDP or TCP) on port 53 of the resolver's primary address.
pub fn plain(r: &Resolver, t: Transport, want_v6: bool) -> Option<Endpoint> {
    let ip = r.primary(want_v6)?;
    Some(Endpoint::new(t, SocketAddr::new(ip, 53)))
}

/// The DoT or DoH endpoint the resolver publishes, with its address bootstrapped.
pub fn encrypted(r: &Resolver, t: Transport, want_v6: bool, timeout: Duration, plain: bool) -> Option<(Endpoint, String)> {
    let (host, url, port) = match t {
        Transport::Dot => (r.dot.clone()?, None, 853),
        Transport::Doh => {
            let url = r.doh.clone()?;
            (url.parse::<http::Uri>().ok()?.host()?.to_string(), Some(url), 443)
        }
        Transport::Udp | Transport::Tcp => return None,
    };
    let (addr, how) = bootstrap(&host, want_v6, r, timeout, plain);
    Some((Endpoint { transport: t, addr: SocketAddr::new(addr?, port), host: Some(host), url }, how))
}

/// Best connection for probes, CDN lookups and monitoring. Plain UDP unless that path is
/// intercepted; then DoT or DoH, which a transparent proxy can't answer; then TCP; then
/// UDP as the last resort. A private (local) resolver is always asked over UDP: it *is*
/// the device on the path. `planned` holds endpoints already bootstrapped for this run.
pub fn open_best(r: &Resolver, planned: &[Endpoint], timeout: Duration, avoid_udp: bool) -> Option<(Box<dyn Exchanger>, &'static str)> {
    let local = r.any_ip().is_some_and(|ip| catalog::is_private(&ip));
    let open_plain = |t: Transport| {
        let ep = plain(r, t, false).or_else(|| plain(r, t, true))?;
        net::open(&ep, timeout).ok().map(|x| (x, t.label()))
    };
    if r.udp
        && (!avoid_udp || local)
        && let Some(x) = open_plain(Transport::Udp)
    {
        return Some(x);
    }
    if let Some(x) = open_encrypted(r, planned, timeout, true) {
        return Some(x);
    }
    if !r.udp {
        return None;
    }
    open_plain(Transport::Tcp).or_else(|| open_plain(Transport::Udp))
}

/// A DoT (preferred) or DoH connection to the resolver, if it publishes one. `plain` says whether
/// finding its address may use plain DNS (see `bootstrap`).
pub fn open_encrypted(r: &Resolver, planned: &[Endpoint], timeout: Duration, plain: bool) -> Option<(Box<dyn Exchanger>, &'static str)> {
    [Transport::Dot, Transport::Doh].into_iter().find_map(|t| {
        let ep = planned.iter().find(|e| e.transport == t).cloned().or_else(|| encrypted(r, t, false, timeout, plain).map(|(e, _)| e))?;
        net::open(&ep, timeout).ok().map(|x| (x, t.label()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_only_bootstrap_sends_no_plain_dns() {
        let r = Resolver::custom("192.0.2.53".parse().unwrap(), None, Some("dot.bootstrap-test.invalid".into()), None);
        let eps = lookup_servers(&r, false);
        assert!(!eps.is_empty() && eps.iter().all(|(e, _)| e.transport == Transport::Dot && e.addr.port() == 853 && e.host.is_some()));
        assert!(lookup_servers(&r, true).iter().all(|(e, _)| e.transport == Transport::Udp));
    }
}
