//! The `.dirsnap` file format (G-019): a compact, versioned, checksummed binary dump of a tree.
//!
//! ```text
//! magic    8 bytes  "DIRUSNAP"
//! version  u32 LE   1
//! flags    u32 LE   bit 0: the scan used exclusions (the tree omits entries that exist on disk)
//! created  i64 LE   Unix seconds, 0 when the clock is unusable
//! count    u64 LE   number of node records
//! records  count x { [parent varint, absent for node 0], tag u8, name_len varint, name bytes,
//!                    logical varint, allocated varint, attributes varint, [mtime zigzag varint] }
//! checksum 32 bytes BLAKE3 over everything before it
//! ```
//!
//! Records are in arena order, so a parent always comes before its children and a loaded tree keeps
//! the node ids of the saved one. `tag`: bits 0-1 kind (0 dir, 1 file, 2 link), bit 2 error, bit 3
//! name_lossy, bit 4 has mtime; the other bits must be zero.
//!
//! The reader treats the file as hostile: sizes are checked against the real file length before
//! anything is allocated, a parent reference must point at an earlier directory (so cycles and
//! out-of-range ids cannot be expressed), names are validated, counts are capped, and the checksum
//! and the end of the file are verified. Nothing here panics.

use dirustat_model::{ModelError, NodeData, NodeKind, Tree};
use std::fmt;
use std::io::{self, Read, Write};

pub const MAGIC: &[u8; 8] = b"DIRUSNAP";
pub const VERSION: u32 = 1;
/// Most nodes a snapshot may hold (reading and writing).
pub const HARD_MAX_NODES: u64 = 20_000_000;
/// Most nodes loaded when the caller gives no limit.
pub const DEFAULT_LOAD_NODES: u64 = 5_000_000;
/// Largest snapshot file that is read, in bytes.
pub const MAX_FILE_BYTES: u64 = 2 << 30;
/// Longest stored name, in bytes (the root holds a full path).
const MAX_NAME_BYTES: u64 = 65_536;
/// Smallest possible record: tag, name length, two sizes, attributes (the root has no parent).
const MIN_RECORD_BYTES: u64 = 5;
const HEADER_BYTES: u64 = 32;
const CHECKSUM_BYTES: u64 = 32;
const FLAG_PARTIAL: u32 = 1;
const TAG_ERROR: u8 = 1 << 2;
const TAG_LOSSY: u8 = 1 << 3;
const TAG_MTIME: u8 = 1 << 4;
const TAG_RESERVED: u8 = 0xe0;
const BUF_BYTES: usize = 64 * 1024;

/// What the header says about a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Unix seconds when it was saved (0 = unknown).
    pub created: i64,
    /// The saved scan used exclusions.
    pub partial: bool,
    pub nodes: u64,
}

/// A decoded snapshot.
#[derive(Debug)]
pub struct Loaded {
    pub tree: Tree,
    pub meta: Meta,
}

/// Why a snapshot could not be read or written.
#[derive(Debug)]
pub enum SnapshotError {
    Io(io::Error),
    /// Not a snapshot file at all.
    BadMagic,
    /// A newer (or unknown) format version.
    Version(u32),
    /// The content breaks the format; the text names the first problem.
    Corrupt(&'static str),
    /// The stored checksum does not match the content.
    Checksum,
    /// A limit (file size, node count) was exceeded.
    TooLarge(&'static str),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::BadMagic => f.write_str("not a diRUSTat snapshot (bad magic)"),
            Self::Version(v) => write!(f, "unsupported snapshot version {v} (this build reads {VERSION})"),
            Self::Corrupt(why) => write!(f, "corrupt snapshot: {why}"),
            Self::Checksum => f.write_str("corrupt snapshot: checksum mismatch"),
            Self::TooLarge(what) => write!(f, "snapshot too large: {what}"),
        }
    }
}

impl From<io::Error> for SnapshotError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::UnexpectedEof { Self::Corrupt("the file is truncated") } else { Self::Io(e) }
    }
}

// ---------------------------------------------------------------------------------- writing

struct HashingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
    bytes: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(buf.get(..n).unwrap_or_default());
        self.bytes = self.bytes.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn put_varint<W: Write>(out: &mut W, mut value: u64) -> io::Result<()> {
    let mut bytes = [0_u8; 10];
    let mut len = 0;
    loop {
        let low = u8::try_from(value & 0x7f).unwrap_or(0);
        value >>= 7;
        let byte = if value == 0 { low } else { low | 0x80 };
        if let Some(slot) = bytes.get_mut(len) {
            *slot = byte;
        }
        len += 1;
        if value == 0 {
            break;
        }
    }
    out.write_all(bytes.get(..len).unwrap_or_default())
}

const fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)).cast_unsigned()
}

const fn unzigzag(v: u64) -> i64 {
    (v >> 1).cast_signed() ^ -((v & 1).cast_signed())
}

/// Writes `tree` as a snapshot. Returns the number of bytes written (checksum included).
pub fn write<W: Write>(out: W, tree: &Tree, partial: bool, created: i64) -> Result<u64, SnapshotError> {
    let count = u64::try_from(tree.len()).unwrap_or(u64::MAX);
    if count > HARD_MAX_NODES {
        return Err(SnapshotError::TooLarge("the tree has more nodes than a snapshot can hold"));
    }
    let mut w =
        HashingWriter { inner: io::BufWriter::with_capacity(BUF_BYTES, out), hasher: blake3::Hasher::new(), bytes: 0 };
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&(if partial { FLAG_PARTIAL } else { 0 }).to_le_bytes())?;
    w.write_all(&created.to_le_bytes())?;
    w.write_all(&count.to_le_bytes())?;
    for index in 0..tree.len() {
        let node = tree.node_at(index).and_then(|id| tree.get(id)).ok_or(SnapshotError::Corrupt("node vanished"))?;
        if index > 0 {
            let parent = node.parent().ok_or(SnapshotError::Corrupt("a non-root node has no parent"))?;
            put_varint(&mut w, u64::try_from(parent.index()).unwrap_or(u64::MAX))?;
        }
        let data = node.data();
        let kind: u8 = match node.kind() {
            NodeKind::Dir => 0,
            NodeKind::File => 1,
            NodeKind::Link => 2,
        };
        let tag = kind
            | if data.error { TAG_ERROR } else { 0 }
            | if data.name_lossy { TAG_LOSSY } else { 0 }
            | if data.mtime.is_some() { TAG_MTIME } else { 0 };
        w.write_all(&[tag])?;
        put_varint(&mut w, u64::try_from(node.name().len()).unwrap_or(u64::MAX))?;
        w.write_all(node.name().as_bytes())?;
        put_varint(&mut w, data.logical_size)?;
        put_varint(&mut w, data.allocated_size)?;
        put_varint(&mut w, u64::from(data.attributes))?;
        if let Some(mtime) = data.mtime {
            put_varint(&mut w, zigzag(mtime))?;
        }
    }
    let digest = w.hasher.finalize();
    let HashingWriter { mut inner, bytes, .. } = w;
    inner.write_all(digest.as_bytes())?;
    inner.flush()?;
    Ok(bytes.saturating_add(CHECKSUM_BYTES))
}

// ---------------------------------------------------------------------------------- reading

/// A buffered reader that hashes what is consumed (lazily, per chunk).
struct Source<R> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
    end: usize,
    hashed: usize,
    hasher: blake3::Hasher,
}

impl<R: Read> Source<R> {
    fn new(inner: R) -> Self {
        Self { inner, buf: vec![0; BUF_BYTES], pos: 0, end: 0, hashed: 0, hasher: blake3::Hasher::new() }
    }

    fn sync_hash(&mut self) {
        if let Some(done) = self.buf.get(self.hashed..self.pos) {
            self.hasher.update(done);
        }
        self.hashed = self.pos;
    }

    fn fill(&mut self) -> Result<(), SnapshotError> {
        self.sync_hash();
        let n = loop {
            match self.inner.read(&mut self.buf) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        };
        if n == 0 {
            return Err(SnapshotError::Corrupt("the file is truncated"));
        }
        (self.pos, self.end, self.hashed) = (0, n, 0);
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, SnapshotError> {
        if self.pos >= self.end {
            self.fill()?;
        }
        let byte = *self.buf.get(self.pos).ok_or(SnapshotError::Corrupt("the file is truncated"))?;
        self.pos += 1;
        Ok(byte)
    }

    fn fill_exact(&mut self, out: &mut [u8]) -> Result<(), SnapshotError> {
        let mut done = 0;
        while done < out.len() {
            if self.pos >= self.end {
                self.fill()?;
            }
            let want = out.len() - done;
            let take = want.min(self.end - self.pos);
            let src = self.buf.get(self.pos..self.pos + take).ok_or(SnapshotError::Corrupt("the file is truncated"))?;
            out.get_mut(done..done + take).ok_or(SnapshotError::Corrupt("the file is truncated"))?.copy_from_slice(src);
            self.pos += take;
            done += take;
        }
        Ok(())
    }

    fn varint(&mut self) -> Result<u64, SnapshotError> {
        let mut value = 0_u64;
        for shift in (0..70).step_by(7) {
            let byte = self.u8()?;
            let low = u64::from(byte & 0x7f);
            if shift == 63 && low > 1 {
                return Err(SnapshotError::Corrupt("a number is out of range"));
            }
            value |= low << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(SnapshotError::Corrupt("a number is too long"))
    }

    /// Hash of everything consumed so far, the stored checksum after it, and a check for the end of
    /// the file.
    fn finish(mut self) -> Result<([u8; 32], [u8; 32]), SnapshotError> {
        self.sync_hash();
        let computed = *self.hasher.finalize().as_bytes();
        let mut stored = [0_u8; 32];
        self.fill_exact(&mut stored)?;
        if self.pos < self.end || self.fill().is_ok() {
            return Err(SnapshotError::Corrupt("unexpected data after the checksum"));
        }
        Ok((computed, stored))
    }
}

fn model_error(e: ModelError) -> SnapshotError {
    match e {
        ModelError::NotADirectory => SnapshotError::Corrupt("a node's parent is not a directory"),
        ModelError::UnknownNode => SnapshotError::Corrupt("a parent id is out of range"),
        ModelError::TooManyNodes => SnapshotError::TooLarge("too many nodes"),
    }
}

/// Reads a snapshot of `file_len` bytes from `input`, refusing more than `max_nodes` nodes.
pub fn read<R: Read>(input: R, file_len: u64, max_nodes: u64) -> Result<Loaded, SnapshotError> {
    if file_len > MAX_FILE_BYTES {
        return Err(SnapshotError::TooLarge("the file is larger than 2 GiB"));
    }
    if file_len < HEADER_BYTES + CHECKSUM_BYTES + MIN_RECORD_BYTES {
        return Err(SnapshotError::Corrupt("the file is too short"));
    }
    let mut src = Source::new(input.take(file_len));
    let mut magic = [0_u8; 8];
    src.fill_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(SnapshotError::BadMagic);
    }
    let mut word = [0_u8; 4];
    src.fill_exact(&mut word)?;
    let version = u32::from_le_bytes(word);
    if version != VERSION {
        return Err(SnapshotError::Version(version));
    }
    src.fill_exact(&mut word)?;
    let flags = u32::from_le_bytes(word);
    if flags & !FLAG_PARTIAL != 0 {
        return Err(SnapshotError::Corrupt("unknown flags"));
    }
    let mut quad = [0_u8; 8];
    src.fill_exact(&mut quad)?;
    let created = i64::from_le_bytes(quad);
    src.fill_exact(&mut quad)?;
    let count = u64::from_le_bytes(quad);
    if count == 0 {
        return Err(SnapshotError::Corrupt("a snapshot has at least the root"));
    }
    if count > max_nodes.min(HARD_MAX_NODES) {
        return Err(SnapshotError::TooLarge("more nodes than the limit allows"));
    }
    // The records need at least this much room: a lying count cannot make us allocate or loop.
    let room = file_len - HEADER_BYTES - CHECKSUM_BYTES;
    if count.saturating_mul(MIN_RECORD_BYTES) > room {
        return Err(SnapshotError::Corrupt("the node count does not fit in the file"));
    }

    let mut tree: Option<Tree> = None;
    let mut name_buf: Vec<u8> = Vec::new();
    for index in 0..count {
        let parent = if index == 0 { None } else { Some(src.varint()?) };
        let tag = src.u8()?;
        if tag & TAG_RESERVED != 0 {
            return Err(SnapshotError::Corrupt("unknown record flags"));
        }
        let kind = match tag & 3 {
            0 => NodeKind::Dir,
            1 => NodeKind::File,
            2 => NodeKind::Link,
            _ => return Err(SnapshotError::Corrupt("unknown node kind")),
        };
        let name_len = src.varint()?;
        if name_len > MAX_NAME_BYTES || name_len > room {
            return Err(SnapshotError::Corrupt("a name is too long"));
        }
        name_buf.clear();
        name_buf.resize(usize::try_from(name_len).map_err(|_| SnapshotError::Corrupt("a name is too long"))?, 0);
        src.fill_exact(&mut name_buf)?;
        let name = std::str::from_utf8(&name_buf).map_err(|_| SnapshotError::Corrupt("a name is not valid UTF-8"))?;
        if name.contains('\0') {
            return Err(SnapshotError::Corrupt("a name contains NUL"));
        }
        let logical_size = src.varint()?;
        let allocated_size = src.varint()?;
        let attributes = u32::try_from(src.varint()?).map_err(|_| SnapshotError::Corrupt("attributes out of range"))?;
        let mtime = if tag & TAG_MTIME != 0 { Some(unzigzag(src.varint()?)) } else { None };
        let data = NodeData {
            logical_size,
            allocated_size,
            mtime,
            attributes,
            error: tag & TAG_ERROR != 0,
            name_lossy: tag & TAG_LOSSY != 0,
        };
        match (&mut tree, parent) {
            (None, None) => {
                if kind != NodeKind::Dir {
                    return Err(SnapshotError::Corrupt("the root is not a directory"));
                }
                let mut root = Tree::new(name);
                let root_id = root.root();
                root.set_node_data(root_id, data).map_err(model_error)?;
                tree = Some(root);
            }
            (Some(tree), Some(parent)) => {
                if parent >= index {
                    return Err(SnapshotError::Corrupt("a parent id does not point at an earlier node"));
                }
                if name.is_empty() || name.contains(['/', '\\']) {
                    return Err(SnapshotError::Corrupt("a node name is empty or contains a path separator"));
                }
                let parent_id = usize::try_from(parent)
                    .ok()
                    .and_then(|p| tree.node_at(p))
                    .ok_or(SnapshotError::Corrupt("a parent id is out of range"))?;
                tree.add_node(parent_id, name, kind, data).map_err(model_error)?;
            }
            _ => return Err(SnapshotError::Corrupt("inconsistent record order")),
        }
    }
    let (computed, stored) = src.finish()?;
    if computed != stored {
        return Err(SnapshotError::Checksum);
    }
    let mut tree = tree.ok_or(SnapshotError::Corrupt("the file has no root"))?;
    tree.aggregate();
    Ok(Loaded { tree, meta: Meta { created, partial: flags & FLAG_PARTIAL != 0, nodes: count } })
}
