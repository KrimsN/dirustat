//! Client side of the control channel (used by `mcp --connect`).

use crate::dispatcher::{Backend, MAX_RESPONSE_BYTES};
use crate::error::ControlError;
use crate::key::read_key_file;
use crate::line::{LineRead, read_line_bounded};
use serde_json::{Value, json};
use std::io::{self, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
struct Conn {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Conn {
    fn open(sock: &SocketAddr, io_timeout: Duration) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(sock, Duration::from_secs(5))?;
        stream.set_read_timeout(Some(io_timeout))?;
        stream.set_write_timeout(Some(io_timeout))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { reader, writer: stream })
    }

    fn exchange(&mut self, request: &Value, id: u64) -> io::Result<Value> {
        self.writer.write_all(request.to_string().as_bytes())?;
        self.writer.write_all(
            b"
",
        )?;
        self.writer.flush()?;
        let mut buf = Vec::new();
        match read_line_bounded(&mut self.reader, MAX_RESPONSE_BYTES, &mut buf, None)? {
            LineRead::Line => {}
            LineRead::Eof => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "server closed the connection")),
            LineRead::TooLong { .. } => return Err(io::Error::new(io::ErrorKind::InvalidData, "response too large")),
        }
        let response: Value =
            serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // A reply for another request means the stream is out of step (for example after a timed
        // out call whose answer arrived late); it must not be taken for this call's answer.
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "response id does not match the request"));
        }
        Ok(response)
    }
}

/// A connection to a running control server. Refuses non-loopback addresses (the key never
/// leaves the machine). After any I/O error or timeout the connection is dropped and the next
/// call reconnects, so a late answer can never be paired with a newer request.
#[derive(Debug)]
pub struct ControlClient {
    sock: SocketAddr,
    io_timeout: Duration,
    conn: Mutex<Option<Conn>>,
    key: String,
    next_id: AtomicU64,
}

impl ControlClient {
    /// Connects to `addr` (`127.0.0.1:<port>`) with an explicit key.
    pub fn connect(addr: &str, key: &str) -> io::Result<Self> {
        Self::connect_with_timeout(addr, key, IO_TIMEOUT)
    }

    /// Like [`Self::connect`] with an explicit per-call I/O timeout.
    pub fn connect_with_timeout(addr: &str, key: &str, io_timeout: Duration) -> io::Result<Self> {
        let sock: SocketAddr = addr
            .to_socket_addrs()?
            .find(SocketAddr::is_ipv4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "address did not resolve"))?;
        if !sock.ip().is_loopback() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "control channel is loopback only"));
        }
        // Prove the server is reachable, but do not keep the socket: the server drops connections
        // that stay silent before their first request, so the real one opens on the first call.
        drop(Conn::open(&sock, io_timeout)?);
        Ok(Self { sock, io_timeout, conn: Mutex::new(None), key: key.to_owned(), next_id: AtomicU64::new(1) })
    }

    /// Connects to `addr`, reading the key from `control-key.local-<port>` in `key_dir`.
    pub fn connect_with_key_dir(addr: &str, key_dir: &Path) -> io::Result<Self> {
        let port = addr
            .rsplit_once(':')
            .and_then(|(_, p)| p.parse::<u16>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "address must be host:port"))?;
        Self::connect(addr, &read_key_file(key_dir, port)?)
    }

    fn roundtrip(&self, method: &str, params: &Value) -> io::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({ "id": id, "key": self.key, "method": method, "params": params });
        let mut slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(Conn::open(&self.sock, self.io_timeout)?);
        }
        let Some(conn) = slot.as_mut() else {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "no connection"));
        };
        let result = conn.exchange(&request, id);
        if result.is_err() {
            // Broken or out of step: drop it; the next call reconnects.
            *slot = None;
        }
        result
    }
}

impl Backend for ControlClient {
    fn call(&self, method: &str, params: &Value) -> Result<Value, ControlError> {
        let response =
            self.roundtrip(method, params).map_err(|e| ControlError::internal(format!("connection: {e}")))?;
        if response.get("ok") == Some(&Value::Bool(true)) {
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
        let err = response.get("error");
        let field = |name: &str| err.and_then(|e| e.get(name)).and_then(Value::as_str).unwrap_or("unknown");
        Err(ControlError::new(field("code"), field("message")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::thread;

    /// Server that answers each connection's requests with `reply(request_id, call_index)`;
    /// `None` means "stay silent" (client times out).
    fn fake_server(reply: impl Fn(u64, usize) -> Option<String> + Send + 'static) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            let mut calls = 0;
            // Serve a few connections: the reachability probe, the first call, a reconnect.
            for _ in 0..3 {
                let Ok((stream, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let id = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["id"].as_u64()).unwrap_or(0);
                    if let Some(r) = reply(id, calls) {
                        let _ = writeln!(writer, "{r}");
                    }
                    calls += 1;
                    line.clear();
                }
            }
        });
        (addr, handle)
    }

    #[test]
    fn mismatched_response_id_is_an_error_and_drops_the_connection() {
        let (addr, h) = fake_server(|id, call| {
            // First call: reply with a wrong id. Later calls: correct.
            let rid = if call == 0 { id + 100 } else { id };
            Some(json!({"id": rid, "ok": true, "result": call}).to_string())
        });
        let c = ControlClient::connect(&addr, "k").unwrap();
        let first = c.call("x", &json!({}));
        assert!(first.is_err());
        // The next call reconnects and succeeds.
        assert_eq!(c.call("x", &json!({})).unwrap(), json!(1));
        drop(c);
        let _ = h.join();
    }

    #[test]
    fn timeout_drops_the_connection_so_a_late_answer_is_not_misread() {
        let (addr, h) = fake_server(|id, call| {
            if call == 0 { None } else { Some(json!({"id": id, "ok": true, "result": "fresh"}).to_string()) }
        });
        let c = ControlClient::connect_with_timeout(&addr, "k", Duration::from_millis(200)).unwrap();
        assert!(c.call("slow", &json!({})).is_err());
        assert_eq!(c.call("again", &json!({})).unwrap(), json!("fresh"));
        drop(c);
        let _ = h.join();
    }
}
