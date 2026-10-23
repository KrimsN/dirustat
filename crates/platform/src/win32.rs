//! The only `unsafe` in this crate (ADR 0009): thin Win32 calls for drive enumeration, drive type,
//! free space, a no-replace rename and the shell properties window. Every function takes and returns safe values.
#![allow(unsafe_code)]

use crate::{DriveInfo, MAX_DRIVES, SpaceInfo};
use core::ptr::{null, null_mut};

use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, MoveFileExW,
};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SetThreadErrorMode};
use windows_sys::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOVABLE};
use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};
use windows_sys::Win32::UI::Shell::{
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SEE_MASK_INVOKEIDLIST, SHBrowseForFolderW,
    SHELLEXECUTEINFOW, SHGetPathFromIDListW, ShellExecuteExW,
};
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

/// `GetDriveTypeW` code of the volume root `root` (`X:\`); `None` when `root` holds a NUL.
pub(crate) fn drive_type(root: &str) -> Option<u32> {
    let wide_root = wide(root)?;
    // SAFETY: `wide_root` is a NUL-terminated UTF-16 string that outlives the call.
    Some(unsafe { GetDriveTypeW(wide_root.as_ptr()) })
}

/// Renames `from` to `to` without replacing an existing destination and without a copy
/// fallback (`MoveFileExW` with no flags fails when `to` exists or the volumes differ).
pub(crate) fn move_no_replace(from: &str, to: &str) -> std::io::Result<()> {
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL");
    let (src, dst) = (wide(from).ok_or_else(invalid)?, wide(to).ok_or_else(invalid)?);
    // SAFETY: both arguments are NUL-terminated UTF-16 strings that outlive the call; flags 0 means
    // "rename on one volume, never replace".
    let ok = unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), 0) };
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

/// Capacity of the volume containing `path`; `None` on any failure.
pub(crate) fn free_space(path: &str) -> Option<SpaceInfo> {
    disk_free_ex(&wide(path)?)
}

/// Result of the folder dialog: the chosen path, `None` for Cancel, `Err` when it could not be shown.
pub(crate) fn pick_folder(title: &str) -> Result<Option<String>, &'static str> {
    let title = wide(title).ok_or("title contains NUL")?;
    // SAFETY: initialises COM for this thread (the new-style dialog needs it); the reserved pointer
    // must be null. A negative result (a different apartment model already set) is tolerated and
    // not paired below.
    let com = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) } >= 0;
    // SAFETY: `BROWSEINFOW` is a plain C struct of integers and pointers; all-zero is valid.
    let mut info: BROWSEINFOW = unsafe { core::mem::zeroed() };
    info.lpszTitle = title.as_ptr();
    info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
    // SAFETY: `info` is initialised, `title` outlives the call; the dialog runs its own message loop
    // on this thread and returns an owned item id list, or null on Cancel.
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    let result = if pidl.is_null() {
        Ok(None)
    } else {
        let mut buf = [0u16; 261];
        // SAFETY: `pidl` is the live list returned above; `buf` holds the `MAX_PATH` u16s the API writes.
        let ok = unsafe { SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) } != 0;
        // SAFETY: `pidl` was allocated by the shell for the caller to free, and is not used afterwards.
        unsafe { CoTaskMemFree(pidl.cast_const().cast()) };
        if ok { Ok(Some(from_wide(&buf))) } else { Err("the chosen item is not a file system folder") }
    };
    if com {
        // SAFETY: balances the successful `CoInitializeEx` above on the same thread.
        unsafe { CoUninitialize() };
    }
    result
}

/// Result of the save dialog: the chosen path, `None` for Cancel, `Err` when it could not be shown.
/// The dialog asks before replacing an existing file and never changes the working directory.
pub(crate) fn pick_save_file(title: &str, suggested: &str, extension: &str) -> Result<Option<String>, &'static str> {
    const CAPACITY: usize = 32_768;
    let title = wide(title).ok_or("title contains NUL")?;
    let ext = wide(extension).ok_or("extension contains NUL")?;
    // "<EXT> files (*.ext)\0*.ext\0\0": the filter is a list of NUL-terminated pairs.
    let label = format!("{} files (*.{extension})", extension.to_uppercase());
    let pattern = format!("*.{extension}");
    if label.contains('\0') || pattern.contains('\0') {
        return Err("extension contains NUL");
    }
    let filter: Vec<u16> = label.encode_utf16().chain([0]).chain(pattern.encode_utf16()).chain([0, 0]).collect();
    let mut file = vec![0_u16; CAPACITY];
    for (slot, unit) in file.iter_mut().take(CAPACITY.saturating_sub(1)).zip(suggested.encode_utf16()) {
        *slot = unit;
    }
    // SAFETY: `OPENFILENAMEW` is a plain C struct of integers and pointers; all-zero is valid.
    let mut info: OPENFILENAMEW = unsafe { core::mem::zeroed() };
    info.lStructSize = u32::try_from(core::mem::size_of::<OPENFILENAMEW>()).map_err(|_| "struct size")?;
    info.lpstrFilter = filter.as_ptr();
    info.nFilterIndex = 1;
    info.lpstrFile = file.as_mut_ptr();
    info.nMaxFile = u32::try_from(CAPACITY).map_err(|_| "buffer size")?;
    info.lpstrTitle = title.as_ptr();
    info.lpstrDefExt = ext.as_ptr();
    info.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER;
    // SAFETY: initialises COM for this thread; the reserved pointer must be null. A negative result
    // (a different apartment model already set) is tolerated and not paired below.
    let com = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) } >= 0;
    // SAFETY: `info` is fully initialised; `filter`, `file`, `title` and `ext` are NUL-terminated
    // UTF-16 buffers (`file` writable for `nMaxFile` u16s) that outlive the call. The dialog runs its
    // own message loop on this thread.
    let accepted = unsafe { GetSaveFileNameW(&mut info) } != 0;
    // SAFETY: no preconditions; read right after the call that failed or was cancelled.
    let failure = if accepted { 0 } else { unsafe { CommDlgExtendedError() } };
    if com {
        // SAFETY: balances the successful `CoInitializeEx` above on the same thread.
        unsafe { CoUninitialize() };
    }
    if accepted {
        Ok(Some(from_wide(&file)))
    } else if failure == 0 {
        Ok(None)
    } else {
        Err("the save dialog could not be shown")
    }
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
