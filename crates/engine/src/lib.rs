//! Recursa's engine: a recursive DNS resolver benchmark that measures from the client.
//!
//! Both front ends sit on [`app::AppState`]: the CLI (terminal commands plus the browser UI
//! over a local HTTP server) and the desktop app (native window over IPC).

/// Which build this is: the git commit (`+dirty` when the tree had uncommitted changes) and when it was built.
pub const BUILD: &str = env!("RECURSA_BUILD");

pub mod app;
pub mod bench;
pub mod calibrate;
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

/// Install the process-wide TLS crypto provider. Call once at startup, before any DoT,
/// DoH or HTTPS traffic.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
