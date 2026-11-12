//! Trace the route to a resolver's address with ICMP probes of rising TTL, using Trippy (trippy.rs), or on Windows
//! the system's IP Helper (see [`win`]). The result is a live view, like the Deep Dive: nothing is stored. iOS builds
//! exclude Trippy and get stubs.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::atomic::AtomicBool;

use crate::cymru;
use crate::resolve::Resolve;
use crate::stats::round1;

/// Whether a trace can run here: `env.trace` in the API.
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TraceCap {
    Ok,
    NeedsAdmin,
    Unsupported,
}

/// The protocol of a stored trace: only ICMP is ever run, so a trace read back was ICMP.
fn stored_protocol() -> &'static str {
    "icmp"
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Trace {
    pub target: IpAddr,
    #[serde(skip_deserializing, default = "stored_protocol")]
    pub protocol: &'static str,
    pub rounds: u32,
    pub reached: bool,
    pub elapsed_ms: f64,
    pub hops: Vec<Hop>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Hop {
    pub ttl: u8,
    pub addrs: Vec<IpAddr>,
    pub private: bool,
    pub asn: Option<u32>,
    pub holder: Option<String>,
    pub samples_ms: Vec<Option<f64>>,
    pub best_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub last_ms: Option<f64>,
    pub sent: u32,
    pub recv: u32,
}

/// One round's probes: `(ttl, Some((reply host, round trip ms)))`, or `None` for a probe still unanswered at round end.
/// Probes that were never sent are left out.
type RoundProbes = Vec<(u8, Option<(IpAddr, f64)>)>;

/// Per-TTL table from the rounds. Hops run densely from 1 to the cut: the first TTL answered by `target` (Trippy
/// probes past it in the first round), else the highest TTL that ever answered. Returns the hops and whether the
/// target answered.
#[cfg_attr(target_os = "ios", allow(dead_code))]
fn fold(rounds: &[RoundProbes], target: IpAddr) -> (Vec<Hop>, bool) {
    let replies = || rounds.iter().flatten().filter_map(|(ttl, r)| r.map(|(h, _)| (*ttl, h)));
    let reached_at = replies().filter(|(_, h)| *h == target).map(|(t, _)| t).min();
    let cut = reached_at.or_else(|| replies().map(|(t, _)| t).max()).unwrap_or(0);
    let hops = (1..=cut)
        .map(|ttl| {
            let mut addrs: Vec<IpAddr> = Vec::new();
            let (mut sent, mut recv) = (0, 0);
            let samples_ms: Vec<Option<f64>> = rounds
                .iter()
                .map(|round| {
                    let mut first = None;
                    for (_, r) in round.iter().filter(|(t, _)| *t == ttl) {
                        sent += 1;
                        if let Some((host, ms)) = r {
                            recv += 1;
                            if !addrs.contains(host) {
                                addrs.push(*host);
                            }
                            first.get_or_insert(round1(*ms));
                        }
                    }
                    first
                })
                .collect();
            let got: Vec<f64> = samples_ms.iter().flatten().copied().collect();
            Hop {
                ttl,
                private: addrs.first().is_some_and(crate::catalog::is_private),
                addrs,
                asn: None,
                holder: None,
                best_ms: got.iter().copied().reduce(f64::min),
                avg_ms: (!got.is_empty()).then(|| round1(got.iter().sum::<f64>() / got.len() as f64)),
                last_ms: got.last().copied(),
                samples_ms,
                sent,
                recv,
            }
        })
        .collect();
    (hops, reached_at.is_some())
}

/// Fill in `asn` and `holder` for each public hop from its first address (Team Cymru). A failed lookup leaves both null.
pub(crate) fn enrich(hops: &mut [Hop], res: &Resolve) {
    for h in hops.iter_mut().filter(|h| !h.private) {
        let Some(&addr) = h.addrs.first() else { continue };
        let Ok(Some(o)) = cymru::origin_of(res, addr) else { continue };
        h.asn = Some(o.asn);
        h.holder = cymru::name_of(res, o.asn).ok().flatten().map(|n| cymru::holder_name(&n));
    }
}

/// [`run`], then name the hops with [`enrich`].
pub fn run_enriched(target: IpAddr, res: &Resolve, cancel: &AtomicBool) -> Result<Trace, String> {
    let mut t = run(target, cancel)?;
    enrich(&mut t.hops, res);
    Ok(t)
}

#[cfg(not(any(target_os = "ios", windows)))]
pub fn available() -> TraceCap {
    if cfg!(target_os = "macos") {
        TraceCap::Ok
    } else if cfg!(target_os = "linux") {
        match trippy_privilege::Privilege::discover() {
            Ok(p) if p.has_privileges() => TraceCap::Ok,
            _ => TraceCap::NeedsAdmin,
        }
    } else {
        TraceCap::Unsupported
    }
}

/// Windows probes through IP Helper (see [`win`]), which asks for no rights.
#[cfg(windows)]
pub fn available() -> TraceCap {
    TraceCap::Ok
}

#[cfg(target_os = "ios")]
pub fn available() -> TraceCap {
    TraceCap::Unsupported
}

/// Four rounds of ICMP probes, TTL 1 to 30, about 1 to 5 s. Blocks, so call it from `spawn_blocking`. `cancel` is
/// accepted for symmetry with the other probes but not polled: Trippy's `run_with` can't be stopped from its
/// callback, and the worst case is bounded.
#[cfg(not(target_os = "ios"))]
pub(crate) fn run(target: IpAddr, cancel: &AtomicBool) -> Result<Trace, String> {
    let _ = cancel;
    run_live(target, 0, &|_| {})
}

/// [`run`], handing `on_round` the trace as it stands after each round. Traces that run at the same time need
/// distinct non-zero `id`s: each tracer drops replies carrying another's identifier (0 accepts any).
#[cfg(not(any(target_os = "ios", windows)))]
pub(crate) fn run_live(target: IpAddr, id: u16, on_round: &dyn Fn(&Trace)) -> Result<Trace, String> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use trippy_core::{Builder, PrivilegeMode, ProbeStatus, Protocol};

    const ROUNDS: usize = 4;
    let mode = if cfg!(target_os = "macos") { PrivilegeMode::Unprivileged } else { PrivilegeMode::Privileged };
    let tracer = Builder::new(target)
        .protocol(Protocol::Icmp)
        .privilege_mode(mode)
        .trace_identifier(id)
        .max_rounds(Some(ROUNDS))
        .max_ttl(30)
        .grace_duration(Duration::from_millis(100))
        .max_round_duration(Duration::from_millis(1000))
        .min_round_duration(Duration::from_millis(0))
        .build()
        .map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    let rounds: Mutex<Vec<RoundProbes>> = Mutex::default();
    let snapshot = |rounds: &[RoundProbes]| {
        let (hops, reached) = fold(rounds, target);
        let elapsed_ms = round1(t0.elapsed().as_secs_f64() * 1000.0);
        Trace { target, protocol: "icmp", rounds: rounds.len() as u32, reached, elapsed_ms, hops }
    };
    tracer
        .run_with(|round| {
            let probes = round
                .probes
                .iter()
                .filter_map(|p| match p {
                    ProbeStatus::Complete(c) => {
                        let ms = c.received.duration_since(c.sent).unwrap_or_default().as_secs_f64() * 1000.0;
                        Some((c.ttl.0, Some((c.host, ms))))
                    }
                    ProbeStatus::Awaited(a) => Some((a.ttl.0, None)),
                    _ => None,
                })
                .collect();
            let mut all = rounds.lock().unwrap();
            all.push(probes);
            on_round(&snapshot(&all));
        })
        .map_err(|e| e.to_string())?;
    Ok(snapshot(&rounds.into_inner().unwrap()))
}

/// [`run_live`] on Windows, through IP Helper (see [`win`]). `id` is not needed: the system matches each reply to
/// its own request, so traces running at the same time can't take each other's.
#[cfg(windows)]
pub(crate) fn run_live(target: IpAddr, id: u16, on_round: &dyn Fn(&Trace)) -> Result<Trace, String> {
    const ROUNDS: usize = 4;
    let _ = id;
    let mut prober = win::Prober::open(target)?;
    let t0 = std::time::Instant::now();
    let snapshot = |rounds: &[RoundProbes]| {
        let (hops, reached) = fold(rounds, target);
        let elapsed_ms = round1(t0.elapsed().as_secs_f64() * 1000.0);
        Trace { target, protocol: "icmp", rounds: rounds.len() as u32, reached, elapsed_ms, hops }
    };
    let mut rounds: Vec<RoundProbes> = Vec::with_capacity(ROUNDS);
    // The first round probes to the limit; later ones stop at the TTL the target answered.
    let mut last = win::MAX_TTL;
    for _ in 0..ROUNDS {
        let probes = prober.round(1, last)?;
        let reached_at = probes.iter().filter(|(_, r)| r.is_some_and(|(host, _)| host == target)).map(|(ttl, _)| *ttl).min();
        if let Some(ttl) = reached_at {
            last = last.min(ttl);
        }
        rounds.push(probes);
        on_round(&snapshot(&rounds));
    }
    Ok(snapshot(&rounds))
}

#[cfg(target_os = "ios")]
pub(crate) fn run(_target: IpAddr, _cancel: &AtomicBool) -> Result<Trace, String> {
    Err("Trace route is not available on this platform.".to_string())
}

#[cfg(target_os = "ios")]
pub(crate) fn run_live(_target: IpAddr, _id: u16, _on_round: &dyn Fn(&Trace)) -> Result<Trace, String> {
    Err("Trace route is not available on this platform.".to_string())
}

/// The Windows prober. Echo requests go through IP Helper (`IcmpSendEcho2`, `Icmp6SendEcho2`), each with its own TTL:
/// a router where it expires answers "time exceeded", which the call reports as a reply from that router. Unlike a
/// raw socket this needs no Administrator, and Windows Firewall passes the replies (it drops them for a raw socket
/// unless a rule allows inbound ICMP). It is how `tracert` works.
#[cfg(windows)]
pub(crate) mod win {
    use super::RoundProbes;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ICMP_ECHO_REPLY, IP_OPTION_INFORMATION, Icmp6CreateFile, Icmp6ParseReplies, Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile,
        IcmpParseReplies, IcmpSendEcho2,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForMultipleObjects, WaitForSingleObject};

    pub(crate) const MAX_TTL: u8 = 30;
    /// How long the system waits for one probe's reply.
    const TIMEOUT_MS: u32 = 1000;
    /// Once the target has answered, how much longer the routers before it get.
    const GRACE: Duration = Duration::from_millis(100);
    /// Between two probes of a round, so the target's answer can stop the round before every TTL is sent.
    const PACE: Duration = Duration::from_millis(5);
    const PAYLOAD: usize = 32;
    /// Room for either reply struct, the echoed payload, an ICMP error and the IO_STATUS_BLOCK an asynchronous call
    /// also writes: more than the documented minimum for both families.
    const REPLY_BYTES: usize = 256;
    const IP_SUCCESS: u32 = 0;
    /// Also `IP_HOP_LIMIT_EXCEEDED`, the IPv6 name for the same status.
    const IP_TTL_EXPIRED_TRANSIT: u32 = 11013;
    const AF_INET6: u16 = 23;

    /// `SOCKADDR_IN6`, laid out by hand so no union has to be written through.
    #[repr(C)]
    struct SockaddrIn6 {
        family: u16,
        port: u16,
        flowinfo: u32,
        addr: [u8; 16],
        scope_id: u32,
    }

    /// One request: the event the system signals when it completes, and the reply buffer it fills.
    struct Slot {
        event: HANDLE,
        reply: Vec<u8>,
    }

    pub(crate) struct Prober {
        icmp: HANDLE,
        target: IpAddr,
        payload: [u8; PAYLOAD],
        slots: Vec<Slot>,
        /// Slots no request is using.
        free: Vec<usize>,
        /// Slots whose request a round gave up on. The system still writes the buffer when the request ends, so the
        /// slot is not used again until its event is set.
        stale: Vec<usize>,
    }

    impl Drop for Prober {
        fn drop(&mut self) {
            // A pending request writes into its buffer when it completes: wait for each (its own timeout bounds
            // this) before the buffers are freed.
            for &i in &self.stale {
                unsafe { WaitForSingleObject(self.slots[i].event, TIMEOUT_MS + 500) };
            }
            for s in &self.slots {
                unsafe { CloseHandle(s.event) };
            }
            unsafe { IcmpCloseHandle(self.icmp) };
        }
    }

    fn os_error() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    fn ms_since(sent: Instant) -> f64 {
        sent.elapsed().as_secs_f64() * 1000.0
    }

    impl Prober {
        pub(crate) fn open(target: IpAddr) -> Result<Prober, String> {
            let icmp = unsafe { if target.is_ipv4() { IcmpCreateFile() } else { Icmp6CreateFile() } };
            if icmp == INVALID_HANDLE_VALUE {
                return Err(format!("Could not open an ICMP handle ({}).", os_error()));
            }
            let mut payload = [0u8; PAYLOAD];
            for (i, b) in payload.iter_mut().enumerate() {
                *b = i as u8;
            }
            Ok(Prober { icmp, target, payload, slots: Vec::new(), free: Vec::new(), stale: Vec::new() })
        }

        /// A slot to send from: a free one, else a new one.
        fn slot(&mut self) -> Result<usize, String> {
            if let Some(i) = self.free.pop() {
                return Ok(i);
            }
            // Manual reset: `send` resets it before each request.
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(format!("Could not create an event ({}).", os_error()));
            }
            self.slots.push(Slot { event, reply: vec![0; REPLY_BYTES] });
            Ok(self.slots.len() - 1)
        }

        /// Free the given-up slots whose requests have ended since.
        fn reclaim(&mut self) {
            let slots = &self.slots;
            let (ended, pending): (Vec<usize>, Vec<usize>) =
                self.stale.iter().copied().partition(|&i| unsafe { WaitForSingleObject(slots[i].event, 0) } == WAIT_OBJECT_0);
            self.free.extend(ended);
            self.stale = pending;
        }

        /// Send one echo that expires after `ttl` routers. `Ok(true)` when it is in flight, `Ok(false)` when it
        /// completed on the spot.
        fn send(&mut self, slot: usize, ttl: u8) -> Result<bool, String> {
            let s = &mut self.slots[slot];
            unsafe { ResetEvent(s.event) };
            let options = IP_OPTION_INFORMATION { Ttl: ttl, Tos: 0, Flags: 0, OptionsSize: 0, OptionsData: std::ptr::null_mut() };
            let sent = unsafe {
                match self.target {
                    IpAddr::V4(a) => IcmpSendEcho2(
                        self.icmp,
                        s.event,
                        None,
                        std::ptr::null(),
                        // Network byte order, as IP Helper wants it.
                        u32::from_ne_bytes(a.octets()),
                        self.payload.as_ptr().cast(),
                        PAYLOAD as u16,
                        &options,
                        s.reply.as_mut_ptr().cast(),
                        s.reply.len() as u32,
                        TIMEOUT_MS,
                    ),
                    IpAddr::V6(a) => {
                        // The unspecified source lets the system choose the address.
                        let from = SockaddrIn6 { family: AF_INET6, port: 0, flowinfo: 0, addr: [0; 16], scope_id: 0 };
                        let to = SockaddrIn6 { family: AF_INET6, port: 0, flowinfo: 0, addr: a.octets(), scope_id: 0 };
                        Icmp6SendEcho2(
                            self.icmp,
                            s.event,
                            None,
                            std::ptr::null(),
                            (&raw const from).cast(),
                            (&raw const to).cast(),
                            self.payload.as_ptr().cast(),
                            PAYLOAD as u16,
                            &options,
                            s.reply.as_mut_ptr().cast(),
                            s.reply.len() as u32,
                            TIMEOUT_MS,
                        )
                    }
                }
            };
            if sent != 0 {
                return Ok(false);
            }
            let e = os_error();
            if e.raw_os_error() == Some(ERROR_IO_PENDING as i32) { Ok(true) } else { Err(e.to_string()) }
        }

        /// Who answered a completed request: the target, or the router where the probe expired. `None` for a
        /// timeout or any other failure. Call once per request: parsing rewrites the buffer.
        fn answered(&mut self, slot: usize) -> Option<IpAddr> {
            let buf = &mut self.slots[slot].reply;
            let ok = |status: u32| matches!(status, IP_SUCCESS | IP_TTL_EXPIRED_TRANSIT);
            match self.target {
                IpAddr::V4(_) => {
                    if unsafe { IcmpParseReplies(buf.as_mut_ptr().cast(), buf.len() as u32) } < 1 {
                        return None;
                    }
                    let r: ICMP_ECHO_REPLY = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
                    ok(r.Status).then(|| IpAddr::V4(Ipv4Addr::from(r.Address.to_ne_bytes())))
                }
                IpAddr::V6(_) => {
                    if unsafe { Icmp6ParseReplies(buf.as_mut_ptr().cast(), buf.len() as u32) } < 1 {
                        return None;
                    }
                    // ICMPV6_ECHO_REPLY: a packed IPV6_ADDRESS_EX (port 2, flow 4, address 16, scope 4 = 26 bytes),
                    // then the status at offset 28.
                    let status = u32::from_ne_bytes(buf[28..32].try_into().ok()?);
                    let addr: [u8; 16] = buf[6..22].try_into().ok()?;
                    ok(status).then(|| IpAddr::V6(Ipv6Addr::from(addr)))
                }
            }
        }

        /// One round: a probe for each TTL from `first` to `last`, sent in order a few milliseconds apart and all
        /// awaited together. When the target answers, no higher TTL is sent and the round ends after [`GRACE`].
        /// An error only when no probe could be sent at all.
        pub(crate) fn round(&mut self, first: u8, last: u8) -> Result<RoundProbes, String> {
            self.reclaim();
            let mut out: RoundProbes = Vec::new();
            // In flight: the slot, the TTL and when it was sent.
            let mut open: Vec<(usize, u8, Instant)> = Vec::new();
            let mut next = first;
            let mut reached = false;
            let mut deadline: Option<Instant> = None;
            let mut failed: Option<String> = None;
            loop {
                if !reached && next <= last {
                    let ttl = next;
                    next += 1;
                    match self.slot() {
                        Ok(slot) => {
                            let sent = Instant::now();
                            match self.send(slot, ttl) {
                                Ok(true) => open.push((slot, ttl, sent)),
                                Ok(false) => {
                                    let ms = ms_since(sent);
                                    let reply = self.answered(slot).map(|host| (host, ms));
                                    self.free.push(slot);
                                    reached |= reply.is_some_and(|(host, _)| host == self.target);
                                    out.push((ttl, reply));
                                }
                                Err(e) => {
                                    self.free.push(slot);
                                    failed = Some(e);
                                }
                            }
                        }
                        Err(e) => failed = Some(e),
                    }
                }
                let sending = !reached && next <= last;
                if !sending && open.is_empty() {
                    break;
                }
                let now = Instant::now();
                let wait = if sending {
                    PACE
                } else {
                    // Everything is sent: the system's own timeout ends each request, plus slack for the wake-up.
                    let until = *deadline.get_or_insert(now + Duration::from_millis(u64::from(TIMEOUT_MS) + 200));
                    match until.checked_duration_since(now) {
                        Some(d) if !d.is_zero() => d,
                        _ => break,
                    }
                };
                if open.is_empty() {
                    std::thread::sleep(wait);
                    continue;
                }
                // At most MAX_TTL handles, under the 64 one wait can take.
                let events: Vec<HANDLE> = open.iter().map(|(slot, _, _)| self.slots[*slot].event).collect();
                let r = unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, (wait.as_millis() as u32).max(1)) };
                let i = r.wrapping_sub(WAIT_OBJECT_0) as usize;
                if i < open.len() {
                    let (slot, ttl, sent) = open.remove(i);
                    let ms = ms_since(sent);
                    let reply = self.answered(slot).map(|host| (host, ms));
                    self.free.push(slot);
                    if reply.is_some_and(|(host, _)| host == self.target) {
                        reached = true;
                        let grace = Instant::now() + GRACE;
                        deadline = Some(deadline.map_or(grace, |d| d.min(grace)));
                    }
                    out.push((ttl, reply));
                } else if r == WAIT_FAILED {
                    break;
                }
            }
            // Still unanswered at round end. Their requests are still pending, so the slots wait in `stale`.
            for (slot, ttl, _) in open {
                out.push((ttl, None));
                self.stale.push(slot);
            }
            if let Some(e) = failed
                && out.is_empty()
            {
                return Err(format!("Could not send an ICMP probe ({e})."));
            }
            out.sort_by_key(|(ttl, _)| *ttl);
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn private_ranges() {
        for p in ["192.168.1.1", "10.0.0.1", "172.16.0.1", "100.64.0.1", "169.254.1.1", "127.0.0.1", "fd00::1", "fe80::1", "::1"] {
            assert!(crate::catalog::is_private(&ip(p)), "{p}");
        }
        for p in ["1.1.1.1", "100.128.0.1", "172.32.0.1", "203.0.113.97", "2606:4700:4700::1111"] {
            assert!(!crate::catalog::is_private(&ip(p)), "{p}");
        }
    }

    #[test]
    fn cut_at_target_and_dense() {
        let t = ip("1.1.1.1");
        let r1 = vec![(1, Some((ip("192.168.1.1"), 5.0))), (2, None), (3, Some((t, 11.04))), (4, Some((t, 11.0)))];
        let r2 = vec![(1, Some((ip("192.168.1.1"), 7.0))), (2, None), (3, Some((t, 12.0))), (4, None)];
        let (hops, reached) = fold(&[r1, r2], t);
        assert!(reached);
        assert_eq!(hops.iter().map(|h| h.ttl).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(hops[0].private && !hops[2].private);
        assert_eq!((hops[0].sent, hops[0].recv, hops[0].best_ms, hops[0].avg_ms, hops[0].last_ms), (2, 2, Some(5.0), Some(6.0), Some(7.0)));
        assert_eq!((hops[1].sent, hops[1].recv, hops[1].avg_ms), (2, 0, None));
        assert!(hops[1].addrs.is_empty());
        assert_eq!(hops[2].samples_ms, [Some(11.0), Some(12.0)]);
    }

    #[test]
    fn cut_at_highest_answer_when_target_silent() {
        let t = ip("9.9.9.9");
        let r1 = vec![(1, Some((ip("10.0.0.1"), 1.0))), (2, None), (3, Some((ip("8.8.8.8"), 9.0))), (4, None)];
        let (hops, reached) = fold(&[r1], t);
        assert!(!reached);
        assert_eq!(hops.len(), 3);
        assert_eq!(fold(&[vec![(1, None)]], t).0.len(), 0);
    }
}
