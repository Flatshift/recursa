//! Timestamped DNS transports.
//!
//! UDP is the precision path: a connected, non-blocking socket per target with kernel
//! receive timestamps, so scheduler wake-up after the packet lands never counts toward
//! the measured round trip. TCP, DoT and DoH keep a persistent connection, so handshakes
//! are timed separately from queries. They use userspace stamps from the same clock.

use crate::{clock, dns};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Udp,
    Tcp,
    Dot,
    Doh,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Udp => "UDP",
            Transport::Tcp => "TCP",
            Transport::Dot => "DoT",
            Transport::Doh => "DoH",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub transport: Transport,
    pub addr: SocketAddr,
    /// TLS server name (DoT/DoH).
    pub host: Option<String>,
    /// Full DoH URL.
    pub url: Option<String>,
}

#[derive(Debug)]
pub struct Reply {
    pub bytes: Vec<u8>,
    /// Round trip used for statistics (kernel receive stamp when available).
    pub rtt_ns: u64,
    /// Round trip as seen by userspace after the receive call returned.
    pub user_rtt_ns: u64,
    pub kernel_stamp: bool,
    /// IP TTL / hop limit of the answer (UDP only). An answer that arrives with an
    /// untouched TTL came from the local network, whatever its source address says.
    pub ttl: Option<u8>,
}

/// Router hops an answer crossed, assuming the sender used a common initial TTL.
pub fn hops(ttl: u8) -> u8 {
    let initial = if ttl > 128 {
        255
    } else if ttl > 64 {
        128
    } else {
        64
    };
    initial - ttl
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum XErr {
    Timeout,
    /// ICMP port unreachable / connection refused.
    Refused,
    Io(String),
    Tls(String),
    Http(String),
    /// A DoH server answered with an HTTP error status: the connection itself is fine.
    Status(u16),
}

impl std::fmt::Display for XErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XErr::Timeout => f.write_str("timed out"),
            XErr::Refused => f.write_str("connection refused"),
            XErr::Io(e) => write!(f, "network error: {e}"),
            XErr::Tls(e) => write!(f, "TLS error: {e}"),
            XErr::Http(e) => write!(f, "HTTP error: {e}"),
            XErr::Status(code) => write!(f, "HTTP status {code}"),
        }
    }
}

/// The unspecified bind address of the same family as `addr`.
pub(crate) fn unspecified(addr: &std::net::IpAddr) -> &'static str {
    if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }
}

fn io_err(e: io::Error) -> XErr {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => XErr::Timeout,
        io::ErrorKind::ConnectionRefused => XErr::Refused,
        _ => XErr::Io(e.to_string()),
    }
}

pub trait Exchanger: Send {
    fn exchange(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr>;
    /// Connection or handshake time for the most recent (re)connect, if any.
    fn setup_ns(&self) -> Option<u64> {
        None
    }
}

/// Open a transport for `ep`. Stream transports connect (and handshake) here, outside
/// of any timed query.
pub fn open(ep: &Endpoint, timeout: Duration) -> Result<Box<dyn Exchanger>, XErr> {
    Ok(match ep.transport {
        Transport::Udp => Box::new(UdpX::connect(ep.addr).map_err(io_err)?),
        Transport::Tcp => {
            let mut t = TcpX { addr: ep.addr, stream: None, setup_ns: None, buf: Vec::with_capacity(4096) };
            t.ensure(timeout)?;
            Box::new(t)
        }
        Transport::Dot => {
            let host = ep.host.clone().ok_or_else(|| XErr::Tls("no TLS server name".into()))?;
            let mut t = DotX::new(ep.addr, &host)?;
            t.ensure(timeout)?;
            Box::new(t)
        }
        Transport::Doh => {
            let url = ep.url.clone().ok_or_else(|| XErr::Http("no DoH URL".into()))?;
            let mut t = DohX::new(ep.addr, &url)?;
            t.ensure(timeout)?;
            Box::new(t)
        }
    })
}

// ---------------------------------------------------------------------------------------
// UDP with kernel receive timestamps
// ---------------------------------------------------------------------------------------

#[cfg(unix)]
pub struct UdpX {
    sock: socket2::Socket,
    kernel_ts: bool,
    buf: Box<[u8]>,
    cmsg: [u64; 32],
}

/// Largest DNS message we accept on any transport.
const MAX_MSG: usize = 65535;

#[cfg(unix)]
impl UdpX {
    pub fn connect(addr: SocketAddr) -> io::Result<Self> {
        use socket2::{Domain, Protocol, Socket, Type};
        let sock = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
        sock.set_nonblocking(true)?;
        let _ = sock.set_recv_buffer_size(1 << 20);
        let kernel_ts = enable_rx_timestamps(&sock);
        enable_rx_ttl(&sock, addr.is_ipv6());
        sock.connect(&addr.into())?;
        Ok(UdpX { sock, kernel_ts, buf: vec![0u8; MAX_MSG].into_boxed_slice(), cmsg: [0u64; 32] })
    }

    pub fn kernel_timestamps(&self) -> bool {
        self.kernel_ts
    }

    fn fd(&self) -> libc::c_int {
        use std::os::fd::AsRawFd;
        self.sock.as_raw_fd()
    }

    /// Receive one datagram, its kernel arrival stamp (ns in the [`clock`] domain) and
    /// the TTL it arrived with.
    fn recv(&mut self) -> io::Result<(usize, Option<u64>, Option<u8>)> {
        let fd = self.fd();
        let mut iov = libc::iovec { iov_base: self.buf.as_mut_ptr() as *mut libc::c_void, iov_len: self.buf.len() };
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = self.cmsg.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = std::mem::size_of_val(&self.cmsg) as _;
        let n = unsafe { libc::recvmsg(fd, &mut msg, 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let (mut stamp, mut ttl) = (None, None);
        unsafe {
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                let h = &*c;
                let data = libc::CMSG_DATA(c);
                match (h.cmsg_level, h.cmsg_type) {
                    // macOS and iOS report the IPv4 TTL as one byte, Linux as an int.
                    #[cfg(any(target_os = "macos", target_os = "ios"))]
                    (libc::IPPROTO_IP, libc::IP_RECVTTL) => ttl = Some(*data),
                    #[cfg(target_os = "linux")]
                    (libc::IPPROTO_IP, libc::IP_TTL) => ttl = Some(std::ptr::read_unaligned(data as *const libc::c_int) as u8),
                    (libc::IPPROTO_IPV6, libc::IPV6_HOPLIMIT) => ttl = Some(std::ptr::read_unaligned(data as *const libc::c_int) as u8),
                    _ => {}
                }
                stamp = stamp.or(arrival_stamp(c));
                c = libc::CMSG_NXTHDR(&msg, c);
            }
        }
        Ok((n as usize, stamp, ttl))
    }

    /// Discard late answers to earlier, timed-out queries.
    fn drain(&mut self) {
        while self.recv().is_ok() {}
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
const SO_TIMESTAMP_MONOTONIC: libc::c_int = 0x0800;
#[cfg(any(target_os = "macos", target_os = "ios"))]
const SCM_TIMESTAMP_MONOTONIC: libc::c_int = 0x04;

#[cfg(unix)]
fn enable_rx_timestamps(sock: &socket2::Socket) -> bool {
    use std::os::fd::AsRawFd;
    enable_rx_timestamps_fd(sock.as_raw_fd())
}

#[cfg(unix)]
pub(crate) fn enable_rx_timestamps_fd(fd: libc::c_int) -> bool {
    let one: libc::c_int = 1;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let opt = SO_TIMESTAMP_MONOTONIC;
    #[cfg(target_os = "linux")]
    let opt = libc::SO_TIMESTAMPNS;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux")))]
    return false;
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "linux"))]
    unsafe {
        libc::setsockopt(fd, libc::SOL_SOCKET, opt, &one as *const _ as *const libc::c_void, std::mem::size_of_val(&one) as libc::socklen_t)
            == 0
    }
}

/// Ask the kernel to stamp datagrams arriving on a plain std socket (see [`recv_stamped`]). False where unsupported.
pub fn stamp_arrivals(sock: &std::net::UdpSocket) -> bool {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        enable_rx_timestamps_fd(sock.as_raw_fd())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        win::enable_rx_timestamps(sock.as_raw_socket() as win::SOCKET)
    }
}

/// The kernel arrival stamp (ns, in the [`clock`] domain) in one control message, if it is that.
#[cfg(unix)]
unsafe fn arrival_stamp(c: *const libc::cmsghdr) -> Option<u64> {
    unsafe {
        let h = &*c;
        if h.cmsg_level != libc::SOL_SOCKET {
            return None;
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        if h.cmsg_type == SCM_TIMESTAMP_MONOTONIC {
            let ticks = std::ptr::read_unaligned(libc::CMSG_DATA(c) as *const u64);
            return Some(clock::ticks_to_ns(ticks));
        }
        #[cfg(target_os = "linux")]
        if h.cmsg_type == libc::SCM_TIMESTAMPNS {
            let t = std::ptr::read_unaligned(libc::CMSG_DATA(c) as *const libc::timespec);
            return Some(t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64);
        }
        None
    }
}

/// Receive one datagram on a std socket, with the kernel's arrival stamp when [`stamp_arrivals`] worked: the moment the
/// packet came in, not the moment this thread got scheduled to read it. Honours the socket's read timeout.
pub fn recv_stamped(sock: &std::net::UdpSocket, buf: &mut [u8]) -> io::Result<(usize, Option<u64>)> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        recv_stamped_fd(sock.as_raw_fd(), buf)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        win::recv_stamped(sock.as_raw_socket() as win::SOCKET, buf)
    }
}

/// [`recv_stamped`] over a raw descriptor, for sockets that are not `UdpSocket`s (the link check's ICMP socket).
#[cfg(unix)]
pub(crate) fn recv_stamped_fd(fd: libc::c_int, buf: &mut [u8]) -> io::Result<(usize, Option<u64>)> {
    let mut cmsg = [0u64; 32];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr() as *mut libc::c_void, iov_len: buf.len() };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = std::mem::size_of_val(&cmsg) as _;
    let n = unsafe { libc::recvmsg(fd, &mut msg, 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut stamp = None;
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            stamp = stamp.or(arrival_stamp(c));
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    Ok((n as usize, stamp))
}

#[cfg(unix)]
fn enable_rx_ttl(sock: &socket2::Socket, v6: bool) {
    use std::os::fd::AsRawFd;
    let one: libc::c_int = 1;
    let (level, opt) = if v6 { (libc::IPPROTO_IPV6, libc::IPV6_RECVHOPLIMIT) } else { (libc::IPPROTO_IP, libc::IP_RECVTTL) };
    unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            level,
            opt,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of_val(&one) as libc::socklen_t,
        );
    }
}

#[cfg(unix)]
impl Exchanger for UdpX {
    fn exchange(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        self.drain();
        let fd = self.fd();
        let t0 = clock::now_ns();
        let deadline = t0 + timeout.as_nanos() as u64;
        let sent = unsafe { libc::send(fd, query.as_ptr() as *const libc::c_void, query.len(), 0) };
        if sent < 0 {
            return Err(io_err(io::Error::last_os_error()));
        }
        loop {
            let now = clock::now_ns();
            if now >= deadline {
                return Err(XErr::Timeout);
            }
            let wait_ms = ((deadline - now).div_ceil(1_000_000)).min(i32::MAX as u64) as libc::c_int;
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let r = unsafe { libc::poll(&mut pfd, 1, wait_ms) };
            if r == 0 {
                continue;
            }
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(io_err(e));
            }
            match self.recv() {
                Ok((len, kstamp, ttl)) => {
                    let t_user = clock::now_ns();
                    let resp = &self.buf[..len];
                    if !dns::is_response_to(resp, query) {
                        continue;
                    }
                    let t_rx = kstamp.filter(|k| *k >= t0).unwrap_or(t_user);
                    return Ok(Reply {
                        bytes: resp.to_vec(),
                        rtt_ns: t_rx.saturating_sub(t0),
                        user_rtt_ns: t_user.saturating_sub(t0),
                        kernel_stamp: kstamp.is_some(),
                        ttl,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_err(e)),
            }
        }
    }
}

/// Kernel receive stamps on Windows: `SIO_TIMESTAMPING` (Windows 10 2004 and later) makes the stack attach an
/// `SO_TIMESTAMP` control message, in performance-counter ticks, to each datagram read with `WSARecvMsg`.
#[cfg(windows)]
mod win {
    use crate::clock;
    use std::ffi::c_void;
    use std::io;
    use std::ptr::null_mut;
    use std::sync::OnceLock;
    pub use windows_sys::Win32::Networking::WinSock::SOCKET;
    use windows_sys::Win32::Networking::WinSock::{
        CMSGHDR, LPFN_WSARECVMSG, SIO_GET_EXTENSION_FUNCTION_POINTER, SIO_TIMESTAMPING, SO_TIMESTAMP, SOCKET_ERROR, SOL_SOCKET,
        TIMESTAMPING_CONFIG, TIMESTAMPING_FLAG_RX, WSABUF, WSAID_WSARECVMSG, WSAIoctl, WSAMSG, recv,
    };

    /// `WSARecvMsg` is an extension function: Winsock hands out its address through an ioctl.
    fn recv_msg(s: SOCKET) -> LPFN_WSARECVMSG {
        static F: OnceLock<LPFN_WSARECVMSG> = OnceLock::new();
        if let Some(f) = F.get() {
            return *f;
        }
        let guid = WSAID_WSARECVMSG;
        let mut f: LPFN_WSARECVMSG = None;
        let mut out = 0u32;
        let r = unsafe {
            WSAIoctl(
                s,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                &guid as *const _ as *const c_void,
                size_of_val(&guid) as u32,
                &mut f as *mut _ as *mut c_void,
                size_of_val(&f) as u32,
                &mut out,
                null_mut(),
                None,
            )
        };
        // Only a found address is kept: a socket the ioctl refuses must not decide for every later one.
        if r != 0 || f.is_none() {
            return None;
        }
        let _ = F.set(f);
        f
    }

    /// Ask the stack to stamp datagrams as they arrive. False where unsupported (older Windows 10).
    pub fn enable_rx_timestamps(s: SOCKET) -> bool {
        let cfg = TIMESTAMPING_CONFIG { Flags: TIMESTAMPING_FLAG_RX, TxTimestampsBuffered: 0 };
        let mut out = 0u32;
        let r = unsafe {
            WSAIoctl(
                s,
                SIO_TIMESTAMPING,
                &cfg as *const _ as *const c_void,
                size_of_val(&cfg) as u32,
                null_mut(),
                0,
                &mut out,
                null_mut(),
                None,
            )
        };
        r == 0 && recv_msg(s).is_some()
    }

    /// Receive one datagram with the stack's arrival stamp (ns, in the [`clock`] domain) when there is one. Honours
    /// the socket's read timeout like `recv`.
    pub fn recv_stamped(s: SOCKET, buf: &mut [u8]) -> io::Result<(usize, Option<u64>)> {
        let len = buf.len().min(i32::MAX as usize);
        let Some(wsa_recv_msg) = recv_msg(s) else {
            let n = unsafe { recv(s, buf.as_mut_ptr(), len as i32, 0) };
            return if n == SOCKET_ERROR { Err(io::Error::last_os_error()) } else { Ok((n as usize, None)) };
        };
        let mut ctl = [0u64; 32];
        let mut data = WSABUF { len: len as u32, buf: buf.as_mut_ptr() };
        let mut msg = WSAMSG {
            name: null_mut(),
            namelen: 0,
            lpBuffers: &mut data,
            dwBufferCount: 1,
            Control: WSABUF { len: size_of_val(&ctl) as u32, buf: ctl.as_mut_ptr() as *mut u8 },
            dwFlags: 0,
        };
        let mut n = 0u32;
        if unsafe { wsa_recv_msg(s, &mut msg, &mut n, null_mut(), None) } == SOCKET_ERROR {
            return Err(io::Error::last_os_error());
        }
        let mut stamp = None;
        let base = ctl.as_ptr() as *const u8;
        let end = (msg.Control.len as usize).min(size_of_val(&ctl));
        let hdr = size_of::<CMSGHDR>();
        // Winsock lays control messages out like POSIX cmsgs: each header aligned to the pointer size, its
        // data to eight bytes (WSA_CMSGDATA_ALIGN).
        let data = hdr.next_multiple_of(8);
        let mut off = 0;
        while off + hdr <= end {
            let h = unsafe { std::ptr::read_unaligned(base.add(off) as *const CMSGHDR) };
            if h.cmsg_len < hdr || off + h.cmsg_len > end {
                break;
            }
            if h.cmsg_level == SOL_SOCKET && h.cmsg_type == SO_TIMESTAMP as i32 && h.cmsg_len >= data + 8 {
                let ticks = unsafe { std::ptr::read_unaligned(base.add(off + data) as *const u64) };
                // The stack stamps where the adapter hands the packet up (the NDIS boundary), so loopback
                // traffic, and adapters without software timestamping, carry a zero stamp: no stamp at all.
                if ticks != 0 {
                    stamp = Some(clock::ticks_to_ns(ticks));
                }
            }
            off += h.cmsg_len.next_multiple_of(size_of::<usize>());
        }
        Ok((n as usize, stamp))
    }
}

#[cfg(windows)]
pub struct UdpX {
    sock: std::net::UdpSocket,
    kernel_ts: bool,
    buf: Box<[u8]>,
}

#[cfg(windows)]
impl UdpX {
    pub fn connect(addr: SocketAddr) -> io::Result<Self> {
        let bind: SocketAddr = unspecified(&addr.ip()).parse().unwrap();
        let sock = std::net::UdpSocket::bind(bind)?;
        sock.connect(addr)?;
        let kernel_ts = stamp_arrivals(&sock);
        Ok(UdpX { sock, kernel_ts, buf: vec![0u8; MAX_MSG].into_boxed_slice() })
    }
    pub fn kernel_timestamps(&self) -> bool {
        self.kernel_ts
    }
}

#[cfg(windows)]
impl Exchanger for UdpX {
    fn exchange(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        let t0 = clock::now_ns();
        let deadline = t0 + timeout.as_nanos() as u64;
        self.sock.send(query).map_err(io_err)?;
        loop {
            let now = clock::now_ns();
            if now >= deadline {
                return Err(XErr::Timeout);
            }
            self.sock.set_read_timeout(Some(Duration::from_nanos(deadline - now))).map_err(io_err)?;
            // Windows reports an ICMP port unreachable on a connected UDP socket as a reset (WSAECONNRESET).
            let (len, kstamp) = recv_stamped(&self.sock, &mut self.buf[..]).map_err(|e| match e.kind() {
                io::ErrorKind::ConnectionReset => XErr::Refused,
                _ => io_err(e),
            })?;
            let t_user = clock::now_ns();
            let resp = &self.buf[..len];
            if !dns::is_response_to(resp, query) {
                continue;
            }
            let t_rx = kstamp.filter(|k| *k >= t0).unwrap_or(t_user);
            return Ok(Reply {
                bytes: resp.to_vec(),
                rtt_ns: t_rx.saturating_sub(t0),
                user_rtt_ns: t_user.saturating_sub(t0),
                kernel_stamp: kstamp.is_some(),
                ttl: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------------------
// TCP (RFC 7766) and DoT (RFC 7858): length-prefixed frames on a persistent stream
// ---------------------------------------------------------------------------------------

fn write_frame<W: Write>(w: &mut W, query: &[u8], scratch: &mut Vec<u8>) -> io::Result<()> {
    scratch.clear();
    scratch.extend_from_slice(&(query.len() as u16).to_be_bytes());
    scratch.extend_from_slice(query);
    w.write_all(scratch)?;
    w.flush()
}

fn read_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len)?;
    let mut body = vec![0u8; u16::from_be_bytes(len) as usize];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// One query as a frame on an armed stream (TCP or DoT), then the frame that answers it. Replies to other queries are
/// skipped; the stream's deadline ends the wait.
fn framed_exchange<S: Read + Write>(s: &mut S, query: &[u8], scratch: &mut Vec<u8>) -> Result<Reply, XErr> {
    let t0 = clock::now_ns();
    write_frame(s, query, scratch).map_err(io_err)?;
    loop {
        let body = read_frame(s).map_err(io_err)?;
        let rtt_ns = clock::since(t0);
        if dns::is_response_to(&body, query) {
            return Ok(Reply { bytes: body, rtt_ns, user_rtt_ns: rtt_ns, kernel_stamp: false, ttl: None });
        }
    }
}

/// A TCP stream that refuses to read or write once its deadline has passed, however slowly the peer sends.
///
/// Two ways to hold the deadline. A measuring stream ([`Bounded::new`], [`Bounded::arm`]) keeps one socket timeout
/// for the exchange and checks the clock before each call, so a peer that sends one byte at a time holds it for at
/// most twice the timeout, not once per byte of a 64 KiB frame, and no system call lands inside the timed window. A
/// strict stream ([`Bounded::until`], for NTS-KE, which isn't timed that finely) sets the socket timeout to the time
/// left before every call, so nothing runs past the deadline at all.
pub struct Bounded {
    tcp: TcpStream,
    deadline: Instant,
    strict: bool,
}

impl Bounded {
    fn new(tcp: TcpStream, timeout: Duration) -> io::Result<Self> {
        let mut b = Bounded { tcp, deadline: Instant::now(), strict: false };
        b.arm(timeout)?;
        Ok(b)
    }

    /// A strict stream whose every read and write must finish before `deadline`.
    pub(crate) fn until(tcp: TcpStream, deadline: Instant) -> Self {
        Bounded { tcp, deadline, strict: true }
    }

    /// Start a new exchange: `timeout` from now, for the stream and for each call on it.
    fn arm(&mut self, timeout: Duration) -> io::Result<()> {
        self.tcp.set_read_timeout(Some(timeout))?;
        self.tcp.set_write_timeout(Some(timeout))?;
        self.deadline = Instant::now() + timeout;
        Ok(())
    }

    /// Time left before the deadline, or a timeout error once there is none.
    fn left(&self) -> io::Result<Duration> {
        Some(self.deadline.saturating_duration_since(Instant::now())).filter(|d| !d.is_zero()).ok_or_else(|| io::ErrorKind::TimedOut.into())
    }
}

impl Read for Bounded {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.left()?;
        if self.strict {
            self.tcp.set_read_timeout(Some(left))?;
        }
        self.tcp.read(buf)
    }
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let left = self.left()?;
        if self.strict {
            self.tcp.set_write_timeout(Some(left))?;
        }
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

fn connect_tcp(addr: SocketAddr, timeout: Duration) -> Result<TcpStream, XErr> {
    let s = TcpStream::connect_timeout(&addr, timeout).map_err(io_err)?;
    s.set_nodelay(true).map_err(io_err)?;
    Ok(s)
}

pub struct TcpX {
    addr: SocketAddr,
    stream: Option<Bounded>,
    setup_ns: Option<u64>,
    buf: Vec<u8>,
}

impl TcpX {
    fn ensure(&mut self, timeout: Duration) -> Result<(), XErr> {
        if self.stream.is_none() {
            let t = clock::now_ns();
            self.stream = Some(Bounded::new(connect_tcp(self.addr, timeout)?, timeout).map_err(io_err)?);
            self.setup_ns = Some(clock::since(t));
        }
        Ok(())
    }

    fn once(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        let s = self.stream.as_mut().expect("connected");
        s.arm(timeout).map_err(io_err)?;
        framed_exchange(s, query, &mut self.buf)
    }
}

impl Exchanger for TcpX {
    fn exchange(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        self.ensure(timeout)?;
        match self.once(query, timeout) {
            Ok(r) => Ok(r),
            Err(XErr::Timeout) => {
                self.stream = None;
                Err(XErr::Timeout)
            }
            Err(_) => {
                // Servers close idle connections; reconnect once and retry.
                self.stream = None;
                self.ensure(timeout)?;
                let r = self.once(query, timeout);
                if r.is_err() {
                    self.stream = None;
                }
                r
            }
        }
    }
    fn setup_ns(&self) -> Option<u64> {
        self.setup_ns
    }
}

pub(crate) fn root_store() -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    roots
}

fn tls_config(alpn_h2: bool) -> Arc<rustls::ClientConfig> {
    static DOT: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    static DOH: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    let cell = if alpn_h2 { &DOH } else { &DOT };
    cell.get_or_init(|| {
        let mut c = rustls::ClientConfig::builder().with_root_certificates(root_store()).with_no_client_auth();
        // DoT sends no ALPN: some servers reject unknown protocol names outright.
        c.alpn_protocols = if alpn_h2 { vec![b"h2".to_vec()] } else { vec![] };
        Arc::new(c)
    })
    .clone()
}

pub struct DotX {
    addr: SocketAddr,
    name: rustls::pki_types::ServerName<'static>,
    stream: Option<rustls::StreamOwned<rustls::ClientConnection, Bounded>>,
    setup_ns: Option<u64>,
    buf: Vec<u8>,
}

impl DotX {
    fn new(addr: SocketAddr, host: &str) -> Result<Self, XErr> {
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| XErr::Tls(e.to_string()))?;
        Ok(DotX { addr, name, stream: None, setup_ns: None, buf: Vec::with_capacity(4096) })
    }

    fn ensure(&mut self, timeout: Duration) -> Result<(), XErr> {
        if self.stream.is_some() {
            return Ok(());
        }
        let t = clock::now_ns();
        // The handshake gets one deadline, however slowly the server sends its flight.
        let mut sock = Bounded::new(connect_tcp(self.addr, timeout)?, timeout).map_err(io_err)?;
        let mut conn = rustls::ClientConnection::new(tls_config(false), self.name.clone()).map_err(|e| XErr::Tls(e.to_string()))?;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).map_err(|e| match e.kind() {
                io::ErrorKind::InvalidData => XErr::Tls(e.to_string()),
                _ => io_err(e),
            })?;
        }
        self.setup_ns = Some(clock::since(t));
        self.stream = Some(rustls::StreamOwned::new(conn, sock));
        Ok(())
    }

    fn once(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        let s = self.stream.as_mut().expect("connected");
        s.sock.arm(timeout).map_err(io_err)?;
        framed_exchange(s, query, &mut self.buf)
    }
}

impl Exchanger for DotX {
    fn exchange(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        self.ensure(timeout)?;
        match self.once(query, timeout) {
            Ok(r) => Ok(r),
            Err(XErr::Timeout) => {
                self.stream = None;
                Err(XErr::Timeout)
            }
            Err(_) => {
                self.stream = None;
                self.ensure(timeout)?;
                let r = self.once(query, timeout);
                if r.is_err() {
                    self.stream = None;
                }
                r
            }
        }
    }
    fn setup_ns(&self) -> Option<u64> {
        self.setup_ns
    }
}

// ---------------------------------------------------------------------------------------
// DoH (RFC 8484) over HTTP/2, driven by a private single-threaded runtime
// ---------------------------------------------------------------------------------------

pub struct DohX {
    addr: SocketAddr,
    uri: http::Uri,
    name: rustls::pki_types::ServerName<'static>,
    rt: tokio::runtime::Runtime,
    sender: Option<h2::client::SendRequest<Bytes>>,
    /// The task driving the HTTP/2 connection behind `sender`.
    conn: Option<tokio::task::JoinHandle<()>>,
    setup_ns: Option<u64>,
}

impl DohX {
    fn new(addr: SocketAddr, url: &str) -> Result<Self, XErr> {
        let uri: http::Uri = url.parse().map_err(|e: http::uri::InvalidUri| XErr::Http(e.to_string()))?;
        let host = uri.host().ok_or_else(|| XErr::Http("DoH URL has no host".into()))?.to_string();
        let name = rustls::pki_types::ServerName::try_from(host).map_err(|e| XErr::Tls(e.to_string()))?;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(io_err)?;
        Ok(DohX { addr, uri, name, rt, sender: None, conn: None, setup_ns: None })
    }

    /// Forget the connection, so the next query opens a fresh one. Its task is stopped too:
    /// on a dead socket it would otherwise wait forever.
    fn disconnect(&mut self) {
        self.sender = None;
        if let Some(task) = self.conn.take() {
            task.abort();
        }
    }

    fn ensure(&mut self, timeout: Duration) -> Result<(), XErr> {
        if self.sender.is_some() {
            return Ok(());
        }
        let addr = self.addr;
        let name = self.name.clone();
        let t = clock::now_ns();
        let (sender, conn) = self.rt.block_on(async move {
            tokio::time::timeout(timeout, async move {
                let tcp = tokio::net::TcpStream::connect(addr).await.map_err(io_err)?;
                tcp.set_nodelay(true).map_err(io_err)?;
                let tls =
                    tokio_rustls::TlsConnector::from(tls_config(true)).connect(name, tcp).await.map_err(|e| XErr::Tls(e.to_string()))?;
                if tls.get_ref().1.alpn_protocol() != Some(b"h2") {
                    return Err(XErr::Http("server did not negotiate HTTP/2".into()));
                }
                let (send, conn) = h2::client::handshake(tls).await.map_err(|e| XErr::Http(e.to_string()))?;
                let task = tokio::spawn(async move {
                    let _ = conn.await;
                });
                let send = send.ready().await.map_err(|e| XErr::Http(e.to_string()))?;
                Ok((send, task))
            })
            .await
            .map_err(|_| XErr::Timeout)?
        })?;
        self.setup_ns = Some(clock::since(t));
        self.sender = Some(sender);
        self.conn = Some(conn);
        Ok(())
    }

    fn once(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        let sender = self.sender.clone().expect("connected");
        let uri = self.uri.clone();
        let body = Bytes::copy_from_slice(query);
        self.rt.block_on(async move {
            tokio::time::timeout(timeout, async move {
                let mut send = sender.ready().await.map_err(|e| XErr::Http(e.to_string()))?;
                let req = http::Request::post(uri)
                    .header("content-type", "application/dns-message")
                    .header("accept", "application/dns-message")
                    .header("content-length", body.len())
                    .body(())
                    .map_err(|e| XErr::Http(e.to_string()))?;
                let t0 = clock::now_ns();
                let (resp, mut stream) = send.send_request(req, false).map_err(|e| XErr::Http(e.to_string()))?;
                stream.send_data(body, true).map_err(|e| XErr::Http(e.to_string()))?;
                let resp = resp.await.map_err(|e| XErr::Http(e.to_string()))?;
                if !resp.status().is_success() {
                    return Err(XErr::Status(resp.status().as_u16()));
                }
                // A captive portal or proxy can answer 200 with a web page.
                let ct = resp.headers().get(http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
                if !ct.trim_start().to_ascii_lowercase().starts_with("application/dns-message") {
                    return Err(XErr::Http(format!("not a DNS reply (content-type {:?})", ct)));
                }
                let mut rx = resp.into_body();
                let mut out = Vec::with_capacity(512);
                while let Some(chunk) = rx.data().await {
                    let chunk = chunk.map_err(|e| XErr::Http(e.to_string()))?;
                    let _ = rx.flow_control().release_capacity(chunk.len());
                    if out.len() + chunk.len() > MAX_MSG {
                        return Err(XErr::Http("response larger than a DNS message".into()));
                    }
                    out.extend_from_slice(&chunk);
                }
                let rtt_ns = clock::since(t0);
                if !dns::is_response_to(&out, query) {
                    return Err(XErr::Http("reply doesn't match the query".into()));
                }
                Ok(Reply { bytes: out, rtt_ns, user_rtt_ns: rtt_ns, kernel_stamp: false, ttl: None })
            })
            .await
            .map_err(|_| XErr::Timeout)?
        })
    }
}

impl Exchanger for DohX {
    fn exchange(&mut self, query: &[u8], timeout: Duration) -> Result<Reply, XErr> {
        self.ensure(timeout)?;
        match self.once(query, timeout) {
            Ok(r) => Ok(r),
            // The server answered; retrying the same query would only get the same status.
            Err(e @ XErr::Status(_)) => Err(e),
            // As with TCP and DoT, a timeout drops the connection: a silently dead one would
            // otherwise time out every later query too, and nothing on it would notice.
            Err(XErr::Timeout) => {
                self.disconnect();
                Err(XErr::Timeout)
            }
            Err(_) => {
                // GOAWAY or a reset connection: reconnect once.
                self.disconnect();
                self.ensure(timeout)?;
                let r = self.once(query, timeout);
                if matches!(r, Err(ref e) if !matches!(e, XErr::Status(_))) {
                    self.disconnect();
                }
                r
            }
        }
    }
    fn setup_ns(&self) -> Option<u64> {
        self.setup_ns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::QuerySpec;
    use std::net::TcpListener;

    /// A server that accepts, sends `head`, then one zero byte every 50 ms until the client goes away.
    fn dripping(head: &'static [u8]) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                std::thread::spawn(move || {
                    if c.write_all(head).is_err() {
                        return;
                    }
                    while c.write_all(&[0]).is_ok() {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                });
            }
        });
        addr
    }

    fn query() -> Vec<u8> {
        dns::encode_query(1, &QuerySpec::new("example.com", dns::TYPE_A)).unwrap()
    }

    #[test]
    fn a_dripping_tcp_server_cannot_hold_a_query() {
        // A full-size frame, one byte at a time: each read succeeds well inside the socket timeout.
        let ep = Endpoint { transport: Transport::Tcp, addr: dripping(&[0xff, 0xff]), host: None, url: None };
        let timeout = Duration::from_millis(300);
        let mut x = open(&ep, timeout).unwrap();
        let t = Instant::now();
        assert!(matches!(x.exchange(&query(), timeout), Err(XErr::Timeout)));
        assert!(t.elapsed() < 3 * timeout, "took {:?}", t.elapsed());
    }

    #[test]
    fn a_dripping_tls_server_cannot_hold_the_handshake() {
        // A TLS handshake record header announcing 16 KiB, then its body one byte at a time.
        let ep = Endpoint {
            transport: Transport::Dot,
            addr: dripping(&[0x16, 0x03, 0x03, 0x40, 0x00]),
            host: Some("dns.example".into()),
            url: None,
        };
        let timeout = Duration::from_millis(300);
        let t = Instant::now();
        assert!(matches!(open(&ep, timeout), Err(XErr::Timeout)));
        assert!(t.elapsed() < 3 * timeout, "took {:?}", t.elapsed());
    }
}
