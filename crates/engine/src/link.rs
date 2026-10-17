//! Link check: how the first hop behaved while a run measured.
//!
//! A sidecar thread sends one ICMP echo to the default gateway every 100 ms and times each reply with the kernel's
//! arrival stamp, the way DNS answers are timed. A noisy Wi-Fi link (a channel scan, AWDL going off-channel) delays
//! every query by the same stalls, so without this a bad link reads as a slow resolver. The pinger runs at normal
//! priority, apart from the measuring threads, and only from the start of warm-up to the end of the measured phases.
//! It uses an unprivileged ICMP datagram socket (macOS allows it without root; Linux needs `ping_group_range`). IPv4
//! only. `RECURSA_NO_LINK=1` turns it off. On Windows the echoes go through `IcmpSendEcho2` (iphlpapi, no privilege
//! needed) and are timed in userspace around the event that completes each one; the reply's own `RoundTripTime` is
//! whole milliseconds, so it is not used.
//!
//! When the link was noisy the same thread also says why, as far as the Mac can tell. Each tick it echoes 127.0.0.1 as
//! well (a slow loopback means the Mac itself is busy) and notes how late the tick was; once a second it reads the
//! gateway interface's and awdl0's byte, error and drop counters (macOS). [`classify`] reads those around the stalls.

use serde_json::{Value, json};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

/// Echo interval.
#[cfg(any(unix, windows))]
const INTERVAL_MS: u64 = 100;
/// An echo with no reply after this long is lost.
#[cfg(any(unix, windows))]
const LOSS_NS: u64 = 1_000_000_000;
/// The `ping` default payload.
#[cfg(any(unix, windows))]
const PAYLOAD: usize = 56;
/// A reply slower than this is a stall (or 5 x the typical reply, if that is larger).
const STALL_FLOOR_US: u32 = 20_000;
const STALL_FACTOR: u32 = 5;
/// The link is noisy when more than this share of echoes stall...
const NOISY_STALL_SHARE: f64 = 0.02;
/// ...or its 95th percentile is over this. (web/src/components/InstrumentStrip.svelte mirrors both.)
const NOISY_P95_US: u32 = 20_000;
/// Cause rules, applied to the one-second host rows that contain a stall. Loopback echo or tick lag over these: the Mac is busy.
const BUSY_LOOPBACK_US: u32 = 5_000;
const BUSY_LAG_US: u32 = 20_000;
/// en0 receive plus transmit over this: other traffic on the Mac.
const TRAFFIC_BPS: u64 = 5_000_000;
/// awdl0 receive plus transmit over this: AirDrop or Continuity.
const AWDL_BPS: u64 = 100_000;
/// Ticks per host row (1 Hz).
#[cfg(any(unix, windows))]
const TICKS_PER_ROW: usize = 10;

/// What the gateway's echoes looked like over a run.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkSummary {
    pub sent: u32,
    pub lost: u32,
    pub p50_us: u32,
    pub p95_us: u32,
    pub max_us: u32,
    pub stalls: u32,
    /// A reply slower than this, or lost, is a stall.
    pub limit_us: u32,
}

impl LinkSummary {
    /// Stalls above 2% of the echoes, or a p95 over 20 ms.
    pub fn noisy(&self) -> bool {
        self.stalls as f64 > NOISY_STALL_SHARE * self.sent as f64 || self.p95_us > NOISY_P95_US
    }

    /// Echoes that got an answer.
    pub fn replies(&self) -> u32 {
        self.sent - self.lost
    }
}

/// Summarise `(at_ms, us)` echoes, `us` being 0 for a lost one. Percentiles are over the replies; a stall is a reply
/// slower than max(20 ms, 5 x p50), or a lost echo.
pub fn summarize(samples: &[(u32, u32)]) -> LinkSummary {
    let mut us: Vec<u32> = samples.iter().map(|s| s.1).filter(|&u| u > 0).collect();
    us.sort_unstable();
    let pct = |p: f64| -> u32 {
        if us.is_empty() {
            return 0;
        }
        let rank = (p * us.len() as f64).ceil() as usize;
        us[rank.clamp(1, us.len()) - 1]
    };
    let p50 = pct(0.5);
    let limit = STALL_FLOOR_US.max(p50.saturating_mul(STALL_FACTOR));
    let sent = samples.len() as u32;
    let lost = sent - us.len() as u32;
    let slow = us.iter().filter(|&&u| u > limit).count() as u32;
    LinkSummary {
        sent,
        lost,
        p50_us: p50,
        p95_us: pct(0.95),
        max_us: us.last().copied().unwrap_or(0),
        stalls: lost + slow,
        limit_us: limit,
    }
}

/// One second of host signals. `None` is a signal this platform or run could not read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostRow {
    /// Milliseconds into the run, at the first tick of the second.
    pub at_ms: u32,
    /// Slowest loopback echo of the second, a lost one counting as the loss timeout.
    pub lo_us: Option<u32>,
    /// Latest a tick was sent, against its schedule.
    pub lag_us: u32,
    pub rx_bps: Option<u64>,
    pub tx_bps: Option<u64>,
    /// Errors plus drops on the gateway interface during the second.
    pub err: Option<u32>,
    /// awdl0 receive plus transmit; `None` when it is absent or down.
    pub awdl_bps: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostRun {
    pub rows: Vec<HostRow>,
}

/// The samples one pinger collected.
pub struct LinkRun {
    pub gateway: IpAddr,
    pub samples: Vec<(u32, u32)>,
    pub host: HostRun,
}

/// What most likely stalled the link, with the worst figure seen during the stalls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cause {
    Busy { lo_us: u32, lag_us: u32 },
    Traffic { bps: u64 },
    Awdl { bps: u64 },
    Driver,
    Unknown,
}

impl Cause {
    pub fn name(&self) -> &'static str {
        match self {
            Cause::Busy { .. } => "busy",
            Cause::Traffic { .. } => "traffic",
            Cause::Awdl { .. } => "awdl",
            Cause::Driver => "driver",
            Cause::Unknown => "unknown",
        }
    }
}

/// Name the cause of a noisy link: the first rule that matches the host rows holding a stalled echo.
pub fn classify(sum: &LinkSummary, samples: &[(u32, u32)], host: &[HostRow]) -> Cause {
    let mut stalled = vec![false; host.len()];
    for &(at, us) in samples {
        if us == 0 || us > sum.limit_us {
            let row = host.partition_point(|r| r.at_ms <= at);
            if row > 0 {
                stalled[row - 1] = true;
            }
        }
    }
    let rows = || host.iter().zip(&stalled).filter(|(_, s)| **s).map(|(r, _)| r);
    let lo_us = rows().filter_map(|r| r.lo_us).max().unwrap_or(0);
    let lag_us = rows().map(|r| r.lag_us).max().unwrap_or(0);
    if lo_us > BUSY_LOOPBACK_US || lag_us > BUSY_LAG_US {
        return Cause::Busy { lo_us, lag_us };
    }
    let bps = rows().map(|r| r.rx_bps.unwrap_or(0) + r.tx_bps.unwrap_or(0)).max().unwrap_or(0);
    if bps > TRAFFIC_BPS {
        return Cause::Traffic { bps };
    }
    let bps = rows().filter_map(|r| r.awdl_bps).max().unwrap_or(0);
    if bps > AWDL_BPS {
        return Cause::Awdl { bps };
    }
    if rows().any(|r| r.err.unwrap_or(0) > 0) {
        return Cause::Driver;
    }
    Cause::Unknown
}

/// The gateway echoes `(at_ms, us)` and the host rows a pinger collected.
type Pinged = (Vec<(u32, u32)>, HostRun);

/// A running pinger. Dropping it stops the thread.
pub struct LinkProbe {
    gateway: IpAddr,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<Pinged>>,
}

impl Drop for LinkProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl LinkProbe {
    /// Stop pinging and collect the echoes. Waits for outstanding ones to be answered or time out, at most a second.
    pub fn stop(mut self) -> LinkRun {
        self.stop.store(true, Ordering::Relaxed);
        let (samples, host) = self.join.take().and_then(|j| j.join().ok()).unwrap_or_default();
        LinkRun { gateway: self.gateway, samples, host }
    }
}

/// `RECURSA_NO_LINK=1` switches the link check off entirely (for comparing runs with and without it).
pub fn disabled() -> bool {
    std::env::var_os("RECURSA_NO_LINK").is_some_and(|v| v == "1")
}

/// Start pinging the default gateway. `at_ms` in the samples counts from `t0`, like `Sample.at`. The error is the
/// reason there is no link check (shown to the user).
pub fn start(t0: Instant) -> Result<LinkProbe, String> {
    let gateway = crate::sysdns::default_gateway().ok_or("No default gateway found.")?;
    let IpAddr::V4(gw) = gateway else { return Err("The default gateway is not an IPv4 address.".into()) };
    #[cfg(any(unix, windows))]
    {
        // The interface the gateway is on, for its counters (macOS).
        let iface =
            if cfg!(target_os = "macos") { crate::sysdns::link().and_then(|l| l["interface"].as_str().map(str::to_string)) } else { None };
        let sock = imp::open(gw, iface)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let join = std::thread::Builder::new()
            .name("recursa-link".into())
            .spawn(move || imp::sample(sock, t0, &flag))
            .map_err(|e| format!("Could not start the link check ({e})."))?;
        Ok(LinkProbe { gateway, stop, join: Some(join) })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (t0, gw);
        Err("The link check is not available on this platform.".into())
    }
}

/// The run-body value for a finished pinger, and the warning to show when the link was noisy.
pub fn report(r: Result<LinkRun, String>) -> (Value, Option<String>) {
    let unavailable = |reason: String| (json!({"state": "unavailable", "reason": reason}), None);
    let run = match r {
        Ok(run) => run,
        Err(reason) => return unavailable(reason),
    };
    let sum = summarize(&run.samples);
    if sum.replies() == 0 {
        return unavailable(format!("{} did not answer ping.", run.gateway));
    }
    let ms = |us: u32| us as f64 / 1000.0;
    let mut value = json!({
        "gateway": run.gateway.to_string(),
        "sent": sum.sent, "lost": sum.lost,
        "p50": ms(sum.p50_us), "p95": ms(sum.p95_us), "max": ms(sum.max_us),
        "stalls": sum.stalls,
        "samples": run.samples.iter().map(|&(at, us)| [at, us]).collect::<Vec<_>>(),
    });
    let cause = sum.noisy().then(|| classify(&sum, &run.samples, &run.host.rows));
    value["host"] = json!({
        "samples": run.host.rows.iter().map(|r| json!([r.at_ms, r.lo_us, r.lag_us, r.rx_bps, r.tx_bps, r.err, r.awdl_bps])).collect::<Vec<_>>(),
        "cause": cause.map(|c| c.name()),
    });
    let note = cause.map(|c| note_text(&sum, c));
    (value, note)
}

fn fmt_ms(us: u32) -> String {
    if us < 10_000 { format!("{:.1}", us as f64 / 1000.0) } else { format!("{}", (us as f64 / 1000.0).round() as u32) }
}

fn fmt_bps(bps: u64) -> String {
    match bps {
        b if b >= 10_000_000 => format!("{} Mbps", (b as f64 / 1e6).round() as u64),
        b if b >= 1_000_000 => format!("{:.1} Mbps", b as f64 / 1e6),
        b => format!("{} kbps", (b as f64 / 1e3).round() as u64),
    }
}

/// What the user calls the machine in the cause texts.
const MACHINE: &str = if cfg!(target_os = "macos") { "Mac" } else { "computer" };

fn cause_text(c: Cause) -> String {
    match c {
        Cause::Busy { lo_us, .. } if lo_us > BUSY_LOOPBACK_US => {
            format!("Likely cause: your {MACHINE} was busy (even pings to itself took up to {} ms).", fmt_ms(lo_us))
        }
        Cause::Busy { lag_us, .. } => {
            format!("Likely cause: your {MACHINE} was busy (the link check itself ran up to {} ms late).", fmt_ms(lag_us))
        }
        Cause::Traffic { bps } => format!("Likely cause: other traffic on this {MACHINE} (up to {}).", fmt_bps(bps)),
        Cause::Awdl { bps } => {
            format!("Likely cause: AirDrop or another Apple device feature using the Wi-Fi radio (up to {}).", fmt_bps(bps))
        }
        Cause::Driver => "Likely cause: the network driver reported errors or dropped packets during the stalls.".into(),
        Cause::Unknown => {
            format!(
                "Your {MACHINE}'s Wi-Fi link stalled with nothing else to blame; turning Wi-Fi off and on, or restarting, usually clears this."
            )
        }
    }
}

fn note_text(s: &LinkSummary, cause: Cause) -> String {
    let unanswered = match s.lost {
        0 => String::new(),
        n => format!(", and {n} of {} went unanswered", s.sent),
    };
    format!(
        "Your network was slow during this run: replies from your router took up to {} ms (typically {} ms){unanswered}. Speed scores include those delays, so treat them as inflated, and run again on a quieter link. {}",
        fmt_ms(s.max_us),
        fmt_ms(s.p50_us),
        cause_text(cause)
    )
}

#[cfg(unix)]
mod imp {
    use super::*;
    use crate::{clock, net};
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    pub(super) struct Pinger {
        sock: Socket,
        dest: SockAddr,
        /// Tags our payloads, so a reply is ours whatever the kernel did to the ICMP id.
        nonce: [u8; 8],
        stamped: bool,
        /// The loopback echo; the check goes on without it if it cannot be opened.
        lo: Option<Loopback>,
        /// The gateway's interface, for its counters.
        iface: Option<String>,
    }

    struct Loopback {
        sock: Socket,
        dest: SockAddr,
        stamped: bool,
    }

    pub(super) fn open(gw: Ipv4Addr, iface: Option<String>) -> Result<Pinger, String> {
        let sock =
            Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)).map_err(|e| format!("Could not open an ICMP socket ({e})."))?;
        let stamped = net::enable_rx_timestamps_fd(sock.as_raw_fd());
        let dest = SockAddr::from(SocketAddrV4::new(gw, 0));
        let lo = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::ICMPV4)).ok().and_then(|sock| {
            sock.set_nonblocking(true).ok()?;
            let stamped = net::enable_rx_timestamps_fd(sock.as_raw_fd());
            Some(Loopback { sock, dest: SockAddr::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)), stamped })
        });
        Ok(Pinger { sock, dest, nonce: fastrand::u64(..).to_le_bytes(), stamped, lo, iface })
    }

    /// One interface's `if_data` counters (32-bit on macOS, so they wrap).
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) struct IfCounters {
        pub ibytes: u32,
        pub obytes: u32,
        pub ierrors: u32,
        pub oerrors: u32,
        pub iqdrops: u32,
        pub up: bool,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub(super) struct Counters {
        pub en: Option<IfCounters>,
        pub awdl: Option<IfCounters>,
    }

    /// Counters of the gateway interface and of awdl0.
    #[cfg(target_os = "macos")]
    fn read_counters(en: Option<&str>) -> Counters {
        use std::ffi::CStr;
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        let mut out = Counters::default();
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return out;
        }
        let mut p = head;
        while !p.is_null() {
            let a = unsafe { &*p };
            p = a.ifa_next;
            if a.ifa_addr.is_null() || a.ifa_data.is_null() || unsafe { (*a.ifa_addr).sa_family } as libc::c_int != libc::AF_LINK {
                continue;
            }
            let name = unsafe { CStr::from_ptr(a.ifa_name) }.to_bytes();
            let slot = if Some(name) == en.map(str::as_bytes) {
                &mut out.en
            } else if name == b"awdl0" {
                &mut out.awdl
            } else {
                continue;
            };
            let d = unsafe { &*(a.ifa_data as *const libc::if_data) };
            *slot = Some(IfCounters {
                ibytes: d.ifi_ibytes,
                obytes: d.ifi_obytes,
                ierrors: d.ifi_ierrors,
                oerrors: d.ifi_oerrors,
                iqdrops: d.ifi_iqdrops,
                up: a.ifa_flags & libc::IFF_UP as libc::c_uint != 0,
            });
        }
        unsafe { libc::freeifaddrs(head) };
        out
    }

    #[cfg(not(target_os = "macos"))]
    fn read_counters(_en: Option<&str>) -> Counters {
        Counters::default()
    }

    /// The host signals of a stretch of `secs` between two counter reads: en0 rx and tx bits per second, its errors and
    /// drops, and awdl0 bits per second (`None` unless it was up at both ends).
    pub(super) fn rates(a: &Counters, b: &Counters, secs: f64) -> (Option<u64>, Option<u64>, Option<u32>, Option<u64>) {
        let bps = |bytes: u32| (bytes as f64 * 8.0 / secs).round() as u64;
        if secs < 0.1 {
            return (None, None, None, None);
        }
        let en = a.en.zip(b.en);
        let rx = en.map(|(a, b)| bps(b.ibytes.wrapping_sub(a.ibytes)));
        let tx = en.map(|(a, b)| bps(b.obytes.wrapping_sub(a.obytes)));
        let err = en.map(|(a, b)| {
            b.ierrors
                .wrapping_sub(a.ierrors)
                .saturating_add(b.oerrors.wrapping_sub(a.oerrors))
                .saturating_add(b.iqdrops.wrapping_sub(a.iqdrops))
        });
        let awdl = a
            .awdl
            .zip(b.awdl)
            .filter(|(a, b)| a.up && b.up)
            .map(|(a, b)| bps(b.ibytes.wrapping_sub(a.ibytes)) + bps(b.obytes.wrapping_sub(a.obytes)));
        (rx, tx, err, awdl)
    }

    fn checksum(b: &[u8]) -> u16 {
        let mut sum: u32 = b.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32).sum();
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    /// An echo request: type 8, the id, the sequence number, then the nonce and a fill pattern.
    pub(super) fn echo_request(id: u16, seq: u16, nonce: &[u8; 8]) -> Vec<u8> {
        let mut p = vec![0u8; 8 + PAYLOAD];
        p[0] = 8;
        p[4..6].copy_from_slice(&id.to_be_bytes());
        p[6..8].copy_from_slice(&seq.to_be_bytes());
        p[8..16].copy_from_slice(nonce);
        for (i, b) in p[16..].iter_mut().enumerate() {
            *b = i as u8;
        }
        let c = checksum(&p);
        p[2..4].copy_from_slice(&c.to_be_bytes());
        p
    }

    /// The sequence number of an echo reply carrying our nonce. macOS hands over the IP header too; Linux does not.
    pub(super) fn parse_reply(buf: &[u8], nonce: &[u8; 8]) -> Option<u16> {
        let icmp = match buf.first() {
            Some(b) if b >> 4 == 4 => buf.get((b & 0xf) as usize * 4..)?,
            _ => buf,
        };
        if icmp.len() < 16 || icmp[0] != 0 || icmp[8..16] != nonce[..] {
            return None;
        }
        Some(u16::from_be_bytes([icmp[6], icmp[7]]))
    }

    struct Echo {
        /// The sequence number sent; it wraps, so it is only compared among the echoes still open.
        seq: u16,
        at: u32,
        sent_ns: u64,
        us: u32,
        open: bool,
        /// How late the tick was sent, against its schedule.
        lag_us: u32,
        lo_sent_ns: u64,
        lo_us: u32,
        lo_open: bool,
    }

    /// Give the reply to echo `seq` its time, if that echo is still waiting (on the loopback side or the gateway's).
    fn settle(echoes: &mut [Echo], base: usize, seq: u16, arrived: u64, loopback: bool) {
        let waiting = |e: &Echo| if loopback { e.lo_open } else { e.open };
        let Some(e) = echoes[base..].iter_mut().find(|e| waiting(e) && e.seq == seq) else {
            return;
        };
        if loopback {
            e.lo_us = (arrived.saturating_sub(e.lo_sent_ns) / 1000).clamp(1, u32::MAX as u64) as u32;
            e.lo_open = false;
        } else {
            e.us = (arrived.saturating_sub(e.sent_ns) / 1000).clamp(1, u32::MAX as u64) as u32;
            e.open = false;
        }
    }

    pub(super) fn sample(p: Pinger, t0: Instant, stop: &AtomicBool) -> Pinged {
        let id = std::process::id() as u16;
        let fd = p.sock.as_raw_fd();
        let mut echoes: Vec<Echo> = Vec::new();
        // Counter reads at the first tick of each second, and one when stopping.
        let mut snaps: Vec<(Instant, Counters)> = Vec::new();
        let mut closed = false;
        // Everything before `lo` is settled.
        let mut lo = 0;
        let mut next = Instant::now();
        let mut seq = 0u16;
        let mut buf = [0u8; 256];
        loop {
            let stopping = stop.load(Ordering::Relaxed);
            let now = Instant::now();
            if !stopping && now >= next {
                let at = t0.elapsed().as_millis() as u32;
                let sent_ns = clock::now_ns();
                let lag_us = Instant::now().saturating_duration_since(next).as_micros().min(u32::MAX as u128) as u32;
                let ok = p.sock.send_to(&echo_request(id, seq, &p.nonce), &p.dest).is_ok();
                let lo_sent_ns = clock::now_ns();
                let lo_ok = p.lo.as_ref().is_some_and(|l| l.sock.send_to(&echo_request(id, seq, &p.nonce), &l.dest).is_ok());
                if echoes.len().is_multiple_of(TICKS_PER_ROW) {
                    snaps.push((Instant::now(), read_counters(p.iface.as_deref())));
                }
                echoes.push(Echo { seq, at, sent_ns, us: 0, open: ok, lag_us, lo_sent_ns, lo_us: 0, lo_open: lo_ok });
                seq = seq.wrapping_add(1);
                next += Duration::from_millis(INTERVAL_MS);
                if next < now {
                    next = now + Duration::from_millis(INTERVAL_MS);
                }
            }
            if stopping && !closed {
                snaps.push((Instant::now(), read_counters(p.iface.as_deref())));
                closed = true;
            }
            // Loopback replies come back at once and carry the kernel's stamp, so reading them here loses nothing.
            if let Some(l) = &p.lo {
                while let Ok((n, stamp)) = net::recv_stamped_fd(l.sock.as_raw_fd(), &mut buf) {
                    let arrived = if l.stamped { stamp.unwrap_or_else(clock::now_ns) } else { clock::now_ns() };
                    if let Some(reply) = parse_reply(&buf[..n], &p.nonce) {
                        settle(&mut echoes, lo, reply, arrived, true);
                    }
                }
            }
            let t = clock::now_ns();
            for e in &mut echoes[lo..] {
                if e.open && t.saturating_sub(e.sent_ns) > LOSS_NS {
                    e.open = false;
                }
                if e.lo_open && t.saturating_sub(e.lo_sent_ns) > LOSS_NS {
                    e.lo_open = false;
                }
            }
            while lo < echoes.len() && !echoes[lo].open && !echoes[lo].lo_open {
                lo += 1;
            }
            if stopping && lo == echoes.len() {
                break;
            }
            let wait = if stopping { Duration::from_millis(20) } else { next.saturating_duration_since(Instant::now()) };
            let _ = p.sock.set_read_timeout(Some(wait.clamp(Duration::from_millis(1), Duration::from_millis(50))));
            let Ok((n, stamp)) = net::recv_stamped_fd(fd, &mut buf) else { continue };
            let arrived = if p.stamped { stamp.unwrap_or_else(clock::now_ns) } else { clock::now_ns() };
            let Some(reply) = parse_reply(&buf[..n], &p.nonce) else { continue };
            settle(&mut echoes, lo, reply, arrived, false);
        }
        // A loopback that never answered is no loopback, not a busy Mac.
        let lo_works = echoes.iter().any(|e| e.lo_us > 0);
        let rows = echoes
            .chunks(TICKS_PER_ROW)
            .enumerate()
            .map(|(k, group)| {
                let lo_us =
                    lo_works.then(|| group.iter().map(|e| if e.lo_us == 0 { (LOSS_NS / 1000) as u32 } else { e.lo_us }).max().unwrap_or(0));
                let (rx_bps, tx_bps, err, awdl_bps) = match (snaps.get(k), snaps.get(k + 1)) {
                    (Some((t_a, a)), Some((t_b, b))) => rates(a, b, t_b.duration_since(*t_a).as_secs_f64()),
                    _ => (None, None, None, None),
                };
                HostRow {
                    at_ms: group[0].at,
                    lo_us,
                    lag_us: group.iter().map(|e| e.lag_us).max().unwrap_or(0),
                    rx_bps,
                    tx_bps,
                    err,
                    awdl_bps,
                }
            })
            .collect();
        (echoes.iter().map(|e| (e.at, e.us)).collect(), HostRun { rows })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn c(ib: u32, ob: u32, errs: u32, up: bool) -> IfCounters {
            IfCounters { ibytes: ib, obytes: ob, ierrors: errs, oerrors: 0, iqdrops: 0, up }
        }

        #[test]
        fn a_reply_settles_its_own_echo_across_the_sequence_wrap() {
            let echo = |seq: u16| Echo { seq, at: 0, sent_ns: 0, us: 0, open: true, lag_us: 0, lo_sent_ns: 0, lo_us: 0, lo_open: false };
            let mut e = vec![echo(65_534), echo(65_535), echo(0), echo(1)];
            settle(&mut e, 0, 0, 5_000, false);
            assert_eq!((e[2].us, e[2].open), (5, false));
            assert!(e[0].open && e[1].open && e[3].open);
        }

        #[test]
        fn counter_deltas_wrap_and_become_rates() {
            let a = Counters { en: Some(c(u32::MAX - 99, 0, 5, true)), awdl: Some(c(0, 0, 0, true)) };
            let b = Counters { en: Some(c(125_000 - 100, 250_000, 7, true)), awdl: Some(c(0, 12_500, 0, true)) };
            // Over 2 s: 125 kB received is 500 kbps, 250 kB sent is 1 Mbps, awdl0 12.5 kB is 50 kbps.
            let (rx, tx, err, awdl) = rates(&a, &b, 2.0);
            assert_eq!((rx, tx, err, awdl), (Some(500_000), Some(1_000_000), Some(2), Some(50_000)));
        }

        #[test]
        fn rates_need_both_ends_and_an_up_awdl() {
            let a = Counters { en: Some(c(0, 0, 0, true)), awdl: Some(c(0, 0, 0, false)) };
            let b = Counters { en: None, awdl: Some(c(0, 900, 0, true)) };
            assert_eq!(rates(&a, &b, 1.0), (None, None, None, None));
            let b = Counters { en: Some(c(0, 0, 0, true)), awdl: Some(c(0, 900, 0, true)) };
            assert_eq!(rates(&a, &b, 1.0), (Some(0), Some(0), Some(0), None), "awdl0 was down at the start");
            assert_eq!(rates(&a, &b, 0.01), (None, None, None, None));
        }

        #[test]
        fn request_is_a_valid_echo() {
            let nonce = [7u8; 8];
            let p = echo_request(0x1234, 9, &nonce);
            assert_eq!(p.len(), 64);
            assert_eq!(p[0], 8);
            assert_eq!(u16::from_be_bytes([p[6], p[7]]), 9);
            // A correct checksum makes the whole message sum to zero.
            assert_eq!(checksum(&p), 0);
        }

        #[test]
        fn reply_is_matched_by_nonce_and_seq_with_or_without_ip_header() {
            let nonce = [7u8; 8];
            let mut icmp = echo_request(1, 42, &nonce);
            icmp[0] = 0;
            assert_eq!(parse_reply(&icmp, &nonce), Some(42));
            let mut with_ip = vec![0x45u8; 1];
            with_ip.extend_from_slice(&[0u8; 19]);
            with_ip.extend_from_slice(&icmp);
            assert_eq!(parse_reply(&with_ip, &nonce), Some(42));
            assert_eq!(parse_reply(&icmp, &[8u8; 8]), None);
            icmp[0] = 8;
            assert_eq!(parse_reply(&icmp, &nonce), None);
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::clock;
    use std::net::Ipv4Addr;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ICMP_ECHO_REPLY, IP_SUCCESS, IcmpCloseHandle, IcmpCreateFile, IcmpParseReplies, IcmpSendEcho2,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForMultipleObjects};

    /// Echoes that can be in flight at once: the loss timeout holds ten, plus slack for late completions.
    const SLOTS: usize = 16;

    /// One outstanding echo: the event `IcmpSendEcho2` signals when it completes, and the reply buffer it fills.
    struct Slot {
        event: HANDLE,
        reply: Vec<u8>,
        /// The echo this slot is carrying, as an index into the sample list; `None` when free.
        echo: Option<usize>,
    }

    pub(super) struct Pinger {
        icmp: HANDLE,
        /// The gateway, in network byte order as IP Helper wants it.
        dest: u32,
        payload: [u8; PAYLOAD],
        slots: Vec<Slot>,
    }

    // The handles belong to the process; only the link thread uses them.
    unsafe impl Send for Pinger {}

    impl Drop for Pinger {
        fn drop(&mut self) {
            for s in &self.slots {
                unsafe { CloseHandle(s.event) };
            }
            unsafe { IcmpCloseHandle(self.icmp) };
        }
    }

    pub(super) fn open(gw: Ipv4Addr, _iface: Option<String>) -> Result<Pinger, String> {
        let icmp = unsafe { IcmpCreateFile() };
        if icmp == INVALID_HANDLE_VALUE {
            return Err(format!("Could not open an ICMP handle ({}).", std::io::Error::last_os_error()));
        }
        let mut p = Pinger { icmp, dest: u32::from_ne_bytes(gw.octets()), payload: [0; PAYLOAD], slots: Vec::new() };
        for (i, b) in p.payload.iter_mut().enumerate() {
            *b = i as u8;
        }
        for _ in 0..SLOTS {
            // Manual reset: the slot resets it before each send.
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(format!("Could not create an event ({}).", std::io::Error::last_os_error()));
            }
            // The documented minimum: one reply struct, the echoed data, and room for an ICMP error.
            p.slots.push(Slot { event, reply: vec![0; std::mem::size_of::<ICMP_ECHO_REPLY>() + PAYLOAD + 8], echo: None });
        }
        Ok(p)
    }

    /// Send one echo from a free slot. `Ok(true)` when it is in flight, `Ok(false)` when it was answered on the spot.
    fn send(p: &mut Pinger, slot: usize) -> Result<bool, ()> {
        let s = &mut p.slots[slot];
        unsafe { ResetEvent(s.event) };
        let r = unsafe {
            IcmpSendEcho2(
                p.icmp,
                s.event,
                None,
                std::ptr::null(),
                p.dest,
                p.payload.as_ptr().cast(),
                PAYLOAD as u16,
                std::ptr::null(),
                s.reply.as_mut_ptr().cast(),
                s.reply.len() as u32,
                (LOSS_NS / 1_000_000) as u32,
            )
        };
        if r != 0 {
            return Ok(false);
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_IO_PENDING as i32) { Ok(true) } else { Err(()) }
    }

    /// Whether a completed slot holds a successful reply from `dest`.
    fn replied(s: &mut Slot, dest: u32) -> bool {
        let n = unsafe { IcmpParseReplies(s.reply.as_mut_ptr().cast(), s.reply.len() as u32) };
        n >= 1 && reply_ok(&s.reply, dest)
    }

    /// Reads the first reply in a buffer `IcmpParseReplies` accepted.
    pub(super) fn reply_ok(buf: &[u8], dest: u32) -> bool {
        if buf.len() < std::mem::size_of::<ICMP_ECHO_REPLY>() {
            return false;
        }
        let r: ICMP_ECHO_REPLY = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
        r.Status == IP_SUCCESS && r.Address == dest
    }

    struct Echo {
        at: u32,
        sent_ns: u64,
        us: u32,
        open: bool,
        lag_us: u32,
    }

    pub(super) fn sample(mut p: Pinger, t0: Instant, stop: &AtomicBool) -> Pinged {
        let mut echoes: Vec<Echo> = Vec::new();
        let mut next = Instant::now();
        loop {
            let stopping = stop.load(Ordering::Relaxed);
            let now = Instant::now();
            if !stopping && now >= next {
                let at = t0.elapsed().as_millis() as u32;
                let lag_us = Instant::now().saturating_duration_since(next).as_micros().min(u32::MAX as u128) as u32;
                let free = p.slots.iter().position(|s| s.echo.is_none());
                let sent_ns = clock::now_ns();
                let mut e = Echo { at, sent_ns, us: 0, open: false, lag_us };
                if let Some(i) = free {
                    match send(&mut p, i) {
                        Ok(true) => {
                            p.slots[i].echo = Some(echoes.len());
                            e.open = true;
                        }
                        Ok(false) => {
                            if reply_ok(&p.slots[i].reply, p.dest) {
                                e.us = (clock::now_ns().saturating_sub(sent_ns) / 1000).clamp(1, u32::MAX as u64) as u32;
                            }
                        }
                        Err(()) => {}
                    }
                }
                echoes.push(e);
                next += Duration::from_millis(INTERVAL_MS);
                if next < now {
                    next = now + Duration::from_millis(INTERVAL_MS);
                }
            }
            let t = clock::now_ns();
            for e in &mut echoes {
                if e.open && t.saturating_sub(e.sent_ns) > LOSS_NS {
                    e.open = false;
                }
            }
            let busy: Vec<usize> = (0..p.slots.len()).filter(|&i| p.slots[i].echo.is_some()).collect();
            if stopping && busy.is_empty() {
                break;
            }
            // Wait for the first completion, or until the next tick is due. When stopping, the loss timeout bounds
            // the wait: every slot completes by then.
            let wait = if stopping { Duration::from_millis(20) } else { next.saturating_duration_since(Instant::now()) };
            let wait = wait.clamp(Duration::from_millis(1), Duration::from_millis(50)).as_millis() as u32;
            if busy.is_empty() {
                std::thread::sleep(Duration::from_millis(wait as u64));
                continue;
            }
            let handles: Vec<HANDLE> = busy.iter().map(|&i| p.slots[i].event).collect();
            let r = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, wait) };
            if r == WAIT_FAILED {
                std::thread::sleep(Duration::from_millis(wait as u64));
                continue;
            }
            let k = r.wrapping_sub(WAIT_OBJECT_0) as usize;
            if k >= handles.len() {
                continue;
            }
            // Take the slot that signalled, then any others already signalled, each timed at its own wake-up.
            let arrived = clock::now_ns();
            let i = busy[k];
            let ok = replied(&mut p.slots[i], p.dest);
            if let Some(n) = p.slots[i].echo.take() {
                let e = &mut echoes[n];
                if e.open && ok {
                    e.us = (arrived.saturating_sub(e.sent_ns) / 1000).clamp(1, u32::MAX as u64) as u32;
                }
                e.open = false;
            }
        }
        let rows = echoes
            .chunks(TICKS_PER_ROW)
            .map(|group| HostRow { at_ms: group[0].at, lag_us: group.iter().map(|e| e.lag_us).max().unwrap_or(0), ..Default::default() })
            .collect();
        (echoes.iter().map(|e| (e.at, e.us)).collect(), HostRun { rows })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reply_needs_success_from_the_gateway() {
            let dest = u32::from_ne_bytes([192, 168, 1, 1]);
            let mut r = ICMP_ECHO_REPLY { Address: dest, Status: IP_SUCCESS, ..Default::default() };
            let bytes = |r: &ICMP_ECHO_REPLY| unsafe {
                std::slice::from_raw_parts((r as *const ICMP_ECHO_REPLY).cast::<u8>(), std::mem::size_of::<ICMP_ECHO_REPLY>()).to_vec()
            };
            assert!(reply_ok(&bytes(&r), dest));
            r.Status = 11010; // IP_REQ_TIMED_OUT
            assert!(!reply_ok(&bytes(&r), dest));
            r.Status = IP_SUCCESS;
            r.Address = dest.wrapping_add(1);
            assert!(!reply_ok(&bytes(&r), dest));
            assert!(!reply_ok(&[0; 4], dest));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady(n: u32, us: u32) -> Vec<(u32, u32)> {
        (0..n).map(|i| (i * 100, us)).collect()
    }

    #[test]
    fn clean_link_is_quiet() {
        let s = summarize(&steady(1000, 2100));
        assert_eq!((s.sent, s.lost, s.p50_us, s.p95_us, s.max_us, s.stalls), (1000, 0, 2100, 2100, 2100, 0));
        assert!(!s.noisy());
    }

    #[test]
    fn stall_limit_is_the_larger_of_20_ms_and_5_x_p50() {
        // p50 2 ms: limit 20 ms. 20 ms is not a stall, 20.001 ms is.
        let mut v = steady(1000, 2000);
        v[0].1 = 20_000;
        v[1].1 = 20_001;
        assert_eq!(summarize(&v).stalls, 1);
        // p50 8 ms: limit 40 ms.
        let mut v = steady(1000, 8000);
        v[0].1 = 39_000;
        v[1].1 = 41_000;
        assert_eq!(summarize(&v).stalls, 1);
    }

    #[test]
    fn lost_echoes_are_stalls_and_leave_the_percentiles_alone() {
        let mut v = steady(100, 3000);
        v[10].1 = 0;
        v[20].1 = 0;
        let s = summarize(&v);
        assert_eq!((s.sent, s.lost, s.stalls, s.p50_us, s.max_us), (100, 2, 2, 3000, 3000));
        assert_eq!(s.replies(), 98);
    }

    #[test]
    fn noisy_when_stalls_pass_2_percent() {
        let stalls = |n: usize| {
            let mut v = steady(1000, 2000);
            for e in v.iter_mut().take(n) {
                e.1 = 300_000;
            }
            summarize(&v)
        };
        assert!(!stalls(20).noisy(), "2.0% is not over 2%");
        assert!(stalls(21).noisy());
    }

    #[test]
    fn noisy_when_p95_passes_20_ms() {
        let mut v = steady(100, 2000);
        for e in v.iter_mut().take(10) {
            e.1 = 21_000;
        }
        let s = summarize(&v);
        assert_eq!(s.p95_us, 21_000);
        assert!(s.noisy());
        let mut v = steady(100, 19_000);
        v[0].1 = 20_000;
        assert!(!summarize(&v).noisy());
    }

    #[test]
    fn a_slow_but_steady_link_is_not_stalled_by_the_floor_alone() {
        // 30 ms every time: p50 30 ms so the limit is 150 ms; nothing stalls, but p95 over 20 ms still reads noisy.
        let s = summarize(&steady(100, 30_000));
        assert_eq!(s.stalls, 0);
        assert!(s.noisy());
    }

    #[test]
    fn empty_and_all_lost() {
        let s = summarize(&[]);
        assert_eq!((s.sent, s.stalls), (0, 0));
        assert!(!s.noisy());
        let s = summarize(&[(0, 0), (100, 0)]);
        assert_eq!((s.sent, s.lost, s.stalls, s.p50_us), (2, 2, 2, 0));
        assert_eq!(s.replies(), 0);
    }

    #[test]
    fn report_shapes() {
        let (v, note) = report(Err("No default gateway found.".into()));
        assert_eq!(v["state"], "unavailable");
        assert!(note.is_none());
        let gw: IpAddr = "192.168.1.1".parse().unwrap();
        let (v, _) = report(Ok(LinkRun { gateway: gw, samples: vec![(0, 0), (100, 0)], host: HostRun::default() }));
        assert_eq!(v["state"], "unavailable");
        let mut samples = steady(200, 2100);
        samples[5].1 = 363_000;
        samples[6].1 = 300_000;
        samples[7].1 = 250_000;
        samples[8].1 = 250_000;
        samples[9].1 = 250_000;
        let (v, note) = report(Ok(LinkRun { gateway: gw, samples, host: HostRun::default() }));
        assert_eq!((v["gateway"].as_str(), v["sent"].as_u64(), v["stalls"].as_u64()), (Some("192.168.1.1"), Some(200), Some(5)));
        assert_eq!(v["max"].as_f64(), Some(363.0));
        assert_eq!(v["samples"][5], json!([500, 363_000]));
        let note = note.expect("5 of 200 stalled");
        assert!(note.contains("up to 363 ms (typically 2.1 ms)."), "{note}");
        // No host rows: the stalls have nothing else to blame.
        assert_eq!(v["host"]["cause"], "unknown");
        assert!(note.ends_with("usually clears this."), "{note}");
    }

    /// 1 Hz host rows over `n` seconds, all quiet.
    fn quiet(n: u32) -> Vec<HostRow> {
        (0..n)
            .map(|i| HostRow {
                at_ms: i * 1000,
                lo_us: Some(80),
                lag_us: 400,
                rx_bps: Some(200_000),
                tx_bps: Some(100_000),
                err: Some(0),
                awdl_bps: Some(0),
            })
            .collect()
    }

    /// A noisy run: ten stalled echoes, all in the second starting at 3000 ms, over a background of 2 ms replies.
    fn stalled_at_3s() -> (Vec<(u32, u32)>, LinkSummary) {
        let mut v = steady(100, 2000);
        for e in v.iter_mut().skip(30).take(10) {
            e.1 = 250_000;
        }
        let sum = summarize(&v);
        assert!(sum.noisy());
        (v, sum)
    }

    fn cause_with(change: impl Fn(&mut HostRow)) -> Cause {
        let (v, sum) = stalled_at_3s();
        let mut host = quiet(10);
        change(&mut host[3]);
        classify(&sum, &v, &host)
    }

    #[test]
    fn quiet_host_leaves_the_cause_unknown() {
        assert_eq!(cause_with(|_| {}), Cause::Unknown);
    }

    #[test]
    fn slow_loopback_or_late_ticks_mean_busy() {
        assert_eq!(cause_with(|r| r.lo_us = Some(12_000)), Cause::Busy { lo_us: 12_000, lag_us: 400 });
        assert_eq!(cause_with(|r| r.lag_us = 45_000), Cause::Busy { lo_us: 80, lag_us: 45_000 });
        assert_eq!(cause_with(|r| r.lo_us = Some(5_000)), Cause::Unknown, "5 ms is not over 5 ms");
    }

    #[test]
    fn heavy_en0_traffic_is_other_traffic() {
        assert_eq!(cause_with(|r| r.rx_bps = Some(38_000_000)), Cause::Traffic { bps: 38_100_000 });
        assert_eq!(cause_with(|r| r.tx_bps = Some(4_700_000)), Cause::Unknown, "4.8 Mbps in all");
    }

    #[test]
    fn awdl_traffic_is_airdrop() {
        assert_eq!(cause_with(|r| r.awdl_bps = Some(2_000_000)), Cause::Awdl { bps: 2_000_000 });
        assert_eq!(cause_with(|r| r.awdl_bps = Some(100_000)), Cause::Unknown);
    }

    #[test]
    fn en0_errors_or_drops_mean_driver() {
        assert_eq!(cause_with(|r| r.err = Some(3)), Cause::Driver);
    }

    #[test]
    fn rules_apply_in_order() {
        let both = |r: &mut HostRow| {
            r.lag_us = 30_000;
            r.rx_bps = Some(50_000_000);
            r.awdl_bps = Some(1_000_000);
            r.err = Some(1);
        };
        assert!(matches!(cause_with(both), Cause::Busy { .. }));
        assert!(matches!(
            cause_with(|r| {
                both(r);
                r.lag_us = 0
            }),
            Cause::Traffic { .. }
        ));
        assert!(matches!(
            cause_with(|r| {
                both(r);
                r.lag_us = 0;
                r.rx_bps = None
            }),
            Cause::Awdl { .. }
        ));
    }

    #[test]
    fn only_the_rows_with_a_stall_count() {
        // The busy second is the one after the stall.
        let (v, sum) = stalled_at_3s();
        let mut host = quiet(10);
        host[4].lag_us = 90_000;
        host[2].rx_bps = Some(90_000_000);
        assert_eq!(classify(&sum, &v, &host), Cause::Unknown);
    }

    #[test]
    fn missing_signals_do_not_trip_a_rule() {
        let (v, sum) = stalled_at_3s();
        let host: Vec<HostRow> = (0..10).map(|i| HostRow { at_ms: i * 1000, lag_us: 300, ..Default::default() }).collect();
        assert_eq!(classify(&sum, &v, &host), Cause::Unknown);
    }

    #[test]
    fn clean_run_has_no_cause_and_no_note() {
        let gw: IpAddr = "192.168.1.1".parse().unwrap();
        let host = HostRun { rows: quiet(20) };
        let (v, note) = report(Ok(LinkRun { gateway: gw, samples: steady(200, 2100), host }));
        assert!(note.is_none());
        assert!(v["host"]["cause"].is_null());
        assert_eq!(v["host"]["samples"][1], json!([1000, 80, 400, 200_000, 100_000, 0, 0]));
    }

    #[test]
    fn noisy_note_names_the_cause_with_its_figure() {
        let gw: IpAddr = "192.168.1.1".parse().unwrap();
        let (samples, _) = stalled_at_3s();
        let mut rows = quiet(10);
        rows[3].rx_bps = Some(38_000_000);
        rows[3].lo_us = None;
        let host = HostRun { rows };
        let (v, note) = report(Ok(LinkRun { gateway: gw, samples, host }));
        assert_eq!(v["host"]["cause"], "traffic");
        assert_eq!(v["host"]["samples"][3][1], Value::Null);
        let note = note.expect("noisy");
        assert!(note.ends_with(&format!("Likely cause: other traffic on this {MACHINE} (up to 38 Mbps).")), "{note}");
    }
}
