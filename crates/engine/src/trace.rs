//! Trace the route to a resolver's address with ICMP probes of rising TTL, using Trippy (trippy.rs).
//! The result is a live view, like the Deep Dive: nothing is stored. iOS builds exclude Trippy and get stubs.

use serde::Serialize;
use std::net::IpAddr;
use std::sync::atomic::AtomicBool;

use crate::cymru;
use crate::resolve::Resolve;

/// Whether a trace can run here: `env.trace` in the API.
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TraceCap {
    Ok,
    NeedsAdmin,
    Unsupported,
}

#[derive(Serialize, Clone, Debug)]
pub struct Trace {
    pub target: IpAddr,
    pub protocol: &'static str,
    pub rounds: u32,
    pub reached: bool,
    pub elapsed_ms: f64,
    pub hops: Vec<Hop>,
}

#[derive(Serialize, Clone, Debug)]
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

fn round1(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

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

#[cfg(not(target_os = "ios"))]
pub fn available() -> TraceCap {
    if cfg!(target_os = "macos") {
        TraceCap::Ok
    } else if cfg!(any(target_os = "linux", windows)) {
        match trippy_privilege::Privilege::discover() {
            Ok(p) if p.has_privileges() => TraceCap::Ok,
            _ => TraceCap::NeedsAdmin,
        }
    } else {
        TraceCap::Unsupported
    }
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
#[cfg(not(target_os = "ios"))]
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

#[cfg(target_os = "ios")]
pub(crate) fn run(_target: IpAddr, _cancel: &AtomicBool) -> Result<Trace, String> {
    Err("Trace route is not available on this platform.".to_string())
}

#[cfg(target_os = "ios")]
pub(crate) fn run_live(_target: IpAddr, _id: u16, _on_round: &dyn Fn(&Trace)) -> Result<Trace, String> {
    Err("Trace route is not available on this platform.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn private_ranges() {
        for p in ["192.168.1.1", "10.0.0.1", "172.20.20.4", "100.64.0.1", "169.254.1.1", "127.0.0.1", "fd00::1", "fe80::1", "::1"] {
            assert!(crate::catalog::is_private(&ip(p)), "{p}");
        }
        for p in ["1.1.1.1", "100.128.0.1", "172.32.0.1", "151.181.161.97", "2606:4700:4700::1111"] {
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
