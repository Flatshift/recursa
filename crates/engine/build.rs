// Stamps the build into the binary as `RECURSA_BUILD` ("d8c0189 · 2026-10-10 03:41 PDT"), so the About screens can show
// which build is running. It reruns when the sources, data, built UI or git state change, which is when a new build starts.
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    for p in ["src", "../../data", "../../web/dist", "../../.git/HEAD", "../../.git/index"] {
        // Only paths that exist: a missing one would make cargo rerun this script on every build.
        if p.starts_with("../../.git") && !std::path::Path::new(p).exists() {
            continue;
        }
        println!("cargo:rerun-if-changed={p}");
    }
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let commit = run("git", &["rev-parse", "--short=7", "HEAD"]).filter(|s| !s.is_empty());
    let dirty = run("git", &["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    // SOURCE_DATE_EPOCH pins the stamp for reproducible builds (BSD date takes -r, GNU date -d @).
    let when = match std::env::var("SOURCE_DATE_EPOCH").ok().filter(|e| e.bytes().all(|b| b.is_ascii_digit()) && !e.is_empty()) {
        Some(e) => run("date", &["-u", "-r", &e, "+%Y-%m-%d %H:%M %Z"])
            .or_else(|| run("date", &["-u", "-d", &format!("@{e}"), "+%Y-%m-%d %H:%M %Z"])),
        None => run("date", &["+%Y-%m-%d %H:%M %Z"]),
    }
    .unwrap_or_default();
    let build = match commit {
        Some(c) => format!("{c}{}", if dirty { "+dirty" } else { "" }),
        None => "unknown".into(),
    };
    let build = if when.is_empty() { build } else { format!("{build} · {when}") };
    println!("cargo:rustc-env=RECURSA_BUILD={build}");
}
