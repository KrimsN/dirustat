//! The only `unsafe` in the workspace (ADR 0002): thin Win32 calls for a read-only volume handle.
//!
//! Everything here returns safe values (`RawVolumeData`, byte counts, `VolumeError`); callers never see
//! a pointer. Validation of offsets, lengths and returned numbers lives in the safe `volume` module.
#![allow(unsafe_code)]

use core::ffi::c_void;
use core::mem::size_of;
use core::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_PATH_NOT_FOUND,
    ERROR_PRIVILEGE_NOT_HELD, GENERIC_READ, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
};
use windows_sys::Win32::System::IO::{DeviceIoControl, OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};

use crate::volume::{RawVolumeData, VolumeError};

/// Owned read-only volume handle; closed on drop.
#[derive(Debug)]
pub(crate) struct RawHandle(HANDLE);

// SAFETY: a Win32 file handle is a process-wide kernel object id, not tied to a thread. Reads go
// through `ReadFile` with an explicit offset, so no per-handle state is shared between threads.
unsafe impl Send for RawHandle {}

fn map_error(code: u32) -> VolumeError {
    match code {
        ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD => VolumeError::AccessDenied,
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => VolumeError::NotFound,
        other => VolumeError::Io(other),
    }
}

fn last_error() -> VolumeError {
    // SAFETY: `GetLastError` has no preconditions and only reads thread-local state.
    map_error(unsafe { GetLastError() })
}

impl RawHandle {
    /// Opens `path` (a NUL-terminated UTF-16 volume path such as `\\.\C:`) for reading only.
    pub(crate) fn open_read(path: &[u16]) -> Result<Self, VolumeError> {
        if path.last() != Some(&0) {
            return Err(VolumeError::InvalidName);
        }
        // SAFETY: `path` is a valid NUL-terminated UTF-16 string that outlives the call; the security
        // attributes and template handle are allowed to be null for `OPEN_EXISTING`.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(last_error());
        }
        Ok(Self(handle))
    }

    /// Queries `FSCTL_GET_NTFS_VOLUME_DATA`; a non-NTFS volume yields `NotNtfs`.
    pub(crate) fn ntfs_volume_data(&self) -> Result<RawVolumeData, VolumeError> {
        // SAFETY: `NTFS_VOLUME_DATA_BUFFER` is a plain C struct of integers; all-zero is a valid value.
        let mut buf: NTFS_VOLUME_DATA_BUFFER = unsafe { core::mem::zeroed() };
        let mut returned: u32 = 0;
        let out_len = u32::try_from(size_of::<NTFS_VOLUME_DATA_BUFFER>()).unwrap_or(u32::MAX);
        // SAFETY: the handle is open for the lifetime of `self`; the output pointer addresses `buf`,
        // which is `out_len` bytes; the input buffer is null with length 0 and the call is synchronous
        // (null `OVERLAPPED`), so nothing is retained after it returns.
        let ok = unsafe {
            DeviceIoControl(
                self.0,
                FSCTL_GET_NTFS_VOLUME_DATA,
                null(),
                0,
                (&raw mut buf).cast::<c_void>(),
                out_len,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(match last_error() {
                VolumeError::Io(ERROR_INVALID_FUNCTION) => VolumeError::NotNtfs,
                other => other,
            });
        }
        Ok(RawVolumeData {
            volume_serial_number: buf.VolumeSerialNumber,
            number_sectors: buf.NumberSectors,
            total_clusters: buf.TotalClusters,
            bytes_per_sector: buf.BytesPerSector,
            bytes_per_cluster: buf.BytesPerCluster,
            bytes_per_file_record_segment: buf.BytesPerFileRecordSegment,
            mft_valid_data_length: buf.MftValidDataLength,
            mft_start_lcn: buf.MftStartLcn,
        })
    }

    /// Reads at most `buf.len()` bytes at the absolute byte `offset`; returns the count actually read.
    pub(crate) fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, VolumeError> {
        let len = u32::try_from(buf.len()).map_err(|_| VolumeError::InvalidRange)?;
        let mut overlapped = OVERLAPPED {
            Internal: 0,
            InternalHigh: 0,
            Anonymous: OVERLAPPED_0 {
                Anonymous: OVERLAPPED_0_0 {
                    // Splitting a u64 into two u32 halves: the truncation is the intent.
                    Offset: (offset & u64::from(u32::MAX)) as u32,
                    OffsetHigh: (offset >> 32) as u32,
                },
            },
            hEvent: null_mut(),
        };
        let mut read: u32 = 0;
        // SAFETY: the handle is open; `buf` is a valid writable slice of `len` bytes; `overlapped` and
        // `read` live on this stack frame and the handle is synchronous (no `FILE_FLAG_OVERLAPPED`), so
        // the call completes before returning and keeps no pointer to either.
        let ok = unsafe { ReadFile(self.0, buf.as_mut_ptr().cast(), len, &mut read, &mut overlapped) };
        if ok == 0 {
            return Err(last_error());
        }
        Ok(usize::try_from(read).unwrap_or(0).min(buf.len()))
    }
}

impl Drop for RawHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful `CreateFileW`, is owned by `self` and is
        // closed exactly once here. A failed close has no recovery, so the result is ignored.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
