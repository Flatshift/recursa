//! Self-calibration: how much latency could the instrument itself be adding?
//!
//! A responder thread echoes DNS queries on loopback and is measured with the exact
//! code path used for real resolvers. The responder busy-polls so its own wake-up
//! doesn't inflate the result: the loopback round trip is then the tool's send path,
//! two trips through the kernel's loopback stack, and the receive stamp. That is an
//! upper bound on what the instrument adds to any measurement.
//! The gap between the kernel receive stamp and the moment userspace saw the packet
//! is the scheduler delay that kernel timestamps remove from every measurement.

use crate::net::{Exchanger, UdpX};
use crate::{clock, dns, sysdns};
use serde::Serialize;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug, Serialize)]
pub struct Calibration {
    pub clock: &'static str,
    /// Kernel receive stamps are available: the socket got them turned on and, on Windows, the adapter
    /// that Internet traffic leaves through delivers them.
    pub kernel_rx: bool,
    pub samples: u32,
    /// How many of the samples actually carried a kernel stamp. Windows stamps packets where the adapter
    /// hands them up, so loopback answers never carry one there and the wake-up figures are empty.
    pub stamped: u32,
    pub floor_p50_us: f64,
    pub floor_p99_us: f64,
    pub wake_p50_us: f64,
    pub wake_p99_us: f64,
    pub at: i64,
}

pub(crate) fn kernel_stamps_available() -> bool {
    static K: OnceLock<bool> = OnceLock::new();
    *K.get_or_init(|| UdpX::connect("127.0.0.1:9".parse().unwrap()).is_ok_and(|u| u.kernel_timestamps()) && sysdns::adapter_rx_stamps())
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    crate::stats::nearest_rank(v, q).unwrap_or(f64::NAN)
}

/// Samples for the calibration at startup and in the background when none is stored.
pub(crate) const STARTUP_SAMPLES: u32 = 200;
/// Samples for a re-check requested from the UI.
pub(crate) const RECHECK_SAMPLES: u32 = 300;
/// Samples for the CLI `calibrate` command's full run.
pub const CLI_SAMPLES: u32 = 1000;

pub fn run(samples: u32) -> std::io::Result<Calibration> {
    let echo = UdpSocket::bind("127.0.0.1:0")?;
    echo.set_nonblocking(true)?;
    let addr: SocketAddr = echo.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let responder = std::thread::Builder::new().name("recursa-echo".into()).spawn(move || {
        clock::prioritize_thread();
        let mut buf = [0u8; 1500];
        while !stop2.load(Ordering::Relaxed) {
            match echo.recv_from(&mut buf) {
                Ok((n, from)) if n >= 12 => {
                    buf[2] |= 0x80;
                    let _ = echo.send_to(&buf[..n], from);
                }
                _ => std::hint::spin_loop(),
            }
        }
    })?;

    // Measure from a dedicated, prioritized thread, exactly like a benchmark worker.
    let result = std::thread::scope(|sc| sc.spawn(|| measure(addr, samples)).join());
    stop.store(true, Ordering::Relaxed);
    let _ = responder.join();
    let (kernel_rx, mut rtt, mut wake) = result.map_err(|_| std::io::Error::other("calibration thread panicked"))??;

    Ok(Calibration {
        clock: clock::SOURCE,
        kernel_rx,
        samples: rtt.len() as u32,
        stamped: wake.len() as u32,
        floor_p50_us: pct(&mut rtt, 0.5),
        floor_p99_us: pct(&mut rtt, 0.99),
        wake_p50_us: pct(&mut wake, 0.5),
        wake_p99_us: pct(&mut wake, 0.99),
        at: crate::bench::now_ms(),
    })
}

/// Round trips (µs) and kernel-vs-userspace receive gaps (µs) against the echo.
fn measure(addr: SocketAddr, samples: u32) -> std::io::Result<(bool, Vec<f64>, Vec<f64>)> {
    clock::prioritize_thread();
    let mut client = UdpX::connect(addr)?;
    let kernel_rx = client.kernel_timestamps() && sysdns::adapter_rx_stamps();
    let q = dns::encode_query(0, &dns::QuerySpec::new("calibration.recursa.invalid", dns::TYPE_A)).expect("static name");
    let mut rtt = Vec::with_capacity(samples as usize);
    let mut wake = Vec::with_capacity(samples as usize);
    for i in 0..samples + 20 {
        let mut msg = q.clone();
        dns::set_id(&mut msg, i as u16);
        if let Ok(r) = client.exchange(&msg, Duration::from_millis(200))
            && i >= 20
        {
            rtt.push(r.rtt_ns as f64 / 1000.0);
            if r.kernel_stamp {
                wake.push(r.user_rtt_ns.saturating_sub(r.rtt_ns) as f64 / 1000.0);
            }
        }
        std::thread::sleep(Duration::from_micros(500));
    }
    Ok((kernel_rx, rtt, wake))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same measurement against a public resolver, for a machine whose loopback is not stamped
    /// (Windows): `cargo test -p recursa-engine --release -- --ignored --nocapture stamps_on_the_network`.
    #[test]
    #[ignore = "asks a public resolver"]
    fn stamps_on_the_network() {
        clock::prioritize_thread();
        let mut client = UdpX::connect("1.1.1.1:53".parse().unwrap()).unwrap();
        let socket_rx = client.kernel_timestamps();
        let adapter_rx = sysdns::adapter_rx_stamps();
        let q = dns::encode_query(0, &dns::QuerySpec::new("calibration.recursa.invalid", dns::TYPE_A)).unwrap();
        let (mut rtt, mut user, mut gap) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..100u16 {
            let mut msg = q.clone();
            dns::set_id(&mut msg, i);
            if let Ok(r) = client.exchange(&msg, Duration::from_millis(500)) {
                rtt.push(r.rtt_ns as f64 / 1000.0);
                user.push(r.user_rtt_ns as f64 / 1000.0);
                if r.kernel_stamp {
                    gap.push(r.user_rtt_ns.saturating_sub(r.rtt_ns) as f64 / 1000.0);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        println!("clock            {}", clock::SOURCE);
        let stamps = match (socket_rx, adapter_rx) {
            (true, true) => "enabled, the adapter stamps",
            (true, false) => "enabled, but the adapter does not stamp (userspace fallback)",
            (false, _) => "no (userspace fallback)",
        };
        println!("kernel rx stamps {stamps}");
        println!("replies          {} of 100, {} kernel-stamped", rtt.len(), gap.len());
        println!("rtt              p50 {:.1} µs   p99 {:.1} µs", pct(&mut rtt, 0.5), pct(&mut rtt, 0.99));
        println!("userspace rtt    p50 {:.1} µs   p99 {:.1} µs", pct(&mut user, 0.5), pct(&mut user, 0.99));
        println!("wake-up removed  p50 {:.1} µs   p99 {:.1} µs", pct(&mut gap, 0.5), pct(&mut gap, 0.99));
        assert!(!rtt.is_empty(), "no answer from 1.1.1.1");
        if socket_rx && adapter_rx {
            assert!(!gap.is_empty(), "stamps are on, yet no answer carried one");
        }
    }
}
