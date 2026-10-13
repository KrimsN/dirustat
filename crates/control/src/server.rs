//! TCP server: JSON lines on 127.0.0.1 only, key required, bounded in size, rate and time.

use crate::bridge::UiBridge;
use crate::dispatcher::{Dispatcher, MAX_REQUEST_BYTES, SharedSession};
use crate::error::{self, ControlError};
use crate::key::{generate_key, write_key_file};
use crate::line::{LineRead, read_line_bounded};
use serde_json::json;
use std::fs;
use std::io::{self, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Concurrent connections served at once; further ones are dropped.
pub const MAX_CONNECTIONS: usize = 16;
/// Requests allowed per connection per [`RATE_WINDOW`].
pub const RATE_LIMIT: u32 = 200;
pub const RATE_WINDOW: Duration = Duration::from_secs(1);
/// Requests allowed per [`RATE_WINDOW`] across all connections together, so many connections
/// cannot multiply the per-connection budget.
pub const GLOBAL_RATE_LIMIT: u32 = 1000;
/// A connection must send a request carrying the right key within this time of connecting;
/// idle or wrong-key peers give their slot back.
pub const PRE_AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// A request line must arrive within this time of the previous response (idle and slow-drip bound).
pub const LINE_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const READ_POLL: Duration = Duration::from_millis(500);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Tunable limits of a [`ControlServer`] (tests use small ones).
#[derive(Clone, Copy, Debug)]
pub struct ServerLimits {
    pub max_connections: usize,
    pub conn_rate: u32,
    pub global_rate: u32,
    pub pre_auth_timeout: Duration,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            max_connections: MAX_CONNECTIONS,
            conn_rate: RATE_LIMIT,
            global_rate: GLOBAL_RATE_LIMIT,
            pre_auth_timeout: PRE_AUTH_TIMEOUT,
        }
    }
}

/// State shared by all connection threads.
#[derive(Debug)]
struct Shared {
    limits: ServerLimits,
    global: Mutex<RateLimiter>,
}

/// Fixed-window request counter.
#[derive(Debug)]
pub struct RateLimiter {
    max: u32,
    window: Duration,
    start: Instant,
    count: u32,
}

impl RateLimiter {
    pub fn new(max: u32, window: Duration, now: Instant) -> Self {
        Self { max, window, start: now, count: 0 }
    }

    /// Records a request at `now`; false if the limit for the current window is exceeded.
    pub fn allow(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.start) >= self.window {
            self.start = now;
            self.count = 0;
        }
        if self.count >= self.max {
            return false;
        }
        self.count = self.count.saturating_add(1);
        true
    }
}

/// A running control server. Dropping it stops it and removes the key file.
#[derive(Debug)]
pub struct ControlServer {
    port: u16,
    key: String,
    key_path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// Binds `127.0.0.1:port` (0 = any free port), generates a key and writes it to
    /// `control-key.local-<port>` in `key_dir`, then serves in background threads.
    pub fn start(port: u16, key_dir: &Path, session: SharedSession, bridge: Arc<dyn UiBridge>) -> io::Result<Self> {
        Self::start_with_limits(port, key_dir, session, bridge, ServerLimits::default())
    }

    /// Like [`Self::start`] with explicit limits. Fails (and starts nothing) when the OS random
    /// source or the key file is unavailable: the channel is never run without a strong key.
    pub fn start_with_limits(
        port: u16,
        key_dir: &Path,
        session: SharedSession,
        bridge: Arc<dyn UiBridge>,
        limits: ServerLimits,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let key = generate_key()?;
        let key_path = write_key_file(key_dir, port, &key)?;
        let dispatcher = Dispatcher::new(session, bridge).with_key(key.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let accept_stop = Arc::clone(&stop);
        let shared = Arc::new(Shared {
            limits,
            global: Mutex::new(RateLimiter::new(limits.global_rate, RATE_WINDOW, Instant::now())),
        });
        let spawned = thread::Builder::new()
            .name("dirustat-control-accept".to_owned())
            .spawn(move || accept_loop(&listener, &dispatcher, &accept_stop, &shared));
        let thread = match spawned {
            Ok(t) => t,
            Err(e) => {
                let _ = fs::remove_file(&key_path);
                return Err(e);
            }
        };
        Ok(Self { port, key, key_path, stop, thread: Some(thread) })
    }

    /// The port actually bound.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn key_path(&self) -> &Path {
        &self.key_path
    }

    /// Stops accepting, lets connections end and removes the key file.
    pub fn shutdown(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = fs::remove_file(&self.key_path);
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop_now();
    }
}

fn accept_loop(listener: &TcpListener, dispatcher: &Dispatcher, stop: &Arc<AtomicBool>, shared: &Arc<Shared>) {
    let active = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() || active.load(Ordering::SeqCst) >= shared.limits.max_connections {
                    continue;
                }
                active.fetch_add(1, Ordering::SeqCst);
                let (d, s, a) = (dispatcher.clone(), Arc::clone(stop), Arc::clone(&active));
                let sh = Arc::clone(shared);
                let spawned = thread::Builder::new().name("dirustat-control-conn".to_owned()).spawn(move || {
                    serve_connection(stream, &d, &s, &sh);
                    a.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    active.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL_INTERVAL),
            Err(_) => thread::sleep(POLL_INTERVAL.saturating_mul(4)),
        }
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

fn write_line(stream: &mut TcpStream, line: &str) -> io::Result<()> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn error_line(err: &ControlError) -> String {
    json!({ "id": null, "ok": false, "error": { "code": err.code(), "message": err.message() } }).to_string()
}

fn serve_connection(stream: TcpStream, dispatcher: &Dispatcher, stop: &AtomicBool, shared: &Shared) {
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(READ_POLL)).is_err()
        || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
    {
        return;
    }
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut limiter = RateLimiter::new(shared.limits.conn_rate, RATE_WINDOW, Instant::now());
    let mut buf = Vec::new();
    // Until a request carries the right key the whole connection lives on this short deadline.
    let mut authed = false;
    let mut deadline = Instant::now() + shared.limits.pre_auth_timeout;
    while !stop.load(Ordering::SeqCst) {
        match read_line_bounded(&mut reader, MAX_REQUEST_BYTES, &mut buf, Some(deadline)) {
            Ok(LineRead::Eof) => return,
            Ok(LineRead::TooLong { .. }) => {
                let err = ControlError::new(error::TOO_LARGE, "request exceeds the size limit");
                let _ = write_line(&mut writer, &error_line(&err));
                return;
            }
            Ok(LineRead::Line) => {
                let (response, ok) = answer(dispatcher, &buf, &mut limiter, &shared.global);
                authed |= ok;
                buf.clear();
                if let Some(response) = response
                    && write_line(&mut writer, &response).is_err()
                {
                    return;
                }
                if authed {
                    deadline = Instant::now() + LINE_TIMEOUT;
                }
            }
            // Poll timeout: partial data stays in `buf`; the deadline bounds slow peers.
            Err(e) if is_timeout(&e) && Instant::now() < deadline => {}
            Err(_) => return,
        }
    }
}

/// Answers one line; the flag is true when the request passed the key check.
fn answer(
    dispatcher: &Dispatcher,
    line: &[u8],
    limiter: &mut RateLimiter,
    global: &Mutex<RateLimiter>,
) -> (Option<String>, bool) {
    if line.iter().all(u8::is_ascii_whitespace) {
        return (None, false);
    }
    let now = Instant::now();
    let global_ok = global.lock().unwrap_or_else(PoisonError::into_inner).allow(now);
    if !global_ok || !limiter.allow(now) {
        return (Some(error_line(&ControlError::new(error::RATE_LIMITED, "too many requests"))), false);
    }
    match std::str::from_utf8(line) {
        Ok(text) => {
            let (response, ok) = dispatcher.handle_line_checked(text);
            (Some(response), ok)
        }
        Err(_) => (Some(error_line(&ControlError::new(error::BAD_REQUEST, "request is not valid UTF-8"))), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::NullBridge;
    use crate::dispatcher::share;
    use dirustat_engine::Session;
    use serde_json::Value;
    use std::io::{BufRead, Read};

    fn start() -> (ControlServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let server = ControlServer::start(0, dir.path(), share(Session::new()), Arc::new(NullBridge)).unwrap();
        (server, dir)
    }

    struct Conn {
        reader: BufReader<TcpStream>,
        writer: TcpStream,
    }

    impl Conn {
        fn open(port: u16) -> Self {
            let s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            Self { reader: BufReader::new(s.try_clone().unwrap()), writer: s }
        }
        fn send_raw(&mut self, bytes: &[u8]) {
            self.writer.write_all(bytes).unwrap();
        }
        fn recv(&mut self) -> Value {
            let mut line = String::new();
            self.reader.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }
        fn ask(&mut self, req: &Value) -> Value {
            self.send_raw(format!("{req}\n").as_bytes());
            self.recv()
        }
    }

    #[test]
    fn loopback_round_trip_with_key_file() {
        let (server, dir) = start();
        assert_ne!(server.port(), 0);
        let key = crate::key::read_key_file(dir.path(), server.port()).unwrap();
        assert_eq!(key, server.key());
        let mut c = Conn::open(server.port());
        let v = c.ask(&json!({"id": 1, "key": key, "method": "session.state"}));
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["result"]["nodes"], 1);
        // Wrong key, then malformed JSON, then a good request on the same connection.
        let v = c.ask(&json!({"id": 2, "key": "bad", "method": "session.state"}));
        assert_eq!(v["error"]["code"], "unauthorized");
        c.send_raw(b"{oops\n\n");
        assert_eq!(c.recv()["error"]["code"], "bad_request");
        let v = c.ask(&json!({"id": 3, "key": server.key(), "method": "engine.commands"}));
        assert_eq!(v["id"], 3);
        let v = c.ask(&json!({"id": 4, "key": server.key(), "method": "ui.state"}));
        assert_eq!(v["error"]["code"], "unsupported");
        let path = server.key_path().to_owned();
        assert!(path.exists());
        server.shutdown();
        assert!(!path.exists());
    }

    #[test]
    fn oversized_request_is_rejected_and_connection_closed() {
        let (server, _dir) = start();
        let mut c = Conn::open(server.port());
        let mut big = vec![b'a'; MAX_REQUEST_BYTES + 10];
        big.push(b'\n');
        // The server may close while we are still writing; ignore a broken pipe.
        let _ = c.writer.write_all(&big);
        let v = c.recv();
        assert_eq!(v["error"]["code"], "too_large");
        let mut rest = Vec::new();
        let _ = c.reader.read_to_end(&mut rest);
        // The server is still alive for new connections.
        let mut c2 = Conn::open(server.port());
        let v = c2.ask(&json!({"id": 1, "key": server.key(), "method": "session.state"}));
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn invalid_utf8_gets_an_error() {
        let (server, _dir) = start();
        let mut c = Conn::open(server.port());
        c.send_raw(b"\xff\xfe\n");
        assert_eq!(c.recv()["error"]["code"], "bad_request");
    }

    #[test]
    fn rate_limiter_windows() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(2, Duration::from_secs(1), t0);
        assert!(l.allow(t0));
        assert!(l.allow(t0));
        assert!(!l.allow(t0));
        assert!(l.allow(t0 + Duration::from_secs(2)));
    }

    #[test]
    fn rate_limit_is_reported_over_the_wire() {
        let (server, _dir) = start();
        let mut c = Conn::open(server.port());
        let req = format!("{}\n", json!({"id": 1, "key": server.key(), "method": "session.state"}));
        let burst = usize::try_from(RATE_LIMIT).unwrap() + 50;
        c.send_raw(req.repeat(burst).as_bytes());
        let mut limited = 0;
        for _ in 0..burst {
            if c.recv()["error"]["code"] == "rate_limited" {
                limited += 1;
            }
        }
        assert!(limited > 0);
    }

    fn start_limited(limits: ServerLimits) -> (ControlServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let server =
            ControlServer::start_with_limits(0, dir.path(), share(Session::new()), Arc::new(NullBridge), limits)
                .unwrap();
        (server, dir)
    }

    fn ping(port: u16, key: &str) -> Value {
        Conn::open(port).ask(&json!({"id": 1, "key": key, "method": "session.state"}))
    }

    #[test]
    fn idle_unauthenticated_connections_free_their_slots() {
        let limits = ServerLimits {
            max_connections: 3,
            pre_auth_timeout: Duration::from_millis(300),
            ..ServerLimits::default()
        };
        let (server, _dir) = start_limited(limits);
        // Fill every slot with silent connections.
        let idle: Vec<TcpStream> =
            (0..3).map(|_| TcpStream::connect((Ipv4Addr::LOCALHOST, server.port())).unwrap()).collect();
        thread::sleep(Duration::from_millis(1200));
        // The idle peers were dropped by the server, so a real client gets in.
        assert_eq!(ping(server.port(), server.key())["ok"], true);
        drop(idle);
    }

    #[test]
    fn wrong_key_does_not_extend_the_pre_auth_deadline() {
        let limits = ServerLimits { pre_auth_timeout: Duration::from_millis(400), ..ServerLimits::default() };
        let (server, _dir) = start_limited(limits);
        let mut c = Conn::open(server.port());
        let started = Instant::now();
        let mut closed = false;
        while started.elapsed() < Duration::from_secs(5) {
            if c.writer.write_all(b"{\"id\":1,\"key\":\"bad\",\"method\":\"session.state\"}\n").is_err() {
                closed = true;
                break;
            }
            let mut line = String::new();
            if c.reader.read_line(&mut line).unwrap_or(0) == 0 {
                closed = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(closed, "a peer that never authenticates must be disconnected");
    }

    #[test]
    fn authenticated_connection_survives_past_the_pre_auth_deadline() {
        let limits = ServerLimits { pre_auth_timeout: Duration::from_millis(300), ..ServerLimits::default() };
        let (server, _dir) = start_limited(limits);
        let mut c = Conn::open(server.port());
        assert_eq!(c.ask(&json!({"id": 1, "key": server.key(), "method": "session.state"}))["ok"], true);
        thread::sleep(Duration::from_millis(900));
        assert_eq!(c.ask(&json!({"id": 2, "key": server.key(), "method": "session.state"}))["ok"], true);
    }

    #[test]
    fn global_rate_limit_applies_across_connections() {
        let limits = ServerLimits { global_rate: 5, ..ServerLimits::default() };
        let (server, _dir) = start_limited(limits);
        let mut limited = 0;
        // Each connection stays far below the per-connection budget; together they exceed 5.
        for _ in 0..12 {
            if ping(server.port(), server.key())["error"]["code"] == "rate_limited" {
                limited += 1;
            }
        }
        assert!(limited > 0);
    }

    #[test]
    fn mid_line_close_frees_the_slot() {
        let limits = ServerLimits { max_connections: 2, ..ServerLimits::default() };
        let (server, _dir) = start_limited(limits);
        for _ in 0..6 {
            let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port())).unwrap();
            s.write_all(b"{\"id\":1,\"key\":\"").unwrap();
            drop(s);
        }
        thread::sleep(Duration::from_millis(300));
        assert_eq!(ping(server.port(), server.key())["ok"], true);
    }

    #[test]
    fn port_in_use_is_an_error() {
        let (server, dir) = start();
        let second = ControlServer::start(server.port(), dir.path(), share(Session::new()), Arc::new(NullBridge));
        assert!(second.is_err());
    }
}
