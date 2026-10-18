# ADR 0009: Platform services crate and a second isolated `unsafe` zone

- Status: Accepted · 2026-10-18

## Context

M4 needs OS services the portable code cannot provide: the list of drives with labels and free space, the recycle bin, "show in Explorer" and the shell properties window. The engine must stay free of FFI (ADR 0002 allowed `windows-sys` for "platform services" in principle but only `dirustat-ntfs` may contain `unsafe`). Standard Rust has no API for enumerating drives, free space or the properties verb.

## Decision

- New crate `dirustat-platform` (L1, next to `scan`) with the trait `PlatformServices: Send + Sync + Debug` (`drives`, `free_space`, `move_to_recycle_bin`, `reveal_in_file_manager`, `show_properties`, optional `open_default`) and its own `PlatformError` (`Unsupported`, `NotFound`, `InvalidPath`, `PermissionDenied`, `Failed`).
- `SystemPlatform` is the real implementation. On Windows it uses the `trash` crate for the recycle bin (safe code, MIT OR Apache-2.0), `explorer.exe /select,"<path>"` spawned without a shell for reveal, and Win32 calls for drives, free space and properties. On other OSes every call validates its input and returns `Unsupported`.
- `FakePlatform` (public, in-crate) records calls, serves a configurable drive list and "recycles" into a private temporary directory, so tests of upper layers never touch the real bin or open windows.
- `unsafe` is confined to `crates/platform/src/win32.rs`: crate-level `unsafe_code = "deny"`, `#![allow(unsafe_code)]` only in that module, `undocumented_unsafe_blocks = "deny"`, a `// SAFETY:` comment on every block. The module exposes only safe functions (`drives`, `free_space`, `show_properties`) and uses the already accepted `windows-sys`. All path validation (empty, relative, NUL, over 32767 UTF-16 units, quotes in the explorer argument) is safe code in front of it.
- The `Session` owns an `Arc<dyn PlatformServices>` (default `SystemPlatform`, replaceable with `Session::set_platform`). Commands `platform.reveal`, `platform.properties` and `platform.free_space` are thin wrappers; none is destructive.

## Consequences

- The audit surface for `unsafe` is now two small files: `ntfs/src/win32.rs` and `platform/src/win32.rs`. Any further `unsafe` needs a new ADR.
- Drive enumeration skips network, optical and RAM drives on purpose: a dead network share can block for tens of seconds and `list.drives` must not hang. Revisit with a background refresh if network drives are wanted.
- Trade-off against the `windows` crate or shelling out to PowerShell/`wmic`: a few audited `unsafe` blocks are cheaper and faster than a process spawn or a large dependency, and they keep behaviour deterministic.
- `trash` pulls the `windows` crate (0.62) next to `windows-sys` on Windows; `cargo deny` stays green, duplicate versions are only warnings.
- Deletion commands (another task) must go through `move_to_recycle_bin` and `docs/safety.md`; this crate does not decide whether to delete.
