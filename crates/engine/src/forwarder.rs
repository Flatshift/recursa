//! Is a local resolver a DNS forwarder, like the dnsmasq inside most home routers and Pi-hole?
//!
//! Such a resolver isn't an alternative to choose: it answers repeats from a small cache on your
//! own network and forwards everything else to whatever it is set to use. Benchmarks show it as a
//! baseline and never name it the best pick, but ISP and other resolvers still compete.
//!
//! dnsmasq (and Pi-hole, which is built on it) answers the standard CHAOS query `version.bind` with
//! its own name, which is far more reliable than guessing from an address. A router that doesn't
//! answer is still treated as a forwarder when it is this computer's default gateway.

use crate::dns::{self, QuerySpec};
use crate::net::{Exchanger, UdpX};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// Text a device on the network supplied, made safe to show: printable ASCII, at most 80 characters.
pub fn clean(s: &str) -> String {
    s.chars().filter(|c| (' '..='~').contains(c)).take(80).collect::<String>().trim().to_string()
}

/// What `version.bind` says, as a short description, if it names a forwarder we know.
pub fn describe(version: &str) -> Option<String> {
    let v = version.trim();
    let lower = v.to_ascii_lowercase();
    if lower.contains("pi-hole") {
        Some(format!("Pi-hole ({v})"))
    } else if lower.contains("dnsmasq") {
        Some(format!("dnsmasq ({v})"))
    } else {
        None
    }
}

/// The first non-empty answer, cleaned, to a CHAOS TXT question (`version.bind`, `id.server`) asked through `x`.
pub fn chaos_txt(x: &mut dyn Exchanger, name: &str, timeout: Duration) -> Option<String> {
    let m = dns::ask_msg(x, &QuerySpec::chaos_txt(name), timeout).ok()?;
    m.txt_answers().into_iter().map(|t| clean(&t)).find(|t| !t.is_empty())
}

/// Ask a resolver for its version name over plain DNS. Blocking; a second at most.
pub fn version_bind(ip: IpAddr) -> Option<String> {
    let mut x = UdpX::connect(SocketAddr::new(ip, 53)).ok()?;
    chaos_txt(&mut x, "version.bind", Duration::from_millis(900))
}

/// A description of the forwarder at `ip`, when it is one: dnsmasq or Pi-hole by what it says it is, or
/// the default gateway (a router's DNS proxy). Only private addresses are ever asked. Blocking.
pub fn identify(ip: IpAddr, gateway: Option<IpAddr>) -> Option<String> {
    if !crate::catalog::is_private(&ip) {
        return None;
    }
    let named = version_bind(ip).and_then(|v| describe(&v));
    match (named, gateway == Some(ip)) {
        (Some(n), true) => Some(format!("{n}, your default gateway")),
        (Some(n), false) => Some(n),
        (None, true) => Some("your default gateway (a router's DNS proxy)".into()),
        (None, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dnsmasq_and_pi_hole_name_themselves() {
        assert_eq!(describe("dnsmasq-pi-hole-v2.93").as_deref(), Some("Pi-hole (dnsmasq-pi-hole-v2.93)"));
        assert_eq!(describe("dnsmasq-UNKNOWN").as_deref(), Some("dnsmasq (dnsmasq-UNKNOWN)"));
        assert_eq!(describe("dnsmasq-2.90").as_deref(), Some("dnsmasq (dnsmasq-2.90)"));
        // Real recursive resolvers that run on a LAN are alternatives like any other.
        assert_eq!(describe("unbound 1.19.0"), None);
        assert_eq!(describe("9.18.24-1-Debian"), None);
        assert_eq!(describe(""), None);
    }

    #[test]
    fn text_from_the_network_is_made_safe_to_show() {
        assert_eq!(clean("dnsmasq-2.90"), "dnsmasq-2.90");
        assert_eq!(clean("a\u{0}b\u{1b}[31mc\n"), "ab[31mc");
        assert_eq!(clean(&"x".repeat(300)).len(), 80);
        assert_eq!(clean("  \u{7f}é "), "");
    }

    #[test]
    fn public_addresses_are_never_asked() {
        assert_eq!(identify("1.1.1.1".parse().unwrap(), None), None);
        assert_eq!(identify("203.0.113.9".parse().unwrap(), Some("203.0.113.9".parse().unwrap())), None);
    }
}
