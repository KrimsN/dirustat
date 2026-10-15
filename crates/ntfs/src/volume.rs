//! Safe volume access: name validation, bounds checks and a read-only view over the Win32 handle.
//!
//! All numbers that come from the OS are validated here before they reach callers (never-crash
//! rules): sector sizes must be sane, reads must be sector-aligned, inside the volume and capped.

use std::fmt;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use crate::win32::RawHandle;

/// Largest single `read_at` request (64 MiB); bigger MFT reads are chunked by the caller.
pub const MAX_READ_BYTES: usize = 64 * 1024 * 1024;

/// Why a volume could not be opened or read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeError {
    /// Not Windows: raw volume access is unavailable.
    Unsupported,
    /// The volume name is not a drive letter such as `C:`.
    InvalidName,
    /// The volume does not exist.
    NotFound,
    /// Administrator rights are required to open the raw volume.
    AccessDenied,
    /// The volume is not NTFS (or the driver does not report NTFS data).
    NotNtfs,
    /// The OS reported geometry that cannot be right (zero or absurd sector size, negative sizes).
    Malformed,
    /// Offset or length is misaligned, empty, too large or outside the volume.
    InvalidRange,
    /// The read returned fewer bytes than requested.
    ShortRead,
    /// Any other Win32 error code.
    Io(u32),
}

impl fmt::Display for VolumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("raw volume access is not supported on this platform"),
            Self::InvalidName => f.write_str("volume name must be a drive letter such as C:"),
            Self::NotFound => f.write_str("volume not found"),
            Self::AccessDenied => f.write_str("administrator rights are required to read the volume"),
            Self::NotNtfs => f.write_str("volume is not NTFS"),
            Self::Malformed => f.write_str("volume reported invalid geometry"),
            Self::InvalidRange => f.write_str("invalid read range"),
            Self::ShortRead => f.write_str("short read from volume"),
            Self::Io(code) => write!(f, "volume I/O error (Win32 code {code})"),
        }
    }
}

impl std::error::Error for VolumeError {}

/// `NTFS_VOLUME_DATA_BUFFER` fields as the OS returned them, before validation.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct RawVolumeData {
    pub volume_serial_number: i64,
    pub number_sectors: i64,
    pub total_clusters: i64,
    pub bytes_per_sector: u32,
    pub bytes_per_cluster: u32,
    pub bytes_per_file_record_segment: u32,
    pub mft_valid_data_length: i64,
    pub mft_start_lcn: i64,
}

/// Validated NTFS geometry of a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NtfsVolumeData {
    pub volume_serial_number: u64,
    pub bytes_per_sector: u32,
    pub bytes_per_cluster: u32,
    /// Size of one MFT file record (usually 1024).
    pub bytes_per_file_record: u32,
    pub total_clusters: u64,
    /// Total volume size in bytes.
    pub total_bytes: u64,
    /// Bytes of the MFT that hold valid records.
    pub mft_valid_data_length: u64,
    /// First cluster of the MFT.
    pub mft_start_lcn: u64,
}

impl NtfsVolumeData {
    /// Byte offset of the MFT on the volume, `None` on overflow.
    pub fn mft_offset(&self) -> Option<u64> {
        self.mft_start_lcn.checked_mul(u64::from(self.bytes_per_cluster))
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn is_pow2_in(value: u32, min: u32, max: u32) -> bool {
    value.is_power_of_two() && (min..=max).contains(&value)
}

/// Converts and validates the OS geometry. Every field is hostile until proven otherwise.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn validate_volume_data(raw: &RawVolumeData) -> Result<NtfsVolumeData, VolumeError> {
    if !is_pow2_in(raw.bytes_per_sector, 512, 65_536)
        || !is_pow2_in(raw.bytes_per_cluster, raw.bytes_per_sector, 2 * 1024 * 1024)
        || !is_pow2_in(raw.bytes_per_file_record_segment, 256, 65_536)
    {
        return Err(VolumeError::Malformed);
    }
    let sectors = u64::try_from(raw.number_sectors).map_err(|_| VolumeError::Malformed)?;
    let total_bytes =
        sectors.checked_mul(u64::from(raw.bytes_per_sector)).filter(|&b| b > 0).ok_or(VolumeError::Malformed)?;
    let total_clusters = u64::try_from(raw.total_clusters).map_err(|_| VolumeError::Malformed)?;
    let mft_start_lcn = u64::try_from(raw.mft_start_lcn).map_err(|_| VolumeError::Malformed)?;
    // The cluster count must fit in the volume and the MFT must start inside it.
    let clusters_bytes = total_clusters.checked_mul(u64::from(raw.bytes_per_cluster));
    if total_clusters == 0 || mft_start_lcn >= total_clusters || clusters_bytes.is_none_or(|b| b > total_bytes) {
        return Err(VolumeError::Malformed);
    }
    Ok(NtfsVolumeData {
        volume_serial_number: raw.volume_serial_number as u64,
        bytes_per_sector: raw.bytes_per_sector,
        bytes_per_cluster: raw.bytes_per_cluster,
        bytes_per_file_record: raw.bytes_per_file_record_segment,
        total_clusters,
        total_bytes,
        mft_valid_data_length: u64::try_from(raw.mft_valid_data_length).map_err(|_| VolumeError::Malformed)?,
        mft_start_lcn,
    })
}

/// Checks that a read of `len` bytes at `offset` is aligned, non-empty, capped and inside the volume.
pub fn check_read_range(data: &NtfsVolumeData, offset: u64, len: usize) -> Result<(), VolumeError> {
    let len64 = u64::try_from(len).map_err(|_| VolumeError::InvalidRange)?;
    let sector = u64::from(data.bytes_per_sector);
    let end = offset.checked_add(len64).ok_or(VolumeError::InvalidRange)?;
    let valid = len > 0
        && len <= MAX_READ_BYTES
        && sector != 0
        && offset.is_multiple_of(sector)
        && len64.is_multiple_of(sector)
        && end <= data.total_bytes;
    if valid { Ok(()) } else { Err(VolumeError::InvalidRange) }
}

/// Turns `C`, `C:`, `C:\` or `\\.\C:` into the NUL-terminated UTF-16 path `\\.\C:`.
pub fn volume_path(name: &str) -> Result<Vec<u16>, VolumeError> {
    let trimmed = name.strip_prefix(r"\\.\").unwrap_or(name);
    let trimmed = trimmed.strip_suffix('\\').unwrap_or(trimmed);
    let letter = match trimmed.as_bytes() {
        [l] | [l, b':'] if l.is_ascii_alphabetic() => l.to_ascii_uppercase(),
        _ => return Err(VolumeError::InvalidName),
    };
    let mut path: Vec<u16> = r"\\.\".encode_utf16().collect();
    path.push(u16::from(letter));
    path.push(u16::from(b':'));
    path.push(0);
    Ok(path)
}

/// Bytes to skip from `addr` to the next multiple of `align` (0 when `align` is 0 or `addr` is aligned).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn align_pad(addr: usize, align: usize) -> usize {
    addr.checked_rem(align).map_or(0, |r| if r == 0 { 0 } else { align - r })
}

/// Whether two geometry reports describe the same volume. `mft_valid_data_length` is left out: it grows
/// while the volume is in use.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn same_volume(a: &NtfsVolumeData, b: &NtfsVolumeData) -> bool {
    a.volume_serial_number == b.volume_serial_number
        && a.bytes_per_sector == b.bytes_per_sector
        && a.bytes_per_cluster == b.bytes_per_cluster
        && a.bytes_per_file_record == b.bytes_per_file_record
        && a.total_clusters == b.total_clusters
        && a.total_bytes == b.total_bytes
        && a.mft_start_lcn == b.mft_start_lcn
}

/// A read-only handle to an NTFS volume with validated geometry.
#[derive(Debug)]
pub struct Volume {
    #[cfg(windows)]
    handle: RawHandle,
    /// A second handle that bypasses the system cache, used for big sequential reads. `None` when it could
    /// not be opened; dropped from use for good as soon as one read through it fails.
    #[cfg(windows)]
    direct: Option<RawHandle>,
    #[cfg(windows)]
    direct_disabled: AtomicBool,
    #[cfg(windows)]
    path: Vec<u16>,
    data: NtfsVolumeData,
}

impl Volume {
    /// Opens the volume read-only and validates its NTFS geometry. Needs administrator rights.
    pub fn open(name: &str) -> Result<Self, VolumeError> {
        let path = volume_path(name)?;
        Self::open_path(&path)
    }

    #[cfg(windows)]
    fn open_path(path: &[u16]) -> Result<Self, VolumeError> {
        let handle = RawHandle::open_read(path, false)?;
        let data = validate_volume_data(&handle.ntfs_volume_data()?)?;
        let direct = RawHandle::open_read(path, true).ok();
        Ok(Self { handle, direct, direct_disabled: AtomicBool::new(false), path: path.to_vec(), data })
    }

    /// Opens the same volume again, with its own handles, so another thread can read in parallel. Fails when
    /// the volume no longer looks the same.
    #[cfg(windows)]
    pub fn reopen(&self) -> Result<Self, VolumeError> {
        let other = Self::open_path(&self.path)?;
        if same_volume(&self.data, &other.data) { Ok(other) } else { Err(VolumeError::Malformed) }
    }

    /// Not available off Windows.
    #[cfg(not(windows))]
    pub fn reopen(&self) -> Result<Self, VolumeError> {
        Err(VolumeError::Unsupported)
    }

    #[cfg(not(windows))]
    fn open_path(_path: &[u16]) -> Result<Self, VolumeError> {
        Err(VolumeError::Unsupported)
    }

    /// Validated NTFS geometry.
    pub fn data(&self) -> &NtfsVolumeData {
        &self.data
    }

    /// Reads `len` bytes at the byte `offset`; both must be sector-aligned and inside the volume.
    pub fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError> {
        check_read_range(&self.data, offset, len)?;
        #[cfg(windows)]
        if let Some(buf) = self.read_direct(offset, len) {
            return Ok(buf);
        }
        let mut buf = vec![0u8; len];
        self.fill(offset, &mut buf)?;
        Ok(buf)
    }

    /// Reads through the cache-bypassing handle into a sector-aligned window of a larger buffer. `None` when
    /// that handle is unavailable or failed (it is then switched off and the caller reads the usual way, so a
    /// volume or driver that rejects unbuffered I/O costs nothing but the first attempt).
    #[cfg(windows)]
    fn read_direct(&self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let handle = self.direct.as_ref().filter(|_| !self.direct_disabled.load(Ordering::Relaxed))?;
        let align = usize::try_from(self.data.bytes_per_sector).ok()?;
        let mut buf = vec![0u8; len.checked_add(align)?];
        let pad = align_pad(buf.as_ptr().addr(), align);
        let window = buf.get_mut(pad..pad.checked_add(len)?)?;
        match handle.read_at(offset, window) {
            Ok(read) if read == len => {}
            _ => {
                self.direct_disabled.store(true, Ordering::Relaxed);
                return None;
            }
        }
        buf.drain(..pad);
        buf.truncate(len);
        Some(buf)
    }

    #[cfg(windows)]
    fn fill(&self, offset: u64, buf: &mut [u8]) -> Result<(), VolumeError> {
        let read = self.handle.read_at(offset, buf)?;
        if read == buf.len() { Ok(()) } else { Err(VolumeError::ShortRead) }
    }

    #[cfg(not(windows))]
    fn fill(&self, _offset: u64, _buf: &mut [u8]) -> Result<(), VolumeError> {
        Err(VolumeError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> RawVolumeData {
        RawVolumeData {
            volume_serial_number: -5,
            number_sectors: 2048,
            total_clusters: 256,
            bytes_per_sector: 512,
            bytes_per_cluster: 4096,
            bytes_per_file_record_segment: 1024,
            mft_valid_data_length: 65_536,
            mft_start_lcn: 4,
        }
    }

    fn data() -> NtfsVolumeData {
        validate_volume_data(&raw()).unwrap_or_else(|_| unreachable_data())
    }

    #[allow(clippy::panic)]
    fn unreachable_data() -> NtfsVolumeData {
        panic!("fixture geometry must validate")
    }

    #[test]
    fn volume_path_accepts_drive_letter_forms() {
        let expected: Vec<u16> = "\\\\.\\C:\0".encode_utf16().collect();
        for name in ["C", "c", "C:", "c:", "C:\\", r"\\.\C:", r"\\.\c:\"] {
            assert_eq!(volume_path(name), Ok(expected.clone()), "{name}");
        }
    }

    #[test]
    fn volume_path_rejects_everything_else() {
        let hostile = [
            "",
            ":",
            "1:",
            "CD:",
            "C:\\Windows",
            r"\\server\share",
            r"\\.\PhysicalDrive0",
            "C:\0",
            "é:",
            "C::",
            r"\\?\C:",
            "C:/",
            " C:",
        ];
        for name in hostile {
            assert_eq!(volume_path(name), Err(VolumeError::InvalidName), "{name:?}");
        }
    }

    #[test]
    fn geometry_validation_accepts_sane_values() {
        let d = data();
        assert_eq!(d.total_bytes, 2048 * 512);
        assert_eq!(d.mft_offset(), Some(4 * 4096));
        assert_eq!(d.volume_serial_number, (-5i64) as u64);
    }

    #[test]
    fn geometry_validation_rejects_hostile_values() {
        let cases: [fn(&mut RawVolumeData); 13] = [
            |r| r.bytes_per_sector = 0,
            |r| r.bytes_per_sector = 513,
            |r| r.bytes_per_sector = 1 << 20,
            |r| r.bytes_per_cluster = 256,
            |r| r.bytes_per_file_record_segment = 0,
            |r| r.bytes_per_file_record_segment = 3000,
            |r| r.number_sectors = -1,
            |r| r.number_sectors = 0,
            |r| r.mft_start_lcn = -1,
            |r| r.mft_start_lcn = i64::MAX,
            |r| r.mft_start_lcn = 256,
            |r| r.total_clusters = 0,
            |r| r.total_clusters = 257,
        ];
        for mutate in cases {
            let mut r = raw();
            mutate(&mut r);
            assert_eq!(validate_volume_data(&r), Err(VolumeError::Malformed));
        }
    }

    #[test]
    fn geometry_validation_survives_extremes() {
        let mut r = raw();
        r.number_sectors = i64::MAX;
        r.bytes_per_sector = 65_536;
        r.bytes_per_cluster = 65_536;
        assert_eq!(validate_volume_data(&r), Err(VolumeError::Malformed));
        let mut r = raw();
        r.mft_start_lcn = i64::MAX;
        // An MFT start outside the volume is rejected.
        assert_eq!(validate_volume_data(&r), Err(VolumeError::Malformed));
    }

    #[test]
    fn read_range_checks() {
        let d = data();
        assert_eq!(check_read_range(&d, 0, 512), Ok(()));
        assert_eq!(check_read_range(&d, d.total_bytes - 512, 512), Ok(()));
        let bad: [(u64, usize); 8] = [
            (0, 0),
            (1, 512),
            (0, 513),
            (d.total_bytes, 512),
            (d.total_bytes - 512, 1024),
            (u64::MAX - 511, 512),
            (u64::MAX, usize::MAX),
            (0, MAX_READ_BYTES + 512),
        ];
        for (offset, len) in bad {
            assert_eq!(check_read_range(&d, offset, len), Err(VolumeError::InvalidRange), "{offset} {len}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn open_is_unsupported_off_windows() {
        assert_eq!(Volume::open("C:").err(), Some(VolumeError::Unsupported));
    }

    #[cfg(windows)]
    #[test]
    fn open_never_panics_and_reports_a_reason() {
        // Without administrator rights this is `AccessDenied`; with them it is Ok or `NotNtfs`.
        match Volume::open("C:") {
            Ok(v) => assert!(v.data().bytes_per_sector >= 512),
            Err(e) => {
                assert!(matches!(e, VolumeError::AccessDenied | VolumeError::NotNtfs | VolumeError::NotFound), "{e}")
            }
        }
    }

    /// Needs administrator rights and a real NTFS `C:`; run with `cargo test -p dirustat-ntfs -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs administrator rights"]
    fn reads_the_ntfs_boot_sector() -> Result<(), VolumeError> {
        let volume = Volume::open("C:")?;
        let sector = usize::try_from(volume.data().bytes_per_sector).map_err(|_| VolumeError::Malformed)?;
        let boot = volume.read_at(0, sector)?;
        assert_eq!(boot.get(3..11), Some(&b"NTFS    "[..]));
        Ok(())
    }
}

#[cfg(test)]
mod align_tests {
    use super::*;

    #[test]
    fn align_pad_reaches_the_next_boundary() {
        assert_eq!(align_pad(0, 512), 0);
        assert_eq!(align_pad(512, 512), 0);
        assert_eq!(align_pad(1, 512), 511);
        assert_eq!(align_pad(513, 512), 511);
        assert_eq!(align_pad(1000, 0), 0);
        for addr in 0..2000usize {
            assert_eq!((addr + align_pad(addr, 64)) % 64, 0);
        }
    }

    #[test]
    fn same_volume_ignores_the_growing_mft_length() {
        let a = NtfsVolumeData {
            volume_serial_number: 1,
            bytes_per_sector: 512,
            bytes_per_cluster: 4096,
            bytes_per_file_record: 1024,
            total_clusters: 10,
            total_bytes: 40_960,
            mft_valid_data_length: 4096,
            mft_start_lcn: 2,
        };
        assert!(same_volume(&a, &NtfsVolumeData { mft_valid_data_length: 8192, ..a }));
        assert!(!same_volume(&a, &NtfsVolumeData { volume_serial_number: 2, ..a }));
        assert!(!same_volume(&a, &NtfsVolumeData { total_bytes: 81_920, ..a }));
    }
}
