//! Recursa desktop: the engine in-process, the Svelte UI in the system WebView, and a
//! menu-bar item that keeps monitoring alive while the window is closed. The app itself is in `lib.rs`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(all(feature = "rpki-local", not(target_os = "ios")))]
    recursa_engine::rpki::child_entry();
    recursa_desktop_lib::run()
}
