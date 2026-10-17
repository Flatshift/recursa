//! Recursa command line: terminal benchmarks, plus the browser UI over a local server.

mod server;

use clap::{Parser, Subcommand, ValueEnum};
use recursa_engine::app::AppState;
use recursa_engine::{bench, calibrate, intercept, net};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

/// `--version` (long form): the copyright and license notice the GPL asks a program to show.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\nCopyright (C) 2026 Brian Morisette",
    "\nLicense GPLv3+: GNU GPL version 3 or later <https://gnu.org/licenses/gpl.html>.",
    "\nThis is free software: you are free to change and redistribute it.",
    "\nThere is NO WARRANTY, to the extent permitted by law."
);

#[derive(Parser)]
#[command(name = "recursa", version, long_version = LONG_VERSION, about = "Benchmark recursive DNS resolvers from this computer.")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Port for the local UI (falls back to a free port if taken).
    #[arg(long, default_value_t = 7353)]
    port: u16,
    /// Don't open a browser window.
    #[arg(long)]
    no_open: bool,
    /// Database path (defaults to the OS application-data directory).
    #[arg(long, global = true)]
    db: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum NtpFormat {
    Table,
    Json,
    Csv,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the local UI (default).
    Serve,
    /// Run a benchmark in the terminal.
    Run {
        /// Comma-separated resolver ids (see `recursa list`). Defaults to the standard set.
        #[arg(long, value_delimiter = ',')]
        resolvers: Vec<String>,
        /// Comma-separated transports: udp,tcp,dot,doh
        #[arg(long, value_delimiter = ',', default_value = "udp")]
        transports: Vec<String>,
        /// Queries per test per target.
        #[arg(long, default_value_t = 30)]
        samples: u32,
        /// Up to 8 queries in flight to different resolvers.
        #[arg(long)]
        quick: bool,
        /// Skip capability probes and CDN mapping.
        #[arg(long)]
        latency_only: bool,
        /// Print the full result as JSON.
        #[arg(long)]
        json: bool,
        /// Comma-separated names for the Cached test instead of the default sites.
        #[arg(long, value_delimiter = ',')]
        cached_names: Vec<String>,
        /// A file of names for the Cached test (one per line, or comma-separated; # starts a comment).
        #[arg(long)]
        cached_names_file: Option<std::path::PathBuf>,
    },
    /// Measure how accurate and how fast public NTP servers are from here.
    Ntp {
        /// Comma-separated server ids or addresses (see data/ntp.json): `host`, `host:port` or `[v6]:port`. Defaults to a
        /// short list. A host that isn't in the catalog is yours to test: it is asked every 2 seconds, not 8, and never votes.
        #[arg(long, value_delimiter = ',')]
        servers: Vec<String>,
        /// Measure every server in the catalog.
        #[arg(long)]
        all: bool,
        /// Queries per address (at most 8), eight seconds apart.
        #[arg(long, default_value_t = 4)]
        samples: u32,
        /// Skip the NTS key-exchange check.
        #[arg(long)]
        no_nts: bool,
        /// Output: table (default), json (with a schema_version) or csv (one row per server).
        #[arg(long, value_enum)]
        format: Option<NtpFormat>,
        /// Act as a monitoring check: print one line with performance data and exit 0 OK, 1 WARNING, 2 CRITICAL or
        /// 3 UNKNOWN. Judges how far the clock is off beyond its own measurement error, not the raw offset.
        #[arg(long)]
        plugin: bool,
        /// With --plugin: WARNING when a server is off by more than this many ms beyond its error bound.
        #[arg(long, short = 'w', default_value_t = 50.0)]
        warning: f64,
        /// With --plugin: CRITICAL beyond this many ms.
        #[arg(long, short = 'c', default_value_t = 200.0)]
        critical: f64,
    },
    /// Measure the tool's own timing floor.
    Calibrate,
    /// Check whether plain DNS (port 53) reaches resolvers or is answered locally.
    Check,
    /// List known resolvers.
    List,
}

fn open_state(cli: &Cli, calibrate: bool) -> Arc<AppState> {
    AppState::open(cli.db.clone(), calibrate).unwrap_or_else(|e| {
        eprintln!("Couldn't open the database {e}");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() {
    #[cfg(feature = "rpki-local")]
    recursa_engine::rpki::child_entry();
    recursa_engine::install_crypto_provider();
    let cli = Cli::parse();
    match cli.cmd {
        None | Some(Cmd::Serve) => serve(cli).await,
        Some(Cmd::Calibrate) => {
            let c = tokio::task::spawn_blocking(|| calibrate::run(1000)).await.unwrap().expect("calibration");
            println!("clock            {}", c.clock);
            let stamps = match (c.kernel_rx, c.stamped) {
                (false, _) if cfg!(windows) => {
                    "no (userspace fallback: needs Windows 10 2004 or later and the adapter's Software Timestamp property set to RxAll)"
                        .to_string()
                }
                (false, _) => "no (userspace fallback)".to_string(),
                (true, 0) => "enabled, none on loopback (the stack stamps at the adapter)".to_string(),
                (true, n) if n == c.samples => "yes".to_string(),
                (true, n) => format!("yes, on {n} of {} answers", c.samples),
            };
            println!("kernel rx stamps {stamps}");
            println!("loopback floor   p50 {:.1} µs   p99 {:.1} µs   ({} samples)", c.floor_p50_us, c.floor_p99_us, c.samples);
            if c.stamped > 0 {
                println!("wake-up removed  p50 {:.1} µs   p99 {:.1} µs", c.wake_p50_us, c.wake_p99_us);
            }
        }
        Some(Cmd::Check) => {
            let v = tokio::task::spawn_blocking(intercept::check).await.unwrap();
            if let Some(l) = v["link"].as_object() {
                let kind = if l["wifi"] == true { "Wi-Fi" } else { "wired/other" };
                println!("link     {} ({kind})", l["interface"].as_str().unwrap_or("?"));
            }
            for r in v["results"].as_array().into_iter().flatten() {
                let f = |k: &str| r[k].as_f64().map(|x| format!("{x:.1} ms")).unwrap_or_else(|| "—".into());
                let hops = r["hops"].as_u64().map(|h| h.to_string()).unwrap_or_else(|| "?".into());
                let flag = if r["intercepted"] == true { "  ⚠ answered locally" } else { "" };
                println!(
                    "{:<10} path {:>9}  udp {:>9}  hops {:>2}{flag}",
                    r["name"].as_str().unwrap_or(""),
                    f("path_ms"),
                    f("udp_ms"),
                    hops
                );
            }
            println!("verdict  {}", v["verdict"].as_str().unwrap_or("unknown"));
            if let Some(s) = v["fingerprint"]["summary"].as_str() {
                println!("who      {s}");
            }
        }
        Some(Cmd::Ntp { ref servers, all, samples, no_nts, format, plugin, warning, critical }) => {
            let cat = recursa_engine::ntp::catalog();
            let entries: Vec<recursa_engine::ntp::Entry> = if all {
                cat
            } else if servers.is_empty() {
                cat.into_iter().filter(|e| e.default).collect()
            } else {
                let mut out: Vec<recursa_engine::ntp::Entry> = Vec::new();
                for s in servers {
                    let e = match cat.iter().find(|e| e.id == *s) {
                        Some(e) => e.clone(),
                        None => recursa_engine::ntp::custom_entry(s).unwrap_or_else(|m| {
                            eprintln!("{m}");
                            std::process::exit(if plugin { 3 } else { 2 });
                        }),
                    };
                    if !out.iter().any(|x| x.id == e.id) {
                        out.push(e);
                    }
                }
                out
            };
            // A run of only your own servers has nothing to be judged against, unless it is a monitor check.
            let (entries, added) = if plugin { (entries, Vec::new()) } else { recursa_engine::ntp::with_references(entries) };
            if !added.is_empty() {
                eprintln!("Added {} to judge it against.", added.join(", "));
            }
            let plan = recursa_engine::ntp::Plan { samples: samples.clamp(1, recursa_engine::ntp::MAX_SAMPLES), ..Default::default() };
            // The same rules as the app: servers asked in the last ten minutes, or that said stop, are left alone.
            let app = open_state(&cli, false);
            let (entries, skipped) = app.ntp_guard(&entries);
            if !plugin {
                for s in &skipped {
                    let mins = (s.until - recursa_engine::bench::now_ms()).max(0) / 60_000 + 1;
                    let why = if s.why == "kiss" {
                        format!("it told us to stop ({})", s.kiss.clone().unwrap_or_default())
                    } else {
                        "asked a few minutes ago".to_string()
                    };
                    eprintln!("Leaving {} alone: {why} (about {mins} more minute{}).", s.host, if mins == 1 { "" } else { "s" });
                }
            }
            if entries.is_empty() {
                if plugin {
                    let c = recursa_engine::ntp_report::check(&[], &skipped, warning, critical);
                    println!("{}", c.line);
                    std::process::exit(c.state.into());
                }
                eprintln!("Nothing to ask: to be gentle, every server picked is being left alone for now.");
                std::process::exit(1);
            }
            if !plugin {
                let spacing = if entries.iter().all(|e| e.kind == "custom") {
                    format!("{}", recursa_engine::ntp::CUSTOM_SPACING.as_secs())
                } else {
                    format!("{}", recursa_engine::ntp::MIN_SPACING.as_secs())
                };
                eprintln!(
                    "Asking {} servers, {} queries each, {} seconds apart{}, at most {} packets in all…",
                    entries.len(),
                    plan.samples,
                    spacing,
                    if entries.iter().any(|e| e.kind == "custom") && spacing != "2" { " (yours: 2)" } else { "" },
                    recursa_engine::ntp::most_packets(entries.len(), plan.samples)
                );
            }
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let (results, consensus) =
                tokio::task::spawn_blocking(move || recursa_engine::ntp::run(&entries, &plan, !no_nts, &cancel, &|_| {})).await.unwrap();
            app.ntp_finished(&results);
            if plugin {
                let c = recursa_engine::ntp_report::check(&results, &skipped, warning, critical);
                println!("{}", c.line);
                std::process::exit(c.state.into());
            }
            match format.unwrap_or(NtpFormat::Table) {
                NtpFormat::Json => println!("{}", recursa_engine::ntp_report::to_json(&results, consensus.as_ref())),
                NtpFormat::Csv => print!("{}", recursa_engine::ntp_report::to_csv(&results)),
                NtpFormat::Table => print_ntp(&results, consensus.as_ref()),
            }
        }
        Some(Cmd::List) => {
            for r in open_state(&cli, false).resolvers() {
                println!("{:<24} {:<18} {:<26} {}", r.id, r.provider, r.variant, r.ipv4.first().cloned().unwrap_or_default());
            }
        }
        Some(Cmd::Run { ref resolvers, ref transports, samples, quick, latency_only, json, ref cached_names, ref cached_names_file }) => {
            // Bad arguments fail before anything touches the database or the network.
            let mut parsed: Vec<net::Transport> = Vec::new();
            for t in transports.iter().map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()) {
                match serde_json::from_value(serde_json::Value::String(t.clone())) {
                    Ok(x) => parsed.push(x),
                    Err(_) => {
                        eprintln!("Unknown transport \"{t}\". Use udp, tcp, dot or doh.");
                        std::process::exit(2);
                    }
                }
            }
            let mut names: Vec<String> = cached_names.clone();
            if let Some(path) = cached_names_file {
                match std::fs::read_to_string(path) {
                    Ok(text) => names.extend(recursa_engine::names::parse_list(&text).0),
                    Err(e) => {
                        eprintln!("Couldn't read {}: {e}", path.display());
                        std::process::exit(2);
                    }
                }
            }
            if !names.is_empty() {
                match recursa_engine::names::validate(names) {
                    Ok(v) => names = v,
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(2);
                    }
                }
            }
            let app = open_state(&cli, true);
            tokio::join!(recursa_engine::app::detect_vantage(app.clone()), recursa_engine::app::detect_interception(app.clone()));
            let ids = if resolvers.is_empty() {
                app.resolvers().iter().filter(|r| r.default).map(|r| r.id.clone()).collect()
            } else {
                resolvers.clone()
            };
            let mut cfg = bench::RunConfig::with_resolvers(ids);
            if !parsed.is_empty() {
                cfg.transports = parsed;
            }
            cfg.samples = samples;
            cfg.mode = if quick { bench::Mode::Quick } else { bench::Mode::Accurate };
            cfg.probes = !latency_only;
            cfg.cdn = !latency_only;
            if !names.is_empty() {
                cfg.cached_names = names;
            }
            let h = match app.launch(cfg).await {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("{}", e.error);
                    std::process::exit(2);
                }
            };
            let (mut rx, _) = h.subscribe();
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let v: serde_json::Value = serde_json::from_str(&ev).unwrap_or_default();
                        match v["type"].as_str() {
                            Some("phase") if !json => eprintln!("· {}", v["phase"].as_str().unwrap_or("")),
                            Some("done") => break,
                            _ => {}
                        }
                    }
                    // Missing a few progress events is fine; only the end matters.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
            let s = h.state().clone();
            if json {
                println!("{}", serde_json::to_string_pretty(&s).unwrap());
            } else {
                print_table(&s);
            }
        }
    }
}

/// The host, with its port when it isn't 123.
fn shown(r: &recursa_engine::ntp::ServerResult) -> String {
    match r.port {
        Some(p) if r.host.contains(':') => format!("[{}]:{p}", r.host),
        Some(p) => format!("{}:{p}", r.host),
        None => r.host.clone(),
    }
}

/// What the NTS check found, in a few words: verified, or the named reason it wasn't.
fn nts_summary(k: &recursa_engine::ntp::NtsKe) -> String {
    let kind = k.timed.as_ref().and_then(|t| t.error_kind.clone()).or_else(|| k.error_kind.clone()).unwrap_or_default();
    match (k.ok, k.authenticated) {
        (true, Some(true)) => "works, authenticated time reply verified".to_string(),
        (true, Some(false)) => format!("key exchange works, but the authenticated reply failed [{kind}]"),
        (true, None) => "key exchange works".to_string(),
        (false, _) => format!("{} [{kind}]", k.error.clone().unwrap_or_default()),
    }
}

/// " (3 independent)" when some voters share an operator or an upstream clock.
fn independent(c: &recursa_engine::ntp::Consensus) -> String {
    if c.independent < c.voters { format!(" ({} independent)", c.independent) } else { String::new() }
}

fn print_ntp(results: &[recursa_engine::ntp::ServerResult], consensus: Option<&recursa_engine::ntp::Consensus>) {
    let f = |v: Option<f64>, d: usize| v.map(|x| format!("{x:.d$}")).unwrap_or_else(|| "—".into());
    println!(
        "\n{:<26} {:>3} {:<8} {:>9} {:>8} {:>8} {:>8} {:>7} {:>6} {:<5} {:<12}",
        "server", "st", "ref", "offset ms", "± ms", "err ≤ ms", "delay ms", "jitter", "ok", "NTS", "IPv6"
    );
    let mut sorted: Vec<&recursa_engine::ntp::ServerResult> = results.iter().collect();
    sorted.sort_by(|a, b| a.deviation_ms.map(f64::abs).unwrap_or(f64::MAX).total_cmp(&b.deviation_ms.map(f64::abs).unwrap_or(f64::MAX)));
    for r in sorted {
        // "auth" is a verified NTS time reply, "yes" only a key exchange that worked, "no" a refusal or a failure.
        let nts = match &r.nts {
            Some(k) if k.authenticated == Some(true) => "auth".to_string(),
            Some(k) if k.ok => "yes".to_string(),
            Some(_) => "no".to_string(),
            None => "—".to_string(),
        };
        if r.nts_only {
            println!("{:<26} {:>3} {:<8} NTS key exchange only: {}", shown(r), "", "", r.nts.as_ref().map(nts_summary).unwrap_or_default());
            continue;
        }
        if r.ok == 0 {
            println!(
                "{:<26} {:>3} {:<8} {}",
                shown(r),
                "",
                "",
                r.kiss
                    .as_ref()
                    .map(|k| format!("told us to stop ({k})"))
                    .or_else(|| r.error.as_ref().map(|e| format!("{e} [{}]", r.error_kind.clone().unwrap_or_default())))
                    .unwrap_or_default()
            );
            continue;
        }
        println!(
            "{:<26} {:>3} {:<8} {:>9} {:>8} {:>8} {:>8} {:>7} {:>3}/{:<2} {:<5} {:<12}{}",
            shown(r),
            r.stratum.map(|s| s.to_string()).unwrap_or_default(),
            r.refid.clone().unwrap_or_default().chars().take(8).collect::<String>(),
            f(r.offset_ms, 2),
            f(r.bound_ms, 1),
            f(r.distance_ms, 1),
            f(r.delay_min_ms, 1),
            f(r.jitter_ms, 2),
            r.ok,
            r.sent,
            nts,
            r.ipv6.clone().unwrap_or_default(),
            match (r.selection, r.addrs_disagree) {
                (Some(recursa_engine::ntp::Selection::Falseticker), _) => "  ✗ falseticker",
                (_, true) => "  ⚠ its addresses disagree",
                _ if r.custom => "  · yours, doesn't vote",
                _ => "",
            }
        );
    }
    match consensus {
        Some(c) if c.median_ms.abs() <= c.bound_ms => println!(
            "\nConsensus of {} servers{}: your clock is within the noise of them (± {:.1} ms, the surest server's error ceiling; the middle half agree within {:.2} ms).",
            c.voters,
            independent(c),
            c.bound_ms,
            c.spread_ms
        ),
        Some(c) => println!(
            "\nConsensus of {} servers{}: your clock is {:.1} ms {} them (± {:.1} ms; the middle half agree within {:.2} ms).",
            c.voters,
            independent(c),
            c.median_ms.abs(),
            if c.median_ms >= 0.0 { "behind" } else { "ahead of" },
            c.bound_ms,
            c.spread_ms
        ),
        None => {
            println!(
                "\nFewer than {} independent servers (different operators and upstream clocks) were close enough to judge, so no verdict on your clock.",
                recursa_engine::ntp::MIN_VOTERS
            )
        }
    }
    if let Some(c) = consensus {
        fn host_of<'a>(results: &'a [recursa_engine::ntp::ServerResult], id: &'a str) -> &'a str {
            results.iter().find(|r| r.id == id).map_or(id, |r| r.host.as_str())
        }
        if c.advice.set.is_empty() {
            println!("No server qualified to recommend: none was a synchronized, agreeing server with its own operator and upstream.");
        } else {
            let set: Vec<&str> = c.advice.set.iter().map(|id| host_of(results, id)).collect();
            println!("To configure together: {}.", set.join(", "));
        }
        for a in &c.advice.avoid {
            println!("Leave out {}: {}.", host_of(results, &a.id), a.why);
        }
    }
}

fn print_table(s: &bench::RunState) {
    println!("\n{:<34} {:>5} {:>9} {:>9} {:>9} {:>8} {:>8}", "resolver", "via", "cached", "uncached", "tld", "cdn", "answered");
    let f = |t: &bench::TargetInfo, k: &str| {
        t.stats.get(k).filter(|s| s.p50.is_finite()).map(|s| format!("{:.1} ms", s.p50)).unwrap_or_else(|| "—".into())
    };
    for r in bench::ranked(s) {
        let t = r.target;
        let name = format!("{} {}", t.provider, t.variant);
        if t.state != "ready" {
            println!("{:<34} {:>5}  {}", name, t.transport.label(), t.reason.clone().unwrap_or_else(|| t.state.clone()));
            continue;
        }
        let cdn = match bench::cdn_penalty(s, t) {
            Some(p) if p < 0.05 => "best".to_string(),
            Some(p) => format!("+{p:.1}"),
            None => "—".into(),
        };
        let flag = if t.intercepted {
            "  ⚠ answered locally"
        } else if bench::is_baseline(s, t) {
            "  · your own resolver: a baseline, never the pick"
        } else {
            ""
        };
        println!(
            "{:<34} {:>5} {:>9} {:>9} {:>9} {:>8} {:>7.1}%{flag}",
            name,
            t.transport.label(),
            f(t, "cached"),
            f(t, "uncached"),
            f(t, "tld"),
            cdn,
            r.success * 100.0
        );
    }
    println!("\nRanked by speed score: cached, uncached and TLD lookups weighted 40/35/25, each median pulled toward its p95.");
    if s.cdn.is_some() {
        println!(
            "cdn: ms per round trip to the CDN edges the resolver chose, vs the nearest any resolver found. It doesn't move the order."
        );
    }
    for n in &s.notes {
        println!("\n[{}] {}", n.level, n.text);
    }
}

/// The browser UI's per-launch API token: 32 bytes from the TLS provider's secure random
/// source (a guessable token would let any local page with the port drive the API).
fn session_token() -> String {
    let mut buf = [0u8; 32];
    rustls::crypto::CryptoProvider::get_default()
        .expect("crypto provider installed at startup")
        .secure_random
        .fill(&mut buf)
        .expect("secure random");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

async fn serve(cli: Cli) {
    let preferred = SocketAddr::from(([127, 0, 0, 1], cli.port));
    let listener = match tokio::net::TcpListener::bind(preferred).await {
        Ok(l) => l,
        Err(_) => tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.expect("bind loopback"),
    };
    let port = listener.local_addr().unwrap().port();
    let app = open_state(&cli, true);
    app.start_background();
    let url = format!("http://127.0.0.1:{port}/");
    eprintln!("Recursa is running at {url}");
    eprintln!("Data: {}", app.store.path.display());
    if let Some(c) = app.calibration.read().unwrap().as_ref() {
        eprintln!(
            "Timing: {} · kernel rx stamps {} · loopback floor {:.1} µs",
            c.clock,
            if c.kernel_rx { "on" } else { "off" },
            c.floor_p50_us
        );
    }
    if !cli.no_open {
        let _ = open::that(&url);
    }
    let token = session_token();
    let srv = Arc::new(server::Server { app, token, port });
    // Live event streams never end on their own, so Ctrl-C exits directly rather than
    // waiting for them to drain. SQLite in WAL mode is safe to stop at any point.
    tokio::select! {
        r = axum::serve(listener, server::router(srv)) => r.expect("server"),
        _ = tokio::signal::ctrl_c() => eprintln!("\nStopped."),
    }
}
