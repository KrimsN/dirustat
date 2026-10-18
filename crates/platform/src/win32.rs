//! The only `unsafe` in this crate (ADR 0009): thin Win32 calls for drive enumeration, free space
//! and the shell properties window. Every function takes and returns safe values.
#![allow(unsafe_code)]

use crate::{DriveInfo, MAX_DRIVES, SpaceInfo};
use core::ptr::{null, null_mut};

use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SetThreadErrorMode};
use windows_sys::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOVABLE};
use windows_sys::Win32::UI::Shell::{SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

/// NUL-terminated UTF-16 copy of `s`; `None` when `s` contains NUL.
fn wide(s: &str) -> Option<Vec<u16>> {
    if s.contains('\0') {
        return None;
    }
    Some(s.encode_utf16().chain(std::iter::once(0)).collect())
}

/// Text of a UTF-16 buffer up to the first NUL (lossy).
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(buf.get(..end).unwrap_or(&[]))
}

fn disk_free_ex(root: &[u16]) -> Option<SpaceInfo> {
    let (mut avail, mut total, mut free_all) = (0u64, 0u64, 0u64);
    // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call; the three out
    // pointers address live local `u64`s.
    let ok = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free_all) };
    (ok != 0).then(|| SpaceInfo { total, free: avail.min(total) })
}

fn volume_label(root: &[u16]) -> String {
    let mut buf = [0u16; 261];
    let len = u32::try_from(buf.len()).unwrap_or(0);
    // SAFETY: `root` is NUL-terminated; `buf` is writable for `len` u16s; all other out parameters
    // are documented as optional and passed as null with zero sizes.
    let ok = unsafe {
        GetVolumeInformationW(root.as_ptr(), buf.as_mut_ptr(), len, null_mut(), null_mut(), null_mut(), null_mut(), 0)
    };
    if ok == 0 { String::new() } else { from_wide(&buf) }
}

/// Fixed and removable (with media) drives. Network, optical and RAM drives are skipped: a dead
/// network share can block for tens of seconds. "No disk" dialogs are suppressed for this thread.
pub(crate) fn drives() -> Vec<DriveInfo> {
    let mut old_mode = 0u32;
    // SAFETY: `SetThreadErrorMode` only reads its arguments; `old_mode` is a live local.
    let changed = unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut old_mode) } != 0;
    // SAFETY: `GetLogicalDrives` has no preconditions.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for bit in 0..26u8 {
        if mask & (1u32 << bit) == 0 || out.len() >= MAX_DRIVES {
            continue;
        }
        let root_text = format!("{}:\\", char::from(b'A'.saturating_add(bit)));
        let Some(root) = wide(&root_text) else { continue };
        // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
        let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
        if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
            continue;
        }
        let Some(space) = disk_free_ex(&root) else { continue };
        out.push(DriveInfo { label: volume_label(&root), path: root_text, total: space.total, free: space.free });
    }
    if changed {
        // SAFETY: restores the mode saved above; the previous-mode pointer is allowed to be null.
        unsafe { SetThreadErrorMode(old_mode, null_mut()) };
    }
    out
}

/// Capacity of the volume containing `path`; `None` on any failure.
pub(crate) fn free_space(path: &str) -> Option<SpaceInfo> {
    disk_free_ex(&wide(path)?)
}

/// Shows the shell properties window of `path`; `true` when the shell accepted the request.
pub(crate) fn show_properties(path: &str) -> bool {
    let Some(file) = wide(path) else { return false };
    let Some(verb) = wide("properties") else { return false };
    // SAFETY: initialises COM for this thread; the reserved pointer must be null. A negative
    // result (e.g. a different apartment model already set) is tolerated and not paired below.
    let com = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) } >= 0;
    // SAFETY: `SHELLEXECUTEINFOW` is a plain C struct of integers and pointers; all-zero is valid.
    let mut info: SHELLEXECUTEINFOW = unsafe { core::mem::zeroed() };
    info.cbSize = u32::try_from(core::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0);
    info.fMask = SEE_MASK_INVOKEIDLIST;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.nShow = SW_SHOW;
    // SAFETY: `info` is fully initialised with its correct size; `verb` and `file` are
    // NUL-terminated UTF-16 buffers that live until after the call returns.
    let ok = unsafe { ShellExecuteExW(&mut info) } != 0;
    if com {
        // SAFETY: balances the successful `CoInitializeEx` above on the same thread.
        unsafe { CoUninitialize() };
    }
    ok
}
