//! Native chrome: the main window, the app menu, the menu-bar item and launch at login.
//!
//! Closing the window destroys its WebView but keeps the process (and so monitoring)
//! alive in the menu bar. Reopening builds a fresh window; all state lives in the engine.

use recursa_engine::app::AppState;
use std::sync::Arc;
use tauri::menu::{AboutMetadata, CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::MAIN;
pub const SETTINGS: &str = "settings";

/// Menu items whose state mirrors the engine or the system.
struct Toggles {
    monitor: CheckMenuItem<Wry>,
    login: Vec<CheckMenuItem<Wry>>,
}

/// Build the menus and the menu-bar item, then open the window unless launched hidden.
pub fn install(app: &AppHandle, hidden: bool) -> tauri::Result<()> {
    let login_app = CheckMenuItem::with_id(app, "login", "Open at Login", true, false, None::<&str>)?;
    let login_tray = CheckMenuItem::with_id(app, "login", "Open at Login", true, false, None::<&str>)?;
    let monitor = CheckMenuItem::with_id(app, "monitor", "Monitoring", true, false, None::<&str>)?;

    app.set_menu(app_menu(app, &login_app)?)?;
    let tray_menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "open", "Open Recursa", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &monitor,
            &login_tray,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some(if cfg!(target_os = "macos") { "Quit Recursa" } else { "Exit" }))?,
        ],
    )?;
    // macOS: a template glyph in the menu bar, and the menu drops on any click. Elsewhere the taskbar may be dark or
    // light, so the tray shows the colour icon; a left click opens the window and a right click the menu.
    let tray = TrayIconBuilder::with_id("recursa").tooltip("Recursa").menu(&tray_menu).show_menu_on_left_click(cfg!(target_os = "macos"));
    #[cfg(target_os = "macos")]
    let tray = tray.icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?).icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let tray = tray.icon(tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?).on_tray_icon_event(|tray, event| {
        use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
            show_main(tray.app_handle(), None);
        }
    });
    tray.build(app)?;
    app.manage(Toggles { monitor, login: vec![login_app, login_tray] });
    sync_menus(app);
    // Keep the menus and the UI in step with monitoring, whoever changed it.
    let mut changes = app.state::<Arc<AppState>>().monitor.subscribe();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while changes.changed().await.is_ok() {
            sync_menus(&handle);
            // Both the main window and Settings show the monitoring state.
            let _ = handle.emit("recursa:monitor", ());
        }
    });
    app.on_menu_event(|app, event| on_menu(app, event.id().as_ref()));

    if hidden {
        set_dock(app, false);
    } else {
        show_main(app, None);
    }
    Ok(())
}

fn app_menu(app: &AppHandle, login: &CheckMenuItem<Wry>) -> tauri::Result<Menu<Wry>> {
    let about = AboutMetadata {
        name: Some("Recursa".into()),
        version: Some(format!("{} (build {})", env!("CARGO_PKG_VERSION"), recursa_engine::BUILD)),
        comments: Some("Find the fastest, best-placed DNS resolver for your network.".into()),
        copyright: Some("Copyright © 2026 Brian Morisette".into()),
        // macOS shows credits under the copyright line, and no comments, so the tagline goes here too.
        credits: Some("Find the fastest, best-placed DNS resolver for your network.\n\nFree software under the GNU General Public License, version 3 or later, with no warranty. License and third-party notices: Settings › About.".into()),
        license: Some("GPL-3.0-or-later".into()),
        ..Default::default()
    };
    let item = |id: &str, text: &str, accel: Option<&str>| MenuItem::with_id(app, id, text, true, accel);
    // The view and panel shortcuts, in the terms keys.ts labels them: ⌥⌘ and ⌃⌘ on a Mac; off a Mac Ctrl+Alt is
    // AltGr on many layouts, so the views take Ctrl+Shift and the sidebar Ctrl+Alt.
    let (view_mod, sidebar) =
        if cfg!(target_os = "macos") { ("Alt+CmdOrCtrl", "Ctrl+CmdOrCtrl+S") } else { ("CmdOrCtrl+Shift", "CmdOrCtrl+Alt+S") };
    let view_accel = |key: &str| format!("{view_mod}+{key}");
    // ⌘⌫ is the Finder's delete key; Ctrl+Backspace deletes a word in every Windows text field, so no key there.
    let delete_key = if cfg!(target_os = "macos") { Some("CmdOrCtrl+Backspace") } else { None };
    let recursa = SubmenuBuilder::new(app, "Recursa")
        .about(Some(about))
        .separator()
        .item(&item("settings", "Settings…", Some("CmdOrCtrl+,"))?)
        .item(login)
        .separator();
    // The application-menu items only macOS has; on Windows muda would drop all but Hide and leave the separators.
    #[cfg(target_os = "macos")]
    let recursa = recursa.services().separator().hide().hide_others().show_all().separator();
    let recursa = recursa.quit().build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("go:/", "New Benchmark", Some("CmdOrCtrl+N"))?)
        .item(&item("run:start", "Start Benchmark", Some("CmdOrCtrl+Enter"))?)
        .item(&item("run:stop", "Stop Benchmark", Some("CmdOrCtrl+."))?)
        .separator()
        .item(&item("export:csv", "Export Samples as CSV…", Some("CmdOrCtrl+Shift+E"))?)
        .item(&item("export:json", "Export Run as JSON…", None)?)
        .separator()
        .item(&item("run:save", "Save Run", Some("CmdOrCtrl+S"))?)
        .item(&item("run:delete", "Delete Run…", delete_key)?)
        .item(&item("runs:clear", "Clear All Runs…", None)?)
        .separator()
        .close_window()
        .build()?;
    // Clipboard items are macOS menu entries; WebView2 handles those keys itself and muda has no Windows versions.
    let edit = SubmenuBuilder::new(app, "Edit");
    #[cfg(target_os = "macos")]
    let edit = edit.undo().redo().separator().cut().copy().paste().select_all().separator();
    let edit = edit.item(&item("find", "Filter…", Some("CmdOrCtrl+F"))?).build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item("go:/", "Benchmark", Some("CmdOrCtrl+1"))?)
        .item(&item("go:/resolvers", "Resolvers", Some("CmdOrCtrl+2"))?)
        .item(&item("go:/reliability", "Reliability", Some("CmdOrCtrl+3"))?)
        .item(&item("go:/method", "Method", Some("CmdOrCtrl+4"))?)
        .item(&item("go:/time", "Time", Some("CmdOrCtrl+5"))?)
        .item(&item("go:/network", "Network", Some("CmdOrCtrl+6"))?)
        .separator()
        .item(&item("view:overview", "Results Overview", Some(view_accel("1").as_str()))?)
        .item(&item("view:latency", "Latency", Some(view_accel("2").as_str()))?)
        .item(&item("view:path", "Path", Some(view_accel("3").as_str()))?)
        .item(&item("view:cdn", "CDN Mapping", Some(view_accel("4").as_str()))?)
        .item(&item("view:caps", "Capabilities", Some(view_accel("5").as_str()))?)
        .item(&item("view:numbers", "Numbers", Some(view_accel("6").as_str()))?)
        .separator()
        .item(&item("toggle:sidebar", "Toggle Sidebar", Some(sidebar))?)
        .item(&item("toggle:inspector", "Toggle Inspector", Some(view_accel("I").as_str()))?)
        .separator();
    // The predefined Full Screen item acts only on macOS; elsewhere the window toggles it itself.
    #[cfg(target_os = "macos")]
    let view = view.fullscreen();
    #[cfg(not(target_os = "macos"))]
    let view = view.item(&item("fullscreen", "Full Screen", Some("F11"))?);
    let view = view.build()?;
    let window: Submenu<Wry> = SubmenuBuilder::new(app, "Window").minimize().maximize().build()?;
    #[cfg(target_os = "macos")]
    window.set_as_windows_menu_for_nsapp()?;
    Menu::with_items(app, &[&recursa, &file, &edit, &view, &window])
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app, None),
        "monitor" => toggle_monitor(app),
        "login" => toggle_login(app),
        "settings" => show_settings(app, None),
        #[cfg(not(target_os = "macos"))]
        "fullscreen" => {
            if let Some(w) = app.get_webview_window(MAIN) {
                let full = !w.is_fullscreen().unwrap_or(false);
                // The menu bar would stay drawn over a full-screen window; F11 still works while it is hidden.
                let _ = if full { w.hide_menu() } else { w.show_menu() };
                let _ = w.set_fullscreen(full);
            }
        }
        // A pick from a context menu the UI asked for (see ipc::context_menu).
        _ if id.starts_with("ctx:") => {
            let _ = app.emit_to(MAIN, "recursa:context", &id[4..]);
        }
        // Everything else happens in the UI; a fresh window opens on the route itself.
        _ => {
            if app.get_webview_window(MAIN).is_some() {
                show_main(app, None);
                let _ = app.emit_to(MAIN, "recursa:menu", id);
            } else {
                show_main(app, id.strip_prefix("go:"));
            }
        }
    }
}

/// Open the Settings window (⌘,), on `section` (a pane id the page knows) when given: one at a time, brought forward if already open.
pub fn show_settings(app: &AppHandle, section: Option<&str>) {
    // A pane id is a short slug; anything else from IPC is ignored rather than spliced into a URL.
    let section =
        section.filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'));
    if let Some(w) = app.get_webview_window(SETTINGS) {
        if let Some(s) = section {
            let _ = w.eval(format!("location.hash = '{s}'"));
        }
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let url = match section {
        Some(s) => format!("settings#{s}"),
        None => "settings".into(),
    };
    // A sectioned window, sized so no pane scrolls at the default size; it may be resized, not maximized.
    let built = WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App(url.into()))
        .title("Settings")
        .inner_size(720.0, 500.0)
        .min_inner_size(640.0, 440.0)
        .resizable(true)
        .minimizable(false)
        .maximizable(false)
        // Its outside links (the GPL, the RPKI agreement) open in the default browser, as the main window's do.
        .on_navigation(keep_inside(app))
        .on_new_window(new_window_to_browser)
        .build();
    match built {
        Ok(w) => {
            // Off macOS the app menu lands on every new window; a dialog has no use for it.
            #[cfg(not(target_os = "macos"))]
            let _ = w.remove_menu();
            let _ = w.set_focus();
        }
        Err(e) => eprintln!("Couldn't open Settings: {e}"),
    }
}

/// Bring the window forward, creating it (optionally on `route`) if it was closed.
pub fn show_main(app: &AppHandle, route: Option<&str>) {
    set_dock(app, true);
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    match build_main(app, route.unwrap_or("/")) {
        Ok(w) => {
            let _ = w.set_focus();
        }
        Err(e) => eprintln!("Couldn't open the window: {e}"),
    }
}

fn build_main(app: &AppHandle, route: &str) -> tauri::Result<WebviewWindow> {
    let url = WebviewUrl::App(route.trim_start_matches('/').into());
    let b = WebviewWindowBuilder::new(app, MAIN, url)
        .title("Recursa")
        .inner_size(1280.0, 860.0)
        .min_inner_size(720.0, 560.0)
        // Links to the outside world open in the default browser, never in this window.
        .on_navigation(keep_inside(app))
        .on_new_window(new_window_to_browser);
    let b = b.on_page_load(|w, p| {
        if p.event() == tauri::webview::PageLoadEvent::Finished {
            // A new page knows nothing of the window's chrome yet.
            #[cfg(target_os = "macos")]
            crate::chrome::sync(&w, true);
            #[cfg(debug_assertions)]
            if crate::smoke::enabled() {
                crate::smoke::run(&w);
            }
            #[cfg(not(any(target_os = "macos", debug_assertions)))]
            let _ = &w;
        }
    });
    #[cfg(target_os = "macos")]
    let b =
        b.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true).traffic_light_position(tauri::LogicalPosition::new(18.0, 29.0));
    let w = b.build()?;
    #[cfg(target_os = "macos")]
    crate::chrome::install(&w);
    Ok(w)
}

/// A window's navigation rule: the app's own pages stay in it, anything else opens in the default browser.
fn keep_inside(app: &AppHandle) -> impl Fn(&Url) -> bool + Send + 'static {
    let dev = dev_url(app);
    move |url| {
        let internal = is_internal(url, dev.as_ref());
        if !internal {
            open_external(url);
        }
        internal
    }
}

/// A page asking for a new window: its link opens in the default browser instead.
fn new_window_to_browser(url: Url, _: tauri::webview::NewWindowFeatures) -> NewWindowResponse<Wry> {
    open_external(&url);
    NewWindowResponse::Deny
}

/// The Vite dev server (`devUrl` in tauri.conf.json), which serves the pages whenever Tauri does, that is in any
/// build without the `custom-protocol` feature (`tauri dev --release` included), not only debug builds.
fn dev_url(app: &AppHandle) -> Option<Url> {
    app.config().build.dev_url.clone().filter(|_| tauri::is_dev())
}

/// The app's own pages: its custom schemes, the hosts Windows serves them on, and `dev`, the dev server's origin.
/// Any other server on this computer, `localhost` included, opens in the browser, not in a Recursa window.
fn is_internal(url: &Url, dev: Option<&Url>) -> bool {
    match (url.scheme(), url.host_str(), url.port()) {
        ("tauri" | "asset" | "ipc", _, _) | ("http" | "https", Some("tauri.localhost" | "ipc.localhost"), None) => true,
        _ => dev.is_some_and(|d| d.origin() == url.origin()),
    }
}

fn open_external(url: &Url) {
    if matches!(url.scheme(), "http" | "https" | "mailto") {
        let _ = open::that_detached(url.as_str());
    }
}

/// A window is gone. The main window's event streams go with it, and the app leaves the
/// Dock once no window is left (Settings can outlive the main window).
pub fn on_window_destroyed(app: &AppHandle, label: &str) {
    if label == MAIN {
        app.state::<crate::ipc::Streams>().clear();
    }
    let other = if label == MAIN { SETTINGS } else { MAIN };
    if app.get_webview_window(other).is_none() {
        set_dock(app, false);
    }
}

fn set_dock(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        use tauri::ActivationPolicy::{Accessory, Regular};
        let _ = app.set_activation_policy(if visible { Regular } else { Accessory });
        if visible {
            sync_dock_icon(app);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
}

/// The Dock icon follows the system appearance: the Paper tile in light mode, the Slate tile
/// (the bundle's icon) in dark. Finder keeps showing the bundle's icon.
pub fn sync_dock_icon(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.run_on_main_thread(|| {
        use objc2::AnyThread;
        use objc2_app_kit::{NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSImage};
        use objc2_foundation::{NSArray, NSData};
        let Some(mtm) = objc2::MainThreadMarker::new() else { return };
        let ns = NSApplication::sharedApplication(mtm);
        let (light, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        let best = ns.effectiveAppearance().bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[light, dark]));
        let png: &[u8] = if best.is_some_and(|name| &*name == dark) {
            include_bytes!("../icons/icon.png")
        } else {
            include_bytes!("../icons/icon-light.png")
        };
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png)) {
            unsafe { ns.setApplicationIconImage(Some(&image)) };
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

fn toggle_monitor(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let mut cfg = state.monitor.borrow().clone();
    cfg.enabled = !cfg.enabled;
    state.set_monitor(cfg);
}

fn toggle_login(app: &AppHandle) {
    let on = login_enabled(app);
    if let Err(e) = set_login(app, !on) {
        eprintln!("Couldn't change Open at Login: {e}");
    }
}

pub fn login_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Turn Open at Login on or off; returns the state it ended up in.
pub fn set_login(app: &AppHandle, on: bool) -> Result<bool, String> {
    let launcher = app.autolaunch();
    let result = if on { launcher.enable() } else { launcher.disable() };
    sync_menus(app);
    result.map_err(|e| e.to_string())?;
    Ok(login_enabled(app))
}

/// Make the check items match the monitoring config and the login item.
fn sync_menus(app: &AppHandle) {
    let Some(t) = app.try_state::<Toggles>() else { return };
    let cfg = app.state::<Arc<AppState>>().monitor.borrow().clone();
    let _ = t.monitor.set_checked(cfg.enabled);
    let _ = t.monitor.set_text(if cfg.enabled { format!("Monitoring Every {} min", cfg.interval_min) } else { "Monitoring".into() });
    let login = login_enabled(app);
    for item in &t.login {
        let _ = item.set_checked(login);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_apps_own_pages_stay_in_its_windows() {
        let dev = Url::parse("http://localhost:5173").unwrap();
        let internal = |u: &str, dev: Option<&Url>| is_internal(&Url::parse(u).unwrap(), dev);
        assert!(internal("tauri://localhost/runs", None));
        assert!(internal("http://tauri.localhost/settings", None));
        assert!(!internal("http://localhost:8080/", Some(&dev)));
        assert!(!internal("http://tauri.localhost:8080/", None));
        assert!(!internal("https://example.com/", None));
        assert!(internal("http://localhost:5173/runs", Some(&dev)));
        assert!(!internal("http://localhost:5173/runs", None));
    }
}
