//! Route-origin filtering on a resolver's path, tested live with DNS-OARC's Check My DNS.
//!
//! The server hands us a one-time name that is served only from an RPKI-invalid route. We ask the resolver
//! under test for it and report whether it answered; the server says whether the query reached its beacon.
//! What this measures is filtering anywhere between the resolver and the DNS-OARC site, not what the resolver's
//! operator runs. The protocol is Check My DNS's own websocket (the one `cmdns-cli` speaks), carried over a
//! minimal RFC 6455 client on a `reqwest` HTTP/1.1 upgrade.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

const URL: &str = "https://cmdns.dns-oarc.net/ws/";
const CHECKS: [&str; 2] = ["trans_rpkiv4", "trans_rpkiv6"];
/// The whole session, from connecting to the last `complete`.
const SESSION_CAP: Duration = Duration::from_secs(30);
/// Largest frame we send or accept.
const MAX_FRAME: usize = 64 << 10;
/// Concurrent sessions to DNS-OARC, so a run over many resolvers doesn't open them all at once.
static SESSIONS: Semaphore = Semaphore::const_new(6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Filtered,
    Unfiltered,
    NotTested,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Filtered => "filtered",
            Status::Unfiltered => "unfiltered",
            Status::NotTested => "not_tested",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RovCheck {
    pub status: Status,
    pub detail: String,
    /// The lookup name minus its first label, without the trailing dot.
    pub beacon: Option<String>,
}

impl RovCheck {
    pub fn to_json(&self) -> Value {
        json!({ "status": self.status.as_str(), "detail": self.detail, "beacon": self.beacon })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RovResult {
    pub v4: RovCheck,
    pub v6: RovCheck,
}

impl RovResult {
    pub fn to_json(&self) -> Value {
        json!({ "v4": self.v4.to_json(), "v6": self.v6.to_json() })
    }
}

/// What the resolver under test did with the beacon name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// A DNS response came back, with any rcode.
    Answered { ok: bool, err: String },
    /// The query was sent and nothing came back.
    Timeout,
    /// The exchanger could not be opened, or sending failed.
    Local(String),
}

/// One check's verdict from what the resolver did and what the server concluded (`succ`, `msg`).
fn verdict(lookup: Option<&Lookup>, complete: Option<&(bool, String)>, answered_this_run: bool, reachable: bool) -> (Status, String) {
    match (lookup, complete) {
        (Some(Lookup::Local(e)), _) => (Status::NotTested, format!("The test query never left this computer: {e}")),
        (Some(Lookup::Timeout), _) if !answered_this_run => (Status::NotTested, "The resolver didn't answer the test name".into()),
        (Some(_), Some((true, msg))) => (Status::Filtered, msg.clone()),
        (Some(_), Some((false, msg))) => (Status::Unfiltered, msg.clone()),
        _ if reachable => (Status::NotTested, "Check My DNS didn't finish".into()),
        _ => (Status::NotTested, "Check My DNS unreachable".into()),
    }
}

/// `beacon` for a lookup name: the name minus its first label and trailing dot.
fn beacon_of(dn: &str) -> Option<String> {
    let rest = dn.trim_end_matches('.').split_once('.')?.1;
    (!rest.is_empty()).then(|| rest.to_string())
}

// ---- Protocol messages

/// What the server said, in the parts we use.
#[derive(Debug, PartialEq)]
enum Event {
    PrepareTotal(usize),
    /// A check is ready. Some of these carry neither id nor name; they still count toward the total.
    Prepared {
        id: Value,
        name: Option<String>,
    },
    Lookup {
        id: Value,
        dn: String,
    },
    Complete {
        id: Value,
        succ: bool,
        msg: String,
    },
    Rating,
}

/// The events in one text frame: it may hold several JSON objects separated by newlines.
fn parse_events(text: &str) -> Vec<Event> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|m| {
            if let Some(p) = m.get("prepare") {
                return match (p["done"].as_bool(), p["total"].as_u64()) {
                    (Some(false), Some(n)) => Some(Event::PrepareTotal(n as usize)),
                    (Some(true), _) => Some(Event::Prepared { id: p["id"].clone(), name: p["name"].as_str().map(str::to_string) }),
                    _ => None,
                };
            }
            if let Some(l) = m.get("lookup") {
                return Some(Event::Lookup { id: l["id"].clone(), dn: l["dn"].as_str()?.to_string() });
            }
            if let Some(c) = m.get("complete") {
                return Some(Event::Complete {
                    id: c["id"].clone(),
                    succ: c["succ"].as_bool().unwrap_or(false),
                    msg: c["msg"].as_str().unwrap_or("").to_string(),
                });
            }
            m.get("rating").map(|_| Event::Rating)
        })
        .collect()
}

/// What the session does next.
#[derive(Debug, PartialEq)]
enum Action {
    Send(String),
    Complete { check: Option<String>, succ: bool, msg: String },
    Finish,
}

/// Where the server is in the exchange: counts the prepared checks and maps ids to check names.
#[derive(Default)]
struct Protocol {
    total: Option<usize>,
    prepared: usize,
    started: bool,
    names: HashMap<String, String>,
}

impl Protocol {
    fn feed(&mut self, text: &str) -> Vec<Action> {
        let mut out = Vec::new();
        for ev in parse_events(text) {
            match ev {
                Event::PrepareTotal(n) => self.total = Some(n),
                Event::Prepared { id, name } => {
                    if let Some(key) = name.as_deref().and_then(check_key) {
                        self.names.insert(id.to_string(), key.to_string());
                    }
                    self.prepared += 1;
                    if !self.started && self.total == Some(self.prepared) {
                        self.started = true;
                        out.push(Action::Send(json!({ "check": { "all": true } }).to_string()));
                    }
                }
                // Answered by the session, which owns the resolver.
                Event::Lookup { .. } => {}
                Event::Complete { id, succ, msg } => {
                    out.push(Action::Complete { check: self.names.get(&id.to_string()).cloned(), succ, msg })
                }
                Event::Rating => out.push(Action::Finish),
            }
        }
        out
    }
}

/// Which of our checks a server-side name is: it has been `trans_rpkiv4` and, live, `RPKI IPv4`.
fn check_key(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    CHECKS.into_iter().find(|c| n.ends_with(&c[c.len() - 2..]))
}

/// The reply to a lookup.
fn lookup_reply(id: &Value, dn: &str, outcome: &Lookup) -> String {
    let (ok, err) = match outcome {
        Lookup::Answered { ok, err } => (*ok, err.as_str()),
        Lookup::Timeout => (false, "timeout"),
        Lookup::Local(_) => (false, "local"),
    };
    json!({ "lookup": { "id": id, "dn": dn, "ok": ok, "err": if ok { "" } else { err } } }).to_string()
}

// ---- Websocket frames (RFC 6455)

/// A client frame: FIN set, masked with `mask`. `None` when the payload is over the cap.
fn encode_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Option<Vec<u8>> {
    if payload.len() > MAX_FRAME {
        return None;
    }
    let mut f = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => f.push(0x80 | n as u8),
        n => {
            f.push(0x80 | 126);
            f.extend_from_slice(&(n as u16).to_be_bytes());
        }
    }
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    Some(f)
}

#[derive(Debug, PartialEq, Eq)]
struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

/// The first frame in `buf` and how many bytes it took. `Ok(None)` means more bytes are needed; `Err` means the
/// frame is over the cap.
fn decode_frame(buf: &[u8]) -> Result<Option<(Frame, usize)>, ()> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let masked = buf[1] & 0x80 != 0;
    let (len, mut at) = match buf[1] & 0x7f {
        126 if buf.len() >= 4 => (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4),
        127 if buf.len() >= 10 => (u64::from_be_bytes(buf[2..10].try_into().unwrap()), 10),
        126 | 127 => return Ok(None),
        n => (n as u64, 2),
    };
    if len > MAX_FRAME as u64 {
        return Err(());
    }
    let len = len as usize;
    let mask = if masked {
        if buf.len() < at + 4 {
            return Ok(None);
        }
        at += 4;
        Some([buf[at - 4], buf[at - 3], buf[at - 2], buf[at - 1]])
    } else {
        None
    };
    if buf.len() < at + len {
        return Ok(None);
    }
    let mut payload = buf[at..at + len].to_vec();
    if let Some(m) = mask {
        payload.iter_mut().enumerate().for_each(|(i, b)| *b ^= m[i % 4]);
    }
    Ok(Some((Frame { fin: buf[0] & 0x80 != 0, opcode: buf[0] & 0x0f, payload }, at + len)))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    b.iter_mut().for_each(|x| *x = fastrand::u8(..));
    b
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// A websocket over an upgraded HTTP connection.
struct Socket {
    io: reqwest::Upgraded,
    buf: Vec<u8>,
}

impl Socket {
    async fn send(&mut self, opcode: u8, payload: &[u8]) -> Option<()> {
        let f = encode_frame(opcode, payload, random_bytes())?;
        self.io.write_all(&f).await.ok()?;
        self.io.flush().await.ok()
    }

    /// The next text message; `None` when the server closed, sent something we don't speak, or the stream ended.
    async fn next_text(&mut self) -> Option<String> {
        loop {
            while let Some((f, used)) = decode_frame(&self.buf).ok()? {
                self.buf.drain(..used);
                match (f.opcode, f.fin) {
                    (1, true) => return String::from_utf8(f.payload).ok(),
                    (9, true) => self.send(10, &f.payload).await?,
                    _ => return None,
                }
            }
            let mut chunk = [0u8; 4096];
            let n = self.io.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

async fn connect(ws: &reqwest::Client) -> Option<Socket> {
    let resp = ws
        .get(URL)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", base64(&random_bytes::<16>()))
        .send()
        .await
        .ok()?;
    if resp.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
        return None;
    }
    Some(Socket { io: resp.upgrade().await.ok()?, buf: Vec::new() })
}

/// The client `probe` needs: HTTP/1.1 only, since a websocket upgrade doesn't exist on HTTP/2.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Recursa-DNS-Benchmark/", env!("CARGO_PKG_VERSION")))
        .http1_only()
        .connect_timeout(Duration::from_secs(8))
        .build()
        .expect("websocket client")
}

// ---- The session

/// What a session collected, kept outside the future so the cap leaves it intact.
#[derive(Default)]
struct Collected {
    reached: bool,
    /// Per check name: the lookup name and what the resolver did with it.
    lookups: HashMap<String, (String, Lookup)>,
    /// Per check name: the server's `succ` and `msg`.
    completes: HashMap<String, (bool, String)>,
}

async fn session(
    ws: &reqwest::Client,
    resolve: &Arc<Mutex<impl FnMut(&str) -> Lookup + Send + 'static>>,
    got: &mut Collected,
) -> Option<()> {
    let mut sock = connect(ws).await?;
    got.reached = true;
    sock.send(1, json!({ "prepare": { "checks": CHECKS } }).to_string().as_bytes()).await?;
    let mut proto = Protocol::default();
    while got.completes.len() < CHECKS.len() {
        let text = sock.next_text().await?;
        // Lookups are answered here, in order, since each needs the resolver's reply before the server goes on.
        for ev in parse_events(&text) {
            if let Event::Lookup { id, dn } = &ev {
                let check = proto.names.get(&id.to_string()).cloned();
                let (r, dn2) = (resolve.clone(), dn.clone());
                let outcome = tokio::task::spawn_blocking(move || (r.lock().unwrap())(&dn2))
                    .await
                    .unwrap_or_else(|_| Lookup::Local("the lookup thread failed".into()));
                if let Some(c) = check {
                    got.lookups.insert(c, (dn.clone(), outcome.clone()));
                }
                sock.send(1, lookup_reply(id, dn, &outcome).as_bytes()).await?;
            }
        }
        for a in proto.feed(&text) {
            match a {
                Action::Send(s) => sock.send(1, s.as_bytes()).await?,
                Action::Complete { check: Some(c), succ, msg } => {
                    got.completes.insert(c, (succ, msg));
                }
                Action::Finish => return Some(()),
                _ => {}
            }
        }
    }
    Some(())
}

/// Test one resolver. `resolve` asks it for a name (blocking); `answered_this_run` says whether it answered
/// anything else in this run, which is what makes a silent beacon lookup count as filtered rather than unknown.
pub async fn probe(ws: &reqwest::Client, resolve: impl FnMut(&str) -> Lookup + Send + 'static, answered_this_run: bool) -> RovResult {
    let resolve = Arc::new(Mutex::new(resolve));
    let mut got = Collected::default();
    if let Ok(_permit) = SESSIONS.acquire().await {
        let _ = tokio::time::timeout(SESSION_CAP, session(ws, &resolve, &mut got)).await;
    }
    let check = |name: &str| {
        let lookup = got.lookups.get(name);
        let (status, detail) = verdict(lookup.map(|l| &l.1), got.completes.get(name), answered_this_run, got.reached);
        RovCheck { status, detail, beacon: lookup.and_then(|l| beacon_of(&l.0)) }
    };
    RovResult { v4: check(CHECKS[0]), v6: check(CHECKS[1]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mask = [0x12, 0x34, 0x56, 0x78];
        let f = encode_frame(1, b"hello", mask).unwrap();
        assert_eq!(f[1], 0x80 | 5);
        let (d, used) = decode_frame(&f).unwrap().unwrap();
        assert_eq!((d, used), (Frame { fin: true, opcode: 1, payload: b"hello".to_vec() }, f.len()));

        let big = vec![b'x'; 300];
        let f = encode_frame(1, &big, mask).unwrap();
        assert_eq!(f[1], 0x80 | 126);
        assert_eq!(&f[2..4], &300u16.to_be_bytes());
        let (d, used) = decode_frame(&f).unwrap().unwrap();
        assert_eq!((d.payload, used), (big, f.len()));
        // A partial frame asks for more bytes.
        assert_eq!(decode_frame(&f[..10]), Ok(None));
        assert!(encode_frame(1, &vec![0; MAX_FRAME + 1], mask).is_none());
    }

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(&[0u8; 16]).len(), 24);
    }

    #[test]
    fn transcript_becomes_actions() {
        let mut p = Protocol::default();
        assert!(p.feed(r#"{"prepare":{"done":false,"total":4}}"#).is_empty());
        assert!(p.feed(r#"{"prepare":{"done":true,"id":"a","name":"RPKI IPv4"}}"#).is_empty());
        // Two messages in one frame, separated by a newline.
        assert!(p.feed("{\"prepare\":{\"done\":true,\"id\":\"b\",\"name\":\"RPKI IPv6\"}}\n{\"progress\":{}}\n").is_empty());
        // The server also sends done messages with no id or name, and they count toward the total.
        assert!(p.feed(r#"{"prepare":{"done":true,"total":0}}"#).is_empty());
        let acts = p.feed(r#"{"prepare":{"done":true,"total":0}}"#);
        assert_eq!(acts, vec![Action::Send(r#"{"check":{"all":true}}"#.into())]);
        assert_eq!(
            parse_events(r#"{"lookup":{"id":"a","dn":"x1.cmdns3-sjc.dns-oarc.net."}}"#),
            vec![Event::Lookup { id: json!("a"), dn: "x1.cmdns3-sjc.dns-oarc.net.".into() }]
        );
        let acts = p.feed(r#"{"complete":{"id":"a","succ":true,"msg":"enabled"}}"#);
        assert_eq!(acts, vec![Action::Complete { check: Some("trans_rpkiv4".into()), succ: true, msg: "enabled".into() }]);
        let acts = p.feed("{\"complete\":{\"id\":\"b\",\"succ\":false,\"msg\":\"no\"}}\n{\"rating\":{\"text\":\"t\",\"class\":\"c\"}}");
        assert_eq!(acts, vec![Action::Complete { check: Some("trans_rpkiv6".into()), succ: false, msg: "no".into() }, Action::Finish]);
    }

    #[test]
    fn replies_and_beacons() {
        let r = lookup_reply(&json!("a"), "x.y.z.", &Lookup::Answered { ok: false, err: "SERVFAIL".into() });
        assert_eq!(r, r#"{"lookup":{"dn":"x.y.z.","err":"SERVFAIL","id":"a","ok":false}}"#);
        assert!(lookup_reply(&json!("a"), "x.y.z.", &Lookup::Timeout).contains(r#""err":"timeout""#));
        assert_eq!(beacon_of("abc.cmdns3-sjc.dns-oarc.net.").as_deref(), Some("cmdns3-sjc.dns-oarc.net"));
    }

    #[test]
    fn verdict_table() {
        let yes = (true, "msg".to_string());
        let no = (false, "msg".to_string());
        let ans = Lookup::Answered { ok: false, err: "SERVFAIL".into() };
        let local = Lookup::Local("refused".into());
        let v = |l: Option<&Lookup>, c: Option<&(bool, String)>, a: bool, r: bool| verdict(l, c, a, r);
        assert_eq!(v(Some(&local), Some(&yes), true, true), (Status::NotTested, "The test query never left this computer: refused".into()));
        assert_eq!(
            v(Some(&Lookup::Timeout), Some(&yes), false, true),
            (Status::NotTested, "The resolver didn't answer the test name".into())
        );
        assert_eq!(v(Some(&ans), Some(&yes), false, true), (Status::Filtered, "msg".into()));
        assert_eq!(v(Some(&Lookup::Timeout), Some(&yes), true, true), (Status::Filtered, "msg".into()));
        assert_eq!(v(Some(&ans), Some(&no), false, true), (Status::Unfiltered, "msg".into()));
        assert_eq!(v(Some(&Lookup::Timeout), Some(&no), true, true), (Status::Unfiltered, "msg".into()));
        assert_eq!(v(Some(&ans), None, true, true), (Status::NotTested, "Check My DNS didn't finish".into()));
        assert_eq!(v(None, None, true, false), (Status::NotTested, "Check My DNS unreachable".into()));
    }
}
