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

/// A connection to a running control server. Refuses non-loopback addresses (the key never
/// leaves the machine).
#[derive(Debug)]
pub struct ControlClient {
    conn: Mutex<Conn>,
    key: String,
    next_id: AtomicU64,
}

impl ControlClient {
    /// Connects to `addr` (`127.0.0.1:<port>`) with an explicit key.
    pub fn connect(addr: &str, key: &str) -> io::Result<Self> {
        let sock: SocketAddr = addr
            .to_socket_addrs()?
            .find(SocketAddr::is_ipv4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "address did not resolve"))?;
        if !sock.ip().is_loopback() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "control channel is loopback only"));
        }
        let stream = TcpStream::connect_timeout(&sock, Duration::from_secs(5))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self { conn: Mutex::new(Conn { reader, writer: stream }), key: key.to_owned(), next_id: AtomicU64::new(1) })
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
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        conn.writer.write_all(request.to_string().as_bytes())?;
        conn.writer.write_all(b"\n")?;
        conn.writer.flush()?;
        let mut buf = Vec::new();
        match read_line_bounded(&mut conn.reader, MAX_RESPONSE_BYTES, &mut buf, None)? {
            LineRead::Line => {}
            LineRead::Eof => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "server closed the connection")),
            LineRead::TooLong { .. } => return Err(io::Error::new(io::ErrorKind::InvalidData, "response too large")),
        }
        serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
