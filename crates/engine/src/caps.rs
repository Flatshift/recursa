//! What each build of Recursa can do. The host picks its row when it opens the engine, the UI reads
//! it from `/api/bootstrap` as `caps`, and `docs/PLATFORMS.md` is generated from the same table, so a
//! feature that differs by platform is decided here once. Every field is required: a new one has to
//! be filled in for every build before anything compiles.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Caps {
    /// Save a run's samples as CSV or the whole run as JSON.
    pub export: bool,
    /// Settings opens in its own window.
    pub settings_window: bool,
    /// Right-click and "…" menus are native menus.
    pub native_menus: bool,
    /// An app menu bar (File › Clear All Runs and the rest).
    pub app_menu: bool,
    /// Open at login, into the menu bar or tray, monitoring with no window open.
    pub login_item: bool,
    /// Trace the route to a resolver with ICMP probes (Trippy).
    pub trace_route: bool,
}

/// One row per build, in the order `docs/PLATFORMS.md` shows them.
#[cfg(test)]
const BUILDS: [(&str, Caps); 4] = [("macOS", Caps::MAC), ("Windows", Caps::WINDOWS), ("iOS", Caps::IOS), ("Browser (CLI)", Caps::BROWSER)];

/// What each field means, for `docs/PLATFORMS.md`.
#[cfg(test)]
type Feature = (&'static str, fn(&Caps) -> bool);
#[cfg(test)]
const FEATURES: [Feature; 6] = [
    ("Export runs as CSV or JSON", |c| c.export),
    ("Settings in its own window", |c| c.settings_window),
    ("Native menus", |c| c.native_menus),
    ("App menu bar", |c| c.app_menu),
    ("Open at login, monitor from the menu bar or tray", |c| c.login_item),
    ("Trace the route to a resolver", |c| c.trace_route),
];

impl Caps {
    pub const MAC: Caps =
        Caps { export: true, settings_window: true, native_menus: true, app_menu: true, login_item: true, trace_route: true };
    pub const WINDOWS: Caps = Caps::MAC;
    pub const IOS: Caps =
        Caps { export: false, settings_window: false, native_menus: false, app_menu: false, login_item: false, trace_route: false };
    pub const BROWSER: Caps =
        Caps { export: true, settings_window: false, native_menus: false, app_menu: false, login_item: false, trace_route: true };

    /// `docs/PLATFORMS.md`: every feature against every build.
    #[cfg(test)]
    fn table() -> String {
        let mut s = String::from(
            "# Platforms\n\nWhat each build of Recursa can do. Generated from `crates/engine/src/caps.rs` by\n`cargo test -p recursa-engine caps`; edit that file, not this one.\n\n| Feature |",
        );
        for (name, _) in BUILDS {
            s += &format!(" {name} |");
        }
        s += "\n|---|";
        s += &"---|".repeat(BUILDS.len());
        for (label, has) in FEATURES {
            s += &format!("\n| {label} |");
            for (_, c) in BUILDS {
                s += if has(&c) { " yes |" } else { " no |" };
            }
        }
        s + "\n\nTrace route needs Administrator on Windows and CAP_NET_RAW or root on Linux; macOS runs it as the user.\n"
    }
}

#[cfg(test)]
mod tests {
    /// The checked-in `docs/PLATFORMS.md` matches the table. `RECURSA_WRITE_PLATFORMS=1` rewrites it.
    #[test]
    fn platforms_doc_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/PLATFORMS.md");
        let want = super::Caps::table();
        if std::env::var_os("RECURSA_WRITE_PLATFORMS").is_some() {
            std::fs::write(path, &want).unwrap();
        }
        // A Windows checkout may turn the line endings into CRLF.
        let have = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "docs/PLATFORMS.md is stale: run RECURSA_WRITE_PLATFORMS=1 cargo test -p recursa-engine caps");
    }
}
