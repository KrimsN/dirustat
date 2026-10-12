# Dev log

New entries go on top. Format: `docs/progress-docs.md`.

## 2026-10-13 · M1.3 · Claude
- Landed: `cargo xtask fixtures` (`xtask/src/fixtures.rs`): splitmix64 plan streamed through a `Sink` trait (disk sink + in-memory sink for tests), iterative generation with exact node count, depth cap 14, `_special/` edge cases (empty dirs, hardlinks, symlink cycle, unicode, 300+ char path via `\\?\`). Refuses a non-empty output directory.
- Numbers: xtask tests 1 -> 9; 100k nodes (seed 1) = 14441 dirs + 85577 files + specials in ~32 s on Windows (file creation bound).
- Fixed on the way: a rounding bug in `split` that produced `nodes + 1` entries (caught by `node_count_is_exact`).
- Still open: M1.4 portable scan; 1M/5M fixtures not generated yet (disk/time heavy), do it when M1.5 needs them.

## 2026-10-13 · M1.2 · Claude
- Landed: workspace dependencies `rayon 1.12`, `serde 1.0`, `serde_json 1.0`, `tracing 0.1`; dev: `tempfile 3.27`, `proptest 1.12`; wired into model/scan/engine/control; rows in `docs/engineering.md`. Installed `cargo-deny` 0.20.2: `cargo deny check` passes (only a duplicate `syn` warning).
- Human decision: dependency set confirmed by the user.
- Still open: M1.3 fixtures; the `model` crate must still pass the wasm check once the target is installed (serde is wasm-safe).

## 2026-10-13 · M1.1 · Claude
- Landed: model carries `NodeData` (logical/allocated size, mtime, attributes, error flag), `SizeMode` selector, subtree counters (`file_count`, `dir_count`, `error_count`), `Tree::add_node`, `mark_error`; all sums saturate. `engine` adapted (`total_size(SizeMode)`).
- Numbers: model tests 3 -> 7.
- Verified: `cargo xtask ci` is green (wasm-core skipped locally).
- Still open: M1.2 (dependency set needs human confirmation), M1.3 fixtures.

## 2026-10-13 · M0 · Claude (language switch)
- Landed: all repository text translated to English; `AGENTS.md` §0 now says repository text is English and chat uses the user's language (ADR 0007). Code comments, Cargo descriptions, agent/command definitions, templates and docs updated.
- Verified: `cargo xtask ci` is green after the change.
- Still open: same as the M0 entry below.

## 2026-10-13 · M0 · Claude
- Landed: repository diRUSTat initialized. Workspace (7 crates + 2 apps + xtask), "never crash" lints (deny unwrap/expect/panic/indexing, `unsafe` forbidden except in `dirustat-ntfs`), `cargo xtask layers|ci`, GitHub Actions CI, `deny.toml`, `perf/budgets.toml`. Agent documentation: `AGENTS.md`, `CLAUDE.md`, `docs/*`, ADRs 0001-0006, `plan/*`. Claude Code configuration: `.claude/commands`, `.claude/agents`.
- Basis of the approach: studied the storytold repositories (photocraft, wordcraft, craft-libs): AGENTS.md/CLAUDE.md, layers with xtask, "everything is a command", control channel/MCP, never-crash, devlog and living progress documents. The private `craftrules` repository is unavailable; its standards were reconstructed from links in public files.
- Numbers: tests: model 3, treemap 2, scan/engine/control/ui/ntfs 1-2 each, xtask 1; parity 0/34 and 0/12.
- Verified: `cargo xtask ci` is green (wasm-core skipped locally: target not installed).
- Still open: M1.1 (model), M1 dependencies, a human decision about reserving the name `dirustat`.
