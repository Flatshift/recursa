//! Resolver catalog: curated public resolvers, discovered system resolvers and
//! user-added custom resolvers share one shape.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Published {
    #[serde(default)]
    pub dnssec: Option<bool>,
    #[serde(default)]
    pub ecs: Option<bool>,
    #[serde(default)]
    pub qmin: Option<bool>,
    #[serde(default)]
    pub filtering: Vec<String>,
    #[serde(default)]
    pub doq: Option<bool>,
    #[serde(default)]
    pub dnscrypt: Option<bool>,
    #[serde(default)]
    pub odoh: Option<bool>,
    #[serde(default)]
    pub logging: Option<String>,
    #[serde(default)]
    pub jurisdiction: Option<String>,
}

/// What an operator itself says about how much traffic one client may send. Only what is published:
/// a resolver with nothing published has no `Limits`, and nothing here is guessed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Limits {
    /// Queries per second from one client address above which the operator says it may throttle, when it gives a number.
    #[serde(default)]
    pub qps: Option<f64>,
    /// What that number applies to, as the operator puts it ("one IPv4 address or IPv6 /64").
    #[serde(default)]
    pub per: Option<String>,
    /// What the operator says, in our words.
    pub text: String,
    /// Where it says so.
    pub source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Links {
    #[serde(default)]
    pub home: Option<String>,
    #[serde(default)]
    pub privacy: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resolver {
    pub id: String,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub variant: String,
    /// public | legacy | system | custom
    pub group: String,
    #[serde(default)]
    pub default: bool,
    /// Answers plain DNS on port 53.
    #[serde(default = "yes")]
    pub udp: bool,
    #[serde(default)]
    pub ipv4: Vec<String>,
    #[serde(default)]
    pub ipv6: Vec<String>,
    #[serde(default)]
    pub dot: Option<String>,
    #[serde(default)]
    pub doh: Option<String>,
    #[serde(default)]
    pub status_feed: Option<String>,
    #[serde(default)]
    pub published: Published,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub links: Links,
    /// What the operator publishes about how much traffic one client may send, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// On a system resolver: the custom entry that names it (same address), so the name
    /// can be removed again. Such an entry is folded in rather than listed twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_id: Option<String>,
    /// On a system resolver that is a local forwarder (dnsmasq, Pi-hole, a router's DNS proxy): what it is.
    /// Such a resolver is measured as a baseline and never named the best pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarder: Option<String>,
    /// On a catalog entry: custom entries the user made for its addresses before it was
    /// listed. They're folded into it (one server, measured once) and their ids still select it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

impl Resolver {
    pub fn label(&self) -> String {
        if self.variant.is_empty() { self.provider.clone() } else { format!("{} {}", self.provider, self.variant) }
    }

    pub fn primary(&self, v6: bool) -> Option<IpAddr> {
        let list = if v6 { &self.ipv6 } else { &self.ipv4 };
        list.iter().find_map(|s| s.parse().ok())
    }

    /// The plain-DNS addresses worth monitoring, primary first: the IPv4 pair when there is
    /// one (IPv6 only for resolvers that have nothing else). Two at most.
    pub fn plain_servers(&self) -> Vec<String> {
        let list = if self.ipv4.iter().any(|s| s.parse::<IpAddr>().is_ok()) { &self.ipv4 } else { &self.ipv6 };
        list.iter().filter(|s| s.parse::<IpAddr>().is_ok()).take(2).cloned().collect()
    }

    /// This resolver reduced to its `i`th plain-DNS address, so the connection helpers that
    /// always pick the primary address reach that one instead.
    pub fn only_server(&self, i: usize) -> Resolver {
        let mut r = self.clone();
        if let Some(ip) = self.plain_servers().get(i) {
            let v4 = ip.parse::<IpAddr>().is_ok_and(|a| a.is_ipv4());
            r.ipv4 = if v4 { vec![ip.clone()] } else { vec![] };
            r.ipv6 = if v4 { vec![] } else { vec![ip.clone()] };
        }
        r
    }

    pub fn custom(ip: IpAddr, name: Option<String>, dot: Option<String>, doh: Option<String>) -> Self {
        let mut r = Resolver {
            id: format!("custom-{}", ip.to_string().replace([':', '.'], "-")),
            provider: name.unwrap_or_else(|| "Custom".into()),
            name: ip.to_string(),
            variant: String::new(),
            group: "custom".into(),
            default: true,
            udp: true,
            ipv4: vec![],
            ipv6: vec![],
            dot,
            doh,
            status_feed: None,
            published: Published::default(),
            notes: None,
            links: Links::default(),
            limits: None,
            custom_id: None,
            aliases: Vec::new(),
            forwarder: None,
        };
        match ip {
            IpAddr::V4(_) => r.ipv4.push(ip.to_string()),
            IpAddr::V6(_) => r.ipv6.push(ip.to_string()),
        }
        r
    }
}

pub fn builtin() -> Vec<Resolver> {
    serde_json::from_str(include_str!("../../../data/catalog.json")).expect("data/catalog.json is valid")
}

/// System resolvers become catalog entries so they flow through the same pipeline.
pub fn system_entries(addrs: &[IpAddr]) -> Vec<Resolver> {
    addrs
        .iter()
        .enumerate()
        .map(|(i, ip)| {
            let mut r = Resolver::custom(*ip, Some("System resolver".into()), None, None);
            r.id = format!("system-{i}");
            r.group = "system".into();
            r.variant = if i == 0 { "Primary".into() } else { format!("#{}", i + 1) };
            r.notes = Some(if is_private(ip) {
                "Configured on this device, usually by your router via DHCP. It may forward to your ISP or to another upstream.".into()
            } else {
                "Configured on this device, usually your ISP's resolver handed out via DHCP.".into()
            });
            r
        })
        .collect()
}

/// An address that is no single host: unspecified, multicast or the IPv4 broadcast address. Like the checks below, it
/// reads an IPv4-mapped IPv6 address (`::ffff:127.0.0.1`) as the IPv4 address it carries.
pub fn is_special(ip: &IpAddr) -> bool {
    let ip = &ip.to_canonical();
    ip.is_unspecified() || ip.is_multicast() || matches!(ip, IpAddr::V4(v) if v.is_broadcast())
}

/// An address no public server should point this computer at: private, loopback, link-local or CGNAT (see
/// [`is_private`]), or [`is_special`].
pub fn is_local_or_special(ip: &IpAddr) -> bool {
    is_special(ip) || is_private(ip)
}

/// [`is_local_or_special`] with CGNAT let through, for answers that can really live there: Tailscale's resolver
/// (100.100.100.100), a carrier's CDN edge. On IPv6, loopback, link-local (fe80::/10, like 169.254/16 on IPv4) and
/// [`is_special`]; unique local addresses stay allowed.
pub fn is_local_or_special_but_cgnat(ip: &IpAddr) -> bool {
    is_special(ip)
        || match ip.to_canonical() {
            IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
            IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xffc0) == 0xfe80,
        }
}

pub fn is_private(ip: &IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local() || v.octets()[0] == 100 && (v.octets()[1] & 0xC0) == 64,
        IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapped_ipv6_address_is_judged_as_the_ipv4_it_carries() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        for local in ["::ffff:127.0.0.1", "::ffff:192.168.1.1", "::ffff:255.255.255.255", "::ffff:224.0.0.1"] {
            assert!(is_local_or_special(&ip(local)), "{local}");
            assert!(is_local_or_special_but_cgnat(&ip(local)), "{local}");
        }
        assert!(!is_local_or_special(&ip("::ffff:1.1.1.1")));
        assert!(is_local_or_special(&ip("100.100.100.100")) && !is_local_or_special_but_cgnat(&ip("100.100.100.100")));
        assert!(is_local_or_special_but_cgnat(&ip("fe80::1")) && !is_local_or_special_but_cgnat(&ip("fd7a:115c:a1e0::53")));
    }

    fn find(id: &str) -> Resolver {
        builtin().into_iter().find(|r| r.id == id).unwrap()
    }

    #[test]
    fn primary_and_secondary_are_each_reachable_alone() {
        let cf = find("cloudflare");
        assert_eq!(cf.plain_servers(), ["1.1.1.1", "1.0.0.1"]);
        assert_eq!(cf.only_server(0).primary(false), "1.1.1.1".parse().ok());
        assert_eq!(cf.only_server(1).primary(false), "1.0.0.1".parse().ok());
        assert_eq!(cf.only_server(1).primary(true), None, "the secondary alone must not fall back to the primary's IPv6");
        // One address, or none beyond IPv6, still works.
        assert_eq!(find("mullvad").plain_servers().len(), 1);
        assert_eq!(Resolver::custom("192.0.2.9".parse().unwrap(), None, None, None).plain_servers(), ["192.0.2.9"]);
        let v6 = Resolver::custom("2001:db8::53".parse().unwrap(), None, None, None);
        assert_eq!(v6.only_server(0).primary(true), "2001:db8::53".parse().ok());
    }

    #[test]
    fn cogent_is_listed_with_both_servers() {
        let r = find("cogent");
        assert_eq!((r.group.as_str(), r.plain_servers()), ("public", vec!["66.28.0.45".to_string(), "66.28.0.61".to_string()]));
        assert!(r.dot.is_none() && r.doh.is_none());
    }
}
