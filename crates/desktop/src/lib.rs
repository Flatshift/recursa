//! Recursa desktop: the engine in-process, the Svelte UI in the system WebView, and a
//! menu-bar item that keeps monitoring alive while the window is closed.

#[cfg(target_os = "macos")]
mod chrome;
mod ipc;
#[cfg(mobile)]
mod mobile;
#[cfg(desktop)]
mod shell;
#[cfg(all(desktop, debug_assertions))]
mod smoke;

use recursa_engine::app::AppState;
use recursa_engine::caps::Caps;
#[cfg(desktop)]
use tauri::RunEvent;
#[cfg(desktop)]
use tauri::{Manager, WindowEvent};
#[cfg(desktop)]
use tauri_plugin_window_state::StateFlags;

#[cfg(target_os = "macos")]
const CAPS: Caps = Caps::MAC;
#[cfg(windows)]
const CAPS: Caps = Caps::WINDOWS;
#[cfg(mobile)]
const CAPS: Caps = Caps::IOS;

/// The database `--db <path>` or `--db=<path>` names, if either is given. A bare `--db` is an error
/// rather than a silent fall-through to the shared database.
fn db_arg(args: &[String]) -> Result<Option<std::path::PathBuf>, String> {
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if let Some(p) = a.strip_prefix("--db=") {
            return Ok(Some(p.into()));
        }
        if a == "--db" {
            return match it.next() {
                Some(p) if !p.starts_with("--") => Ok(Some(p.into())),
                _ => Err("--db needs a path".into()),
            };
        }
    }
    Ok(None)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    recursa_engine::install_crypto_provider();
    let args: Vec<String> = std::env::args().collect();
    // Launch at login starts in the menu bar only.
    #[cfg(desktop)]
    let hidden = args.iter().any(|a| a == "--hidden");
    let context = tauri::generate_context!();
    // Same as the CLI's --db: use another database (the default is the shared one).
    // Calibration runs in the background (start_background), so the window needn't wait.
    let opened = db_arg(&args)
        .map_err(|e| ("Recursa couldn't read its arguments.", e))
        .and_then(|db| AppState::open(db, false, CAPS).map_err(|e| ("Recursa couldn't open its database.", e)));
    let state = match opened {
        Ok(s) => s,
        // iOS has no alert before an event loop runs, so the page says why instead.
        #[cfg(mobile)]
        Err((message, e)) => mobile::fatal(context, message, &e),
        #[cfg(desktop)]
        Err((message, e)) => fail(message, &e),
    };

    #[cfg_attr(mobile, allow(unused_mut))]
    let mut builder = tauri::Builder::default();
    // Must come first: a second launch just brings the running app forward. A smoke run
    // must test its own process, not hand off to an open copy and exit 0.
    #[cfg(all(desktop, debug_assertions))]
    let solo = !smoke::enabled();
    #[cfg(all(desktop, not(debug_assertions)))]
    let solo = true;
    #[cfg(desktop)]
    if solo {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| shell::show_main(app, None)));
    }
    #[cfg(desktop)]
    {
        builder = builder
            .plugin(
                tauri_plugin_window_state::Builder::new()
                    .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
                    // Settings is a dialog with a designed size; a size remembered from an older build must not shrink it.
                    .with_denylist(&[shell::SETTINGS])
                    .build(),
            )
            .plugin(tauri_plugin_autostart::Builder::new().app_name("Recursa").args(["--hidden"]).build());
    }
    let builder = builder
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state.clone())
        .manage(ipc::Streams::default())
        .invoke_handler(tauri::generate_handler![
            ipc::api,
            ipc::subscribe,
            ipc::unsubscribe,
            ipc::export_run,
            ipc::confirm,
            ipc::notify,
            ipc::open_settings,
            ipc::get_login,
            ipc::set_login,
            ipc::context_menu,
            ipc::copy_text
        ])
        .setup(move |app| {
            tauri::async_runtime::spawn(async move { state.start_background() });
            #[cfg(all(desktop, debug_assertions))]
            if smoke::enabled() {
                smoke::install(app.handle());
            }
            #[cfg(desktop)]
            shell::install(app.handle(), hidden)?;
            #[cfg(mobile)]
            mobile::install(app.handle())?;
            Ok(())
        });
    #[cfg(desktop)]
    let builder = builder.on_window_event(|window, event| match event {
        WindowEvent::Destroyed => shell::on_window_destroyed(window.app_handle(), window.label()),
        WindowEvent::ThemeChanged(_) => shell::sync_dock_icon(window.app_handle()),
        _ => {}
    });
    // A failed build leaves no context for mobile::fatal's page, and that page would need the same
    // shell; on iOS this only reaches the device log.
    builder.build(context).unwrap_or_else(|e| fail("Recursa couldn't start.", &e.to_string())).run(|app, event| {
        match event {
            // The last window closed: keep running in the menu bar. Quit still quits.
            #[cfg(desktop)]
            RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => shell::show_main(app, None),
            _ => {
                let _ = app;
            }
        }
    });
}

/// Nothing works without the database or the app shell: say why in a native alert (nobody sees
/// stderr when the app was opened from Finder or at login), then quit. Runs before Tauri's event loop,
/// so it uses AppKit directly; there is no window yet for a dialog plugin sheet to attach to.
fn fail(message: &str, detail: &str) -> ! {
    eprintln!("{message}\n{detail}");
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        use objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication, NSApplicationActivationPolicy};
        use objc2_foundation::NSString;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.runModal();
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (text, caption) = (wide(&format!("{message}\n\n{detail}")), wide("Recursa"));
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR) };
    }
    std::process::exit(1);
}
