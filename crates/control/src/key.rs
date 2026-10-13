//! The window key: generation, the per-OS default directory and the `control-key.local-<port>` file.

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Key length in random bytes (hex-encoded to twice as many characters).
const KEY_BYTES: usize = 32;

/// Generates a fresh key (64 hex characters). Uses `/dev/urandom` where it exists; otherwise the
/// OS-seeded `RandomState` hasher mixed with the clock, pid and a counter. The key only guards a
/// loopback socket against other local processes that cannot read the key file.
pub fn generate_key() -> String {
    let mut bytes = [0_u8; KEY_BYTES];
    if !fill_from_os(&mut bytes) {
        fill_from_hasher(&mut bytes);
    }
    let mut out = String::with_capacity(KEY_BYTES.saturating_mul(2));
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn fill_from_os(bytes: &mut [u8]) -> bool {
    fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(bytes)).is_ok()
}

fn fill_from_hasher(bytes: &mut [u8]) {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for chunk in bytes.chunks_mut(8) {
        // `RandomState::new` draws its keys from the OS random source.
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        let word = hasher.finish().to_le_bytes();
        for (dst, src) in chunk.iter_mut().zip(word) {
            *dst = src;
        }
    }
}

/// File name of the key for a control port.
pub fn key_file_name(port: u16) -> String {
    format!("control-key.local-{port}")
}

/// Default directory for key files: `%APPDATA%\dirustat`, `~/Library/Application Support/diRUSTat`
/// or `$XDG_CONFIG_HOME`/`~/.config` + `dirustat`. `None` if the needed environment is missing.
pub fn default_key_dir() -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        env("APPDATA").map(|p| p.join("dirustat"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|p| p.join("Library").join("Application Support").join("diRUSTat"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|p| p.join(".config"))).map(|p| p.join("dirustat"))
    }
}

/// Writes the key file (owner-only permissions on Unix) and returns its path.
pub fn write_key_file(dir: &Path, port: u16, key: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(key_file_name(port));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(key.as_bytes())?;
    file.flush()?;
    Ok(path)
}

/// Reads the key for `port` from `dir` (surrounding whitespace removed).
pub fn read_key_file(dir: &Path, port: u16) -> io::Result<String> {
    let text = fs::read_to_string(dir.join(key_file_name(port)))?;
    Ok(text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_hex_and_distinct() {
        let a = generate_key();
        let b = generate_key();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        let mut bytes = [0_u8; 32];
        fill_from_hasher(&mut bytes);
        assert!(bytes.iter().any(|b| *b != 0));
    }

    #[test]
    fn key_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        let path = write_key_file(&nested, 4242, "abc").unwrap();
        assert!(path.ends_with("control-key.local-4242"));
        assert_eq!(read_key_file(&nested, 4242).unwrap(), "abc");
        assert!(read_key_file(&nested, 1).is_err());
    }

    #[test]
    fn default_dir_ends_with_app_name() {
        if let Some(dir) = default_key_dir() {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            assert_eq!(name, "dirustat");
        }
    }
}
