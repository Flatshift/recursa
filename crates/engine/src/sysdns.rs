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

#[cfg(target_os = "ios")]
fn platform() -> Vec<IpAddr> {
    // libresolv's view of the configured resolvers (processes can't be spawned on iOS);
    // `res_ninit` is `res_9_ninit` on Darwin.
    use std::net::{Ipv4Addr, Ipv6Addr};
    #[link(name = "resolv")]
    unsafe extern "C" {
        fn res_9_ninit(state: *mut u8) -> libc::c_int;
        fn res_9_getservers(state: *mut u8, set: *mut [u8; 128], cnt: libc::c_int) -> libc::c_int;
        fn res_9_ndestroy(state: *mut u8);
    }
    // `struct __res_9_state` is a few hundred bytes; over-allocate and keep it 8-aligned.
    // `buf` lives to the end of the function, past `res_9_ndestroy`.
    let mut buf = vec![0u64; 1024];
    let state = buf.as_mut_ptr().cast::<u8>();
    // union res_sockaddr_union: a sockaddr_in or sockaddr_in6, 128 bytes.
    let mut set = [[0u8; 128]; 8];
    let mut ips = Vec::new();
    unsafe {
        if res_9_ninit(state) != 0 {
            return ips;
        }
        let n = res_9_getservers(state, set.as_mut_ptr(), set.len() as libc::c_int).max(0) as usize;
        for s in &set[..n.min(set.len())] {
            // sockaddr: [len, family, ..]; AF_INET has the address at 4, AF_INET6 at 8.
            match s[1] as libc::c_int {
                libc::AF_INET => {
                    let a: [u8; 4] = s[4..8].try_into().unwrap();
                    ips.push(IpAddr::V4(Ipv4Addr::from(a)));
                }
                libc::AF_INET6 => {
                    let a: [u8; 16] = s[8..24].try_into().unwrap();
                    ips.push(IpAddr::V6(Ipv6Addr::from(a)));
                }
                _ => {}
            }
        }
        res_9_ndestroy(state);
    }
    ips.retain(|ip| !ip.is_unspecified());
    ips
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", windows)))]
fn platform() -> Vec<IpAddr> {
    resolv_conf("/etc/resolv.conf")
}

#[cfg_attr(target_os = "ios", allow(dead_code))]
/// An address as configuration files print it, without an IPv6 zone ("fe80::1%en0").
fn parse_addr(s: &str) -> Option<IpAddr> {
    s.trim().split('%').next()?.parse().ok()
}

#[cfg_attr(any(windows, target_os = "ios"), allow(dead_code))]
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

#[cfg(target_os = "ios")]
fn gateway_platform() -> Option<IpAddr> {
    bsd::default_route()?.gateway.map(IpAddr::V4)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", windows)))]
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

#[cfg(target_os = "ios")]
fn link_platform() -> Option<(String, bool)> {
    // iOS names its links by kind: en0 is the Wi-Fi radio, pdp_ip* the cellular bearers, utun* tunnels.
    // Other en* links are wired (a USB or Lightning adapter) or the iPad's tethering bridge.
    let iface = bsd::default_route()?.interface?;
    let wifi = iface == "en0";
    Some((iface, wifi))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "ios", windows)))]
fn link_platform() -> Option<(String, bool)> {
    None
}

/// The kernel's IPv4 routing table through `sysctl`, as `route -n get default` would show it: no
/// process to spawn, which iOS does not allow. Used on iOS; on macOS the test below checks it
/// against `route`.
#[cfg(any(target_os = "ios", all(test, target_os = "macos")))]
mod bsd {
    use std::net::Ipv4Addr;

    // libc leaves net/route.h out of its iOS build (the iOS SDK does not ship the header), so the
    // record layout and bits are spelled out here; the macOS test checks them against libc.
    const HDR: usize = 92; // sizeof(struct rt_msghdr): 36 bytes of fields, then 14 u32 of rt_metrics
    const RTF_UP: i32 = 0x1;
    const RTF_GATEWAY: i32 = 0x2;
    const RTF_IFSCOPE: i32 = 0x1000000;
    const RTA_DST: i32 = 0x1;
    const RTA_GATEWAY: i32 = 0x2;
    const RTA_NETMASK: i32 = 0x4;

    #[derive(Debug, PartialEq)]
    pub struct Route {
        /// None for a link route, such as a VPN tunnel that carries the default itself.
        pub gateway: Option<Ipv4Addr>,
        pub interface: Option<String>,
    }

    /// The default route: 0.0.0.0/0, not scoped to one interface.
    pub fn default_route() -> Option<Route> {
        let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, libc::AF_INET, libc::NET_RT_DUMP, 0];
        let mut len = 0usize;
        // SAFETY: a NULL buffer asks for the size; the second call fills a buffer of that size.
        unsafe {
            if libc::sysctl(mib.as_mut_ptr(), mib.len() as u32, std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 {
                return None;
            }
        }
        let mut buf = vec![0u8; len];
        unsafe {
            if libc::sysctl(mib.as_mut_ptr(), mib.len() as u32, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) != 0 {
                return None;
            }
        }
        parse(&buf[..len]).map(|(gateway, index)| Route { gateway, interface: name(index) })
    }

    /// Walks the `rt_msghdr` records for the first unscoped, up route to 0.0.0.0/0, the one the kernel
    /// picks: through a gateway, or a link route (a tunnel) with none. Split defaults (0/1 and 128/1)
    /// aren't 0/0.
    pub fn parse(buf: &[u8]) -> Option<(Option<Ipv4Addr>, u16)> {
        let u16_at = |b: &[u8], i: usize| u16::from_ne_bytes([b[i], b[i + 1]]);
        let i32_at = |b: &[u8], i: usize| i32::from_ne_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let mut off = 0;
        'records: while off + HDR <= buf.len() {
            let m = &buf[off..off + HDR];
            // rt_msghdr: rtm_msglen u16 @0, rtm_index u16 @4, rtm_flags i32 @8, rtm_addrs i32 @12.
            let (msglen, index, flags, addrs) = (u16_at(m, 0) as usize, u16_at(m, 4), i32_at(m, 8), i32_at(m, 12));
            if msglen < HDR || off + msglen > buf.len() {
                break;
            }
            let rec = &buf[off + HDR..off + msglen];
            off += msglen;
            if flags & RTF_UP == 0 || flags & RTF_IFSCOPE != 0 || addrs & RTA_DST == 0 {
                continue;
            }
            // The sockaddrs follow the header in RTA_* bit order, each padded to 4 bytes. A netmask
            // may be cut short after its last nonzero byte, or have no length at all for /0.
            let mut p = 0;
            let (mut dst, mut gw, mut zero_mask) = (None, None, true);
            for bit in 0..32 {
                if addrs & (1 << bit) == 0 {
                    continue;
                }
                let Some(sa) = rec.get(p..).filter(|sa| !sa.is_empty()) else { continue 'records };
                let sa_len = sa[0] as usize;
                let body = sa.get(..sa_len.min(sa.len())).unwrap_or_default();
                let ip = (sa.get(1).copied() == Some(libc::AF_INET as u8))
                    .then(|| body.get(4..8).map(|b| Ipv4Addr::new(b[0], b[1], b[2], b[3])))
                    .flatten();
                match 1 << bit {
                    RTA_DST => dst = ip,
                    RTA_GATEWAY => gw = ip,
                    RTA_NETMASK => zero_mask = body.iter().skip(4).all(|&b| b == 0),
                    _ => {}
                }
                p += if sa_len == 0 { 4 } else { sa_len.div_ceil(4) * 4 };
            }
            if dst != Some(Ipv4Addr::UNSPECIFIED) || !zero_mask {
                continue;
            }
            return Some((gw.filter(|_| flags & RTF_GATEWAY != 0), index));
        }
        None
    }

    fn name(index: u16) -> Option<String> {
        let mut buf = [0u8; libc::IFNAMSIZ];
        // SAFETY: if_indextoname writes at most IFNAMSIZ bytes, NUL-terminated.
        if unsafe { libc::if_indextoname(index as u32, buf.as_mut_ptr().cast()) }.is_null() {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Some(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    #[cfg(all(test, target_os = "macos"))]
    mod tests {
        use super::*;

        #[test]
        fn routing_table_agrees_with_route() {
            let out = std::process::Command::new("/sbin/route").args(["-n", "get", "default"]).output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            let gw = text.lines().find_map(|l| l.trim().strip_prefix("gateway:")).map(|s| s.trim().parse::<Ipv4Addr>().ok());
            let iface = text.lines().find_map(|l| l.trim().strip_prefix("interface:")).map(|s| s.trim().to_string());
            let Some(Some(gw)) = gw else { return }; // offline, or a non-IPv4 default: nothing to compare
            assert_eq!(default_route(), Some(Route { gateway: Some(gw), interface: iface }));
        }

        #[test]
        fn layout_and_bits_match_libc() {
            assert_eq!(HDR, std::mem::size_of::<libc::rt_msghdr>());
            assert_eq!(std::mem::offset_of!(libc::rt_msghdr, rtm_index), 4);
            assert_eq!(std::mem::offset_of!(libc::rt_msghdr, rtm_flags), 8);
            assert_eq!(std::mem::offset_of!(libc::rt_msghdr, rtm_addrs), 12);
            assert_eq!((RTF_UP, RTF_GATEWAY, RTF_IFSCOPE), (libc::RTF_UP, libc::RTF_GATEWAY, libc::RTF_IFSCOPE));
            assert_eq!((RTA_DST, RTA_GATEWAY, RTA_NETMASK), (libc::RTA_DST, libc::RTA_GATEWAY, libc::RTA_NETMASK));
        }

        #[test]
        fn short_or_truncated_records_stop_the_walk() {
            assert_eq!(parse(&[]), None);
            assert_eq!(parse(&[1u8; 7]), None);
            let mut rec = vec![0u8; HDR];
            rec[0] = 200; // claims more than is there
            assert_eq!(parse(&rec), None);
        }

        fn sin(ip: [u8; 4]) -> Vec<u8> {
            let mut sa = vec![16, libc::AF_INET as u8, 0, 0];
            sa.extend(ip);
            sa.extend([0; 8]);
            sa
        }

        fn record(index: u16, flags: i32, addrs: i32, sockaddrs: &[Vec<u8>]) -> Vec<u8> {
            let mut rec = vec![0u8; HDR];
            rec.extend(sockaddrs.concat());
            let len = rec.len() as u16;
            rec[0..2].copy_from_slice(&len.to_ne_bytes());
            rec[4..6].copy_from_slice(&index.to_ne_bytes());
            rec[8..12].copy_from_slice(&flags.to_ne_bytes());
            rec[12..16].copy_from_slice(&addrs.to_ne_bytes());
            rec
        }

        #[test]
        fn takes_the_first_default_and_skips_bad_or_split_routes() {
            let all = RTA_DST | RTA_GATEWAY | RTA_NETMASK;
            let link = vec![20, libc::AF_LINK as u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            let zero_mask = vec![0, 0, 0, 0];
            let half_mask = vec![5, 0, 0, 0, 0x80, 0, 0, 0];
            let split = record(9, RTF_UP, all, &[sin([0; 4]), link.clone(), half_mask]);
            let truncated = record(8, RTF_UP | RTF_GATEWAY, all, &[sin([0; 4])]);
            let tunnel = record(7, RTF_UP, all, &[sin([0; 4]), link, zero_mask.clone()]);
            let wifi = record(4, RTF_UP | RTF_GATEWAY, all, &[sin([0; 4]), sin([192, 168, 1, 1]), zero_mask]);
            assert_eq!(parse(&[split.clone(), truncated.clone(), tunnel.clone(), wifi.clone()].concat()), Some((None, 7)));
            assert_eq!(parse(&[split, truncated, wifi, tunnel].concat()), Some((Some(Ipv4Addr::new(192, 168, 1, 1)), 4)));
        }
    }
}

/// Whether the adapter that Internet traffic leaves through delivers the receive stamps a socket asks for.
/// Windows stamps a packet where the adapter hands it up, and only when the driver's software timestamping
/// is on (its "Software Timestamp" advanced property, off by default); macOS and Linux stamp every receive
/// in the kernel.
#[cfg(windows)]
pub fn adapter_rx_stamps() -> bool {
    win::best().is_some_and(|a| a.rx_stamps)
}

#[cfg(not(windows))]
pub fn adapter_rx_stamps() -> bool {
    true
}

/// The IP Helper API's view of the adapters: the one a packet to the Internet leaves through,
/// its DNS servers, its gateway and whether it is Wi-Fi. No console command, so nothing flashes
/// a window from the desktop app, and nothing depends on the system language.
#[cfg(windows)]
mod win {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, GetBestInterfaceEx,
        INTERFACE_TIMESTAMP_CAPABILITIES, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::{IfOperStatusUp, NET_LUID_LH};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    /// IANA ifType for an IEEE 802.11 interface (the `IfType` of a Wi-Fi adapter).
    const IF_TYPE_IEEE80211: u32 = 71;

    pub struct Adapter {
        pub dns: Vec<IpAddr>,
        pub gateway: Option<IpAddr>,
        pub name: String,
        pub wifi: bool,
        /// The driver stamps every packet it receives (software timestamping, RxAll).
        pub rx_stamps: bool,
    }

    type GetActiveCaps = unsafe extern "system" fn(*const NET_LUID_LH, *mut INTERFACE_TIMESTAMP_CAPABILITIES) -> u32;

    /// `GetInterfaceActiveTimestampCapabilities` arrived with Windows 10 2004. Looked up at run time so the
    /// binary still loads on older systems, where it is simply absent and no adapter stamps.
    fn active_caps() -> Option<GetActiveCaps> {
        static F: OnceLock<Option<GetActiveCaps>> = OnceLock::new();
        *F.get_or_init(|| {
            let lib: Vec<u16> = "iphlpapi.dll".encode_utf16().chain([0]).collect();
            // Already loaded: this module imports GetAdaptersAddresses from it.
            let module = unsafe { GetModuleHandleW(lib.as_ptr()) };
            if module.is_null() {
                return None;
            }
            let f = unsafe { GetProcAddress(module, c"GetInterfaceActiveTimestampCapabilities".as_ptr().cast()) }?;
            Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, GetActiveCaps>(f) })
        })
    }

    fn rx_stamps(a: &IP_ADAPTER_ADDRESSES_LH) -> bool {
        let Some(get) = active_caps() else { return false };
        let mut caps: INTERFACE_TIMESTAMP_CAPABILITIES = unsafe { std::mem::zeroed() };
        let r = unsafe { get(&a.Luid, &mut caps) };
        r == ERROR_SUCCESS && caps.SoftwareCapabilities.AllReceive
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
            // Link-local servers lose their zone here (`fe80::1%12` would go out unscoped), and adapters
            // with no IPv6 DNS list the fec0:0:0:ffff::1-3 placeholders; neither is reachable.
            if let Some(ip) = ip_of(s.Address.lpSockaddr).filter(|ip| !placeholder_v6(ip)) {
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
        Adapter { dns, gateway, name: wide(a.FriendlyName), wifi: a.IfType == IF_TYPE_IEEE80211, rx_stamps: rx_stamps(a) }
    }

    fn placeholder_v6(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V6(v6) => matches!(v6.segments()[0] & 0xffc0, 0xfe80 | 0xfec0),
            IpAddr::V4(_) => false,
        }
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
