//! Local HTTP server for the browser UI: embedded assets, the REST API and Server-Sent Events.
//!
//! Bound to loopback only. Every request must carry an exact loopback Host header,
//! which defeats DNS-rebinding. API calls need the per-launch token, and state-changing
//! calls must come from our own Origin, which defeats CSRF from other local pages.
//! The token is never in a page the server sends: the launch URL carries it in its fragment,
//! or carries a one-time code that `POST /api/session` trades for it, so another local user
//! who can reach the port still can't read it.
//! The API itself lives in the engine ([`AppState::call`]); this is only the transport.

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use recursa_engine::app::{ApiError, AppState};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist"]
struct Assets;

pub struct Server {
    pub app: Arc<AppState>,
    pub token: String,
    /// The one-time code in the URL Recursa opens, until a page trades it for the token.
    pub launch: Mutex<Option<String>>,
    pub port: u16,
}

type Shared = Arc<Server>;

fn err(e: ApiError) -> Response {
    let code = StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(json!({ "error": e.error }))).into_response()
}

/// Constant-time comparison for the session token.
fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn router(srv: Shared) -> Router {
    Router::new()
        .route("/api/runs/{id}/events", get(events))
        .route("/api/runs/{id}/export", get(export))
        .route("/api/session", post(session))
        .route("/api/{*rest}", any(api))
        .fallback(static_asset)
        .layer(middleware::from_fn_with_state(srv.clone(), guard))
        .with_state(srv)
}

async fn guard(State(srv): State<Shared>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let allowed = [format!("127.0.0.1:{}", srv.port), format!("localhost:{}", srv.port), format!("[::1]:{}", srv.port)];
    if !allowed.iter().any(|a| a == host) {
        return (StatusCode::MISDIRECTED_REQUEST, "Recursa only answers on its loopback address.").into_response();
    }
    if req.uri().path().starts_with("/api/") && req.uri().path() != "/api/session" {
        let header_token = req.headers().get("x-recursa-token").and_then(|v| v.to_str().ok());
        let query_token = req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
        if !header_token.or(query_token).is_some_and(|t| token_eq(t, &srv.token)) {
            return err(ApiError::new(401, "Missing or invalid session token. Open the address Recursa printed when it started."));
        }
    }
    if req.uri().path().starts_with("/api/")
        && req.method() != Method::GET
        && let Some(origin) = req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok())
        && !allowed.iter().any(|a| origin == format!("http://{a}"))
    {
        return err(ApiError::new(403, "Cross-origin request refused."));
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    resp
}

#[derive(Deserialize)]
struct SessionQ {
    code: String,
}

/// Trade the launch URL's one-time code for the session token. A used or wrong code gets nothing.
async fn session(State(srv): State<Shared>, Json(q): Json<SessionQ>) -> Response {
    let mut launch = srv.launch.lock().unwrap();
    if !launch.as_deref().is_some_and(|c| token_eq(c, &q.code)) {
        return err(ApiError::new(
            401,
            "This launch link is not valid or was already used. Open the address Recursa printed when it started.",
        ));
    }
    *launch = None;
    Json(json!({ "token": srv.token })).into_response()
}

/// Every REST call: hand the method, path and JSON body to the engine.
async fn api(State(srv): State<Shared>, method: Method, uri: Uri, body: Bytes) -> Response {
    let body: Value = if body.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return err(ApiError::new(400, format!("Invalid JSON: {e}."))),
        }
    };
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    match srv.app.call(method.as_str(), path, body).await {
        Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Err(e) => err(e),
    }
}

async fn static_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return err(ApiError::new(404, "No such endpoint."));
    }
    let (file, name) = match Assets::get(if path.is_empty() { "index.html" } else { path }) {
        Some(f) => (f, if path.is_empty() { "index.html" } else { path }),
        // Client-side routes fall back to the app shell.
        None if !path.contains('.') => match Assets::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return (StatusCode::NOT_FOUND, "UI not built. Run `npm run build` in web/.").into_response(),
        },
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    let mime = file.metadata.mimetype().to_string();
    let mut builder = Response::builder().header(header::CONTENT_TYPE, mime);
    if name == "index.html" {
        builder = builder
            .header(header::CACHE_CONTROL, "no-store")
            .header(
                "content-security-policy",
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
            )
            .header("x-frame-options", "DENY");
        return builder.body(Body::from(file.data.into_owned())).unwrap();
    }
    if name.starts_with("assets/") {
        builder = builder.header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");
    }
    builder.body(Body::from(file.data.into_owned())).unwrap()
}

async fn events(State(srv): State<Shared>, Path(id): Path<String>) -> Response {
    let Some(h) = srv.app.run_handle(&id) else { return err(ApiError::new(404, "Run is not live.")) };
    let (rx, snapshot) = h.subscribe();
    let first = futures_util::stream::once(async move { Event::default().data(snapshot) });
    let rest = tokio_stream::wrappers::BroadcastStream::new(rx).map(|m| match m {
        Ok(s) => Event::default().data(&*s),
        // A lagging client gets told to resync from a fresh snapshot.
        Err(_) => Event::default().data(r#"{"type":"resync"}"#),
    });
    Sse::new(first.chain(rest).map(Ok::<_, Infallible>)).keep_alive(KeepAlive::default()).into_response()
}

#[derive(Deserialize)]
struct ExportQ {
    format: Option<String>,
}

async fn export(State(srv): State<Shared>, Path(id): Path<String>, Query(q): Query<ExportQ>) -> Response {
    let Some(x) = srv.app.export(&id, q.format.as_deref() == Some("csv")) else {
        return err(ApiError::new(404, "No such run."));
    };
    (
        [(header::CONTENT_TYPE, x.mime.to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", x.filename))],
        x.body,
    )
        .into_response()
}
