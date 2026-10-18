//! Session journal of destructive actions (`docs/safety.md`, principle 4).
//!
//! An in-memory ring (newest entries kept) plus an optional JSONL file that is appended to on a
//! best-effort basis: a write error never fails a command. Paths appear in the journal by design
//! (it is local, for recovery from the bin and for debugging); they are never logged.

use serde::Serialize;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Entries kept in memory.
pub const MAX_JOURNAL_ENTRIES: usize = 10_000;
/// The journal file stops growing past this size (best effort, never an error).
const MAX_JOURNAL_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// One destructive action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JournalEntry {
    /// Seconds since the Unix epoch.
    pub time: u64,
    /// Command id (`file.delete`, `file.delete_permanent`, `file.move`).
    pub op: String,
    pub path: String,
    /// Destination of a move.
    pub destination: Option<String>,
    /// Size of the affected subtree in the session size mode, bytes.
    pub size: u64,
    /// `recycle`, `permanent` or `move`.
    pub mode: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Seconds since the Unix epoch (0 when the clock is before it).
pub(crate) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Debug)]
pub(crate) struct Journal {
    entries: VecDeque<JournalEntry>,
    cap: usize,
    file: Option<PathBuf>,
}

impl Journal {
    pub fn new() -> Self {
        Self::with_cap(MAX_JOURNAL_ENTRIES)
    }

    pub fn with_cap(cap: usize) -> Self {
        Self { entries: VecDeque::new(), cap: cap.max(1), file: None }
    }

    /// Mirrors new entries into a JSONL file. The directory is never created: a missing or
    /// read-only settings directory just means there is no file journal.
    pub fn set_file(&mut self, file: Option<PathBuf>) {
        self.file = file;
    }

    pub fn record(&mut self, entry: JournalEntry) {
        if let Some(path) = &self.file {
            append_line(path, &entry);
        }
        while self.entries.len() >= self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Entries newest first, skipping `offset`, at most `limit`.
    pub fn newest_first(&self, offset: usize, limit: usize) -> Vec<&JournalEntry> {
        self.entries.iter().rev().skip(offset).take(limit).collect()
    }
}

fn append_line(path: &PathBuf, entry: &JournalEntry) {
    let Ok(mut line) = serde_json::to_string(entry) else { return };
    line.push('\n');
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_JOURNAL_FILE_BYTES) {
        return;
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else { return };
    // Best effort by design: the in-memory journal is authoritative for the session.
    let _ = file.write_all(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: usize) -> JournalEntry {
        JournalEntry {
            time: 1,
            op: "file.delete".into(),
            path: format!("p{n}"),
            destination: None,
            size: 1,
            mode: "recycle".into(),
            ok: true,
            error: None,
        }
    }

    #[test]
    fn cap_keeps_the_newest_entries() {
        let mut j = Journal::new();
        for n in 0..MAX_JOURNAL_ENTRIES + 50 {
            j.record(entry(n));
        }
        assert_eq!(j.len(), MAX_JOURNAL_ENTRIES);
        let newest = j.newest_first(0, 2);
        assert_eq!(newest.first().map(|e| e.path.as_str()), Some("p10049"));
        assert_eq!(j.newest_first(MAX_JOURNAL_ENTRIES - 1, 5).first().map(|e| e.path.as_str()), Some("p50"));
    }

    #[test]
    fn file_is_appended_and_errors_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("journal.jsonl");
        let mut j = Journal::with_cap(3);
        j.set_file(Some(file.clone()));
        j.record(entry(1));
        j.record(entry(2));
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("\"path\":\"p2\""));
        // A directory that does not exist: nothing is created and nothing fails.
        j.set_file(Some(dir.path().join("missing").join("j.jsonl")));
        j.record(entry(3));
        assert!(!dir.path().join("missing").exists());
        assert_eq!(j.len(), 3);
    }
}
