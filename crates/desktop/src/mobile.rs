//! The iOS shell: one window and none of the desktop chrome (no menus, tray, dock or window state).

use crate::MAIN;
use objc2::msg_send;
use objc2::runtime::AnyObject;
use recursa_engine::app::AppState;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Context, Manager, WebviewUrl, WebviewWindowBuilder};

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    // The UI reads this flag before anything async: Settings is a page here, not a window.
    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("".into()))
        .initialization_script("window.__RECURSA_MOBILE__ = true;")
        .build()?;
    window.with_webview(|webview| {
        // wry leaves WKWebView's scroll view at its defaults, so UIKit insets the page by the status
        // bar and home indicator and `env(safe-area-inset-*)` never sees them. Edge to edge instead:
        // the page lays out under both and pads its own chrome. The Back swipe stays off: the
        // sidebar drawer owns the left edge.
        // SAFETY: `inner()` is the live WKWebView; both selectors exist on every supported iOS.
        unsafe {
            let wk = &*(webview.inner() as *const AnyObject);
            let scroll: *mut AnyObject = msg_send![wk, scrollView];
            // UIScrollViewContentInsetAdjustmentNever
            let _: () = msg_send![scroll, setContentInsetAdjustmentBehavior: 2isize];
        }
    })?;
    keep_awake_while_measuring(app.clone());
    Ok(())
}

/// Keep the screen on while anything measures (a benchmark, a Time run, a single-resolver look):
/// once the phone locks, iOS suspends the app and the run stalls. The engine's flag covers every
/// kind of run, however the page got there or left.
fn keep_awake_while_measuring(app: AppHandle) {
    let measuring = app.state::<Arc<AppState>>().measuring.clone();
    std::thread::spawn(move || {
        let mut on = false;
        loop {
            let now = measuring.load(Ordering::SeqCst);
            if now != on && set_idle_timer_disabled(&app, now).is_ok() {
                on = now;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

/// UIKit wants this on the main thread.
fn set_idle_timer_disabled(app: &AppHandle, on: bool) -> tauri::Result<()> {
    app.run_on_main_thread(move || {
        // SAFETY: both selectors exist on every supported iOS, and this runs on the main thread.
        unsafe {
            let ui: *mut AnyObject = msg_send![objc2::class!(UIApplication), sharedApplication];
            let _: () = msg_send![ui, setIdleTimerDisabled: on];
        }
    })
}

/// Nothing works without the database: run a bare app whose page shows the reason (main.ts reads
/// the flag), rather than vanishing back to the home screen.
pub fn fatal(context: Context<tauri::Wry>, message: &str, detail: &str) -> ! {
    eprintln!("{message}\n{detail}");
    let script = format!("window.__RECURSA_FATAL__ = {};", serde_json::json!({ "message": message, "detail": detail }));
    let _ = tauri::Builder::default()
        .setup(move |app| {
            WebviewWindowBuilder::new(app.handle(), MAIN, WebviewUrl::App("".into())).initialization_script(&script).build()?;
            Ok(())
        })
        .run(context);
    std::process::exit(1);
}
