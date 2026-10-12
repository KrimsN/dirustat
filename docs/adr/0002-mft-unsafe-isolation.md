# ADR 0002: MFT fast path and `unsafe` isolation

- Status: Accepted · 2026-10-13

## Context

WizTree is fast because it reads the NTFS Master File Table directly instead of walking directories. That needs an order of magnitude fewer syscalls but requires opening the volume (`\\.\C:`) and reading raw data: Win32 FFI, i.e. `unsafe`. The rest of the project forbids `unsafe` (`unsafe_code = "forbid"`).

## Decision

- All `unsafe` lives in one crate, `dirustat-ntfs` (L0). It sets `unsafe_code = "deny"` (allowed locally with `#[allow]` on the FFI module) and `clippy::undocumented_unsafe_blocks = "deny"`; every block carries a `// SAFETY:` comment.
- The boundary is crisp: the FFI module returns a safe description (a handle, block reads into `Vec<u8>`); **MFT record parsing is entirely safe code** with bounds checks, fixups, and allocation and iteration limits.
- The parser must have a fuzz target and a property test "does not panic on arbitrary bytes".
- Administrator rights are requested explicitly; without them the scan falls back transparently to the portable path (`dirustat-scan`). The app never elevates silently.
- `windows-sys` (or `windows`) is pulled in only here and in platform services (recycle bin, file manager).

## Consequences

- A security audit shrinks to one small module.
- On non-Windows the crate compiles as a stub (`MftSupport::Unavailable`).
- Any extension of `unsafe` beyond the FFI module of `dirustat-ntfs` requires a new ADR.
