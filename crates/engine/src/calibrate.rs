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
use crate::{clock, dns};
use serde::Serialize;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug, Serialize)]
pub struct Calibration {
    pub clock: &'static str,
    pub kernel_rx: bool,
    pub samples: u32,
    pub floor_p50_us: f64,
    pub floor_p99_us: f64,
    pub wake_p50_us: f64,
    pub wake_p99_us: f64,
    pub at: i64,
}

pub fn kernel_stamps_available() -> bool {
    static K: OnceLock<bool> = OnceLock::new();
    *K.get_or_init(|| UdpX::connect("127.0.0.1:9".parse().unwrap()).map(|u| u.kernel_timestamps()).unwrap_or(false))
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * q).round() as usize]
}

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
    let kernel_rx = client.kernel_timestamps();
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
