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

pub const MAIN: &str = "main";
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
            &PredefinedMenuItem::quit(app, Some("Quit Recursa"))?,
        ],
    )?;
    TrayIconBuilder::with_id("recursa")
        .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Recursa")
        .menu(&tray_menu)
        .show_menu_on_left_click(true)
        .build(app)?;
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
        comments: Some("Benchmark recursive DNS resolvers from this computer.".into()),
        copyright: Some("Copyright © 2026 Brian Morisette".into()),
        // macOS shows credits under the copyright line.
        credits: Some("Free software under the GNU General Public License, version 3 or later, with no warranty. License and third-party notices: Settings › About.".into()),
        license: Some("GPL-3.0-or-later".into()),
        ..Default::default()
    };
    let item = |id: &str, text: &str, accel: Option<&str>| MenuItem::with_id(app, id, text, true, accel);
    // The view and panel shortcuts, in the terms keys.ts labels them: ⌥⌘ and ⌃⌘ on a Mac; off a Mac Ctrl+Alt is
    // AltGr on many layouts, so the views take Ctrl+Shift and the sidebar Ctrl+Alt.
    let (view_mod, sidebar) =
        if cfg!(target_os = "macos") { ("Alt+CmdOrCtrl", "Ctrl+CmdOrCtrl+S") } else { ("CmdOrCtrl+Shift", "CmdOrCtrl+Alt+S") };
    let view_accel = |key: &str| format!("{view_mod}+{key}");
    let recursa = SubmenuBuilder::new(app, "Recursa")
        .about(Some(about))
        .separator()
        .item(&item("settings", "Settings…", Some("CmdOrCtrl+,"))?)
        .item(login)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("go:/", "New Benchmark", Some("CmdOrCtrl+N"))?)
        .item(&item("run:start", "Start Benchmark", Some("CmdOrCtrl+Enter"))?)
        .item(&item("run:stop", "Stop Benchmark", Some("CmdOrCtrl+."))?)
        .separator()
        .item(&item("export:csv", "Export Samples as CSV…", Some("CmdOrCtrl+Shift+E"))?)
        .item(&item("export:json", "Export Run as JSON…", None)?)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .separator()
        .item(&item("find", "Filter…", Some("CmdOrCtrl+F"))?)
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item("go:/", "Benchmark", Some("CmdOrCtrl+1"))?)
        .item(&item("go:/resolvers", "Resolvers", Some("CmdOrCtrl+2"))?)
        .item(&item("go:/reliability", "Reliability", Some("CmdOrCtrl+3"))?)
        .item(&item("go:/method", "Method", Some("CmdOrCtrl+4"))?)
        .item(&item("go:/time", "Time", Some("CmdOrCtrl+5"))?)
        .separator()
        .item(&item("view:overview", "Results Overview", Some(view_accel("1").as_str()))?)
        .item(&item("view:latency", "Latency", Some(view_accel("2").as_str()))?)
        .item(&item("view:cdn", "CDN Mapping", Some(view_accel("3").as_str()))?)
        .item(&item("view:caps", "Capabilities", Some(view_accel("4").as_str()))?)
        .item(&item("view:numbers", "Numbers", Some(view_accel("5").as_str()))?)
        .separator()
        .item(&item("toggle:sidebar", "Toggle Sidebar", Some(sidebar))?)
        .item(&item("toggle:inspector", "Toggle Inspector", Some(view_accel("I").as_str()))?)
        .separator()
        .fullscreen()
        .build()?;
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
        "settings" => show_settings(app),
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

/// The Settings window (⌘,): one at a time, brought forward if already open.
pub fn show_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SETTINGS) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("settings".into()))
        .title("Settings")
        .inner_size(520.0, 400.0)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .on_navigation(is_internal)
        .build();
    match built {
        Ok(w) => {
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
        .on_navigation(|url| {
            let internal = is_internal(url);
            if !internal {
                open_external(url);
            }
            internal
        })
        .on_new_window(|url, _| {
            open_external(&url);
            NewWindowResponse::Deny
        });
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

fn is_internal(url: &Url) -> bool {
    matches!(url.scheme(), "tauri" | "asset") || matches!(url.host_str(), Some("localhost" | "tauri.localhost" | "ipc.localhost"))
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
    let login = app.autolaunch().is_enabled().unwrap_or(false);
    for item in &t.login {
        let _ = item.set_checked(login);
    }
}
