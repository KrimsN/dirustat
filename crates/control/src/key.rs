//! The window key: generation, the per-OS default directory and the `control-key.local-<port>` file.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Key length in random bytes (hex-encoded to twice as many characters).
const KEY_BYTES: usize = 32;

/// Generates a fresh key (64 hex characters) from the OS random generator (`getrandom`). It fails
/// closed: if the OS cannot provide randomness there is no weaker fallback, the caller gets an
/// error and must not start the control channel.
pub fn generate_key() -> io::Result<String> {
    generate_key_with(|bytes| getrandom::fill(bytes).map_err(|e| io::Error::other(format!("OS random source: {e}"))))
}

/// [`generate_key`] with an injectable randomness source (tests simulate a failing RNG).
fn generate_key_with(fill: impl FnOnce(&mut [u8]) -> io::Result<()>) -> io::Result<String> {
    let mut bytes = [0_u8; KEY_BYTES];
    fill(&mut bytes)?;
    let mut out = String::with_capacity(KEY_BYTES.saturating_mul(2));
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
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
///
/// A leftover file (for example from a crashed run, or one planted by another process) is removed
/// first and the new one is created exclusively (`create_new`), so the key is never written
/// through an existing file or link and the file always has the intended permissions.
pub fn write_key_file(dir: &Path, port: u16, key: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(key_file_name(port));
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    let written = file.write_all(key.as_bytes()).and_then(|()| file.flush());
    if let Err(e) = written {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
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
        let a = generate_key().unwrap();
        let b = generate_key().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn failing_rng_fails_closed() {
        let r = generate_key_with(|_| Err(io::Error::other("no entropy")));
        assert!(r.is_err());
    }

    #[test]
    fn stale_key_file_is_replaced_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(key_file_name(77));
        fs::write(&stale, "old-key-that-is-much-longer-than-the-new-one").unwrap();
        let path = write_key_file(dir.path(), 77, "new").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(key_file_name(77))).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
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
