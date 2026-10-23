//! Recursa's engine: a recursive DNS resolver benchmark that measures from the client.
//!
//! Both front ends sit on [`app::AppState`]: the CLI (terminal commands plus the browser UI
//! over a local HTTP server) and the desktop app (native window over IPC).

/// Which build this is: the git commit (`+dirty` when the tree had uncommitted changes) and when it was built.
pub const BUILD: &str = env!("RECURSA_BUILD");

pub mod app;
pub mod bench;
pub mod calibrate;
pub mod caps;
pub mod catalog;
pub mod cdn;
pub mod clock;
pub mod cymru;
pub mod deepdive;
pub mod dns;
pub mod endpoints;
pub mod forwarder;
pub mod intel;
pub mod intercept;
pub mod link;
pub mod lock;
pub mod monitor;
pub mod names;
pub mod net;
pub mod ntp;
pub mod ntp_monitor;
pub mod ntp_report;
pub mod probes;
pub mod regions;
pub mod resolve;
#[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
pub mod rpki;
pub mod stats;
pub mod store;
pub mod sysdns;
pub mod trace;

/// Install the process-wide TLS crypto provider. Call once at startup, before any DoT,
/// DoH or HTTPS traffic.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Bytes from the operating system's secure random source, for values that only work if an attacker can't guess them:
/// DNS query IDs, the random bits of an NTP origin timestamp, NTS unique identifiers and nonces, DNS client cookies.
pub(crate) fn unpredictable<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rustls::crypto::ring::default_provider().secure_random.fill(&mut b).expect("the system random source");
    b
}
