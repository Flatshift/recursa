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
pub(crate) mod catalog;
pub(crate) mod cdn;
pub(crate) mod clock;
pub(crate) mod cymru;
pub(crate) mod deepdive;
pub(crate) mod dns;
pub(crate) mod endpoints;
pub(crate) mod forwarder;
pub(crate) mod intel;
pub mod intercept;
pub(crate) mod link;
pub(crate) mod lock;
pub(crate) mod monitor;
pub mod names;
pub mod net;
pub mod ntp;
pub(crate) mod ntp_monitor;
pub mod ntp_report;
pub(crate) mod probes;
pub(crate) mod regions;
pub mod resolve;
#[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
pub mod rpki;
pub(crate) mod stats;
pub(crate) mod store;
pub mod sysdns;
pub mod trace;

/// Install the process-wide TLS crypto provider. Call once at startup, before any DoT,
/// DoH or HTTPS traffic.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Bytes from the operating system's secure random source, for values that only work if an attacker can't guess them:
/// DNS query IDs, the random bits of an NTP origin timestamp, NTS unique identifiers and nonces, DNS client cookies,
/// the CLI's launch codes and session tokens.
pub fn unpredictable<const N: usize>() -> [u8; N] {
    // Kept once: building the whole provider each time allocates its cipher-suite and key-exchange lists.
    static RANDOM: std::sync::LazyLock<&'static dyn rustls::crypto::SecureRandom> =
        std::sync::LazyLock::new(|| rustls::crypto::ring::default_provider().secure_random);
    let mut b = [0u8; N];
    RANDOM.fill(&mut b).expect("the system random source");
    b
}
