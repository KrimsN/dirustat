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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Concurrent connections served at once; further ones are dropped.
pub const MAX_CONNECTIONS: usize = 16;
/// Requests allowed per connection per [`RATE_WINDOW`].
pub const RATE_LIMIT: u32 = 200;
pub const RATE_WINDOW: Duration = Duration::from_secs(1);
/// A request line must arrive within this time of the previous response (idle and slow-drip bound).
pub const LINE_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const READ_POLL: Duration = Duration::from_millis(500);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

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
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let key = generate_key();
        let key_path = write_key_file(key_dir, port, &key)?;
        let dispatcher = Dispatcher::new(session, bridge).with_key(key.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let accept_stop = Arc::clone(&stop);
        let spawned = thread::Builder::new()
            .name("dirustat-control-accept".to_owned())
            .spawn(move || accept_loop(&listener, &dispatcher, &accept_stop));
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

fn accept_loop(listener: &TcpListener, dispatcher: &Dispatcher, stop: &Arc<AtomicBool>) {
    let active = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() || active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    continue;
                }
                active.fetch_add(1, Ordering::SeqCst);
                let (d, s, a) = (dispatcher.clone(), Arc::clone(stop), Arc::clone(&active));
                let spawned = thread::Builder::new().name("dirustat-control-conn".to_owned()).spawn(move || {
                    serve_connection(stream, &d, &s);
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

fn serve_connection(stream: TcpStream, dispatcher: &Dispatcher, stop: &AtomicBool) {
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(READ_POLL)).is_err()
        || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
    {
        return;
    }
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut limiter = RateLimiter::new(RATE_LIMIT, RATE_WINDOW, Instant::now());
    let mut buf = Vec::new();
    let mut deadline = Instant::now() + LINE_TIMEOUT;
    while !stop.load(Ordering::SeqCst) {
        match read_line_bounded(&mut reader, MAX_REQUEST_BYTES, &mut buf, Some(deadline)) {
            Ok(LineRead::Eof) => return,
            Ok(LineRead::TooLong { .. }) => {
                let err = ControlError::new(error::TOO_LARGE, "request exceeds the size limit");
                let _ = write_line(&mut writer, &error_line(&err));
                return;
            }
            Ok(LineRead::Line) => {
                let response = answer(dispatcher, &buf, &mut limiter);
                buf.clear();
                if let Some(response) = response
                    && write_line(&mut writer, &response).is_err()
                {
                    return;
                }
                deadline = Instant::now() + LINE_TIMEOUT;
            }
            // Poll timeout: partial data stays in `buf`; the deadline bounds slow peers.
            Err(e) if is_timeout(&e) && Instant::now() < deadline => {}
            Err(_) => return,
        }
    }
}

fn answer(dispatcher: &Dispatcher, line: &[u8], limiter: &mut RateLimiter) -> Option<String> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    if !limiter.allow(Instant::now()) {
        return Some(error_line(&ControlError::new(error::RATE_LIMITED, "too many requests")));
    }
    match std::str::from_utf8(line) {
        Ok(text) => Some(dispatcher.handle_line(text)),
        Err(_) => Some(error_line(&ControlError::new(error::BAD_REQUEST, "request is not valid UTF-8"))),
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

    #[test]
    fn port_in_use_is_an_error() {
        let (server, dir) = start();
        let second = ControlServer::start(server.port(), dir.path(), share(Session::new()), Arc::new(NullBridge));
        assert!(second.is_err());
    }
}
