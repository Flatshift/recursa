//! The UI's transport in the desktop app: Tauri commands in place of HTTP, SSE and
//! download links. REST calls go through the same [`AppState::call`] the HTTP server uses.

use recursa_engine::app::{ApiError, AppState};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
#[cfg(desktop)]
use tauri::Manager;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::{Channel, InvokeResponseBody, Response};
#[cfg(desktop)]
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tokio::sync::broadcast::error::RecvError;

#[cfg(desktop)]
use crate::shell::MAIN;

type App<'a> = State<'a, Arc<AppState>>;

/// One REST call: same method, path and body as the HTTP API.
#[tauri::command]
pub async fn api(app: App<'_>, method: String, path: String, body: Option<Value>) -> Result<Response, ApiError> {
    #[cfg(debug_assertions)]
    let started = std::time::Instant::now();
    let result = app.call(&method, &path, body.unwrap_or_default()).await;
    #[cfg(debug_assertions)]
    eprintln!(
        "[ipc] {method} {path} → {} in {:.1} ms",
        result.as_ref().map_or_else(|e| e.status, |_| 200),
        started.elapsed().as_secs_f64() * 1e3
    );
    // The engine's JSON text goes to the UI as is, without being parsed again here.
    result.map(Response::new)
}

/// Live event forwarders, by channel id.
#[derive(Default)]
pub struct Streams(Mutex<HashMap<u32, JoinHandle<()>>>);

impl Streams {
    /// Stop every forwarder (the window that owned the channels is gone).
    #[cfg(desktop)]
    pub fn clear(&self) {
        for (_, task) in self.0.lock().unwrap().drain() {
            task.abort();
        }
    }
}

/// Follow a live run: a snapshot, then every event, with the same `seq` semantics as SSE.
/// Async so the snapshot (the whole run so far) is serialized off the main thread.
#[tauri::command]
pub async fn subscribe(app: App<'_>, streams: State<'_, Streams>, run_id: String, on_event: Channel) -> Result<u32, ApiError> {
    let h = app.run_handle(&run_id).ok_or_else(|| ApiError::new(404, "Run is not live."))?;
    let (mut rx, snapshot) = h.subscribe();
    drop(h);
    let id = on_event.id();
    let task = tauri::async_runtime::spawn(async move {
        let mut msg = snapshot;
        // A failed send means the webview is gone.
        while on_event.send(InvokeResponseBody::Json(msg)).is_ok() {
            msg = match rx.recv().await {
                Ok(s) => s.to_string(),
                // A lagging client gets told to resync from a fresh snapshot.
                Err(RecvError::Lagged(_)) => r#"{"type":"resync"}"#.into(),
                // The run was evicted from memory: the UI fetches it instead.
                Err(RecvError::Closed) => {
                    let _ = on_event.send(InvokeResponseBody::Json(r#"{"type":"end"}"#.into()));
                    break;
                }
            };
        }
    });
    if let Some(old) = streams.0.lock().unwrap().insert(id, task) {
        old.abort();
    }
    Ok(id)
}

#[tauri::command]
pub fn unsubscribe(streams: State<'_, Streams>, id: u32) {
    if let Some(task) = streams.0.lock().unwrap().remove(&id) {
        task.abort();
    }
}

/// Ask before something destructive. WKWebView has no `window.confirm`, so this is a
/// native alert sheet instead.
#[tauri::command]
pub async fn confirm(handle: AppHandle, message: String, ok: String) -> bool {
    #[cfg_attr(mobile, allow(unused_mut))]
    let mut dialog = handle
        .dialog()
        .message(message)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(ok, "Cancel".into()));
    #[cfg(desktop)]
    if let Some(w) = handle.get_webview_window(MAIN) {
        dialog = dialog.parent(&w);
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    dialog.show(move |yes| {
        let _ = tx.send(yes);
    });
    rx.await.unwrap_or(false)
}

/// Save a run through the native save panel. Returns the file name, or null if cancelled.
#[tauri::command]
pub async fn export_run(handle: AppHandle, app: App<'_>, id: String, format: String) -> Result<Option<String>, ApiError> {
    #[cfg(mobile)]
    {
        let _ = (handle, app, id, format);
        Err(ApiError::new(501, "Export isn't available on this platform yet."))
    }
    #[cfg(desktop)]
    {
        let csv = format == "csv";
        let x = app.export(&id, csv).ok_or_else(|| ApiError::new(404, "No such run."))?;
        export_dialog(handle, csv, x).await
    }
}

#[cfg(desktop)]
async fn export_dialog(handle: AppHandle, csv: bool, x: recursa_engine::app::Export) -> Result<Option<String>, ApiError> {
    let (label, ext) = if csv { ("CSV", "csv") } else { ("JSON", "json") };
    let mut dialog = handle.dialog().file().set_file_name(&x.filename).add_filter(label, &[ext]);
    if let Some(w) = handle.get_webview_window(MAIN) {
        dialog = dialog.set_parent(&w);
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    dialog.save_file(move |p| {
        let _ = tx.send(p);
    });
    let Some(path) = rx.await.ok().flatten() else { return Ok(None) };
    let path = path.into_path().map_err(|e| ApiError::new(500, e.to_string()))?;
    std::fs::write(&path, x.body).map_err(|e| ApiError::new(500, format!("Couldn't save the file: {e}.")))?;
    Ok(Some(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}

/// A system notification: a run finished while Recursa was in the background.
#[tauri::command]
pub fn notify(handle: AppHandle, title: String, body: String) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = handle.notification().builder().title(title).body(body).show() {
        eprintln!("Couldn't show a notification: {e}");
    }
}

#[cfg(desktop)]
#[tauri::command]
pub fn open_settings(handle: AppHandle, section: Option<String>) {
    crate::shell::show_settings(&handle, section.as_deref());
}

#[cfg(mobile)]
#[tauri::command]
pub fn open_settings(_handle: AppHandle, _section: Option<String>) {}

#[cfg(desktop)]
#[tauri::command]
pub fn get_login(handle: AppHandle) -> bool {
    crate::shell::login_enabled(&handle)
}

#[cfg(mobile)]
#[tauri::command]
pub fn get_login(_handle: AppHandle) -> bool {
    false
}

#[cfg(desktop)]
#[tauri::command]
pub fn set_login(handle: AppHandle, enabled: bool) -> Result<bool, String> {
    crate::shell::set_login(&handle, enabled)
}

#[cfg(mobile)]
#[tauri::command]
pub fn set_login(_handle: AppHandle, _enabled: bool) -> Result<bool, String> {
    Err("Launch at login isn't available on iOS.".into())
}

#[derive(Deserialize)]
#[cfg_attr(mobile, allow(dead_code))]
pub(crate) struct MenuEntry {
    id: String,
    label: String,
    enabled: bool,
}

/// Pop up a native menu at the pointer. A pick comes back to the window as an
/// `recursa:context` event carrying "<seq>:<id>"; dismissing it sends nothing.
#[cfg(desktop)]
#[tauri::command]
pub fn context_menu(window: WebviewWindow, seq: u32, items: Vec<MenuEntry>) -> Result<(), String> {
    let app = window.app_handle();
    let menu = Menu::new(app).map_err(|e| e.to_string())?;
    for i in items {
        let r = if i.id == "-" {
            menu.append(&PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?)
        } else {
            let item =
                MenuItem::with_id(app, format!("ctx:{seq}:{}", i.id), i.label, i.enabled, None::<&str>).map_err(|e| e.to_string())?;
            menu.append(&item)
        };
        r.map_err(|e| e.to_string())?;
    }
    window.popup_menu(&menu).map_err(|e| e.to_string())
}

#[cfg(mobile)]
#[tauri::command]
pub fn context_menu(_window: WebviewWindow, _seq: u32, _items: Vec<MenuEntry>) -> Result<(), String> {
    Err("Native menus aren't available here.".into())
}

/// Put text on the general pasteboard. A pick from a native menu isn't a user gesture
/// the WebView accepts for its own clipboard API.
#[tauri::command]
pub fn copy_text(app: tauri::AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}
