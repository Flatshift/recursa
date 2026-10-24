//! Local HTTP server for the browser UI: embedded assets, the REST API and Server-Sent Events.
//!
//! Bound to loopback only. Every request must carry an exact loopback Host header,
//! which defeats DNS-rebinding. API calls need a session token, and state-changing
//! calls must come from our own Origin, which defeats CSRF from other local pages.
//! No token is ever in the served page or a launch link: a launch link carries a one-time code in
//! its fragment, which `POST /api/session` trades for a token (see [`Sessions`]), so another local
//! user who can reach the port, or read a browser's command line, gets nothing that lasts. (The
//! page puts its token in the URL of a run's event stream, since EventSource can't send a header;
//! the browser keeps no history of those requests.)
//! The API itself lives in the engine ([`AppState::call`]); this is only the transport.

use axum::body::{Body, Bytes};
use axum::extract::{MatchedPath, Path, Query, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Extension, Json, Router};
use futures_util::StreamExt;
use recursa_engine::app::{ApiError, AppState};
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist"]
struct Assets;

pub struct Server {
    pub app: Arc<AppState>,
    pub sessions: Mutex<Sessions>,
    pub port: u16,
}

/// 32 bytes from the system's secure random source, as hex: a launch code or a session token. A guessable one
/// would let any local page with the port drive the API.
pub fn secret() -> String {
    recursa_engine::unpredictable::<32>().iter().map(|b| format!("{b:02x}")).collect()
}

/// Launch links and the sessions they started. A link carries a one-time code, never a token, because a browser
/// started with a URL keeps it on its command line, where other users of the computer can read it. Each code starts
/// its own session once. A code that comes back a second time means someone else may have traded it first, so the
/// session it started ends there, and a new link is printed.
///
/// That check needs the user to trade the code too. The link Recursa opens itself may never load (the opener can
/// succeed with no display, over SSH), and then its code, which was on a command line, would let whoever read it in
/// unnoticed: unused after [`OPENER_WINDOW`], it stops working.
#[derive(Default)]
pub struct Sessions {
    /// Each link's code, the session token it started, and until when it may start one.
    links: Vec<(String, Option<String>, Option<Instant>)>,
}

/// How long the link Recursa opens itself has to reach the page: a browser loads it within moments or never.
const OPENER_WINDOW: Duration = Duration::from_secs(120);

enum Redeemed {
    Token(String),
    Invalid,
    Reused,
}

impl Sessions {
    /// A new launch link on `port`, good until it is used.
    pub fn link(&mut self, port: u16) -> String {
        self.add(port, None)
    }

    /// A launch link on `port` for the browser Recursa opens itself: unused after [`OPENER_WINDOW`], it stops working.
    pub fn opener_link(&mut self, port: u16) -> String {
        self.add(port, Some(Instant::now() + OPENER_WINDOW))
    }

    fn add(&mut self, port: u16, until: Option<Instant>) -> String {
        let code = secret();
        let url = format!("http://127.0.0.1:{port}/#code={code}");
        self.links.push((code, None, until));
        url
    }

    fn valid(&self, token: &str) -> bool {
        self.links.iter().any(|(_, t, _)| t.as_deref().is_some_and(|t| token_eq(t, token)))
    }

    fn redeem(&mut self, code: &str) -> Redeemed {
        let Some(i) = self.links.iter().position(|(c, ..)| token_eq(c, code)) else { return Redeemed::Invalid };
        match &self.links[i] {
            (_, Some(_), _) => {
                self.links.remove(i);
                return Redeemed::Reused;
            }
            (_, None, Some(until)) if Instant::now() >= *until => {
                self.links.remove(i);
                return Redeemed::Invalid;
            }
            _ => {}
        }
        let token = secret();
        self.links[i].1 = Some(token.clone());
        Redeemed::Token(token)
    }
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

/// A run's event stream, the one route that takes its token in the URL.
const EVENTS: &str = "/api/runs/{id}/events";

pub fn router(srv: Shared) -> Router {
    // Every API route needs a session token, except the one that trades a launch code for it.
    let authed = Router::new()
        .route(EVENTS, get(events))
        .route("/api/runs/{id}/export", get(export))
        .route("/api/{*rest}", any(api))
        .route_layer(middleware::from_fn_with_state(srv.clone(), require_token));
    Router::new()
        .route("/api/session", post(session))
        .merge(authed)
        .fallback(static_asset)
        .layer(middleware::from_fn_with_state(srv.clone(), guard))
        .with_state(srv)
}

/// The session token, from the `x-recursa-token` header or, for an event stream only (EventSource can't send
/// headers), the `token` query parameter. Every other call keeps it out of its URL.
async fn require_token(State(srv): State<Shared>, mut req: Request, next: Next) -> Response {
    let token = {
        let header_token = req.headers().get("x-recursa-token").and_then(|v| v.to_str().ok());
        let stream = req.extensions().get::<MatchedPath>().is_some_and(|p| p.as_str() == EVENTS);
        let query_token = req.uri().query().filter(|_| stream).and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
        header_token.or(query_token).filter(|t| srv.sessions.lock().unwrap().valid(t)).map(str::to_owned)
    };
    let Some(token) = token else {
        return err(ApiError::new(401, "Missing or invalid session token. Open the link Recursa printed."));
    };
    req.extensions_mut().insert(SessionToken(token));
    next.run(req).await
}

/// The session token a request was let in with, for handlers that outlive the check (an event stream).
#[derive(Clone)]
struct SessionToken(String);

async fn guard(State(srv): State<Shared>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let allowed = [format!("127.0.0.1:{}", srv.port), format!("localhost:{}", srv.port), format!("[::1]:{}", srv.port)];
    if !allowed.iter().any(|a| a == host) {
        return (StatusCode::MISDIRECTED_REQUEST, "Recursa only answers on its loopback address.").into_response();
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

/// Trade a launch link's one-time code for a session token. A wrong code gets nothing; a used one ends the session it
/// started and prints a new link.
async fn session(State(srv): State<Shared>, Json(q): Json<SessionQ>) -> Response {
    // The lock is let go before printing: a panic while holding it would poison every later request.
    let redeemed = {
        let mut sessions = srv.sessions.lock().unwrap();
        match sessions.redeem(&q.code) {
            Redeemed::Reused => Err(sessions.link(srv.port)),
            r => Ok(r),
        }
    };
    match redeemed {
        Ok(Redeemed::Token(token)) => Json(json!({ "token": token })).into_response(),
        Ok(_) => err(ApiError::new(401, "This launch link is not valid. Open the link Recursa printed.")),
        Err(link) => {
            // Not eprintln!, which panics when stderr is gone (the terminal closed).
            let _ = writeln!(
                std::io::stderr(),
                "\nA launch link was opened a second time, so the session it started has ended. If you didn't open it\n\
                 twice, someone else on this computer saw it. New link (it opens once): {link}"
            );
            err(ApiError::new(401, "This launch link was already used, so its session has ended. Open the new link Recursa printed."))
        }
    }
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

async fn events(State(srv): State<Shared>, Extension(SessionToken(token)): Extension<SessionToken>, Path(id): Path<String>) -> Response {
    let Some(h) = srv.app.run_handle(&id) else { return err(ApiError::new(404, "Run is not live.")) };
    let (rx, snapshot) = h.subscribe();
    let first = futures_util::stream::once(async move { Event::default().data(snapshot) });
    let rest = tokio_stream::wrappers::BroadcastStream::new(rx).map(|m| match m {
        Ok(s) => Event::default().data(&*s),
        // A lagging client gets told to resync from a fresh snapshot.
        Err(_) => Event::default().data(r#"{"type":"resync"}"#),
    });
    // A session that ends (its launch link came back) loses its open streams too, at their next event.
    let live = move |_: &Event| std::future::ready(srv.sessions.lock().unwrap().valid(&token));
    Sse::new(first.chain(rest).take_while(live).map(Ok::<_, Infallible>)).keep_alive(KeepAlive::default()).into_response()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(link: &str) -> &str {
        link.split_once("#code=").unwrap().1
    }

    #[test]
    fn a_launch_code_starts_one_session_and_a_second_use_ends_it() {
        let mut s = Sessions::default();
        let (a, b) = (s.link(8053), s.link(8053));
        let Redeemed::Token(ta) = s.redeem(code_of(&a)) else { panic!("a fresh code starts a session") };
        assert!(s.valid(&ta));
        assert!(!s.valid(code_of(&a)), "a code is never a token");
        let Redeemed::Token(tb) = s.redeem(code_of(&b)) else { panic!("each link has its own code") };
        assert_ne!(ta, tb);
        assert!(matches!(s.redeem(code_of(&a)), Redeemed::Reused));
        assert!(!s.valid(&ta), "a reused code ends the session it started");
        assert!(s.valid(&tb), "and only that one");
        assert!(matches!(s.redeem(code_of(&a)), Redeemed::Invalid));
        assert!(matches!(s.redeem("nope"), Redeemed::Invalid));
    }

    #[test]
    fn an_opener_link_left_unused_stops_working() {
        let mut s = Sessions::default();
        let late = s.add(8053, Some(Instant::now()));
        assert!(matches!(s.redeem(code_of(&late)), Redeemed::Invalid), "past its window");
        assert!(matches!(s.redeem(code_of(&late)), Redeemed::Invalid), "and gone");
        let prompt = s.opener_link(8053);
        let Redeemed::Token(t) = s.redeem(code_of(&prompt)) else { panic!("an opener link used in time starts a session") };
        assert!(s.valid(&t));
    }
}
