//! MFT file record parsing: update sequence fixups, the attribute walk and the attributes the scanner needs.
//!
//! Layout references are the public NTFS on-disk format documentation. All offsets and lengths are hostile:
//! every read is bounds-checked, every loop is bounded by `ParseLimits` and always makes progress.

use crate::bytes::{le_u16, le_u32, le_u64, slice_at, utf16_units};
use crate::error::{ParseError, ParseLimits};
use crate::runs::{Extent, Run, decode_data_runs, extents_from_runs};

/// Attribute type codes.
pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
pub const ATTR_REPARSE_POINT: u32 = 0xC0;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Record header flag: the record is in use.
const FLAG_IN_USE: u16 = 0x0001;
/// Record header flag: the record is a directory.
const FLAG_DIRECTORY: u16 = 0x0002;

/// Attribute flags.
const ATTR_FLAG_COMPRESSED: u16 = 0x0001;
const ATTR_FLAG_ENCRYPTED: u16 = 0x4000;
const ATTR_FLAG_SPARSE: u16 = 0x8000;

/// Smallest record header (up to and including the base record reference).
const MIN_HEADER: usize = 42;
/// Header size at which the 32-bit record number is present.
const HEADER_WITH_NUMBER: usize = 48;
/// Bytes covered by one update sequence entry in MFT records, whatever the device sector size is.
pub const FIXUP_STRIDE: usize = 512;
/// Common attribute header (resident form).
const RESIDENT_HEADER: usize = 24;
/// Non-resident attribute header without the compressed size field.
const NONRESIDENT_HEADER: usize = 64;
/// Largest plausible file size (the NTFS limit is 2^64 but signed APIs top out at i64::MAX).
const MAX_SIZE: u64 = i64::MAX as u64;
/// Fixed part of a `$FILE_NAME` value.
const FILE_NAME_FIXED: usize = 66;
/// Fixed part of an `$ATTRIBUTE_LIST` entry.
const LIST_ENTRY_FIXED: usize = 26;

/// A file reference: 48-bit MFT record number plus 16-bit sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileRef {
    pub record: u64,
    pub sequence: u16,
}

impl FileRef {
    /// Splits the raw 64-bit on-disk reference.
    pub fn from_raw(raw: u64) -> Self {
        Self { record: raw & 0x0000_FFFF_FFFF_FFFF, sequence: u16::try_from(raw >> 48).unwrap_or(0) }
    }

    /// The raw 64-bit form.
    pub fn to_raw(self) -> u64 {
        (u64::from(self.sequence) << 48) | (self.record & 0x0000_FFFF_FFFF_FFFF)
    }
}

/// `$FILE_NAME` namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Posix,
    Win32,
    Dos,
    Win32AndDos,
}

impl Namespace {
    fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Posix),
            1 => Some(Self::Win32),
            2 => Some(Self::Dos),
            3 => Some(Self::Win32AndDos),
            _ => None,
        }
    }

    /// True for the 8.3-only namespace, which duplicates a long name.
    pub fn is_dos_only(self) -> bool {
        self == Self::Dos
    }

    /// Lower is better: Win32 names (long or shared) first, then POSIX, DOS last.
    fn rank(self) -> u8 {
        match self {
            Self::Win32AndDos => 0,
            Self::Win32 => 1,
            Self::Posix => 2,
            Self::Dos => 3,
        }
    }
}

/// `$STANDARD_INFORMATION`: timestamps (raw FILETIME, 100 ns since 1601) and DOS attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardInfo {
    pub created: u64,
    pub modified: u64,
    pub mft_modified: u64,
    pub accessed: u64,
    /// `FILE_ATTRIBUTE_*` bits (0x10 directory, 0x400 reparse point, ...).
    pub attributes: u32,
}

/// One `$FILE_NAME` attribute (a hard link name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileName {
    pub parent: FileRef,
    pub namespace: Namespace,
    /// Raw UTF-16 code units, possibly with unpaired surrogates; convert with `model::name_from_wide`.
    pub name: Vec<u16>,
    /// `FILE_ATTRIBUTE_*` bits copied into the name (informational, may be stale).
    pub flags: u32,
    /// Real size copied into the name (may be stale; the `$DATA` attribute is authoritative).
    pub real_size: u64,
    pub allocated_size: u64,
}

/// The unnamed `$DATA` attribute: the file contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataStream {
    /// Contents live inside the MFT record.
    pub resident: bool,
    /// Logical size in bytes (resident value length, or the non-resident real size).
    pub size: u64,
    /// Allocated size in bytes as recorded; 0 for resident data. For compressed or sparse streams this is the
    /// uncompressed allocation; see `compressed_size`.
    pub allocated: u64,
    /// Valid data length (bytes past it read as zero); equals `size` for resident data.
    pub initialized: u64,
    /// Bytes actually stored for compressed or sparse streams (the on-disk size), when recorded.
    pub compressed_size: Option<u64>,
    pub compressed: bool,
    pub sparse: bool,
    pub encrypted: bool,
    /// First virtual cluster this attribute piece describes (non-zero in extension records).
    pub start_vcn: u64,
    /// Decoded runs (empty for resident data or when `ParseLimits::decode_runs` is off).
    pub runs: Vec<Run>,
}

/// An alternate (named) data stream. Only the unnamed stream counts toward the file size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedStream {
    pub name: Vec<u16>,
    pub size: u64,
    pub resident: bool,
}

/// One `$ATTRIBUTE_LIST` entry: where an attribute of a fragmented file really lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributeListEntry {
    pub attribute_type: u32,
    pub start_vcn: u64,
    /// The MFT record that holds the attribute (may be an extension record).
    pub segment: FileRef,
    /// The attribute has a name (an alternate stream or index name).
    pub named: bool,
}

/// Presence and, when resident, the entries of the `$ATTRIBUTE_LIST`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeList {
    /// The list is stored outside the record, so `entries` is empty (extension records exist regardless).
    pub nonresident: bool,
    pub entries: Vec<AttributeListEntry>,
}

/// A parsed MFT file record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MftRecord {
    /// Header sequence number (incremented when the record is reused).
    pub sequence: u16,
    pub hard_link_count: u16,
    pub in_use: bool,
    pub is_directory: bool,
    /// Base record for extension records; `None` for base records.
    pub base: Option<FileRef>,
    /// Record number from the header when present (NTFS 3.1+).
    pub record_number: Option<u32>,
    pub standard_info: Option<StandardInfo>,
    /// All `$FILE_NAME` attributes (long, short and hard-link names).
    pub names: Vec<FileName>,
    /// The unnamed `$DATA` attribute, if this record holds one.
    pub data: Option<DataStream>,
    /// Alternate data streams recorded in detail (capped by `ParseLimits::max_named_streams`).
    pub named_streams: Vec<NamedStream>,
    /// Number of alternate data streams in this record, including those beyond the cap.
    pub named_stream_count: usize,
    /// `$ATTRIBUTE_LIST` is present: further attributes may live in extension records.
    pub attribute_list: Option<AttributeList>,
    /// Reparse tag from a resident `$REPARSE_POINT` (0xA000000C symlink, 0xA0000003 mount point/junction).
    pub reparse_tag: Option<u32>,
}

impl MftRecord {
    /// True when this is a base record (not an extension record of another file).
    pub fn is_base(&self) -> bool {
        self.base.is_none()
    }

    /// True when extension records may hold more attributes of this file.
    pub fn has_attribute_list(&self) -> bool {
        self.attribute_list.is_some()
    }

    /// The best single name: Win32/Win32AndDos over POSIX over DOS-only; first wins on ties.
    pub fn best_name(&self) -> Option<&FileName> {
        self.names.iter().min_by_key(|n| n.namespace.rank())
    }

    /// One name per hard link: every name except DOS-only duplicates whose parent also has a non-DOS name.
    pub fn link_names(&self) -> Vec<&FileName> {
        self.names
            .iter()
            .filter(|n| {
                !n.namespace.is_dos_only()
                    || !self.names.iter().any(|o| !o.namespace.is_dos_only() && o.parent.record == n.parent.record)
            })
            .collect()
    }

    /// Logical size of the file contents (unnamed stream), 0 when there is none in this record.
    pub fn logical_size(&self) -> u64 {
        self.data.as_ref().map_or(0, |d| d.size)
    }
}

fn need<T>(value: Option<T>) -> Result<T, ParseError> {
    value.ok_or(ParseError::Truncated)
}

/// Validates and removes the update sequence array of one record, in place.
///
/// Each `FIXUP_STRIDE`-sized (or `bytes_per_sector`-sized) block of a record ends with a copy of the update
/// sequence number; the original bytes are stored in the array. Every trailer must match, otherwise the
/// record was torn and `BadFixup` is returned with `record` unchanged. `record.len()` must be exactly the
/// number of covered blocks times `bytes_per_sector`.
pub fn apply_fixups(record: &mut [u8], bytes_per_sector: usize) -> Result<(), ParseError> {
    if bytes_per_sector < 4 || !bytes_per_sector.is_power_of_two() {
        return Err(ParseError::BadGeometry);
    }
    let usa_offset = usize::from(need(le_u16(record, 4))?);
    let usa_count = usize::from(need(le_u16(record, 6))?);
    let sectors = usa_count.checked_sub(1).filter(|&s| s > 0).ok_or(ParseError::BadFixup)?;
    let covered = sectors.checked_mul(bytes_per_sector).ok_or(ParseError::BadFixup)?;
    if covered != record.len() || usa_offset < 8 {
        return Err(ParseError::BadFixup);
    }
    let usa_len = usa_count.checked_mul(2).ok_or(ParseError::BadFixup)?;
    let usa = slice_at(record, usa_offset, usa_len).ok_or(ParseError::BadFixup)?.to_vec();
    let usn = usa.get(..2).ok_or(ParseError::BadFixup)?;
    // The array itself must not sit in a block trailer, or restoring would overwrite it.
    let usa_end = usa_offset.checked_add(usa_len).ok_or(ParseError::BadFixup)?;
    for k in 1..=sectors {
        let trailer = k.checked_mul(bytes_per_sector).and_then(|e| e.checked_sub(2)).ok_or(ParseError::BadFixup)?;
        if trailer >= usa_offset && trailer < usa_end {
            return Err(ParseError::BadFixup);
        }
        if slice_at(record, trailer, 2) != Some(usn) {
            return Err(ParseError::BadFixup);
        }
    }
    for (k, original) in usa.chunks_exact(2).enumerate().skip(1) {
        let trailer = k.checked_mul(bytes_per_sector).and_then(|e| e.checked_sub(2)).ok_or(ParseError::BadFixup)?;
        let dst =
            record.get_mut(trailer..trailer.checked_add(2).ok_or(ParseError::BadFixup)?).ok_or(ParseError::BadFixup)?;
        dst.copy_from_slice(original);
    }
    Ok(())
}

/// Applies the fixups (with `FIXUP_STRIDE`) and parses the record.
pub fn parse_raw_record(record: &mut [u8], limits: &ParseLimits) -> Result<MftRecord, ParseError> {
    if record.get(..4) != Some(b"FILE".as_slice()) {
        return Err(ParseError::BadMagic);
    }
    apply_fixups(record, FIXUP_STRIDE)?;
    parse_record(record, limits)
}

/// Parses a record whose fixups were already applied.
pub fn parse_record(bytes: &[u8], limits: &ParseLimits) -> Result<MftRecord, ParseError> {
    if bytes.get(..4) != Some(b"FILE".as_slice()) {
        return Err(ParseError::BadMagic);
    }
    if bytes.len() < MIN_HEADER {
        return Err(ParseError::Truncated);
    }
    let usa_offset = usize::from(need(le_u16(bytes, 4))?);
    let sequence = need(le_u16(bytes, 16))?;
    let hard_link_count = need(le_u16(bytes, 18))?;
    let first_attr = usize::from(need(le_u16(bytes, 20))?);
    let flags = need(le_u16(bytes, 22))?;
    let used = usize::try_from(need(le_u32(bytes, 24))?).map_err(|_| ParseError::BadHeader)?;
    let base_raw = need(le_u64(bytes, 32))?;
    let record_number = if usa_offset >= HEADER_WITH_NUMBER { le_u32(bytes, 44) } else { None };
    if used > bytes.len() || first_attr < MIN_HEADER || first_attr > used {
        return Err(ParseError::BadHeader);
    }

    let mut record = MftRecord {
        sequence,
        hard_link_count,
        in_use: flags & FLAG_IN_USE != 0,
        is_directory: flags & FLAG_DIRECTORY != 0,
        base: (base_raw != 0).then(|| FileRef::from_raw(base_raw)),
        record_number,
        standard_info: None,
        names: Vec::new(),
        data: None,
        named_streams: Vec::new(),
        named_stream_count: 0,
        attribute_list: None,
        reparse_tag: None,
    };

    let mut pos = first_attr;
    let mut seen = 0usize;
    while pos < used {
        let ty = need(le_u32(bytes, pos))?;
        if ty == ATTR_END {
            break;
        }
        seen += 1;
        if seen > limits.max_attributes {
            return Err(ParseError::TooManyAttributes);
        }
        let len = usize::try_from(need(le_u32(bytes, pos.saturating_add(4)))?).map_err(|_| ParseError::BadAttribute)?;
        // Minimum length guarantees progress; the attribute must fit inside the used part of the record.
        let end = pos.checked_add(len).filter(|&e| len >= 16 && e <= used).ok_or(ParseError::BadAttribute)?;
        let attr = bytes.get(pos..end).ok_or(ParseError::BadAttribute)?;
        parse_attribute(ty, attr, limits, &mut record)?;
        pos = end;
    }
    Ok(record)
}

/// A view of one attribute header.
struct Attr<'a> {
    bytes: &'a [u8],
    nonresident: bool,
    name_len: usize,
    name_offset: usize,
    flags: u16,
}

impl Attr<'_> {
    fn is_named(&self) -> bool {
        self.name_len > 0
    }

    fn name(&self) -> Result<Vec<u16>, ParseError> {
        utf16_units(self.bytes, self.name_offset, self.name_len).ok_or(ParseError::BadAttribute)
    }

    /// The value of a resident attribute.
    fn resident_value(&self) -> Result<&[u8], ParseError> {
        let len = usize::try_from(le_u32(self.bytes, 16).ok_or(ParseError::BadAttribute)?)
            .map_err(|_| ParseError::BadAttribute)?;
        let offset = usize::from(le_u16(self.bytes, 20).ok_or(ParseError::BadAttribute)?);
        if offset < RESIDENT_HEADER {
            return Err(ParseError::BadAttribute);
        }
        slice_at(self.bytes, offset, len).ok_or(ParseError::BadAttribute)
    }
}

fn parse_attribute(ty: u32, bytes: &[u8], limits: &ParseLimits, record: &mut MftRecord) -> Result<(), ParseError> {
    let resident_header_ok = bytes.len() >= RESIDENT_HEADER;
    let nonresident = bytes.get(8).copied().ok_or(ParseError::BadAttribute)? != 0;
    if !resident_header_ok || (nonresident && bytes.len() < NONRESIDENT_HEADER) {
        return Err(ParseError::BadAttribute);
    }
    let attr = Attr {
        bytes,
        nonresident,
        name_len: usize::from(bytes.get(9).copied().ok_or(ParseError::BadAttribute)?),
        name_offset: usize::from(le_u16(bytes, 10).ok_or(ParseError::BadAttribute)?),
        flags: le_u16(bytes, 12).ok_or(ParseError::BadAttribute)?,
    };
    match ty {
        ATTR_STANDARD_INFORMATION => {
            if record.standard_info.is_none() && !attr.nonresident {
                record.standard_info = parse_standard_info(attr.resident_value()?);
            }
        }
        ATTR_FILE_NAME if !attr.nonresident => {
            record.names.push(parse_file_name(attr.resident_value()?)?);
        }
        ATTR_ATTRIBUTE_LIST => {
            if record.attribute_list.is_none() {
                record.attribute_list = Some(if attr.nonresident {
                    AttributeList { nonresident: true, entries: Vec::new() }
                } else {
                    AttributeList { nonresident: false, entries: parse_attribute_list(attr.resident_value()?, limits)? }
                });
            }
        }
        ATTR_DATA => parse_data(&attr, limits, record)?,
        ATTR_REPARSE_POINT if !attr.nonresident && record.reparse_tag.is_none() => {
            record.reparse_tag = le_u32(attr.resident_value()?, 0);
        }
        _ => {}
    }
    Ok(())
}

fn parse_standard_info(value: &[u8]) -> Option<StandardInfo> {
    Some(StandardInfo {
        created: le_u64(value, 0)?,
        modified: le_u64(value, 8)?,
        mft_modified: le_u64(value, 16)?,
        accessed: le_u64(value, 24)?,
        attributes: le_u32(value, 32)?,
    })
}

fn parse_file_name(value: &[u8]) -> Result<FileName, ParseError> {
    if value.len() < FILE_NAME_FIXED {
        return Err(ParseError::BadAttribute);
    }
    let name_units = usize::from(value.get(64).copied().ok_or(ParseError::BadAttribute)?);
    let namespace =
        Namespace::from_raw(value.get(65).copied().ok_or(ParseError::BadAttribute)?).ok_or(ParseError::BadNamespace)?;
    Ok(FileName {
        parent: FileRef::from_raw(le_u64(value, 0).ok_or(ParseError::BadAttribute)?),
        namespace,
        name: utf16_units(value, FILE_NAME_FIXED, name_units).ok_or(ParseError::BadAttribute)?,
        flags: le_u32(value, 56).ok_or(ParseError::BadAttribute)?,
        real_size: le_u64(value, 48).ok_or(ParseError::BadAttribute)?,
        allocated_size: le_u64(value, 40).ok_or(ParseError::BadAttribute)?,
    })
}

fn parse_attribute_list(value: &[u8], limits: &ParseLimits) -> Result<Vec<AttributeListEntry>, ParseError> {
    let mut entries = Vec::new();
    let mut pos = 0usize;
    while pos < value.len() {
        if entries.len() >= limits.max_list_entries {
            return Err(ParseError::BadAttribute);
        }
        let rec_len = usize::from(le_u16(value, pos.saturating_add(4)).ok_or(ParseError::BadAttribute)?);
        let end = pos.checked_add(rec_len).filter(|&e| rec_len >= LIST_ENTRY_FIXED && e <= value.len());
        let end = end.ok_or(ParseError::BadAttribute)?;
        entries.push(AttributeListEntry {
            attribute_type: le_u32(value, pos).ok_or(ParseError::BadAttribute)?,
            start_vcn: le_u64(value, pos.saturating_add(8)).ok_or(ParseError::BadAttribute)?,
            segment: FileRef::from_raw(le_u64(value, pos.saturating_add(16)).ok_or(ParseError::BadAttribute)?),
            named: value.get(pos.saturating_add(6)).copied().ok_or(ParseError::BadAttribute)? != 0,
        });
        pos = end;
    }
    Ok(entries)
}

fn checked_size(raw: u64) -> Result<u64, ParseError> {
    if raw > MAX_SIZE { Err(ParseError::SizeOverflow) } else { Ok(raw) }
}

fn parse_data(attr: &Attr<'_>, limits: &ParseLimits, record: &mut MftRecord) -> Result<(), ParseError> {
    let bytes = attr.bytes;
    let (stream, named_size) = if attr.nonresident {
        let start_vcn = le_u64(bytes, 16).ok_or(ParseError::BadAttribute)?;
        let runs_offset = usize::from(le_u16(bytes, 32).ok_or(ParseError::BadAttribute)?);
        let compression_unit = le_u16(bytes, 34).ok_or(ParseError::BadAttribute)?;
        let allocated = checked_size(le_u64(bytes, 40).ok_or(ParseError::BadAttribute)?)?;
        let size = checked_size(le_u64(bytes, 48).ok_or(ParseError::BadAttribute)?)?;
        let initialized = checked_size(le_u64(bytes, 56).ok_or(ParseError::BadAttribute)?)?;
        let compressed_size =
            if compression_unit != 0 { le_u64(bytes, 64).map(checked_size).transpose()? } else { None };
        let runs = if limits.decode_runs && !attr.is_named() {
            if runs_offset < NONRESIDENT_HEADER {
                return Err(ParseError::BadAttribute);
            }
            let run_bytes = bytes.get(runs_offset..).ok_or(ParseError::BadAttribute)?;
            decode_data_runs(run_bytes, limits)?
        } else {
            Vec::new()
        };
        (
            DataStream {
                resident: false,
                size,
                allocated,
                initialized,
                compressed_size,
                compressed: attr.flags & ATTR_FLAG_COMPRESSED != 0,
                sparse: attr.flags & ATTR_FLAG_SPARSE != 0,
                encrypted: attr.flags & ATTR_FLAG_ENCRYPTED != 0,
                start_vcn,
                runs,
            },
            size,
        )
    } else {
        let size = u64::try_from(attr.resident_value()?.len()).map_err(|_| ParseError::SizeOverflow)?;
        (
            DataStream {
                resident: true,
                size,
                allocated: 0,
                initialized: size,
                compressed_size: None,
                compressed: false,
                sparse: false,
                encrypted: attr.flags & ATTR_FLAG_ENCRYPTED != 0,
                start_vcn: 0,
                runs: Vec::new(),
            },
            size,
        )
    };

    if attr.is_named() {
        record.named_stream_count = record.named_stream_count.saturating_add(1);
        if record.named_streams.len() < limits.max_named_streams {
            record.named_streams.push(NamedStream {
                name: attr.name()?,
                size: named_size,
                resident: !attr.nonresident,
            });
        }
    } else if record.data.is_none() && stream.start_vcn == 0 {
        // A piece that starts past VCN 0 is a continuation of a fragmented attribute: it says nothing
        // about the file size, so it must not stand in for the whole stream.
        record.data = Some(stream);
    }
    Ok(())
}

/// The extents of the whole `$MFT`, from its own record (record 0).
///
/// Fails with `Incomplete` when the runs of the unnamed `$DATA` do not cover its size, which happens when
/// the `$MFT` is so fragmented that part of the run list sits in extension records (via `$ATTRIBUTE_LIST`);
/// the caller should then fall back to the portable scan.
pub fn mft_extents(record0: &MftRecord, bytes_per_cluster: u32) -> Result<Vec<Extent>, ParseError> {
    let data = record0.data.as_ref().ok_or(ParseError::Incomplete)?;
    if data.resident || data.start_vcn != 0 || bytes_per_cluster == 0 {
        return Err(ParseError::Incomplete);
    }
    let extents = extents_from_runs(&data.runs)?;
    let mut covered = 0u64;
    for e in &extents {
        covered = covered.saturating_add(e.byte_len(bytes_per_cluster).ok_or(ParseError::RunOverflow)?);
    }
    if extents.is_empty() || covered < data.size {
        return Err(ParseError::Incomplete);
    }
    Ok(extents)
}

#[cfg(test)]
mod tests;
