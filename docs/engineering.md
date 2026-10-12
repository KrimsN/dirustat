# Engineering conventions

> Status: 2026-10-13 · revision 2.

## Errors

- Every crate has its own error type (`enum`, `Display` + `std::error::Error`), no `anyhow` in library crates. Applications (`apps/*`) may use a top-level `Result<(), Box<dyn Error>>`.
- An error carries human-readable context and a machine-readable variant that code branches on.
- `CommandError` is the only type command clients (UI/CLI/control/MCP) see; internal errors convert to it at the engine boundary.

## Concurrency

- Heavy work (scan, layout of big trees, export) runs on background threads; the UI thread never blocks. Rayon for data parallelism, channels (`std::sync::mpsc`/`crossbeam`) for progress.
- Shared state is an `Arc<Snapshot>` with immutable contents; publishing a new revision is atomic. Hold mutexes briefly and never call foreign code under a lock.
- Cancellation is cooperative (`AtomicBool`/token), checked in walk loops at least once per directory.
- No blocking filesystem calls on the UI thread.

## Code performance

- Hot structures are compact: `NodeId` is `u32`, names are `Box<str>`, no extra `String` per node. Measure (`docs/performance.md`) before complicating.
- Do not allocate in the layout/render loop: reuse buffers.
- Cache by revision instead of recomputing per frame.

## Comments and in-code docs

- Comments are English and explain the reason and invariants; they do not restate the code. Leave no `TODO` without a task id from `docs/gaps.md` (`// TODO(G-014): ...`).
- Public items get `///` describing behaviour at the edges (empty input, overflow).
- A module starts with `//!`: layer, responsibility and, for a stub, the milestone.

## Logging

`tracing` (from M1). Levels: `error` is a lost user action; `warn` is a degradation (falling back from MFT to the portable scan); `info` marks session milestones; `debug`/`trace` is detail, and user paths appear only there.

## Dependencies

Every dependency is justified in the table below (add a row in the same change). Prefer pure Rust and small dependency trees. Checked by `cargo deny check`.

| Dependency | Consumer crate | Why | Licence | Status |
|---|---|---|---|---|
| `rayon` 1 | scan | work-stealing parallel directory walk (M1.4) | MIT OR Apache-2.0 | accepted 2026-10-13 |
| `serde` 1 (derive) | model, engine, control | serializable state and control protocol | MIT OR Apache-2.0 | accepted 2026-10-13 |
| `serde_json` 1 | engine, control | JSON control channel, settings | MIT OR Apache-2.0 | accepted 2026-10-13 |
| `tracing` 0.1 | scan, engine | structured diagnostics instead of print | MIT | accepted 2026-10-13 |
| `tempfile` 3 (dev) | scan | synthetic trees for scan tests | MIT OR Apache-2.0 | accepted 2026-10-13 |
| `proptest` 1 (dev) | model | property tests for invariants | MIT OR Apache-2.0 | accepted 2026-10-13 |
| `eframe` 0.36 (wgpu, default_fonts, accesskit) | ui, apps/dirustat | native window and event loop on wgpu (ADR 0001); no webview | MIT OR Apache-2.0 | accepted 2026-10-13 |
| `egui` 0.36 | ui, apps/dirustat | immediate-mode UI widgets; alternative: iced/slint (heavier, retained) | MIT OR Apache-2.0 | accepted 2026-10-13 |

Planned (decided in an ADR when introduced): `windows-sys`/`windows` (only `dirustat-ntfs` and platform services), `trash` (recycle bin), `regex` (search), `image` (PNG export).

## Versioning and releases

SemVer for the workspace (`workspace.package.version`). The control protocol is versioned separately (`PROTOCOL_VERSION`); changing it after the first release is a human decision. Release automation is `cargo xtask version` plus a `release.yml` workflow (milestone M6).

## Platform code

Everything platform-specific sits behind a `PlatformServices` trait (recycle bin, file manager, free space, privileges). Implementations live under `cfg(windows)` / `cfg(unix)`; on an unsupported platform return `Unsupported`, never `panic`. Gate platform tests with `cfg` and do not break builds on other OSes.
