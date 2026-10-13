# Dev log

New entries go on top. Format: `docs/progress-docs.md`.

## 2026-10-13 · M3.1 · Claude
- Landed: `dirustat-ntfs` volume access. `win32.rs` (the only `unsafe`, each block with `// SAFETY:`): `CreateFileW` read-only open of `\.\X:`, `FSCTL_GET_NTFS_VOLUME_DATA`, positional `ReadFile`, `CloseHandle` on drop. Safe `volume.rs`: `volume_path` (drive letters only), `validate_volume_data` (power-of-two sector/cluster/record sizes, no negatives, checked multiplication), `check_read_range` (sector-aligned, non-empty, <= 64 MiB, inside the volume), `Volume::{open,data,read_at}`, `VolumeError` with a distinct `AccessDenied`. `probe()` returns `Available` only for an openable NTFS volume; `probe_detailed()` gives the reason. Dependency `windows-sys` 0.61 (already in the tree), row added to `docs/engineering.md`.
- Numbers: ntfs tests 1 -> 8 (+1 ignored); `cargo xtask ci` and `cargo deny check` green.
- Verified: without administrator rights `Volume::open("C:")` returns `AccessDenied` (the ignored boot-sector test fails with exactly that).
- Still open: reading real bytes needs an elevated run of `cargo test -p dirustat-ntfs -- --ignored` (human step); M3.2 record parsing + fuzz target; `scan` does not use the fast path yet.

## 2026-10-13 · M2.1-M2.9 · Claude
- Landed: M2.1 `CommandSpec` registry; commands `scan.start/cancel`, `select.node/clear`, `view.size_mode`, `session.state/poll`, `settings.get/set/reset`, `tree.children`, `tree.path`, `list.largest_files`, `list.drives`. M2.3 `hostile_params_never_panic` over the registry. M2.2 menus and command palette (Ctrl+Shift+P) built from the registry, `ui.*` shell commands. M2.4 control channel (TCP 127.0.0.1, key file via getrandom, limits), `UiBridge` wired in `apps/dirustat` with `--control`. M2.5 `ui.screenshot` (flags `--demo --screenshot --exit-after-screenshot`), screenshot `docs/img/m1-main.png`. M2.6 CLI (`commands`, `scan`, `info`, `run`, `mcp`, `--timeout`). M2.7 MCP stdio server and `mcp --connect`. M2.8 Settings (serde, atomic save, corrupt-file handling) and `LaunchArgs`. M2.9 engine side only: top-N largest files, paged children, drives provider (default empty).
- Numbers: 255 tests pass in `cargo test --workspace`. Parity: WinDirStat 7/34 done (11 partial), WizTree 0/12 done (3 partial).
- Verified: screenshot `docs/img/m1-main.png`; closed G-005, G-006, G-008, G-009, G-010.
- Still open: G-011 (drive picker UI, flat list UI, free space; real Win32 drives need platform services in M4/G-014); review follow-ups G-025 (O(n) `tree.children`/`list.largest_files` under the session lock, cache by revision), G-026 (`info` has no `--timeout`), G-027 (screenshot encode on the UI thread), G-028 (menus only File/Edit/View), G-029 (tree keyboard navigation), G-030 (wasm-core check skipped locally); G-002 cold-cache scan slowness. Next: M3.1 Win32 FFI in `dirustat-ntfs`.

## 2026-10-13 · M1.7-M1.11 (logic + shell) · Claude + 4 parallel agents
- Landed: M1.7 ADR 0008 + `name_lossy` flag + `name_from_wide` (model 7 -> 12 tests). M1.8 eframe/egui 0.36 shell: `Tokens` (dark/light), serde `UiState`, `DirustatApp` (menu, path bar, Scan/Stop, status bar), stub tree/map/ext panels, non-blocking scan polling, headless frame tests (ui 9 tests), `cargo run -p dirustat`. M1.10 logic: `treemap::{hit_test, layout_tree, hit_test_tiles}` (iterative, depth/tile/min-size caps) and `engine::MapLayout`/`MapState` cached by `Session::revision()`, selection (treemap 2 -> 7, engine 8 -> 18 tests). M1.11 logic: `engine::ext` extension stats and deterministic colours.
- Numbers: `cargo xtask ci` green; 84 tests passing.
- Pending human decision: `deny.toml` changes proposed by the UI agent (`BSL-1.0` allow, `Ubuntu-font-1.0` exception for `epaint_default_fonts`); not committed. `cargo deny check` fails until decided.
- Still open: M1.9 tree panel, M1.10/M1.11 rendering in the UI, visual verification/screenshot (M1.12), scanner does not use `add_node_from_wide` yet, wasm target not installed locally, menus are not commands yet (M2).
- Addendum (M1.9-M1.11, commits 95069fa, 3c86398, b4f174c): the panels landed. Tree panel: columns name/size/% of parent/files/mtime, virtualized, sortable, expand/collapse, selection sync. Treemap: squarified rendering with extension colours, hover tooltip, click select, selected-extension highlight. Extension panel: swatch, count, size, percent, sortable, selection. `deny.toml` change (BSL-1.0, Ubuntu-font-1.0 for egui fonts only) was human-approved, so the pending decision above is resolved. Total tests: 90+.
- Not verified/not done: no screenshot (`ui.screenshot` arrives in M2.5; the release app was only smoke-launched for 8 s without a crash), no cushion shading, no keyboard navigation in the tree, menus are placeholders, scan does not use `add_node_from_wide`, wasm target not installed. Parity rows moved to `partial` only; G-005/G-006 stay open.

## 2026-10-13 · M1.6 · Claude
- Landed: `engine::guard::guarded` (private) wraps `Session::run` in `catch_unwind`; a panic becomes `CommandError::Panicked { what, message }` and is logged via `tracing::error!`. New `Session::finish_scan(handle)` joins a scan under the same guard: a scan-thread panic or failure becomes `CommandError::Scan`, a cancelled scan keeps the old tree, otherwise the new tree replaces it. The scan crate already caught worker panics per directory (M1.4).
- Numbers: engine tests 2 -> 8 (str/String/opaque payloads, pass-through, session survives a panicking `test.panic` command, `finish_scan` replaces the tree).
- Still open: the control channel / UI shell must route every call through `Session::run` (M2); cold-cache scan slowness and `xtask perf` (G-002) are unchanged.

## 2026-10-13 · M1.5 · Claude
- Landed: ignored integration test `crates/scan/tests/fixture_scan.rs` (scan vs an independent `std` walk: dirs, files, errors, size bound; path via `DIRUSTAT_FIXTURE`, absolute) and `examples/scan_bench.rs` (prints only aggregates).
- Numbers (Windows 11, release, default rayon threads): 100k fixture (100020 nodes) warm 0.03-0.14 s; 1M fixture (1000020 nodes) warm 1.43 s (~700k nodes/s), within the 25 s budget. The very first scan right after generation was slow: 12.9 s for 100k, 125 s for 1M (~8k nodes/s), i.e. cold cache / antivirus-style overhead on freshly written files.
- Test passes on both fixtures: counts equal the reference walk, 0 errors.
- Still open: explain/avoid the cold-cache slowness (measure after reboot or with Defender exclusion; try `FindFirstFileExW` large fetch); `enforce` stays false; `xtask perf` does not exist yet; G-002 stays open for that.

## 2026-10-13 · M1.4 · Claude
- Landed: `scan::start(root, ScanOptions) -> Result<ScanHandle, ScanError>` with `progress()`, `cancel()`, `is_finished()`, `join() -> ScanOutcome { tree, cancelled }`. Rayon workers read directories; one builder thread owns the `Tree` (no locks), work tracked by a pending counter (iterative). Symlinks/reparse points become `NodeKind::Link` and are never followed; unreadable directories get the error flag and the walk continues; a worker panic is caught and reported as a directory error; hardlinks counted once where `std` exposes identity (Unix only).
- Numbers: scan tests 1 -> 9.
- Limits (documented in the crate header): allocated size is logical rounded up to `cluster_size` (4096 default), sparse/compressed files are not detected, Windows hardlinks are not deduplicated in the portable path (needs the `ntfs` fast path or `unsafe`).
- Still open: M1.5 integration tests on generated fixtures, 100k/1M measurements, `perf/budgets.toml`; hardlink dedup was tested via the builder function only, not on a real Unix FS.

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
