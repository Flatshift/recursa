//! NTP: how accurate, and how fast, public time servers are from this network.
//!
//! One query is the standard NTPv4 exchange (RFC 5905). The client stamps when it sent the request (T1), the
//! server stamps when it received it (T2) and when it replied (T3), and the client stamps the arrival (T4):
//!
//! * offset θ = ((T2 − T1) + (T3 − T4)) / 2: how far ahead of this computer's clock the server's is;
//! * delay  δ = (T4 − T1) − (T3 − T2): the round trip, less the server's own time.
//!
//! θ is exact only if the request and the reply took equally long, which no client can know, so its error is
//! bounded by δ/2. The best estimate comes from the query with the smallest delay (NTP's own clock filter).
//!
//! "Accuracy" here is therefore relative: each server against this computer's clock, and against the consensus
//! of the servers measured, which is how far off this clock is too. The NTS check (RFC 8915) only asks whether a
//! server completes the key exchange that authenticated time starts with; the time itself is measured over plain NTP.

use crate::clock;
use crate::dns;
use crate::net;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Seconds from 1900-01-01 (NTP's epoch) to 1970-01-01.
const NTP_TO_UNIX: i128 = 2_208_988_800;
const NS: i128 = 1_000_000_000;
// How gently servers are asked. A time server is someone's infrastructure, often a volunteer's, so a run is built to
// look like a client that has just started up, and no heavier.
//
// * At most `MAX_SAMPLES` packets to an address in a run: NTP clients send a burst of eight when they start.
/// The most queries one run sends to one address.
pub const MAX_SAMPLES: u32 = 8;
/// The least time between two queries to one address. ntpd's own default is to rate-limit a client whose average is
/// faster than one query in 8 seconds, and NIST has asked for no more than one in 4, so this stays inside both.
pub const MIN_SPACING: Duration = Duration::from_secs(8);
/// A host the user typed is theirs to test, so it may be asked faster than a public server (a monitor needs an answer in
/// seconds); a server that says stop is still believed.
pub const CUSTOM_SPACING: Duration = Duration::from_secs(2);
const CUSTOM_COOLDOWN: Duration = Duration::from_secs(30);
/// A name asked within this long is left alone. At 8 queries a run that is at most 48 packets an hour to an address,
/// fewer than the 56 an ordinary client polling every 64 seconds sends.
pub const COOLDOWN: Duration = Duration::from_secs(600);
/// How long a server that told us to slow down (RATE) is left alone, and one that told us to go away (DENY, RSTR).
const KISS_RATE_BACKOFF: Duration = Duration::from_secs(3600);
const KISS_DENY_BACKOFF: Duration = Duration::from_secs(7 * 86_400);
/// An address that has answered none of this many queries in a row isn't asked again in the run.
const GIVE_UP_AFTER: u32 = 3;

/// Unix nanoseconds to an NTP 32.32 timestamp (era 0, wrapping in 2036 as the protocol does).
fn to_ntp(unix_ns: i128) -> u64 {
    let secs = unix_ns.div_euclid(NS) + NTP_TO_UNIX;
    let frac = (unix_ns.rem_euclid(NS) << 32) / NS;
    (((secs as u64) & 0xffff_ffff) << 32) | frac as u64
}

/// An NTP timestamp to Unix nanoseconds. A timestamp whose top bit is clear is taken to be in era 1
/// (2036 onward), as RFC 4330 advises for clients.
fn from_ntp(ts: u64) -> i128 {
    let mut secs = (ts >> 32) as i128;
    if secs & 0x8000_0000 == 0 {
        secs += 1 << 32;
    }
    let frac = (ts & 0xffff_ffff) as i128;
    (secs - NTP_TO_UNIX) * NS + ((frac * NS) >> 32)
}

/// A 16.16 fixed-point number of seconds, in milliseconds.
fn short_ms(v: u32) -> f64 {
    v as f64 / 65536.0 * 1000.0
}

fn unix_ns_now() -> i128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i128).unwrap_or(0)
}

/// What one query learned.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Sample {
    /// How far ahead of this computer's clock the server's is.
    pub offset_ms: f64,
    /// Round trip less the server's own time.
    pub delay_ms: f64,
    pub stratum: u8,
    /// 0 none, 1 and 2 a leap second is coming, 3 clock not synchronized.
    pub leap: u8,
    pub version: u8,
    /// Reference ID: a source code for stratum 1 ("GPS", "PPS"), the upstream's address otherwise.
    pub refid: String,
    pub root_delay_ms: f64,
    pub root_dispersion_ms: f64,
    /// The server's clock resolution, in milliseconds.
    pub precision_ms: f64,
    pub poll: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NtpError {
    Timeout,
    /// The server answered with a Kiss-o'-Death: stop, or slow down (RATE, DENY, RSTR).
    Kiss(String),
    /// It answered, but its clock is not synchronized (leap indicator 3, or stratum 16).
    Unsynchronized,
    Bad(&'static str),
    Io(String),
    /// An authenticated (NTS) exchange failed: a short machine-readable kind, and what happened.
    Nts(&'static str, String),
}

impl NtpError {
    /// A short, stable name for why a query failed, for scripts and for telling failures apart in the UI.
    pub fn kind(&self) -> &'static str {
        match self {
            NtpError::Timeout => "timeout",
            NtpError::Kiss(_) => "kiss",
            NtpError::Unsynchronized => "unsynchronized",
            NtpError::Bad(_) => "bad_reply",
            NtpError::Io(_) => "network",
            NtpError::Nts(k, _) => k,
        }
    }
}

impl std::fmt::Display for NtpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NtpError::Timeout => write!(f, "no answer"),
            NtpError::Kiss(k) => write!(f, "told us to stop ({k})"),
            NtpError::Unsynchronized => write!(f, "its clock isn't synchronized"),
            NtpError::Bad(why) => write!(f, "{why}"),
            NtpError::Io(e) => write!(f, "{e}"),
            NtpError::Nts(_, why) => write!(f, "{why}"),
        }
    }
}

/// The request: version 4, client mode, with only the transmit timestamp set.
fn request(transmit: u64) -> [u8; 48] {
    let mut p = [0u8; 48];
    p[0] = (4 << 3) | 3;
    p[40..48].copy_from_slice(&transmit.to_be_bytes());
    p
}

fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes(b[..8].try_into().unwrap_or([0; 8]))
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap_or([0; 4]))
}

/// Reads a reply to the request stamped `sent` and works out offset and delay. `t1` and `t4` are the client's own
/// send and arrival times in Unix nanoseconds.
fn read_reply(b: &[u8], sent: u64, t1: i128, t4: i128) -> Result<Sample, NtpError> {
    if b.len() < 48 {
        return Err(NtpError::Bad("a reply shorter than an NTP packet"));
    }
    let (li, vn, mode) = (b[0] >> 6, (b[0] >> 3) & 7, b[0] & 7);
    if mode != 4 || !(3..=4).contains(&vn) {
        return Err(NtpError::Bad("not an NTP server reply"));
    }
    // The server must echo what we sent; anything else is a stray or forged packet.
    if be64(&b[24..]) != sent {
        return Err(NtpError::Bad("a reply to a different request"));
    }
    let stratum = b[1];
    if stratum == 0 {
        let code: String = b[12..16].iter().filter(|c| c.is_ascii_graphic()).map(|c| *c as char).collect();
        return Err(NtpError::Kiss(code));
    }
    if li == 3 || stratum > 15 {
        return Err(NtpError::Unsynchronized);
    }
    let (t2, t3) = (from_ntp(be64(&b[32..])), from_ntp(be64(&b[40..])));
    if be64(&b[32..]) == 0 || be64(&b[40..]) == 0 {
        return Err(NtpError::Bad("a reply with no receive or transmit time"));
    }
    if t3 < t2 {
        return Err(NtpError::Bad("a reply with timestamps that run backwards"));
    }
    let offset = ((t2 - t1) + (t3 - t4)) as f64 / 2.0;
    let delay = ((t4 - t1) - (t3 - t2)) as f64;
    // A little under zero is clock granularity; a lot is a server that stamps nonsense.
    if delay < -1_000_000.0 {
        return Err(NtpError::Bad("a reply with timestamps that run backwards"));
    }
    let refid = if stratum == 1 {
        b[12..16].iter().take_while(|c| **c != 0).filter(|c| c.is_ascii_graphic()).map(|c| *c as char).collect()
    } else {
        format!("{}.{}.{}.{}", b[12], b[13], b[14], b[15])
    };
    Ok(Sample {
        offset_ms: offset / 1e6,
        delay_ms: delay.max(0.0) / 1e6,
        stratum,
        leap: li,
        version: vn,
        refid,
        root_delay_ms: short_ms(be32(&b[4..])),
        root_dispersion_ms: short_ms(be32(&b[8..])),
        precision_ms: 2f64.powi(b[3] as i8 as i32) * 1000.0,
        poll: b[2],
    })
}

/// Checks a reply's extension fields against the request's unique ID; `Ok` accepts it.
type Vet<'a> = dyn FnMut(&[u8], u64) -> Result<(), NtpError> + 'a;

/// One exchange on a socket already connected to the server: sends what `make` builds from the transmit stamp, and
/// takes the first reply that answers it. `vet` gets the whole reply (header and extension fields) and the stamp sent,
/// and can reject it; a `Bad` rejection is a stray packet and keeps waiting, anything else ends the exchange.
fn exchange(sock: &UdpSocket, timeout: Duration, make: &dyn Fn(u64) -> Vec<u8>, vet: &mut Vet) -> Result<Sample, NtpError> {
    // The low 16 bits of the transmit time are random, so a reply can be matched to this request alone (RFC 5905 §8).
    let t1_wall = unix_ns_now();
    let sent = (to_ntp(t1_wall) & !0xffff) | fastrand::u16(..) as u64;
    let t1 = from_ntp(sent);
    let started = Instant::now();
    // The kernel stamps a reply as it arrives, so the time this thread waits to be scheduled isn't counted as network
    // delay. Without that stamp the elapsed time on this thread's own clock stands in for it.
    net::stamp_arrivals(sock);
    let sent_at = clock::now_ns();
    sock.send(&make(sent)).map_err(|e| NtpError::Io(e.to_string()))?;
    let mut buf = [0u8; 2048];
    loop {
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(NtpError::Timeout);
        }
        sock.set_read_timeout(Some(left)).map_err(|e| NtpError::Io(e.to_string()))?;
        match net::recv_stamped(sock, &mut buf) {
            Ok((n, arrived)) => {
                // T4 is T1 plus the round trip, which a clock being adjusted can't distort.
                let trip = match arrived {
                    Some(at) if at >= sent_at && at - sent_at < timeout.as_nanos() as u64 => (at - sent_at) as i128,
                    _ => started.elapsed().as_nanos() as i128,
                };
                let t4 = t1 + trip;
                match read_reply(&buf[..n], sent, t1, t4).and_then(|s| vet(&buf[..n], sent).map(|_| s)) {
                    // Not an answer to this request: keep waiting for the real one.
                    Err(NtpError::Bad(_)) if left > Duration::from_millis(5) => continue,
                    r => return r,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                return Err(NtpError::Timeout);
            }
            Err(e) => return Err(NtpError::Io(e.to_string())),
        }
    }
}

/// One plain NTP exchange on a socket already connected to the server.
pub fn query(sock: &UdpSocket, timeout: Duration) -> Result<Sample, NtpError> {
    exchange(sock, timeout, &|sent| request(sent).to_vec(), &mut |_, _| Ok(()))
}

fn connect(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let sock = UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
    sock.connect(addr)?;
    Ok(sock)
}

// ---------------------------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------------------------

pub(crate) fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    (!sorted.is_empty()).then(|| crate::stats::quantile(sorted, q))
}

/// [`quantile`] over `(value, weight)` pairs sorted by value: straight lines between the values, each sitting at the
/// middle of its weight, rescaled so the first is at 0 and the last at 1. Equal weights give exactly [`quantile`].
fn quantile_weighted(v: &[(f64, f64)], q: f64) -> Option<f64> {
    let (first, last) = (v.first()?, v.last()?);
    let span = v.iter().map(|x| x.1).sum::<f64>() - (first.1 + last.1) / 2.0;
    if v.len() == 1 || span <= 0.0 {
        return Some(first.0);
    }
    let target = q * span;
    let (mut before, mut prev) = (0.0, None);
    for &(x, w) in v {
        let pos = before + (w - first.1) / 2.0;
        before += w;
        if pos == target {
            return Some(x);
        }
        if pos > target {
            return Some(match prev {
                Some((px, pp)) => px + (x - px) * (target - pp) / (pos - pp),
                None => x,
            });
        }
        prev = Some((x, pos));
    }
    Some(last.0)
}

pub(crate) fn sorted(v: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut v: Vec<f64> = v.collect();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

/// What the queries to one address add up to.
#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AddrStats {
    pub ip: String,
    pub sent: u32,
    pub ok: u32,
    /// A Kiss-o'-Death code, if the server sent one.
    pub kiss: Option<String>,
    pub error: Option<String>,
    /// Why nothing answered, as a short stable name (see [`NtpError::kind`]).
    pub error_kind: Option<String>,
    /// The offset from the query with the smallest delay.
    pub offset_ms: Option<f64>,
    /// The smallest, median and 95th-percentile delay.
    pub delay_min_ms: Option<f64>,
    pub delay_ms: Option<f64>,
    pub delay_p95_ms: Option<f64>,
    /// How much the offset moved between queries: the RMS of the other offsets' distance from the best one. None
    /// with fewer than two answers.
    pub jitter_ms: Option<f64>,
    /// The most the offset can be wrong by, from path asymmetry alone: half the smallest delay.
    pub bound_ms: Option<f64>,
    /// The most this offset can be wrong by against UTC itself (RFC 5905's root distance): the server's own error
    /// (half its root delay plus its root dispersion) plus `bound_ms`.
    pub distance_ms: Option<f64>,
    pub stratum: Option<u8>,
    pub refid: Option<String>,
    pub root_delay_ms: Option<f64>,
    pub root_dispersion_ms: Option<f64>,
    pub leap: Option<u8>,
    pub version: Option<u8>,
}

/// Summarizes the outcome of each query to one address.
pub fn summarize(ip: &str, results: &[Result<Sample, NtpError>]) -> AddrStats {
    let ok: Vec<&Sample> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    let mut s = AddrStats { ip: ip.to_string(), sent: results.len() as u32, ok: ok.len() as u32, ..Default::default() };
    s.kiss = results.iter().find_map(|r| if let Err(NtpError::Kiss(k)) = r { Some(k.clone()) } else { None });
    let last = results.iter().rev().find_map(|r| r.as_ref().err()).filter(|_| ok.is_empty());
    s.error = last.map(|e| e.to_string());
    // A Kiss-o'-Death is the most telling thing an address said, so it names the failure even if a later query timed out.
    s.error_kind = last.map(|e| if s.kiss.is_some() { "kiss" } else { e.kind() }.to_string());
    let Some(best) = ok.iter().min_by(|a, b| a.delay_ms.total_cmp(&b.delay_ms)) else { return s };
    let delays = sorted(ok.iter().map(|x| x.delay_ms));
    s.offset_ms = Some(best.offset_ms);
    s.delay_min_ms = delays.first().copied();
    s.delay_ms = quantile(&delays, 0.5);
    // Of fewer than eight answers the 95th percentile is just the slowest one, so it isn't offered as one.
    s.delay_p95_ms = quantile(&delays, 0.95).filter(|_| delays.len() >= MAX_SAMPLES as usize);
    s.bound_ms = Some(best.delay_ms / 2.0);
    s.distance_ms = Some(best.root_delay_ms / 2.0 + best.root_dispersion_ms + best.delay_ms / 2.0);
    // RFC 5905 §10: the RMS over the *other* samples. One answer says nothing about spread, so there is no figure.
    s.jitter_ms =
        (ok.len() > 1).then(|| (ok.iter().map(|x| (x.offset_ms - best.offset_ms).powi(2)).sum::<f64>() / (ok.len() - 1) as f64).sqrt());
    s.stratum = Some(best.stratum);
    s.refid = Some(best.refid.clone());
    s.root_delay_ms = Some(best.root_delay_ms);
    s.root_dispersion_ms = Some(best.root_dispersion_ms);
    s.leap = Some(best.leap);
    s.version = Some(best.version);
    s
}

// ---------------------------------------------------------------------------------------------
// NTS key establishment (RFC 8915)
// ---------------------------------------------------------------------------------------------

/// The result of an NTS-KE handshake, and of the authenticated time exchange that follows it.
#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct NtsKe {
    pub ok: bool,
    pub host: String,
    pub port: u16,
    pub ms: Option<f64>,
    pub cookies: u32,
    /// The AEAD algorithm it chose (15 is AES-SIV-CMAC-256).
    pub aead: Option<u16>,
    /// The NTP server and port it sends clients to, when that isn't the same host and 123.
    pub server: Option<String>,
    pub ntp_port: Option<u16>,
    pub error: Option<String>,
    /// Why the key exchange failed, as a short stable name: `timeout`, `network`, `certificate_invalid`,
    /// `ke_handshake_failed` or `missing_cookies`.
    pub error_kind: Option<String>,
    /// Whether an authenticated query was answered with a reply that checked out. None when none was tried.
    pub authenticated: Option<bool>,
    /// What the authenticated queries found, summarized like any address's: offset, delay, and the failure's
    /// `error_kind` (`aead_failure`, `unauthenticated_response`, …) when they didn't verify.
    pub timed: Option<AddrStats>,
    /// The cookies the server handed out: one is spent per authenticated query.
    #[serde(skip)]
    jar: Vec<Vec<u8>>,
}

/// One NTS-KE record to send: a type, whether it is critical, and a body.
fn record(typ: u16, critical: bool, body: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(4 + body.len());
    r.extend_from_slice(&(typ | if critical { 0x8000 } else { 0 }).to_be_bytes());
    r.extend_from_slice(&(body.len() as u16).to_be_bytes());
    r.extend_from_slice(body);
    r
}

/// The AEAD every NTS server must offer: AES-SIV-CMAC-256, with a 32-byte key.
const AEAD_SIV_256: u16 = 15;

/// The request: negotiate NTPv4 and AES-SIV-CMAC-256.
fn nts_request() -> Vec<u8> {
    let mut r = record(1, true, &0u16.to_be_bytes());
    r.extend(record(4, false, &AEAD_SIV_256.to_be_bytes()));
    r.extend(record(0, true, &[]));
    r
}

/// The whole record stream must arrive within this, and stay within this many records and bytes.
const KE_DEADLINE: Duration = Duration::from_secs(10);
const KE_MAX_RECORDS: usize = 256;
const KE_MAX_BYTES: usize = 64 * 1024;

/// Reads the server's records up to End of Message. The error carries its kind.
fn read_nts(stream: &mut impl Read) -> Result<NtsKe, (&'static str, String)> {
    let io = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ("timeout", e.to_string()),
        _ => ("ke_handshake_failed", e.to_string()),
    };
    let mut out = NtsKe::default();
    let mut proto: Option<u16> = None;
    let (deadline, mut records, mut bytes) = (Instant::now() + KE_DEADLINE, 0, 0);
    loop {
        if Instant::now() > deadline {
            return Err(("timeout", "the server took too long to send its records".into()));
        }
        let mut head = [0u8; 4];
        stream.read_exact(&mut head).map_err(io)?;
        let critical = head[0] & 0x80 != 0;
        let typ = u16::from_be_bytes([head[0], head[1]]) & 0x7fff;
        let len = u16::from_be_bytes([head[2], head[3]]) as usize;
        records += 1;
        bytes += 4 + len;
        if records > KE_MAX_RECORDS || bytes > KE_MAX_BYTES {
            return Err(("ke_handshake_failed", "the server sent too many records".into()));
        }
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).map_err(io)?;
        match typ {
            0 => break,
            1 if len >= 2 => proto = Some(u16::from_be_bytes([body[0], body[1]])),
            2 if len >= 2 => {
                return Err((
                    "ke_handshake_failed",
                    format!("the server refused: NTS-KE error {}", u16::from_be_bytes([body[0], body[1]])),
                ));
            }
            4 if len >= 2 => out.aead = Some(u16::from_be_bytes([body[0], body[1]])),
            5 => {
                out.cookies += 1;
                out.jar.push(body);
            }
            6 => out.server = Some(crate::forwarder::clean(&String::from_utf8_lossy(&body))),
            7 if len >= 2 => out.ntp_port = Some(u16::from_be_bytes([body[0], body[1]])),
            // RFC 8915 §4: a critical record we don't understand ends the exchange.
            t if critical && t > 7 => {
                return Err(("ke_handshake_failed", format!("the server sent a critical record we don't understand (type {t})")));
            }
            _ => {}
        }
        if out.cookies > 64 {
            break;
        }
    }
    if proto != Some(0) {
        return Err(("ke_handshake_failed", "the server didn't agree to NTPv4".into()));
    }
    out.ok = out.aead.is_some() && out.cookies > 0;
    if !out.ok {
        let (kind, why) = if out.cookies == 0 && out.aead.is_some() {
            ("missing_cookies", "the handshake finished but the server sent no cookies")
        } else {
            ("ke_handshake_failed", "the handshake finished without an algorithm or cookies")
        };
        out.error = Some(why.into());
        out.error_kind = Some(kind.into());
    }
    Ok(out)
}

/// The keys an NTS-KE session yields: one signs requests, the other checks replies.
struct NtsKeys {
    c2s: [u8; 32],
    s2c: [u8; 32],
}

/// Sorts a failure on the TLS stream into a kind: a bad certificate, a timeout, or the network.
fn ke_failure(e: &std::io::Error) -> &'static str {
    if let Some(rustls::Error::InvalidCertificate(_)) = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) {
        return "certificate_invalid";
    }
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "timeout",
        _ if e.get_ref().is_some_and(|i| i.is::<rustls::Error>()) => "ke_handshake_failed",
        _ => "network",
    }
}

/// A TCP stream whose every read and write must finish before one instant, however slowly the peer drips.
struct Deadlined {
    tcp: TcpStream,
    deadline: Instant,
}

impl Deadlined {
    /// Time left for the next read or write, or a timeout error once there is none.
    fn left(&self) -> std::io::Result<Duration> {
        Some(self.deadline.saturating_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
            .ok_or_else(|| std::io::ErrorKind::TimedOut.into())
    }
}

impl Read for Deadlined {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.tcp.set_read_timeout(Some(self.left()?))?;
        self.tcp.read(buf)
    }
}

impl Write for Deadlined {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.tcp.set_write_timeout(Some(self.left()?))?;
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tcp.flush()
    }
}

/// Does `host` complete an NTS key exchange on `port` (4460 unless told otherwise)? A TLS 1.3 handshake with a
/// certificate that checks out, then the NTS-KE records; and, if it did, the keys that authenticated queries need.
/// Blocking, and over within `timeout` in all: connecting, the handshake and the records share that one budget.
fn ke_session(host: &str, port: u16, timeout: Duration) -> (NtsKe, Option<NtsKeys>) {
    let mut r = NtsKe { host: host.to_string(), port, ..Default::default() };
    let started = Instant::now();
    let deadline = started + timeout;
    let attempt = || -> Result<(NtsKe, Option<NtsKeys>), (&'static str, String)> {
        let addr = (host, port)
            .to_socket_addrs()
            .map_err(|e| ("dns", e.to_string()))?
            .find(|a| a.is_ipv4())
            .ok_or(("dns", "no IPv4 address".to_string()))?;
        let tcp = TcpStream::connect_timeout(&addr, deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1)))
            .map_err(|e| (ke_failure(&e), e.to_string()))?;
        let tcp = Deadlined { tcp, deadline };
        let mut config = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_root_certificates(crate::net::root_store())
            .with_no_client_auth();
        config.alpn_protocols = vec![b"ntske/1".to_vec()];
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| ("ke_handshake_failed", e.to_string()))?;
        let conn = rustls::ClientConnection::new(std::sync::Arc::new(config), name).map_err(|e| ("ke_handshake_failed", e.to_string()))?;
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        tls.write_all(&nts_request()).map_err(|e| (ke_failure(&e), e.to_string()))?;
        tls.flush().map_err(|e| (ke_failure(&e), e.to_string()))?;
        if tls.conn.alpn_protocol() != Some(b"ntske/1") {
            return Err(("ke_handshake_failed", "the server did not negotiate ntske/1".into()));
        }
        let k = read_nts(&mut tls)?;
        // RFC 8915 §4.2: the keys come from the TLS session, bound to the protocol and algorithm negotiated.
        let keys = (k.ok && k.aead == Some(AEAD_SIV_256)).then(|| {
            let key = |dir: u8| {
                let mut ctx = vec![0, 0];
                ctx.extend(AEAD_SIV_256.to_be_bytes());
                ctx.push(dir);
                tls.conn.export_keying_material([0u8; 32], b"EXPORTER-network-time-security", Some(&ctx)).ok()
            };
            Some(NtsKeys { c2s: key(0)?, s2c: key(1)? })
        });
        Ok((k, keys.flatten()))
    };
    let keys = match attempt() {
        Ok((k, keys)) => {
            r.ok = k.ok;
            r.cookies = k.cookies;
            r.aead = k.aead;
            r.server = k.server;
            r.ntp_port = k.ntp_port;
            r.error = k.error;
            r.error_kind = k.error_kind;
            r.jar = k.jar;
            keys
        }
        Err((kind, why)) => {
            r.error = Some(crate::forwarder::clean(&why));
            r.error_kind = Some(kind.to_string());
            None
        }
    };
    r.ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    (r, keys)
}

/// The key exchange alone, as far as telling whether `host` offers NTS goes.
pub fn nts_ke(host: &str, port: u16, timeout: Duration) -> NtsKe {
    ke_session(host, port, timeout).0
}

// ---------------------------------------------------------------------------------------------
// Authenticated time (RFC 8915 §5)
// ---------------------------------------------------------------------------------------------

const EXT_UNIQUE_ID: u16 = 0x0104;
const EXT_COOKIE: u16 = 0x0204;
const EXT_AUTHENTICATOR: u16 = 0x0404;

/// Appends one NTP extension field: type, length (the whole field, padded to four bytes), body.
fn push_ext(p: &mut Vec<u8>, typ: u16, body: &[u8]) {
    let len = (4 + body.len()).next_multiple_of(4);
    p.extend_from_slice(&typ.to_be_bytes());
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(body);
    p.resize(p.len() + len - 4 - body.len(), 0);
}

fn siv(key: &[u8; 32]) -> Option<aes_siv::siv::Aes128Siv> {
    use aes_siv::KeyInit;
    aes_siv::siv::Aes128Siv::new_from_slice(key).ok()
}

/// An authenticated request: the plain one, a Unique Identifier to tie the reply to it, a cookie, and an authenticator
/// over everything before it.
fn nts_packet(sent: u64, keys: &NtsKeys, uid: &[u8; 32], cookie: &[u8]) -> Vec<u8> {
    let mut p = request(sent).to_vec();
    push_ext(&mut p, EXT_UNIQUE_ID, uid);
    push_ext(&mut p, EXT_COOKIE, cookie);
    let nonce: [u8; 16] = std::array::from_fn(|_| fastrand::u8(..));
    let sealed = siv(&keys.c2s).and_then(|mut c| c.encrypt([&p[..], &nonce[..]], b"").ok()).unwrap_or_default();
    let mut body = Vec::with_capacity(4 + nonce.len() + sealed.len());
    body.extend_from_slice(&(nonce.len() as u16).to_be_bytes());
    body.extend_from_slice(&(sealed.len() as u16).to_be_bytes());
    body.extend_from_slice(&nonce);
    body.extend_from_slice(&sealed);
    push_ext(&mut p, EXT_AUTHENTICATOR, &body);
    p
}

/// Checks that a reply is the server's answer to this request and wasn't touched on the way: it must carry our Unique
/// Identifier and an authenticator that opens under the server-to-client key. Cookies it sends back are added to `fresh`.
fn vet_nts(reply: &[u8], uid: &[u8; 32], keys: &NtsKeys, fresh: &mut Vec<Vec<u8>>) -> Result<(), NtpError> {
    let bad = |kind, why: &str| NtpError::Nts(kind, why.to_string());
    if reply.len() <= 48 {
        return Err(bad("unauthenticated_response", "the server answered with plain NTP: no NTS extension fields"));
    }
    let (mut at, mut uid_ok, mut auth) = (48, false, None);
    while at + 4 <= reply.len() {
        let typ = u16::from_be_bytes([reply[at], reply[at + 1]]);
        let len = u16::from_be_bytes([reply[at + 2], reply[at + 3]]) as usize;
        if len < 4 || !len.is_multiple_of(4) || at + len > reply.len() {
            return Err(bad("malformed_extensions", "an NTP extension field with a bad length"));
        }
        let body = &reply[at + 4..at + len];
        match typ {
            EXT_UNIQUE_ID => uid_ok = body == uid,
            EXT_AUTHENTICATOR => {
                auth = Some((at, body));
                break;
            }
            _ => {}
        }
        at += len;
    }
    let Some((start, body)) = auth else {
        return Err(bad("missing_authenticator", "the reply has no NTS authenticator"));
    };
    if !uid_ok {
        return Err(bad("invalid_unique_id", "the reply doesn't carry this request's unique identifier"));
    }
    if body.len() < 4 {
        return Err(bad("malformed_extensions", "a truncated NTS authenticator"));
    }
    let (nl, cl) = (u16::from_be_bytes([body[0], body[1]]) as usize, u16::from_be_bytes([body[2], body[3]]) as usize);
    let cstart = 4 + nl.next_multiple_of(4);
    if body.len() < cstart + cl {
        return Err(bad("malformed_extensions", "a truncated NTS authenticator"));
    }
    let (nonce, sealed) = (&body[4..4 + nl], &body[cstart..cstart + cl]);
    let plain = siv(&keys.s2c)
        .and_then(|mut c| c.decrypt([&reply[..start], nonce], sealed).ok())
        .ok_or_else(|| bad("aead_failure", "the reply's authenticator doesn't verify: it may have been altered"))?;
    // The encrypted part is more extension fields: the cookies to use next.
    let mut at = 0;
    while at + 4 <= plain.len() {
        let typ = u16::from_be_bytes([plain[at], plain[at + 1]]);
        let len = u16::from_be_bytes([plain[at + 2], plain[at + 3]]) as usize;
        if len < 4 || at + len > plain.len() {
            break;
        }
        if typ == EXT_COOKIE && fresh.len() < 8 {
            fresh.push(plain[at + 4..at + len].to_vec());
        }
        at += len;
    }
    Ok(())
}

/// One authenticated exchange, spending `cookie`.
fn nts_query(sock: &UdpSocket, keys: &NtsKeys, cookie: &[u8], fresh: &mut Vec<Vec<u8>>, timeout: Duration) -> Result<Sample, NtpError> {
    let uid: [u8; 32] = std::array::from_fn(|_| fastrand::u8(..));
    exchange(sock, timeout, &|sent| nts_packet(sent, keys, &uid, cookie), &mut |reply, _| vet_nts(reply, &uid, keys, fresh))
}

/// Does `host` do NTS, and does its time verify? The key exchange first, then up to `plan.samples` authenticated
/// queries to the server it names, spent from the cookies it gave, summarized like any address's.
fn nts_check(host: &str, port: u16, plan: &Plan, cancel: &AtomicBool) -> NtsKe {
    let (mut ke, keys) = ke_session(host, port, Duration::from_millis(4000));
    let Some(keys) = keys.filter(|_| ke.ok && plan.samples > 0) else { return ke };
    let target = ke.server.clone().unwrap_or_else(|| host.to_string());
    let Some(addr) = (target.as_str(), ke.ntp_port.unwrap_or(123)).to_socket_addrs().ok().and_then(|mut a| a.find(|a| a.is_ipv4())) else {
        ke.timed = Some(AddrStats {
            ip: target,
            error: Some("the server NTS-KE named didn't resolve".into()),
            error_kind: Some("dns".into()),
            ..Default::default()
        });
        ke.authenticated = Some(false);
        return ke;
    };
    let ip = addr.ip().to_string();
    let Ok(sock) = connect(addr) else { return ke };
    let spacing = plan.spacing.max(plan.floor);
    if !plan.stagger.is_zero() {
        pause(Duration::from_millis(fastrand::u64(0..plan.stagger.as_millis() as u64)), cancel);
    }
    let mut jar = std::mem::take(&mut ke.jar);
    let (mut results, mut misses) = (Vec::new(), 0);
    for i in 0..plan.samples.min(MAX_SAMPLES) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if i > 0 {
            pause(spacing, cancel);
        }
        if jar.is_empty() {
            results.push(Err(NtpError::Nts("missing_cookies", "ran out of cookies".into())));
            break;
        }
        let cookie = jar.remove(0);
        let mut fresh = Vec::new();
        let r = nts_query(&sock, &keys, &cookie, &mut fresh, plan.timeout);
        if r.is_ok() {
            jar.append(&mut fresh);
        }
        misses = if r.is_ok() { 0 } else { misses + 1 };
        // A reply that fails to verify, or a Kiss-o'-Death, ends it: no point asking again.
        let stop = matches!(r, Err(NtpError::Kiss(_)) | Err(NtpError::Nts(..))) || misses >= GIVE_UP_AFTER;
        results.push(r);
        if stop {
            break;
        }
    }
    let t = summarize(&ip, &results);
    ke.authenticated = Some(t.ok > 0);
    ke.timed = Some(t);
    ke
}

// ---------------------------------------------------------------------------------------------
// Measuring a server
// ---------------------------------------------------------------------------------------------

/// What measuring one named server found: every address it resolves to (up to four), together.
#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ServerResult {
    pub id: String,
    pub host: String,
    pub addrs: Vec<AddrStats>,
    /// Why nothing was measured, when nothing was: the name didn't resolve, say.
    pub error: Option<String>,
    /// Why, as a short stable name: `dns`, `no_address`, `timeout`, `kiss`, `unsynchronized`, `bad_reply`, `network`.
    pub error_kind: Option<String>,
    /// The median of the addresses' offsets, delays and so on: the server's headline numbers.
    pub offset_ms: Option<f64>,
    pub delay_min_ms: Option<f64>,
    pub delay_ms: Option<f64>,
    pub delay_p95_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub bound_ms: Option<f64>,
    /// The median of the addresses' [`AddrStats::distance_ms`]: the most this server can be wrong about UTC, as seen from here.
    pub distance_ms: Option<f64>,
    /// How far apart the addresses' offsets are, from the lowest to the highest, when two or more answered.
    pub addr_spread_ms: Option<f64>,
    /// Two addresses of this name disagree by more than their error bounds allow: one of them is wrong.
    pub addrs_disagree: bool,
    pub stratum: Option<u8>,
    pub refid: Option<String>,
    pub root_delay_ms: Option<f64>,
    pub root_dispersion_ms: Option<f64>,
    /// Queries sent and answered, over all addresses.
    pub sent: u32,
    pub ok: u32,
    pub kiss: Option<String>,
    /// How far this server's clock is from the consensus of all servers measured, filled in once they all are.
    pub deviation_ms: Option<f64>,
    /// Whether `deviation_ms` is within what this server's and the consensus's error bounds allow. None when there is
    /// no consensus to compare with.
    pub agrees: Option<bool>,
    /// Whether the servers' error intervals put this one among the majority that overlap (a truechimer) or outside
    /// it (a falseticker). None for servers that didn't vote, or when no majority overlapped.
    pub selection: Option<Selection>,
    pub nts: Option<NtsKe>,
    /// IPv6: "answers", "unreachable" (an address, but this network can't reach it) or "no address"; None when IPv6 wasn't tried.
    pub ipv6: Option<String>,
    /// Only the NTS handshake was checked: this host doesn't answer plain NTP.
    pub nts_only: bool,
    /// A host the user typed, not one of the catalogue's: it is measured and judged against the others but never votes.
    pub custom: bool,
    /// The UDP port, when it isn't 123.
    pub port: Option<u16>,
    /// Who runs it, from the catalogue; servers of one operator are counted as one voice.
    pub operator: String,
}

fn median_of(v: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    quantile(&sorted(v.flatten()), 0.5)
}

impl ServerResult {
    /// How far this server's offset can be from UTC: its root distance, or just the path bound if that is missing.
    pub fn reach_ms(&self) -> Option<f64> {
        self.distance_ms.or(self.bound_ms)
    }

    /// Fills the headline numbers from the addresses.
    fn roll_up(&mut self) {
        let live: Vec<&AddrStats> = self.addrs.iter().filter(|a| a.ok > 0).collect();
        self.sent = self.addrs.iter().map(|a| a.sent).sum();
        self.ok = self.addrs.iter().map(|a| a.ok).sum();
        self.kiss = self.addrs.iter().find_map(|a| a.kiss.clone());
        self.offset_ms = median_of(live.iter().map(|a| a.offset_ms));
        self.delay_min_ms = median_of(live.iter().map(|a| a.delay_min_ms));
        self.delay_ms = median_of(live.iter().map(|a| a.delay_ms));
        self.delay_p95_ms = median_of(live.iter().map(|a| a.delay_p95_ms));
        self.jitter_ms = median_of(live.iter().map(|a| a.jitter_ms));
        self.bound_ms = median_of(live.iter().map(|a| a.bound_ms));
        self.distance_ms = median_of(live.iter().map(|a| a.distance_ms));
        let offsets: Vec<(f64, f64)> = live.iter().filter_map(|a| Some((a.offset_ms?, a.distance_ms.or(a.bound_ms)?))).collect();
        self.addr_spread_ms = (offsets.len() > 1).then(|| {
            let o = sorted(offsets.iter().map(|p| p.0));
            o[o.len() - 1] - o[0]
        });
        self.addrs_disagree = offsets.iter().enumerate().any(|(i, a)| offsets[i + 1..].iter().any(|b| (a.0 - b.0).abs() > a.1 + b.1));
        self.root_delay_ms = median_of(live.iter().map(|a| a.root_delay_ms));
        self.root_dispersion_ms = median_of(live.iter().map(|a| a.root_dispersion_ms));
        self.stratum = live.iter().filter_map(|a| a.stratum).min();
        self.refid = live.first().and_then(|a| a.refid.clone());
        if live.is_empty() {
            self.error = self.error.take().or_else(|| self.addrs.iter().find_map(|a| a.error.clone())).or(Some("no answer".into()));
            let kind = self.error_kind.take().or_else(|| {
                let kinds = self.addrs.iter().filter_map(|a| a.error_kind.as_deref());
                kinds.clone().find(|k| *k == "kiss").or_else(|| kinds.clone().find(|k| *k != "timeout")).map(String::from)
            });
            self.error_kind = Some(kind.unwrap_or_else(|| "timeout".into()));
        }
    }
}

/// How a run is asked to go.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Queries per address.
    pub samples: u32,
    /// Between queries to one address; never less than [`MIN_SPACING`].
    pub spacing: Duration,
    /// The shortest `spacing` honoured: [`MIN_SPACING`] for public servers.
    pub floor: Duration,
    /// Each address waits a random time up to this before its first query, so a run isn't one burst at every server at once.
    pub stagger: Duration,
    pub timeout: Duration,
    /// Also try one IPv6 address per server that has one.
    pub ipv6: bool,
}

impl Default for Plan {
    fn default() -> Self {
        Plan {
            samples: 4,
            spacing: MIN_SPACING,
            floor: MIN_SPACING,
            stagger: MIN_SPACING,
            timeout: Duration::from_millis(2500),
            ipv6: true,
        }
    }
}

impl Plan {
    /// The plan for a host the user typed: asked [`CUSTOM_SPACING`] apart, with no stagger.
    fn for_custom(&self) -> Plan {
        Plan { spacing: CUSTOM_SPACING, floor: CUSTOM_SPACING, stagger: Duration::ZERO, ..self.clone() }
    }
}

/// Waits `d`, in short slices so a cancel is heard.
fn pause(d: Duration, cancel: &AtomicBool) {
    let end = Instant::now() + d;
    while Instant::now() < end && !cancel.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())));
    }
}

/// The first IPv6 address `host` has, asked of the system resolver: the system's lookup leaves AAAA records out when
/// this computer has no IPv6 route, which would hide that the server has one.
fn aaaa(host: &str, port: u16) -> Option<SocketAddr> {
    let m = crate::resolve::Resolve::new(crate::sysdns::discover().into_iter().take(1).collect()).query(host, dns::TYPE_AAAA).ok()?;
    m.ipv6_answers().first().map(|a| SocketAddr::new(IpAddr::V6(*a), port))
}

/// Queries one address `plan.samples` times.
fn measure_addr(addr: SocketAddr, plan: &Plan, cancel: &AtomicBool) -> AddrStats {
    let spacing = plan.spacing.max(plan.floor);
    let sock = match connect(addr) {
        Ok(s) => s,
        Err(e) => {
            return AddrStats { ip: addr.ip().to_string(), error: Some(crate::forwarder::clean(&e.to_string())), ..Default::default() };
        }
    };
    let mut results = Vec::new();
    if !plan.stagger.is_zero() {
        pause(Duration::from_millis(fastrand::u64(0..plan.stagger.as_millis() as u64)), cancel);
    }
    let mut misses = 0;
    for i in 0..plan.samples.min(MAX_SAMPLES) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if i > 0 {
            pause(spacing, cancel);
        }
        let r = query(&sock, plan.timeout);
        // A server that says stop is believed, and one that doesn't answer isn't pestered.
        misses = if r.is_ok() { 0 } else { misses + 1 };
        let stop = matches!(r, Err(NtpError::Kiss(_))) || misses >= GIVE_UP_AFTER;
        results.push(r);
        if stop {
            break;
        }
    }
    summarize(&addr.ip().to_string(), &results)
}

/// Measures one named server: resolves it, asks each address (at most four IPv4, one IPv6) in turn from its own thread,
/// and rolls them up. `nts` is the key-exchange host and port to check, if any.
fn measure_server(id: &str, host: &str, port: u16, nts: Option<(&str, u16)>, plan: &Plan, cancel: &AtomicBool) -> ServerResult {
    let mut res = ServerResult { id: id.to_string(), host: host.to_string(), ..Default::default() };
    let all: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => {
            res.error = Some("the name didn't resolve".into());
            res.error_kind = Some("dns".into());
            return res;
        }
    };
    let mut v4: Vec<SocketAddr> = Vec::new();
    for a in all.iter().filter(|a| a.is_ipv4()) {
        if v4.len() < 4 && !v4.iter().any(|b| b.ip() == a.ip()) {
            v4.push(*a);
        }
    }
    // The extra AAAA lookup is only for a name, and only when IPv6 is wanted; otherwise IPv6 isn't tried or reported.
    let six = all.iter().find(|a| a.is_ipv6()).copied();
    let six = six.or_else(|| (plan.ipv6 && host.parse::<IpAddr>().is_err()).then(|| aaaa(host, port)).flatten());
    let v6 = six.filter(|_| plan.ipv6);
    if plan.ipv6 {
        res.ipv6 = Some(if six.is_some() { "unreachable".into() } else { "no address".into() });
    }
    if v4.is_empty() && v6.is_none() {
        res.error = Some("it has no IPv4 address".into());
        res.error_kind = Some("no_address".into());
        return res;
    }
    // Many short-lived workers (one per address), so small stacks.
    let (addrs, six, key) = std::thread::scope(|sc| {
        let small = || std::thread::Builder::new().stack_size(512 << 10);
        let hs: Vec<_> = v4.iter().map(|a| small().spawn_scoped(sc, move || measure_addr(*a, plan, cancel))).collect();
        let six = v6.map(|a| small().spawn_scoped(sc, move || measure_addr(a, plan, cancel)));
        let key = nts.map(|(h, p)| std::thread::Builder::new().stack_size(1 << 20).spawn_scoped(sc, move || nts_check(h, p, plan, cancel)));
        (
            hs.into_iter().filter_map(|h| h.ok()?.join().ok()).collect::<Vec<_>>(),
            six.and_then(|h| h.ok()?.join().ok()),
            key.and_then(|h| h.ok()?.join().ok()),
        )
    });
    res.addrs = addrs;
    res.nts = key;
    if let Some(s) = six {
        if s.ok > 0 {
            res.ipv6 = Some("answers".into());
        }
        res.addrs.push(s);
    }
    res.roll_up();
    res
}

/// The fewest voters a consensus needs: with three a bad clock can be outvoted (RFC 5905 §11.2 asks for the same).
pub const MIN_VOTERS: usize = 3;

/// What RFC 5905's selection made of a server whose clock was put to the vote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Selection {
    /// Its interval overlaps the majority's.
    Truechimer,
    /// It can't be right if the majority is.
    Falseticker,
}

/// RFC 5905 §11.2.1: each `(offset, reach)` is an interval the true offset should lie in. Finds the smallest span
/// that holds the midpoints of a majority of the intervals and that the majority all overlap, allowing more
/// falsetickers (f) one at a time until it exists. Returns, for each, whether its midpoint is inside the span, or
/// None if no majority overlap at all.
fn select(spans: &[(f64, f64)]) -> Option<Vec<bool>> {
    let n = spans.len();
    let mut edges: Vec<(f64, i32)> = spans.iter().flat_map(|&(o, r)| [(o - r, -1), (o, 0), (o + r, 1)]).collect();
    // Touching intervals count as overlapping: lower edges sort before upper ones at the same value.
    edges.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut f = 0;
    while 2 * f < n {
        let want = (n - f) as i32;
        let (mut chime, mut found) = (0, 0);
        let mut low = None;
        for &(v, t) in &edges {
            chime -= t;
            if chime >= want {
                low = Some(v);
                break;
            }
            if t == 0 {
                found += 1;
            }
        }
        chime = 0;
        let mut high = None;
        for &(v, t) in edges.iter().rev() {
            chime += t;
            if chime >= want {
                high = Some(v);
                break;
            }
            if t == 0 {
                found += 1;
            }
        }
        if let (Some(l), Some(h)) = (low, high)
            && found <= f
            && l <= h
        {
            return Some(spans.iter().map(|&(o, _)| o >= l && o <= h).collect());
        }
        f += 1;
    }
    None
}

/// Sorts servers into groups that fail together: the same operator, or the same upstream server (a stratum 2 or 3
/// reference ID is its upstream's address; at stratum 1 it only names a kind of clock). Returns each one's group
/// number, counting from 0 in order of first appearance. A server with no operator named stands alone.
fn independence_groups(items: &[(&str, Option<&str>)]) -> Vec<usize> {
    let mut group: Vec<usize> = (0..items.len()).collect();
    for i in 0..items.len() {
        for j in 0..i {
            let same_op = !items[i].0.is_empty() && items[i].0 == items[j].0;
            let same_up = items[i].1.is_some() && items[i].1 == items[j].1;
            if same_op || same_up {
                let (from, to) = (group[i].max(group[j]), group[i].min(group[j]));
                for g in group.iter_mut() {
                    if *g == from {
                        *g = to;
                    }
                }
            }
        }
    }
    let mut seen: Vec<usize> = Vec::new();
    group
        .iter()
        .map(|g| match seen.iter().position(|s| s == g) {
            Some(p) => p,
            None => {
                seen.push(*g);
                seen.len() - 1
            }
        })
        .collect()
}

/// The consensus of the servers measured: the median offset of the truechimers among those good enough to vote,
/// and how far the middle half of them spread. A server votes if it is synchronized (stratum 1 to 3), answered most
/// queries and its offset is pinned down to 50 ms; of those, [`select`] drops the falsetickers, whose error
/// intervals miss the majority's. Fills in each server's deviation from the consensus of the *others*, so a
/// server that sits at the median isn't credited with agreeing with itself, and whether it agrees: the two
/// can both be right if the gap is within what each one's error bound allows. The consensus's own bound is the
/// tightest of the voters', since servers reached over the same path share its error and agreeing with each other
/// doesn't shrink it.
pub fn consensus(results: &mut [ServerResult]) -> Option<Consensus> {
    let votes = |r: &ServerResult| {
        !r.custom
            && r.offset_ms.is_some()
            && r.stratum.is_some_and(|s| (1..=3).contains(&s))
            && r.sent > 0
            && r.ok * 2 >= r.sent
            && r.bound_ms.is_some_and(|b| b <= 50.0)
    };
    let idx: Vec<usize> = (0..results.len()).filter(|&i| votes(&results[i])).collect();
    let spans: Vec<(f64, f64)> = idx.iter().filter_map(|&i| Some((results[i].offset_ms?, results[i].reach_ms()?))).collect();
    for r in results.iter_mut() {
        r.selection = None;
    }
    if let Some(keep) = select(&spans) {
        for (&i, k) in idx.iter().zip(keep) {
            results[i].selection = Some(if k { Selection::Truechimer } else { Selection::Falseticker });
        }
    }
    // Falsetickers are measured against the consensus but don't make it.
    let counts = |r: &ServerResult| votes(r) && r.selection != Some(Selection::Falseticker);
    let counted: Vec<usize> = (0..results.len()).filter(|&i| counts(&results[i]) && results[i].offset_ms.is_some()).collect();
    let items: Vec<(&str, Option<&str>)> = counted
        .iter()
        .map(|&i| {
            let r = &results[i];
            (r.operator.as_str(), r.stratum.filter(|&s| s >= 2).and(r.refid.as_deref()))
        })
        .collect();
    let groups = independence_groups(&items);
    let independent = groups.iter().max().map_or(0, |m| m + 1);
    // A voice shared by several servers is spread over them: each weighs 1 / its group's size, so a group is worth
    // one server whatever its size, before the median and quartiles are taken.
    let size = |g: usize| groups.iter().filter(|x| **x == g).count();
    let mut voters: Vec<(usize, f64, f64)> =
        counted.iter().zip(&groups).map(|(&i, &g)| (i, results[i].offset_ms.unwrap_or(0.0), 1.0 / size(g) as f64)).collect();
    voters.sort_by(|a, b| a.1.total_cmp(&b.1));
    let weighted = |skip: Option<usize>| -> Vec<(f64, f64)> { voters.iter().filter(|v| Some(v.0) != skip).map(|v| (v.1, v.2)).collect() };
    let all = weighted(None);
    let median = quantile_weighted(&all, 0.5)?;
    let spread = quantile_weighted(&all, 0.75)? - quantile_weighted(&all, 0.25)?;
    let voted = independent;
    let n_voters = counted.len();
    let bound = results.iter().filter(|r| counts(r)).filter_map(|r| r.reach_ms()).min_by(f64::total_cmp)?;
    for (k, r) in results.iter_mut().enumerate() {
        r.agrees = None;
        let Some(o) = r.offset_ms else {
            r.deviation_ms = None;
            continue;
        };
        r.deviation_ms = if counts(r) {
            // Everyone else's median: leave this server's own vote out.
            quantile_weighted(&weighted(Some(k)), 0.5).map(|m| o - m)
        } else {
            Some(o - median)
        };
        r.agrees = r.deviation_ms.zip(r.reach_ms()).filter(|_| voted >= MIN_VOTERS).map(|(d, reach)| d.abs() <= reach + bound);
    }
    // Each server still gets its deviation above, but one or two voters can't outvote a bad clock, so there is no
    // verdict on this computer's clock.
    (voted >= MIN_VOTERS).then_some(Consensus {
        median_ms: median,
        spread_ms: spread,
        voters: n_voters,
        independent,
        bound_ms: bound,
        advice: Advice::default(),
    })
}

/// What to do about the servers measured.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Advice {
    /// Up to four servers worth configuring together, best first: truechimers that agree, synchronized to a clock
    /// within three steps, from different operators and upstreams so one failure can't take them all, and none that
    /// smears the leap second (which shouldn't be mixed with servers that step).
    pub set: Vec<String>,
    /// Servers to leave out, and the first reason found.
    pub avoid: Vec<Avoid>,
    /// Servers that qualified but didn't make the set, and why: another server of the same operator or upstream
    /// ranked ahead, or the set was already full.
    pub passed: Vec<Avoid>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Avoid {
    pub id: String,
    pub why: String,
}

/// Picks [`Advice`] from a finished run. Pools are never named, and a server that just didn't answer isn't
/// blamed: that may be this network.
fn advise(entries: &[Entry], results: &[ServerResult]) -> Advice {
    let entry = |id: &str| entries.iter().find(|e| e.id == id);
    let mut cands: Vec<(&Entry, &ServerResult)> = results
        .iter()
        .filter_map(|r| Some((entry(&r.id)?, r)))
        .filter(|(e, r)| {
            e.kind != "pool"
                && !e.nts_only
                && r.ok > 0
                && r.selection == Some(Selection::Truechimer)
                && r.agrees != Some(false)
                && !r.addrs_disagree
                && !e.leap.as_deref().is_some_and(|l| l.to_lowercase().contains("smear"))
        })
        .collect();
    // Best first by the error ceiling plus jitter: how far the offset can be wrong, and how much it wanders. That is
    // NTP's own synchronization distance, and it favours a near server only as far as being near makes it more certain.
    let fit = |r: &ServerResult| r.reach_ms().unwrap_or(f64::MAX) + r.jitter_ms.unwrap_or(0.0);
    cands.sort_by(|a, b| fit(a.1).total_cmp(&fit(b.1)));
    let (mut operators, mut upstreams) = (Vec::<(String, String)>::new(), Vec::<(String, String)>::new());
    let mut set = Vec::new();
    let mut passed = Vec::new();
    for (e, r) in cands {
        // A stratum 1 reference ID names a kind of clock (GPS, PPS), shared by unrelated servers; below that it
        // identifies the upstream server itself, which two servers sharing have in common.
        let up = r.stratum.filter(|&s| s >= 2).and(r.refid.clone());
        let name_of = |id: &str| entry(id).map_or(id.to_string(), |e| e.name.clone());
        let why = if set.len() >= 4 {
            Some("four better servers are already in the set".to_string())
        } else if let Some((_, by)) = operators.iter().find(|(o, _)| *o == e.operator) {
            Some(format!("{} is from the same operator and ranks ahead", name_of(by)))
        } else {
            up.as_ref()
                .and_then(|u| upstreams.iter().find(|(x, _)| x == u))
                .map(|(_, by)| format!("it shares an upstream clock with {}, which ranks ahead", name_of(by)))
        };
        if let Some(why) = why {
            passed.push(Avoid { id: r.id.clone(), why });
            continue;
        }
        operators.push((e.operator.clone(), r.id.clone()));
        if let Some(u) = up {
            upstreams.push((u, r.id.clone()));
        }
        set.push(r.id.clone());
    }
    let avoid = results
        .iter()
        .filter_map(|r| {
            let e = entry(&r.id).filter(|e| e.kind != "pool" && !e.nts_only)?;
            let why = if let Some(k) = &r.kiss {
                format!("it told us to stop ({k})")
            } else if r.ok == 0 {
                return None;
            } else if r.selection == Some(Selection::Falseticker) {
                "its clock disagrees with the majority by more than both their error ceilings allow".to_string()
            } else if r.stratum.is_none_or(|s| !(1..=3).contains(&s)) {
                format!("it isn't synchronized to a close clock (stratum {})", r.stratum.map_or("?".into(), |s| s.to_string()))
            } else if r.addrs_disagree {
                "its own addresses disagree with each other".to_string()
            } else if r.distance_ms.zip(r.bound_ms).is_some_and(|(d, b)| d - b > 50.0) {
                "its own clock chain is loose: it admits an error of more than 50 ms".to_string()
            } else {
                return None;
            };
            Some(Avoid { id: e.id.clone(), why })
        })
        .collect();
    Advice { set, avoid, passed }
}

/// When each name was last asked, and any Kiss-o'-Death it sent, kept between runs so that running again doesn't pester a server.
#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
pub struct Asked {
    /// When the last run started (Unix milliseconds).
    pub at: i64,
    /// The Kiss-o'-Death code it sent, and until when it is left alone.
    #[serde(default)]
    pub kiss: Option<String>,
    #[serde(default)]
    pub until: Option<i64>,
}

pub type Ledger = std::collections::BTreeMap<String, Asked>;

/// A server left out of a run, and why.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Skipped {
    pub id: String,
    pub host: String,
    /// "recent" (asked a few minutes ago) or "kiss" (it told us to stop or slow down).
    pub why: String,
    /// When it may be asked again (Unix milliseconds).
    pub until: i64,
    pub kiss: Option<String>,
}

/// Splits `entries` into those that may be asked now and those that may not: a name asked in the last ten minutes, or one
/// that sent a Kiss-o'-Death and is still being left alone.
pub fn guard(entries: &[Entry], ledger: &Ledger, now: i64) -> (Vec<Entry>, Vec<Skipped>) {
    let (mut ask, mut skip) = (Vec::new(), Vec::new());
    for e in entries {
        let key = if e.nts_only { e.nts.as_deref().unwrap_or(&e.host) } else { &e.host };
        let cool = if e.kind == "custom" { CUSTOM_COOLDOWN } else { COOLDOWN };
        let why = ledger.get(key).and_then(|a| match (a.until.filter(|u| *u > now), a.at + cool.as_millis() as i64) {
            (Some(u), _) => Some(("kiss", u, a.kiss.clone())),
            (None, next) if next > now => Some(("recent", next, None)),
            _ => None,
        });
        match why {
            Some((why, until, kiss)) => skip.push(Skipped { id: e.id.clone(), host: e.host.clone(), why: why.into(), until, kiss }),
            None => ask.push(e.clone()),
        }
    }
    (ask, skip)
}

/// Notes that these servers are being asked now (so a second run straight after leaves them be), and forgets anything
/// older than a month.
pub fn stamp(ledger: &mut Ledger, entries: &[Entry], now: i64) {
    for e in entries {
        let key = if e.nts_only { e.nts.clone().unwrap_or_else(|| e.host.clone()) } else { e.host.clone() };
        let a = ledger.entry(key).or_default();
        a.at = now;
        if a.until.is_some_and(|u| u <= now) {
            a.until = None;
            a.kiss = None;
        }
    }
    ledger.retain(|_, a| a.at > now - 30 * 86_400_000);
}

/// Remembers who told us to stop: RATE for an hour, DENY and RSTR for a week.
pub fn remember_kisses(ledger: &mut Ledger, results: &[ServerResult], now: i64) {
    for r in results {
        let Some(k) = &r.kiss else { continue };
        let back = if k == "RATE" { KISS_RATE_BACKOFF } else { KISS_DENY_BACKOFF };
        let a = ledger.entry(r.host.clone()).or_default();
        a.kiss = Some(k.clone());
        a.until = Some(now + back.as_millis() as i64);
    }
}

/// The most a run could send: names × addresses per name (four IPv4, one IPv6 and the authenticated NTS server) × queries.
/// Most names have one address.
pub fn most_packets(names: usize, samples: u32) -> usize {
    names * 6 * samples.min(MAX_SAMPLES) as usize
}

/// One server in the catalog: who runs it, and what its operator says about it. What can be measured is
/// measured; what is listed here is only what the operator publishes, and a field left out means nothing was found.
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Entry {
    pub id: String,
    /// What to call it.
    pub name: String,
    pub operator: String,
    /// service (a company's public NTP), lab (a national time service) or pool (volunteer servers).
    pub kind: String,
    pub host: String,
    /// The UDP port, when it isn't 123 (a host the user typed).
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub country: Option<String>,
    /// The host (and port, when not 4460) to try an NTS key exchange with, if the server might offer NTS.
    #[serde(default)]
    pub nts: Option<String>,
    /// Whether the operator documents NTS for it.
    #[serde(default)]
    pub nts_documented: bool,
    /// A key-exchange host that doesn't answer plain NTP: only the NTS handshake can be checked.
    #[serde(default)]
    pub nts_only: bool,
    /// How it handles a leap second, in the operator's words, if it says.
    #[serde(default)]
    pub leap: Option<String>,
    #[serde(default)]
    pub anycast: Option<bool>,
    /// What the operator says about use: limits, who may use it, what to avoid mixing it with.
    #[serde(default)]
    pub policy: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    /// Picked by default.
    #[serde(default)]
    pub default: bool,
}

pub fn catalog() -> Vec<Entry> {
    static CATALOG: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(include_str!("../../../data/ntp.json")).expect("data/ntp.json is valid")).clone()
}

/// A run of only the user's own servers has nothing to be judged against, so a few steady catalog servers come along.
/// Returns the entries and the names of the ones added.
pub fn with_references(mut entries: Vec<Entry>) -> (Vec<Entry>, Vec<String>) {
    let mut added = Vec::new();
    if !entries.is_empty() && entries.iter().all(|e| e.kind == "custom") {
        for e in catalog().into_iter().filter(|e| e.default && !e.nts_only && e.kind != "pool").take(4) {
            added.push(e.name.clone());
            entries.push(e);
        }
    }
    (entries, added)
}

/// Most hosts a person may add to one run.
pub const MAX_CUSTOM: usize = 4;

/// A server the user typed: `host`, `host:port`, `1.2.3.4:port` or `[2001:db8::1]:port`. It is measured like any other
/// and shown beside the rest, but it never votes: only a person can say it is a good reference. A host that is in the
/// catalog (on the default port) is that catalog entry.
pub fn custom_entry(text: &str) -> Result<Entry, String> {
    let t = text.trim();
    let bad = |why: &str| Err(format!("\u{201c}{}\u{201d} isn't a time server address: {why}.", t.chars().take(60).collect::<String>()));
    if t.is_empty() {
        return bad("it is empty");
    }
    let (host, port) = if let Some(rest) = t.strip_prefix('[') {
        let Some((h, after)) = rest.split_once(']') else { return bad("a bracketed IPv6 address needs a closing ]") };
        let port = match after.strip_prefix(':') {
            Some(p) => Some(p),
            None if after.is_empty() => None,
            None => return bad("only a port may follow the ]"),
        };
        (h, port)
    } else if t.parse::<std::net::Ipv6Addr>().is_ok() {
        (t, None)
    } else {
        match t.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (t, None),
        }
    };
    let port = match port {
        None => None,
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 => Some(n),
            _ => return bad("the port must be 1 to 65535"),
        },
    };
    let host = host.trim_end_matches('.');
    let literal = host.parse::<IpAddr>().is_ok();
    let name_ok = !host.is_empty()
        && host.len() <= 253
        && host
            .split('.')
            .all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        && !host.starts_with('-');
    if !literal && !name_ok {
        return bad("use a host name or an IP address");
    }
    let host = host.to_ascii_lowercase();
    let port = port.filter(|p| *p != 123);
    if port.is_none()
        && let Some(e) = catalog().into_iter().find(|e| e.host == host)
    {
        return Ok(e);
    }
    Ok(Entry {
        id: match port {
            Some(p) => format!("custom:{host}:{p}"),
            None => format!("custom:{host}"),
        },
        name: match port {
            Some(p) if host.contains(':') => format!("[{host}]:{p}"),
            Some(p) => format!("{host}:{p}"),
            None => host.clone(),
        },
        operator: "You".into(),
        kind: "custom".into(),
        nts: (!literal).then(|| host.clone()),
        host,
        port,
        country: None,
        nts_documented: false,
        nts_only: false,
        leap: None,
        anycast: None,
        policy: None,
        link: None,
        default: false,
    })
}

/// How the servers' clocks line up: the median offset of the servers fit to vote, their spread, and how many voted.
#[derive(Clone, Debug, Serialize)]
pub struct Consensus {
    pub median_ms: f64,
    pub spread_ms: f64,
    pub voters: usize,
    /// How many of the voters are independent: servers of one operator, or fed by one upstream server, count once, since
    /// they fail together. The verdict needs [`MIN_VOTERS`] of these, not of `voters`.
    pub independent: usize,
    /// How far the median can be wrong: the tightest voter's [`ServerResult::reach_ms`]. Much larger than `spread_ms`
    /// on a typical network, because the voters share this computer's path.
    pub bound_ms: f64,
    /// What to configure and what to avoid, from the same measurements.
    pub advice: Advice,
}

/// Measures every entry at once (each on its own thread, each address on another), calls `on_result` as each
/// finishes, and returns them in catalog order with the consensus worked out.
pub fn run(
    entries: &[Entry],
    plan: &Plan,
    nts: bool,
    cancel: &AtomicBool,
    on_result: &(dyn Fn(&ServerResult) + Sync),
) -> (Vec<ServerResult>, Option<Consensus>) {
    let mut results: Vec<ServerResult> = std::thread::scope(|sc| {
        let hs: Vec<_> = entries
            .iter()
            .map(|e| {
                std::thread::Builder::new().stack_size(512 << 10).spawn_scoped(sc, move || {
                    let own;
                    let plan = if e.kind == "custom" {
                        own = plan.for_custom();
                        &own
                    } else {
                        plan
                    };
                    let key = e.nts.as_deref().filter(|_| nts).map(|k| match k.rsplit_once(':') {
                        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) => (h, p.parse().unwrap_or(4460)),
                        _ => (k, 4460),
                    });
                    let r = if e.nts_only {
                        let (h, p) = key.unwrap_or((e.host.as_str(), 4460));
                        ServerResult {
                            id: e.id.clone(),
                            host: e.host.clone(),
                            nts: nts.then(|| nts_check(h, p, plan, cancel)),
                            nts_only: true,
                            ..Default::default()
                        }
                    } else {
                        measure_server(&e.id, &e.host, e.port.unwrap_or(123), key, plan, cancel)
                    };
                    let r = ServerResult { custom: e.kind == "custom", port: e.port, operator: e.operator.clone(), ..r };
                    on_result(&r);
                    r
                })
            })
            .collect();
        hs.into_iter()
            .zip(entries)
            .map(|(h, e)| {
                h.ok().and_then(|h| h.join().ok()).unwrap_or_else(|| ServerResult {
                    id: e.id.clone(),
                    host: e.host.clone(),
                    error: Some("couldn't start".into()),
                    error_kind: Some("internal".into()),
                    ..Default::default()
                })
            })
            .collect()
    });
    let mut c = consensus(&mut results);
    if let Some(c) = c.as_mut() {
        c.advice = advise(entries, &results);
    }
    (results, c)
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn the_catalog_is_well_formed() {
        let c = catalog();
        let mut ids = std::collections::HashSet::new();
        for e in &c {
            assert!(ids.insert(e.id.clone()), "duplicate id {}", e.id);
            assert!(matches!(e.kind.as_str(), "service" | "lab" | "pool"), "{}", e.id);
            assert!(e.host.contains('.') && !e.host.contains("://") && e.host == e.host.to_ascii_lowercase(), "{}", e.host);
            if let Some(n) = &e.nts {
                assert!(n.contains('.'), "{}", e.id);
            }
            assert!(!e.nts_documented || e.nts.is_some(), "{} documents NTS but has nothing to check", e.id);
            assert!(!e.nts_only || e.nts.is_some(), "{} is NTS-only but has no key-exchange host", e.id);
            if let Some(l) = &e.link {
                assert!(l.starts_with("https://"), "{}", e.id);
            }
        }
        assert!(c.iter().any(|e| e.default) && c.len() > 40);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(offset_ns: i128, one_way_ns: i128, stratum: u8, sent: u64, t1: i128) -> Vec<u8> {
        // A server whose clock is `offset_ns` ahead, with equal paths each way and no processing time.
        let t2 = t1 + one_way_ns + offset_ns;
        let mut b = vec![0u8; 48];
        b[0] = (4 << 3) | 4;
        b[1] = stratum;
        b[3] = (-20i8) as u8;
        b[12..16].copy_from_slice(b"GPS\0");
        b[24..32].copy_from_slice(&sent.to_be_bytes());
        b[32..40].copy_from_slice(&to_ntp(t2).to_be_bytes());
        b[40..48].copy_from_slice(&to_ntp(t2).to_be_bytes());
        b
    }

    #[test]
    fn timestamps_round_trip_and_cross_the_2036_rollover() {
        let now = 1_791_000_000i128 * NS + 123_456_789;
        assert!((from_ntp(to_ntp(now)) - now).abs() < 2, "{}", from_ntp(to_ntp(now)) - now);
        // The era-0 timestamp for 2037 has a clear top bit; it must still read as 2037.
        let y2037 = 2_114_380_800i128 * NS;
        assert!((from_ntp(to_ntp(y2037)) - y2037).abs() < 2);
        assert_eq!(from_ntp(to_ntp(0)), 0);
    }

    #[test]
    fn offset_and_delay_come_out_of_the_four_timestamps() {
        let t1 = 1_791_000_000i128 * NS;
        let sent = to_ntp(t1);
        // The server is 25 ms ahead and each direction takes 10 ms: T4 is 20 ms after T1.
        let b = reply(25_000_000, 10_000_000, 2, sent, from_ntp(sent));
        let s = read_reply(&b, sent, from_ntp(sent), from_ntp(sent) + 20_000_000).unwrap();
        assert!((s.offset_ms - 25.0).abs() < 0.001, "{}", s.offset_ms);
        assert!((s.delay_ms - 20.0).abs() < 0.001, "{}", s.delay_ms);
        assert_eq!((s.stratum, s.version, s.refid.as_str()), (2, 4, "71.80.83.0"));
        assert!((s.precision_ms - 2f64.powi(-20) * 1000.0).abs() < 1e-9);
        // Stratum 1 names its source.
        let b = reply(0, 5_000_000, 1, sent, from_ntp(sent));
        assert_eq!(read_reply(&b, sent, from_ntp(sent), from_ntp(sent) + 10_000_000).unwrap().refid, "GPS");
    }

    #[test]
    fn a_path_that_is_slower_one_way_shifts_the_offset_by_half_the_difference() {
        // 5 ms there, 15 ms back, clocks equal: θ comes out -5 ms. That is the error δ/2 = 10 ms bounds.
        let t1 = 1_791_000_000i128 * NS;
        let sent = to_ntp(t1);
        let b = reply(0, 5_000_000, 2, sent, from_ntp(sent));
        let s = read_reply(&b, sent, from_ntp(sent), from_ntp(sent) + 20_000_000).unwrap();
        assert!((s.offset_ms + 5.0).abs() < 0.001 && (s.delay_ms - 20.0).abs() < 0.001, "{s:?}");
    }

    #[test]
    fn replies_that_cannot_be_trusted_are_refused() {
        let t1 = 1_791_000_000i128 * NS;
        let sent = to_ntp(t1);
        let good = reply(0, 5_000_000, 2, sent, from_ntp(sent));
        let t4 = from_ntp(sent) + 10_000_000;
        let check = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = good.clone();
            f(&mut b);
            read_reply(&b, sent, from_ntp(sent), t4)
        };
        assert!(matches!(check(&|b| b[24] ^= 1), Err(NtpError::Bad(_))), "a reply to some other request");
        assert!(matches!(check(&|b| b[0] = (4 << 3) | 3), Err(NtpError::Bad(_))), "a client-mode packet");
        assert!(matches!(check(&|b| b.truncate(20)), Err(NtpError::Bad(_))));
        assert_eq!(check(&|b| b[0] |= 0xc0), Err(NtpError::Unsynchronized));
        assert_eq!(check(&|b| b[1] = 16), Err(NtpError::Unsynchronized));
        assert!(matches!(check(&|b| b[40..48].fill(0)), Err(NtpError::Bad(_))));
        assert!(matches!(check(&|b| b[32..40].fill(0)), Err(NtpError::Bad(_))), "no receive time");
        // Transmitted before it was received.
        assert!(matches!(
            check(&|b| {
                let t3 = to_ntp(from_ntp(be64(&b[32..])) - 1_000_000);
                b[40..48].copy_from_slice(&t3.to_be_bytes());
            }),
            Err(NtpError::Bad(_))
        ));
        // A Kiss-o'-Death names itself.
        let kod = check(&|b| {
            b[1] = 0;
            b[12..16].copy_from_slice(b"RATE");
        });
        assert_eq!(kod, Err(NtpError::Kiss("RATE".into())));
    }

    #[test]
    fn the_best_offset_is_from_the_query_with_the_smallest_delay() {
        let s = |offset_ms: f64, delay_ms: f64| {
            Ok(Sample {
                offset_ms,
                delay_ms,
                stratum: 2,
                leap: 0,
                version: 4,
                refid: "x".into(),
                root_delay_ms: 1.0,
                root_dispersion_ms: 2.0,
                precision_ms: 0.001,
                poll: 3,
            })
        };
        let r = vec![s(5.0, 30.0), s(1.0, 12.0), s(3.0, 20.0), Err(NtpError::Timeout)];
        let a = summarize("192.0.2.1", &r);
        assert_eq!((a.sent, a.ok), (4, 3));
        assert_eq!((a.offset_ms, a.bound_ms, a.delay_min_ms, a.delay_ms), (Some(1.0), Some(6.0), Some(12.0), Some(20.0)));
        // Jitter is how far the other offsets sit from the best one: sqrt((16 + 4) / 2).
        assert!((a.jitter_ms.unwrap() - 10.0f64.sqrt()).abs() < 1e-9);
        // One answer has no spread to report; zero would read as perfectly steady.
        assert_eq!(summarize("192.0.2.3", &r[1..2]).jitter_ms, None);
        let none = summarize("192.0.2.2", &[Err(NtpError::Timeout), Err(NtpError::Kiss("RATE".into()))]);
        assert_eq!((none.ok, none.kiss.as_deref(), none.offset_ms), (0, Some("RATE"), None));
    }

    #[test]
    fn the_consensus_is_the_median_of_servers_fit_to_vote() {
        let srv = |id: &str, offset: f64, stratum: u8, ok: u32, bound: f64| ServerResult {
            id: id.into(),
            offset_ms: Some(offset),
            stratum: Some(stratum),
            sent: 8,
            ok,
            bound_ms: Some(bound),
            ..Default::default()
        };
        let mut v = vec![
            srv("a", 2.0, 1, 8, 5.0),
            srv("b", 2.4, 2, 8, 9.0),
            srv("c", 1.8, 2, 8, 12.0),
            // None of these votes: unsynchronized, mostly lost, and unpinned.
            srv("d", 900.0, 16, 8, 5.0),
            srv("e", 300.0, 2, 2, 5.0),
            srv("f", 400.0, 2, 8, 80.0),
        ];
        let c = consensus(&mut v).unwrap();
        assert_eq!((c.voters, c.median_ms), (3, 2.0));
        assert!((c.spread_ms - 0.3).abs() < 1e-9, "{}", c.spread_ms);
        // The consensus is no surer than the surest voter: servers on one path share its error.
        assert_eq!(c.bound_ms, 5.0);
        // Within reach of the consensus, or not: a 898 ms gap is nowhere near covered by 5 + 5 ms.
        assert_eq!((v[0].agrees, v[1].agrees, v[3].agrees), (Some(true), Some(true), Some(false)));
        // b (2.4) against the median of a and c (2.0 and 1.8 → 1.9); a against b and c (2.1).
        assert_eq!(v[1].deviation_ms.map(|d| (d * 10.0).round()), Some(5.0));
        assert_eq!(v[0].deviation_ms.map(|d| (d * 10.0).round()), Some(-1.0));
        // A server at the median is not credited with agreeing with itself: with only itself and one other, it is compared with that.
        let mut two = vec![srv("x", 1.0, 1, 8, 5.0), srv("y", 4.0, 1, 8, 5.0)];
        // Two voters can't outvote a bad clock, so there is no verdict, but each still gets a deviation.
        assert!(consensus(&mut two).is_none());
        assert_eq!((two[0].deviation_ms, two[1].deviation_ms), (Some(-3.0), Some(3.0)));
        // With no verdict there is nothing to agree with.
        assert_eq!((two[0].agrees, two[1].agrees), (None, None));
        // Their deviation is still reported, even though they didn't vote.
        assert_eq!(v[3].deviation_ms, Some(898.0));
        assert!(consensus(&mut []).is_none());
    }

    #[test]
    fn a_servers_reach_is_its_root_distance_and_addresses_that_disagree_are_flagged() {
        let addr = |ip: &str, offset: f64, distance: f64| AddrStats {
            ip: ip.into(),
            ok: 4,
            sent: 4,
            offset_ms: Some(offset),
            bound_ms: Some(1.0),
            distance_ms: Some(distance),
            ..Default::default()
        };
        let roll = |addrs: Vec<AddrStats>| {
            let mut r = ServerResult { addrs, ..Default::default() };
            r.roll_up();
            r
        };
        // 3 ms apart, but each could be 2 ms out: they can both be right.
        let ok = roll(vec![addr("192.0.2.1", 0.0, 2.0), addr("192.0.2.2", 3.0, 2.0)]);
        assert!(!ok.addrs_disagree);
        assert_eq!((ok.addr_spread_ms, ok.distance_ms, ok.reach_ms()), (Some(3.0), Some(2.0), Some(2.0)));
        // 200 ms apart cannot be.
        let bad = roll(vec![addr("192.0.2.1", 0.0, 2.0), addr("192.0.2.2", 200.0, 2.0)]);
        assert!(bad.addrs_disagree);
        // One address has no spread, and no reach beyond its path bound if the server gave no root distance.
        let mut one = addr("192.0.2.1", 0.0, 2.0);
        one.distance_ms = None;
        let one = roll(vec![one]);
        assert_eq!((one.addr_spread_ms, one.addrs_disagree, one.reach_ms()), (None, false, Some(1.0)));
    }

    #[test]
    fn a_clock_outside_the_majoritys_intervals_is_a_falseticker() {
        // Three that overlap around 2 ms, one 300 ms out whose interval nowhere near reaches them.
        let keep = select(&[(2.0, 5.0), (2.4, 9.0), (1.8, 12.0), (300.0, 5.0)]).unwrap();
        assert_eq!(keep, vec![true, true, true, false]);
        // Wide intervals that all overlap leave nobody out, however far apart the midpoints.
        assert_eq!(select(&[(0.0, 30.0), (20.0, 30.0), (-20.0, 30.0)]).unwrap(), vec![true; 3]);
        // Two disjoint pairs: no majority overlaps, so nobody is picked.
        assert!(select(&[(0.0, 1.0), (0.5, 1.0), (100.0, 1.0), (100.5, 1.0)]).is_none());
        // Two servers can't outvote each other; one can stand alone.
        assert!(select(&[(0.0, 1.0), (50.0, 1.0)]).is_none());
        assert_eq!(select(&[(7.0, 1.0)]).unwrap(), vec![true]);
        assert!(select(&[]).is_none());
    }

    #[test]
    fn servers_that_fail_together_count_as_one_voice() {
        let srv = |id: &str, op: &str, offset: f64, refid: &str, stratum: u8| ServerResult {
            id: id.into(),
            operator: op.into(),
            offset_ms: Some(offset),
            stratum: Some(stratum),
            refid: Some(refid.into()),
            sent: 4,
            ok: 4,
            bound_ms: Some(20.0),
            ..Default::default()
        };
        // Three servers of one operator, and two more sharing an upstream: three voices, not five.
        let mut v = vec![
            srv("a1", "A", 10.0, "GPS", 1),
            srv("a2", "A", 10.0, "GPS", 1),
            srv("a3", "A", 10.0, "GPS", 1),
            srv("b", "B", 0.0, "192.0.2.9", 2),
            srv("c", "C", 0.0, "192.0.2.9", 2),
        ];
        assert!(consensus(&mut v).is_none());
        let mut four = v.clone();
        four.push(srv("d", "D", 1.0, "198.51.100.1", 2));
        let c = consensus(&mut four).unwrap();
        assert_eq!((c.voters, c.independent), (6, 3));
        // The three A servers weigh as much as one: the median sits between B/C's side and D, not at A's 10.
        assert!(c.median_ms < 5.0, "median {}", c.median_ms);
        // Two voices are not enough to outvote a bad clock, however many servers speak for them.
        let mut two: Vec<_> = v.drain(..).take(4).collect();
        assert!(consensus(&mut two).is_none());
        // Without operator names or shared upstreams every server stands alone.
        let mut alone = vec![srv("x", "", 1.0, "GPS", 1), srv("y", "", 2.0, "GPS", 1), srv("z", "", 3.0, "PPS", 1)];
        let c = consensus(&mut alone).unwrap();
        assert_eq!((c.voters, c.independent, c.median_ms), (3, 3, 2.0));
    }

    #[test]
    fn a_big_group_still_weighs_one_voice() {
        let srv = |id: &str, op: &str, offset: f64, refid: &str| ServerResult {
            id: id.into(),
            operator: op.into(),
            offset_ms: Some(offset),
            stratum: Some(2),
            refid: Some(refid.into()),
            sent: 4,
            ok: 4,
            bound_ms: Some(20.0),
            ..Default::default()
        };
        // Five servers of one operator against three others: four voices, and the five must not carry the median.
        let mut v: Vec<_> = (0..5).map(|i| srv(&format!("a{i}"), "A", 10.0, "192.0.2.1")).collect();
        v.extend([srv("b", "B", 0.0, "192.0.2.2"), srv("c", "C", 1.0, "192.0.2.3"), srv("d", "D", 2.0, "192.0.2.4")]);
        let c = consensus(&mut v).unwrap();
        assert_eq!((c.voters, c.independent), (8, 4));
        assert!(c.median_ms < 3.0, "median {}", c.median_ms);
        // Equal weights are the plain quantile.
        let w: Vec<(f64, f64)> = [1.0, 2.0, 4.0, 8.0, 16.0].iter().map(|x| (*x, 0.25)).collect();
        for q in [0.0, 0.25, 0.5, 0.9, 1.0] {
            let plain = quantile(&[1.0, 2.0, 4.0, 8.0, 16.0], q).unwrap();
            assert!((quantile_weighted(&w, q).unwrap() - plain).abs() < 1e-9, "q={q}");
        }
    }

    #[test]
    fn falsetickers_are_measured_against_the_consensus_but_dont_make_it() {
        let srv = |id: &str, offset: f64| ServerResult {
            id: id.into(),
            offset_ms: Some(offset),
            stratum: Some(2),
            sent: 4,
            ok: 4,
            bound_ms: Some(3.0),
            ..Default::default()
        };
        let mut v = vec![srv("a", 1.0), srv("b", 1.2), srv("c", 0.8), srv("liar", 40.0)];
        let c = consensus(&mut v).unwrap();
        assert_eq!((c.voters, c.median_ms), (3, 1.0));
        assert_eq!(v[3].selection, Some(Selection::Falseticker));
        assert_eq!(v[0].selection, Some(Selection::Truechimer));
        assert_eq!((v[3].deviation_ms, v[3].agrees), (Some(39.0), Some(false)));
    }

    #[test]
    fn the_advised_set_is_diverse_and_the_avoid_list_says_why() {
        let entry = |id: &str, operator: &str, kind: &str, leap: Option<&str>| -> Entry {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": id, "operator": operator, "kind": kind, "host": format!("{id}.example"), "leap": leap
            }))
            .unwrap()
        };
        let entries = vec![
            entry("near", "A", "service", None),
            entry("twin", "A", "service", None),
            entry("sharer", "B", "service", None),
            entry("other", "C", "service", None),
            entry("smear", "D", "service", Some("Smears the leap second over 24 hours")),
            entry("pool", "E", "pool", None),
            entry("liar", "F", "service", None),
            entry("rude", "G", "service", None),
        ];
        let srv = |id: &str, stratum: u8, refid: &str, bound: f64, sel: Selection| ServerResult {
            id: id.into(),
            stratum: Some(stratum),
            refid: Some(refid.into()),
            sent: 4,
            ok: 4,
            bound_ms: Some(bound),
            selection: Some(sel),
            ..Default::default()
        };
        use Selection::*;
        let mut results = vec![
            srv("near", 1, "GPS", 2.0, Truechimer),
            srv("twin", 1, "GPS", 3.0, Truechimer),
            // The same upstream as "other" below, which is slower, so "other" is left out.
            srv("sharer", 2, "10.0.0.1", 4.0, Truechimer),
            srv("other", 2, "10.0.0.1", 5.0, Truechimer),
            srv("smear", 1, "GOOG", 1.0, Truechimer),
            srv("pool", 2, "1.2.3.4", 1.0, Truechimer),
            srv("liar", 1, "GPS", 2.0, Falseticker),
            ServerResult { kiss: Some("RATE".into()), ok: 0, ..srv("rude", 1, "x", 1.0, Truechimer) },
        ];
        let a = advise(&entries, &results);
        // One per operator (twin is A's second), one per upstream, no pool, no smearer, no falseticker.
        assert_eq!(a.set, vec!["near", "sharer"]);
        let passed = |id: &str| a.passed.iter().find(|x| x.id == id).map(|x| x.why.clone());
        assert!(passed("twin").unwrap().contains("same operator"));
        assert!(passed("other").unwrap().contains("upstream"));
        assert_eq!(passed("near"), None);
        let why = |id: &str| a.avoid.iter().find(|x| x.id == id).map(|x| x.why.clone());
        assert!(why("liar").unwrap().contains("disagrees"));
        assert!(why("rude").unwrap().contains("RATE"));
        assert_eq!(why("near"), None);
        // A server that is only slow to answer is not blamed.
        results[0].ok = 0;
        assert_eq!(advise(&entries, &results).avoid.iter().filter(|x| x.id == "near").count(), 0);
    }

    #[test]
    fn a_steadier_server_outranks_one_with_the_same_error_ceiling() {
        let entry = |id: &str, operator: &str| -> Entry {
            serde_json::from_value(serde_json::json!({
                "id": id, "name": id, "operator": operator, "kind": "service", "host": format!("{id}.example")
            }))
            .unwrap()
        };
        let entries = vec![entry("jumpy", "A"), entry("steady", "B")];
        let srv = |id: &str, jitter: f64| ServerResult {
            id: id.into(),
            stratum: Some(1),
            refid: Some("GPS".into()),
            sent: 4,
            ok: 4,
            bound_ms: Some(7.0),
            jitter_ms: Some(jitter),
            selection: Some(Selection::Truechimer),
            ..Default::default()
        };
        let results = vec![srv("jumpy", 0.9), srv("steady", 0.2)];
        assert_eq!(advise(&entries, &results).set, vec!["steady", "jumpy"]);
    }

    #[test]
    fn root_distance_adds_the_servers_own_error_to_the_path_bound() {
        let s = |delay_ms: f64| {
            Ok(Sample {
                offset_ms: 0.5,
                delay_ms,
                stratum: 2,
                leap: 0,
                version: 4,
                refid: "x".into(),
                root_delay_ms: 4.0,
                root_dispersion_ms: 1.5,
                precision_ms: 0.001,
                poll: 3,
            })
        };
        // Best query is the 10 ms one: 4/2 + 1.5 + 10/2.
        let a = summarize("192.0.2.1", &[s(30.0), s(10.0)]);
        assert_eq!(a.distance_ms, Some(8.5));
        assert_eq!(summarize("192.0.2.2", &[Err(NtpError::Timeout)]).distance_ms, None);
    }

    #[test]
    fn nts_key_exchange_records_are_read_back() {
        // What a server sends: NPN (NTPv4), AEAD 15, a server and a port, two cookies, End of Message.
        let mut b = record(1, true, &0u16.to_be_bytes());
        b.extend(record(4, false, &15u16.to_be_bytes()));
        b.extend(record(6, false, b"time.example.org"));
        b.extend(record(7, false, &4123u16.to_be_bytes()));
        b.extend(record(5, false, &[7u8; 100]));
        b.extend(record(5, false, &[8u8; 100]));
        b.extend(record(0, true, &[]));
        let k = read_nts(&mut &b[..]).unwrap();
        assert_eq!((k.ok, k.cookies, k.aead, k.ntp_port), (true, 2, Some(15), Some(4123)));
        assert_eq!(k.server.as_deref(), Some("time.example.org"));
        // An error record, a missing protocol and a handshake without cookies all fail, each in its own words.
        let mut e = record(2, true, &2u16.to_be_bytes());
        e.extend(record(0, true, &[]));
        assert!(read_nts(&mut &e[..]).unwrap_err().1.contains("NTS-KE error 2"));
        let mut n = record(4, false, &15u16.to_be_bytes());
        n.extend(record(0, true, &[]));
        assert!(read_nts(&mut &n[..]).unwrap_err().1.contains("NTPv4"));
        let mut c = record(1, true, &0u16.to_be_bytes());
        c.extend(record(4, false, &15u16.to_be_bytes()));
        c.extend(record(0, true, &[]));
        assert!(!read_nts(&mut &c[..]).unwrap().ok);
        // An unknown record is skipped unless it is critical.
        let mut u = record(1, true, &0u16.to_be_bytes());
        u.extend(record(99, false, &[1, 2]));
        u.extend(record(4, false, &15u16.to_be_bytes()));
        u.extend(record(5, false, &[7u8; 100]));
        u.extend(record(0, true, &[]));
        assert!(read_nts(&mut &u[..]).unwrap().ok);
        let mut x = record(1, true, &0u16.to_be_bytes());
        x.extend(record(99, true, &[1, 2]));
        x.extend(record(0, true, &[]));
        assert!(read_nts(&mut &x[..]).unwrap_err().1.contains("critical"));
        // A stream that never ends is cut off.
        let endless: Vec<u8> = (0..KE_MAX_RECORDS + 2).flat_map(|_| record(99, false, &[0u8; 8])).collect();
        assert!(read_nts(&mut &endless[..]).unwrap_err().1.contains("too many"));
        // The request is NPN, AEAD 15, End of Message.
        assert_eq!(nts_request(), [0x80, 1, 0, 2, 0, 0, 0, 4, 0, 2, 0, 15, 0x80, 0, 0, 0]);
    }

    fn entry(id: &str, host: &str) -> Entry {
        Entry {
            id: id.into(),
            name: id.into(),
            operator: "x".into(),
            kind: "service".into(),
            host: host.into(),
            port: None,
            country: None,
            nts: None,
            nts_documented: false,
            nts_only: false,
            leap: None,
            anycast: None,
            policy: None,
            link: None,
            default: false,
        }
    }

    #[test]
    fn a_server_asked_recently_or_that_said_stop_is_left_alone() {
        let now = 1_791_000_000_000i64;
        let min = 60_000;
        let es = [entry("a", "a.example"), entry("b", "b.example"), entry("c", "c.example"), entry("d", "d.example")];
        let mut l = Ledger::new();
        l.insert("a.example".into(), Asked { at: now - 3 * min, ..Default::default() });
        l.insert("b.example".into(), Asked { at: now - 11 * min, ..Default::default() });
        l.insert("c.example".into(), Asked { at: now - 2 * 86_400_000, kiss: Some("DENY".into()), until: Some(now + 5 * 86_400_000) });
        let (ask, skip) = guard(&es, &l, now);
        assert_eq!(ask.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["b", "d"], "ten minutes is over; a name never asked is fine");
        assert_eq!((skip[0].why.as_str(), skip[0].until), ("recent", now + 7 * min));
        assert_eq!((skip[1].why.as_str(), skip[1].kiss.as_deref()), ("kiss", Some("DENY")));
        // A Kiss-o'-Death that has run out no longer counts.
        l.insert("c.example".into(), Asked { at: now - 9 * 86_400_000, kiss: Some("DENY".into()), until: Some(now - 1) });
        assert_eq!(guard(&es, &l, now).0.len(), 3);
    }

    #[test]
    fn a_run_is_stamped_and_remembers_who_told_it_to_stop() {
        let now = 1_791_000_000_000i64;
        let es = [entry("a", "a.example"), entry("k", "nts.example")];
        let mut l = Ledger::new();
        l.insert("old.example".into(), Asked { at: now - 40 * 86_400_000, ..Default::default() });
        stamp(&mut l, &es, now);
        assert_eq!(l["a.example"].at, now);
        assert!(!l.contains_key("old.example"), "forgotten after a month");
        let kissed = ServerResult { host: "a.example".into(), kiss: Some("RATE".into()), ..Default::default() };
        remember_kisses(&mut l, &[kissed], now);
        assert_eq!((l["a.example"].kiss.as_deref(), l["a.example"].until), (Some("RATE"), Some(now + 3_600_000)));
        let denied = ServerResult { host: "a.example".into(), kiss: Some("DENY".into()), ..Default::default() };
        remember_kisses(&mut l, &[denied], now);
        assert_eq!(l["a.example"].until, Some(now + 7 * 86_400_000));
        // A name that was stopped and is asked again after the wait comes off the list.
        stamp(&mut l, &es, now + 8 * 86_400_000);
        assert_eq!(l["a.example"].until, None);
        // The ceiling on what a run can send is stated, and small.
        assert_eq!(most_packets(11, 4), 264);
        assert_eq!(most_packets(60, 99), 60 * 6 * MAX_SAMPLES as usize);
    }

    #[test]
    fn a_server_dripping_bytes_cannot_hold_the_key_exchange_open() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            // The header of a large handshake record, then one byte every 300 ms: no single read ever times out.
            for b in [0x16, 0x03, 0x03, 0x40, 0x00].into_iter().chain(std::iter::repeat_n(0, 60)) {
                if s.write_all(&[b]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        let t = Instant::now();
        let k = nts_ke("127.0.0.1", port, Duration::from_secs(1));
        assert!(t.elapsed() < Duration::from_secs(2), "took {:?}", t.elapsed());
        assert!(!k.ok);
        assert_eq!(k.error_kind.as_deref(), Some("timeout"), "{k:?}");
        server.join().unwrap();
    }

    /// The network half of the key exchange (`RECURSA_TEST_NTS=<host>`).
    #[test]
    #[ignore = "needs an NTS server to ask"]
    fn a_real_nts_server_hands_out_cookies() {
        let Ok(host) = std::env::var("RECURSA_TEST_NTS") else { return };
        let k = nts_ke(&host, 4460, Duration::from_secs(4));
        assert!(k.ok, "{k:?}");
        eprintln!("{k:?}");
    }

    /// The network half: ask a real server (`RECURSA_TEST_NTP=<host>`).
    #[test]
    #[ignore = "needs an NTP server to ask"]
    fn a_real_server_answers() {
        let Ok(host) = std::env::var("RECURSA_TEST_NTP") else { return };
        let plan = Plan { samples: 2, stagger: Duration::ZERO, ..Default::default() };
        let r = measure_server("t", &host, 123, None, &plan, &AtomicBool::new(false));
        assert!(r.ok > 0, "{r:?}");
        assert!(r.offset_ms.is_some_and(|o| o.abs() < 5000.0));
    }
}

#[cfg(test)]
mod exchange_tests {
    use super::*;

    /// A server on loopback that waits `lag` before answering with a clock `skew_ms` ahead of ours.
    fn serve(lag: Duration, skew_ms: i64) -> SocketAddr {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = sock.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            while let Ok((n, from)) = sock.recv_from(&mut buf) {
                if n < 48 {
                    continue;
                }
                let skew = skew_ms as i128 * 1_000_000;
                let rx = to_ntp(unix_ns_now() + skew);
                std::thread::sleep(lag);
                let tx = to_ntp(unix_ns_now() + skew);
                let mut p = [0u8; 48];
                p[0] = (4 << 3) | 4;
                p[1] = 2;
                p[24..32].copy_from_slice(&buf[40..48]);
                p[32..40].copy_from_slice(&rx.to_be_bytes());
                p[40..48].copy_from_slice(&tx.to_be_bytes());
                let _ = sock.send_to(&p, from);
            }
        });
        addr
    }

    #[test]
    fn a_reply_is_timed_from_the_kernels_stamp_and_the_offset_comes_out() {
        let addr = serve(Duration::from_millis(40), 25);
        let sock = connect(addr).unwrap();
        let s = query(&sock, Duration::from_secs(2)).unwrap();
        // The server sat on the request for 40 ms before stamping both its times, which NTP counts as server time, not
        // delay; what remains is loopback, well under a few ms even on a loaded machine.
        assert!(s.delay_ms < 20.0, "delay {}", s.delay_ms);
        assert!((s.offset_ms - 25.0).abs() < 20.0, "offset {}", s.offset_ms);
        assert_eq!(s.stratum, 2);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn the_kernel_stamps_a_datagram_as_it_arrives() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(net::stamp_arrivals(&rx));
        let before = clock::now_ns();
        UdpSocket::bind("127.0.0.1:0").unwrap().send_to(b"x", rx.local_addr().unwrap()).unwrap();
        let mut buf = [0u8; 8];
        let (n, at) = net::recv_stamped(&rx, &mut buf).unwrap();
        let at = at.expect("a kernel stamp");
        assert_eq!(n, 1);
        assert!(at >= before && at - before < 2_000_000_000, "stamp {at} vs {before}");
    }

    /// Windows stamps packets where the adapter hands them up, so loopback traffic gets none and the
    /// stamped read must still deliver the datagram. Any stamp that does come must be in the clock's domain.
    #[cfg(windows)]
    #[test]
    fn the_stack_stamps_a_datagram_as_it_arrives() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        let _enabled = net::stamp_arrivals(&rx);
        let before = clock::now_ns();
        UdpSocket::bind("127.0.0.1:0").unwrap().send_to(b"x", rx.local_addr().unwrap()).unwrap();
        let mut buf = [0u8; 8];
        let (n, at) = net::recv_stamped(&rx, &mut buf).unwrap();
        assert_eq!(n, 1);
        if let Some(at) = at {
            assert!(at >= before && at - before < 2_000_000_000, "stamp {at} vs {before}");
        }
    }

    #[test]
    fn a_server_that_never_answers_times_out() {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let c = connect(sock.local_addr().unwrap()).unwrap();
        assert!(matches!(query(&c, Duration::from_millis(150)), Err(NtpError::Timeout)));
    }
}

#[cfg(test)]
mod nts_tests {
    use super::*;

    fn keys() -> NtsKeys {
        NtsKeys { c2s: [7; 32], s2c: [9; 32] }
    }

    /// What a server would send back: a plain reply to the request, our Unique Identifier, and an authenticator
    /// under the server-to-client key whose encrypted part carries `cookies`.
    fn reply_to(request: &[u8], uid: &[u8], key: &[u8; 32], cookies: &[&[u8]]) -> Vec<u8> {
        let mut p = request[..48].to_vec();
        p[0] = (4 << 3) | 4;
        p[1] = 2;
        push_ext(&mut p, EXT_UNIQUE_ID, uid);
        let mut inner = Vec::new();
        for c in cookies {
            push_ext(&mut inner, EXT_COOKIE, c);
        }
        let nonce = [3u8; 16];
        let sealed = siv(key).unwrap().encrypt([&p[..], &nonce[..]], &inner[..]).unwrap();
        let mut body = Vec::new();
        body.extend_from_slice(&16u16.to_be_bytes());
        body.extend_from_slice(&(sealed.len() as u16).to_be_bytes());
        body.extend_from_slice(&nonce);
        body.extend_from_slice(&sealed);
        push_ext(&mut p, EXT_AUTHENTICATOR, &body);
        p
    }

    fn setup() -> (Vec<u8>, [u8; 32]) {
        let uid = [5u8; 32];
        (nts_packet(1, &keys(), &uid, &[0xAB; 100]), uid)
    }

    fn kind(r: Result<(), NtpError>) -> &'static str {
        r.map(|_| "ok").unwrap_or_else(|e| e.kind())
    }

    #[test]
    fn a_request_carries_the_unique_id_the_cookie_and_an_authenticator_the_server_can_open() {
        let (req, uid) = setup();
        let k = keys();
        // Header, then Unique ID, Cookie, Authenticator, each a multiple of four long.
        assert_eq!(&req[48..50], &EXT_UNIQUE_ID.to_be_bytes());
        assert_eq!(&req[48 + 4..48 + 36], &uid);
        let cookie_at = 48 + 36;
        assert_eq!(&req[cookie_at..cookie_at + 2], &EXT_COOKIE.to_be_bytes());
        assert_eq!(u16::from_be_bytes([req[cookie_at + 2], req[cookie_at + 3]]), 104);
        let auth_at = cookie_at + 104;
        assert_eq!(&req[auth_at..auth_at + 2], &EXT_AUTHENTICATOR.to_be_bytes());
        // The server opens it with the client-to-server key over everything before it plus the nonce.
        let body = &req[auth_at + 4..];
        let (nl, cl) = (u16::from_be_bytes([body[0], body[1]]) as usize, u16::from_be_bytes([body[2], body[3]]) as usize);
        assert_eq!(nl, 16);
        let (nonce, sealed) = (&body[4..4 + nl], &body[4 + nl..4 + nl + cl]);
        let plain = siv(&k.c2s).unwrap().decrypt([&req[..auth_at], nonce], sealed).unwrap();
        assert!(plain.is_empty(), "a request encrypts nothing");
        assert!(siv(&k.s2c).unwrap().decrypt([&req[..auth_at], nonce], sealed).is_err(), "the other direction's key must not open it");
    }

    #[test]
    fn a_genuine_reply_verifies_and_hands_over_fresh_cookies() {
        let (req, uid) = setup();
        let reply = reply_to(&req, &uid, &keys().s2c, &[&[1; 100], &[2; 100]]);
        let mut fresh = Vec::new();
        assert_eq!(kind(vet_nts(&reply, &uid, &keys(), &mut fresh)), "ok");
        assert_eq!(fresh.len(), 2);
        assert_eq!(fresh[0].len(), 100);
    }

    #[test]
    fn a_reply_that_isnt_right_is_named() {
        let (req, uid) = setup();
        let good = reply_to(&req, &uid, &keys().s2c, &[&[1; 100]]);
        let vet = |r: &[u8], uid: &[u8; 32]| kind(vet_nts(r, uid, &keys(), &mut Vec::new()));
        // Sealed with the wrong key, or altered in flight (the header is covered): the authenticator doesn't open.
        assert_eq!(vet(&reply_to(&req, &uid, &keys().c2s, &[]), &uid), "aead_failure");
        let mut altered = good.clone();
        altered[2] ^= 1;
        assert_eq!(vet(&altered, &uid), "aead_failure");
        // Somebody else's answer: a different identifier.
        assert_eq!(vet(&good, &[6u8; 32]), "invalid_unique_id");
        // A server that ignores the extension fields and answers plain NTP.
        assert_eq!(vet(&good[..48], &uid), "unauthenticated_response");
        // Identifier but no authenticator.
        let mut bare = good[..48].to_vec();
        push_ext(&mut bare, EXT_UNIQUE_ID, &uid);
        assert_eq!(vet(&bare, &uid), "missing_authenticator");
        // A field whose length runs past the packet, and one that is truncated.
        let mut bad = good[..48].to_vec();
        bad.extend_from_slice(&EXT_UNIQUE_ID.to_be_bytes());
        bad.extend_from_slice(&200u16.to_be_bytes());
        bad.extend_from_slice(&[0; 8]);
        assert_eq!(vet(&bad, &uid), "malformed_extensions");
        let mut short = good[..48].to_vec();
        push_ext(&mut short, EXT_UNIQUE_ID, &uid);
        push_ext(&mut short, EXT_AUTHENTICATOR, &[0, 16]);
        assert_eq!(vet(&short, &uid), "malformed_extensions");
    }

    #[test]
    fn every_failure_has_a_stable_name() {
        assert_eq!(NtpError::Timeout.kind(), "timeout");
        assert_eq!(NtpError::Kiss("RATE".into()).kind(), "kiss");
        assert_eq!(NtpError::Unsynchronized.kind(), "unsynchronized");
        assert_eq!(NtpError::Bad("x").kind(), "bad_reply");
        assert_eq!(NtpError::Io("x".into()).kind(), "network");
        assert_eq!(NtpError::Nts("aead_failure", "x".into()).kind(), "aead_failure");
    }

    #[test]
    fn a_server_that_said_stop_is_a_kiss_and_one_that_answered_nothing_is_a_timeout() {
        let kissed = [Err(NtpError::Kiss("RATE".into())), Err(NtpError::Kiss("RATE".into()))];
        let s = summarize("127.0.0.1", &kissed);
        assert_eq!((s.error_kind.as_deref(), s.kiss.as_deref()), (Some("kiss"), Some("RATE")));
        let silent = vec![Err(NtpError::Timeout), Err(NtpError::Timeout), Err(NtpError::Timeout)];
        assert_eq!(summarize("127.0.0.1", &silent).error_kind.as_deref(), Some("timeout"));
    }

    #[test]
    fn typed_addresses_are_understood_and_bad_ones_refused() {
        let e = custom_entry("Time.Example.com").unwrap();
        assert_eq!(
            (e.host.as_str(), e.port, e.kind.as_str(), e.id.as_str()),
            ("time.example.com", None, "custom", "custom:time.example.com")
        );
        assert_eq!(e.nts.as_deref(), Some("time.example.com"));
        let e = custom_entry("ntp.lan:1123").unwrap();
        assert_eq!((e.host.as_str(), e.port, e.name.as_str()), ("ntp.lan", Some(1123), "ntp.lan:1123"));
        let e = custom_entry("192.168.1.1").unwrap();
        assert_eq!((e.host.as_str(), e.nts.is_none()), ("192.168.1.1", true));
        let e = custom_entry("[2001:db8::1]:124").unwrap();
        assert_eq!((e.host.as_str(), e.port, e.name.as_str()), ("2001:db8::1", Some(124), "[2001:db8::1]:124"));
        assert_eq!(custom_entry("2001:db8::1").unwrap().port, None);
        // Port 123 is the default, and a catalog host is the catalog's entry, not a custom one.
        assert_eq!(custom_entry("ntp.lan:123").unwrap().port, None);
        let first = catalog().into_iter().find(|e| e.kind != "pool").unwrap();
        assert_eq!(custom_entry(&first.host).unwrap().id, first.id);
        for bad in [
            "",
            "  ",
            "ntp.lan:0",
            "ntp.lan:70000",
            "ntp.lan:abc",
            "http://x.org",
            "a b",
            "-x.org",
            "x..org",
            "[::1",
            "[::1]x",
            "bad/host",
            &"a".repeat(300),
        ] {
            assert!(custom_entry(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_host_you_typed_is_asked_sooner_but_still_believed_when_it_says_stop() {
        let mine = custom_entry("ntp.lan").unwrap();
        let public = Entry { id: "p".into(), host: "p.example".into(), kind: "service".into(), ..mine.clone() };
        let now = 10_000_000_000i64;
        let mut ledger = Ledger::default();
        stamp(&mut ledger, &[mine.clone(), public.clone()], now - 60_000);
        let (ask, skip) = guard(&[mine.clone(), public.clone()], &ledger, now);
        assert_eq!((ask.len(), skip.len()), (1, 1));
        assert_eq!(ask[0].id, mine.id);
        assert_eq!(skip[0].why, "recent");
        let (ask, _) = guard(std::slice::from_ref(&mine), &ledger, now + 60_000);
        assert_eq!(ask.len(), 1);
        let kissed = ServerResult { id: mine.id.clone(), host: mine.host.clone(), kiss: Some("RATE".into()), ..Default::default() };
        remember_kisses(&mut ledger, &[kissed], now);
        let (ask, skip) = guard(&[mine], &ledger, now + 120_000);
        assert_eq!((ask.len(), skip.first().map(|s| s.why.as_str())), (0, Some("kiss")));
    }

    #[test]
    fn your_own_servers_never_vote_and_get_references_to_be_judged_by() {
        let (list, added) = with_references(vec![custom_entry("ntp.lan").unwrap()]);
        assert_eq!(added.len(), 4);
        assert_eq!(list.len(), 5);
        let (list, added) = with_references(vec![catalog().remove(0), custom_entry("ntp.lan").unwrap()]);
        assert_eq!((list.len(), added.len()), (2, 0));
        // The plan for a host of your own is quick; the public plan keeps its floor.
        let p = Plan::default();
        assert_eq!((p.floor, p.for_custom().spacing, p.for_custom().stagger), (MIN_SPACING, CUSTOM_SPACING, Duration::ZERO));
    }

    #[test]
    fn a_custom_server_is_judged_but_does_not_move_the_consensus() {
        let mk = |id: &str, off: f64, custom: bool| ServerResult {
            id: id.into(),
            host: format!("{id}.example"),
            stratum: Some(2),
            sent: 4,
            ok: 4,
            offset_ms: Some(off),
            bound_ms: Some(2.0),
            distance_ms: Some(4.0),
            custom,
            ..Default::default()
        };
        let mut v = vec![mk("a", 1.0, false), mk("b", 1.2, false), mk("c", 0.8, false), mk("mine", 40.0, true)];
        let c = consensus(&mut v).unwrap();
        assert_eq!(c.voters, 3);
        assert!((c.median_ms - 1.0).abs() < 1e-9, "{}", c.median_ms);
        assert_eq!(v[3].selection, None);
        assert!(v[3].deviation_ms.is_some_and(|d| d > 30.0) && v[3].agrees == Some(false), "{:?}", v[3]);
    }
}
