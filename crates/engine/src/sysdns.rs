//! Discover the resolvers this machine is configured to use.

use std::net::IpAddr;

pub fn discover() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    for ip in platform() {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out.truncate(4);
    out
}

#[cfg(target_os = "macos")]
fn platform() -> Vec<IpAddr> {
    // `scutil --dns` lists the effective resolver configuration; resolver #1 is the
    // default (unscoped) one. /etc/resolv.conf is a compatibility copy.
    let mut ips = Vec::new();
    if let Ok(o) = std::process::Command::new("/usr/sbin/scutil").arg("--dns").output() {
        let text = String::from_utf8_lossy(&o.stdout);
        let mut in_default = false;
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with("resolver #") {
                in_default = l == "resolver #1";
                continue;
            }
            // "nameserver[0] : 2001:db8::53": IPv6 addresses carry colons of their own.
            if in_default
                && l.starts_with("nameserver[")
                && let Some(v) = l.split_once(':').and_then(|(_, v)| parse_addr(v))
            {
                ips.push(v);
            }
            if l.starts_with("DNS configuration (for scoped queries)") {
                break;
            }
        }
    }
    if ips.is_empty() {
        ips = resolv_conf("/etc/resolv.conf");
    }
    ips
}

#[cfg(target_os = "linux")]
fn platform() -> Vec<IpAddr> {
    let ips = resolv_conf("/etc/resolv.conf");
    // systemd-resolved stub: read its upstreams instead of 127.0.0.53.
    if ips.iter().all(|ip| ip.is_loopback()) {
        let upstream = resolv_conf("/run/systemd/resolve/resolv.conf");
        if !upstream.is_empty() {
            return upstream;
        }
    }
    ips
}

#[cfg(windows)]
fn platform() -> Vec<IpAddr> {
    win::best().map(|a| a.dns).unwrap_or_default()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn platform() -> Vec<IpAddr> {
    resolv_conf("/etc/resolv.conf")
}

/// An address as configuration files print it, without an IPv6 zone ("fe80::1%en0").
fn parse_addr(s: &str) -> Option<IpAddr> {
    s.trim().split('%').next()?.parse().ok()
}

#[cfg_attr(windows, allow(dead_code))]
fn resolv_conf(path: &str) -> Vec<IpAddr> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some("nameserver")).then(|| it.next()).flatten()
        })
        .filter_map(parse_addr)
        .collect()
}

/// The interface traffic to the Internet leaves through, and whether it is Wi-Fi.
/// Interception features on access points (UniFi DNS Assistance, for example) apply
/// per SSID, so knowing the link helps explain a result.
pub fn link() -> Option<serde_json::Value> {
    let (iface, wifi) = link_platform()?;
    Some(serde_json::json!({ "interface": iface, "wifi": wifi }))
}

/// This computer's default gateway, the router its Internet traffic leaves through.
pub fn default_gateway() -> Option<IpAddr> {
    gateway_platform()
}

#[cfg(target_os = "macos")]
fn gateway_platform() -> Option<IpAddr> {
    let out = std::process::Command::new("/sbin/route").args(["-n", "get", "default"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().find_map(|l| l.trim().strip_prefix("gateway:")).and_then(parse_addr)
}

#[cfg(target_os = "linux")]
fn gateway_platform() -> Option<IpAddr> {
    let out = std::process::Command::new("ip").args(["route", "show", "default"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut words = text.split_whitespace();
    words.by_ref().skip_while(|w| *w != "via").nth(1).and_then(parse_addr)
}

#[cfg(windows)]
fn gateway_platform() -> Option<IpAddr> {
    win::best()?.gateway
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn gateway_platform() -> Option<IpAddr> {
    None
}

#[cfg(target_os = "macos")]
fn link_platform() -> Option<(String, bool)> {
    let out = std::process::Command::new("/sbin/route").args(["-n", "get", "1.1.1.1"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let iface = text.lines().find_map(|l| l.trim().strip_prefix("interface:")).map(|s| s.trim().to_string())?;
    let ports = std::process::Command::new("/usr/sbin/networksetup").arg("-listallhardwareports").output().ok()?;
    let ports = String::from_utf8_lossy(&ports.stdout);
    // Blocks of "Hardware Port: Wi-Fi" followed by "Device: en0".
    let wifi = ports.split("\n\n").any(|b| b.contains("Hardware Port: Wi-Fi") && b.lines().any(|l| l.trim() == format!("Device: {iface}")));
    Some((iface, wifi))
}

#[cfg(target_os = "linux")]
fn link_platform() -> Option<(String, bool)> {
    let out = std::process::Command::new("ip").args(["route", "get", "1.1.1.1"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut words = text.split_whitespace();
    let iface = words.by_ref().skip_while(|w| *w != "dev").nth(1)?.to_string();
    let wifi = std::path::Path::new(&format!("/sys/class/net/{iface}/wireless")).exists();
    Some((iface, wifi))
}

#[cfg(windows)]
fn link_platform() -> Option<(String, bool)> {
    let a = win::best()?;
    Some((a.name, a.wifi))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn link_platform() -> Option<(String, bool)> {
    None
}

/// The IP Helper API's view of the adapters: the one a packet to the Internet leaves through,
/// its DNS servers, its gateway and whether it is Wi-Fi. No console command, so nothing flashes
/// a window from the desktop app, and nothing depends on the system language.
#[cfg(windows)]
mod win {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, GetBestInterfaceEx,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6};

    /// IANA ifType for an IEEE 802.11 interface (the `IfType` of a Wi-Fi adapter).
    const IF_TYPE_IEEE80211: u32 = 71;

    pub struct Adapter {
        pub dns: Vec<IpAddr>,
        pub gateway: Option<IpAddr>,
        pub name: String,
        pub wifi: bool,
    }

    /// The adapter a packet to 1.1.1.1 leaves through; failing that, the first adapter that is up
    /// and has a gateway.
    pub fn best() -> Option<Adapter> {
        let mut dest: SOCKADDR_IN = unsafe { std::mem::zeroed() };
        dest.sin_family = AF_INET;
        dest.sin_addr.S_un.S_addr = u32::from(Ipv4Addr::new(1, 1, 1, 1)).to_be();
        let mut index: u32 = 0;
        let routed = unsafe { GetBestInterfaceEx((&raw const dest).cast::<SOCKADDR>(), &mut index) } == ERROR_SUCCESS;
        let buf = adapters()?;
        let mut fallback = None;
        let mut p = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !p.is_null() {
            let a = unsafe { &*p };
            if a.OperStatus == IfOperStatusUp {
                if routed && unsafe { a.Anonymous1.Anonymous.IfIndex } == index {
                    return Some(describe(a));
                }
                if fallback.is_none() && !a.FirstGatewayAddress.is_null() {
                    fallback = Some(describe(a));
                }
            }
            p = a.Next;
        }
        fallback
    }

    /// The adapter list in a buffer of its own (its entries point into it), grown as the API asks.
    fn adapters() -> Option<Vec<u64>> {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_INCLUDE_GATEWAYS;
        let mut size: u32 = 16 * 1024;
        for _ in 0..4 {
            let mut buf = vec![0u64; size as usize / 8 + 1];
            let r = unsafe {
                GetAdaptersAddresses(
                    AF_UNSPEC as u32,
                    flags,
                    std::ptr::null(),
                    buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            match r {
                ERROR_SUCCESS => return Some(buf),
                ERROR_BUFFER_OVERFLOW => continue,
                _ => return None,
            }
        }
        None
    }

    fn describe(a: &IP_ADAPTER_ADDRESSES_LH) -> Adapter {
        let mut dns = Vec::new();
        let mut d = a.FirstDnsServerAddress;
        while !d.is_null() {
            let s = unsafe { &*d };
            if let Some(ip) = ip_of(s.Address.lpSockaddr) {
                dns.push(ip);
            }
            d = s.Next;
        }
        let mut gateways = Vec::new();
        let mut g = a.FirstGatewayAddress;
        while !g.is_null() {
            let s = unsafe { &*g };
            gateways.extend(ip_of(s.Address.lpSockaddr));
            g = s.Next;
        }
        // The link check pings over IPv4, so an IPv4 gateway comes first.
        let gateway = gateways.iter().find(|ip| ip.is_ipv4()).or(gateways.first()).copied();
        Adapter { dns, gateway, name: wide(a.FriendlyName), wifi: a.IfType == IF_TYPE_IEEE80211 }
    }

    fn ip_of(p: *const SOCKADDR) -> Option<IpAddr> {
        if p.is_null() {
            return None;
        }
        match unsafe { (*p).sa_family } {
            AF_INET => {
                let v4 = unsafe { &*p.cast::<SOCKADDR_IN>() };
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(unsafe { v4.sin_addr.S_un.S_addr }))))
            }
            AF_INET6 => {
                let v6 = unsafe { &*p.cast::<SOCKADDR_IN6>() };
                Some(IpAddr::V6(Ipv6Addr::from(unsafe { v6.sin6_addr.u.Byte })))
            }
            _ => None,
        }
    }

    fn wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_with_colons_and_zones() {
        let line = "nameserver[1] : 2001:db8::53";
        assert_eq!(line.split_once(':').and_then(|(_, v)| parse_addr(v)), "2001:db8::53".parse().ok());
        assert_eq!(parse_addr(" fe80::1%en0 "), "fe80::1".parse().ok());
        assert_eq!(parse_addr("192.168.1.53"), "192.168.1.53".parse().ok());
    }
}
