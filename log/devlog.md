# Dev log

New entries go on top. Format: `docs/progress-docs.md`.

## 2026-10-18 · M4.4 UI part 1 (search, drives, context menu, exclusions) · Claude
- Landed (`crates/ui`): search bar (Ctrl+F, `ui.toggle_search`) driving `search.find` with debounce (300 ms, Enter runs at once, one run per criteria+revision), modes, case/path toggles, size and extension filters, inline friendly errors (invalid regex, bad size), result list with click-to-select, highlight of `Session::search_hits` in tree and treemap, Clear = `search.clear`; drive picker on the start screen and in a Drives menu (fill bars, click starts `scan.start`), `ui.refresh_drives`; status-bar free space via `platform.free_space` (read once per scan root and revision); context menu on tree rows and treemap tiles (`platform.reveal`, `platform.properties`, `ui.copy_path`); Preferences window (`ui.preferences`) with the exclusion editor over `settings.set scan.exclude`; shell menu/shortcut overrides for engine commands without a menu (`ENGINE_MENU`: Edit > Clear search, Reveal in file manager Ctrl+Shift+R, Properties Alt+Enter); demo session now uses a fake platform with synthetic drives (`DIRUSTAT_DEMO_EMPTY=1 dirustat --demo` shows the start screen).
- Numbers: ui tests now 91 (was about 60); screenshots `docs/qa/evidence/ui-m44-search-{dark,light}.png`, `ui-m44-start-{dark,light}.png`.
- Still open: the search runs on the UI thread under the session lock (bounded by the engine time budget and 500-hit cap; a 1M-node search is not measured), Preferences window and context menu popup are covered by headless tests but not screenshotted, no delete entry (G-015 UI), date filter, settings are saved on exit only.

## 2026-10-18 · G-042 background treemap layout · Claude
- Landed: `Session.tree` is an `Arc<Tree>`; `map_poll` / `map_ready_view` / `map_view_async` run the hierarchical layout on a background thread (one at a time). While it runs the map keeps the previous layout of the same tree and size mode, rescaled to the new bounds; the panel shows "Laying out..." and repaints every 30 ms. A layout of a replaced tree is dropped, a dead layout thread caches an empty layout (no retry loop), a failed thread spawn falls back to computing inline. The UI colour caches are keyed by a layout generation.
- Numbers: engine tests 85 -> 89 (async arrival equals sync result, resize keeps the old picture inside the bounds, replaced tree drops the result, invalid bounds).
- Not verified: resize on `1m-seed1` in the real window (frame times), screenshot right after a scan can still show the "Laying out..." state for a few frames.
- Still open: G-040, G-045, G-053-G-055.

## 2026-10-18 · QA fixes · Claude
- Landed (20 commits, grouped by crate): treemap QA-2 (tiles expanded largest-first, cap cuts the smallest), QA-1 (undrawable children merged into a remainder tile), QA-102 (layout bounds quantized, cached tiles rescaled on resize). scan QA-3 (Windows hardlinks deduplicated via the safe `dirustat_ntfs::file_identity` wrapper), QA-5 (allocated-size policy documented and pinned). control QA-4 (UNC guard for every spelling and every path-taking command), QA-6 (pre-auth slots and rate budget separate), QA-7 (`ui.screenshot` targets restricted for remote callers), QA-8 (`session.tree` rejects limit 0), QA-9 (protocol docs fixed, float request ids rejected), QA-112 (scan root normalized, `tree.path` joined with one separator). ui QA-103, QA-104, QA-105, QA-106, QA-107, QA-108, QA-109, QA-113 (too-small hint), QA-115 (partial).
- Numbers: portable scan of the 100k fixture 0.04 s -> about 1.1 s because every non-empty file is now opened for its hardlink identity (G-040). `cargo xtask ci` green.
- Closed gaps: G-038, G-039, G-041, G-043, G-044, G-046, G-047, G-048. The duplicate G-038 (MFT outlier) is now G-052.
- Still open: G-040 (identity cost), G-042 (layout still on the UI thread at a quantization step crossing), G-045 (allocated-size parity), G-053 (treemap tooltip joins with `/`), G-054 (`select.node` cleared when a scan finishes), G-055 (QA-115 skipped items), G-049, G-050, G-051.
- NOT verified visually: maximized `100k-seed1` and `1m-seed1` screenshot re-test of the treemap, scan dimming of stale views, the "too small" hint.

## 2026-10-15 · M3.5 measurement · human run + Claude
- Landed: parallel prefetching MFT reader (<= 4 readers, 8 MiB requests, unbuffered second handle), rayon record parsing, per-phase timings in `MftStats`/`mft_bench`, synthetic CPU bench `mft_synth_bench` (1M records in memory: 0.53-0.65 s total, was 1.1-1.4 s).
- Numbers (release, elevated, 1,000,029-node fixture, MFT 977 MiB, dynamic VHDX on NVMe): MFT backend 0.31 s / 0.74 s / 3.26 s over 3 runs; parse 44 ms, merge 30 ms, build ~157 ms. Portable walk on the same volume: 5.76 s / 0.84 s / 2.31 s. Same fixture on a VHDX that lives on a Storage Space (HDD-backed, thin): MFT 65 s / 10 s / 54 s, read 25-29 MiB/s per thread, CPU phases unchanged (~0.25 s), so that run measured the disk.
- Verified: MFT and portable trees agree (1,000,029 vs 1,000,026 nodes); `orphan_links=0`, `cycle_entries=0`, `corrupt=0`, so the 3-node difference is not an orphan effect (the portable walk marks one unreadable folder as an error and does not enter it; MFT sees the system files too). Not yet explained exactly: which nodes differ.
- Exit criterion (<= 3 s per 1M): met in 2 of 3 runs, missed once (3.26 s, ~2.7 s unexplained inside `collect` while parse was 44 ms and disk wait 325 ms). Budget `enforce` stays false (G-052). Physical floor: ~1 GB of MFT needs >= 350 MB/s sustained, so an HDD cannot meet 3 s.
- Still open: G-052 (outlier), extension-record merging, ADS sizes, `$MFT::$BITMAP` to skip unused records, a node-diff check MFT vs portable.

## 2026-10-13 · M3.5 (partial) · human run + Claude
- Verified on a real volume (elevated shell, debug build): `cargo test -p dirustat-ntfs -- --ignored` passes (`reads_the_ntfs_boot_sector`: the boot sector of `C:` carries the `NTFS    ` signature). `cargo test -p dirustat-scan -- --ignored`: `real_volume_scan_uses_the_mft` passes, i.e. a scan of `C:\` ran on the MFT backend without fallback and produced more than 1000 nodes; it took 26.15 s in a debug build on a real system volume (not a budget measurement).
- Not an issue: `scan_matches_reference_walk_on_fixture` fails under `--ignored` unless `DIRUSTAT_FIXTURE` points at a generated fixture (M1.5 behaviour).
- Still open: the exit criterion (1M files <= 3 s, release build, VHDX fixture) is not measured; G-012 stays open for that, extension-record merging and ADS sizes.

## 2026-10-13 · M3.2-M3.6 + review fixes · Claude
- Landed: M3.2 safe MFT record parser in `dirustat-ntfs` (fixups, `$STANDARD_INFORMATION`/`$FILE_NAME`/`$DATA`, data runs, hostile-input limits), `MftReader`, proptests, `crates/ntfs/fuzz` targets. M3.3 tree built from MFT records by parent ids. M3.4 transparent fallback to the portable scan; the scan reports `backend` and `fallback_reason`. M3.6 `scan.fast_path_status`, `scan.prefer_fast_path` setting, administrator-rights hint in the UI. Never-crash review fixes: root normalisation, allocation caps, name sanitising, geometry checks, cancellation. `session.state` gained `progress.backend`, `progress.fallback_reason` and `last_scan{backend,fallback_reason}` (documented in `docs/control-protocol.md`).
- Numbers: `cargo test --workspace` = 381 passed, 3 ignored; ntfs 69 (+1 ignored), scan 57 (+1 ignored), engine 76, ui 57. Parity: WizTree B1 moved to partial (WinDirStat 7/34 done, 11 partial; WizTree 0/12 done, 5 partial, recounted from rows).
- Verified: unit, property and synthetic-image tests only. NOT verified on a real volume; MFT scan speed is NOT measured, so the M3 exit criterion (≤ 3 s per 1M files) is not met.
- Still open: M3.5 needs an ELEVATED human run on a VHDX with 1M files: `cargo test -p dirustat-ntfs -- --ignored` and `cargo test -p dirustat-scan -- --ignored`, then record the time. G-012 remainder: extension records (`$ATTRIBUTE_LIST`), alternate data stream sizes. Review deferrals G-031-G-037: late I/O failure wastes the MFT pass, duplicate nodes on cycles in the orphan pass, sparse allocated size when `compression_unit == 0`, O(n²) `link_names`, progress frozen during build, no whole-image fuzz target, UI does not show `fallback_reason`. Real elevation stays for M6 (G-013). Next after M3.5: M4.

## 2026-10-13 · M3.1 · Claude
- Landed: `dirustat-ntfs` volume access. `win32.rs` (the only `unsafe`, each block with `// SAFETY:`): `CreateFileW` read-only open of `\\.\X:`, `FSCTL_GET_NTFS_VOLUME_DATA`, positional `ReadFile`, `CloseHandle` on drop. Safe `volume.rs`: `volume_path` (drive letters only), `validate_volume_data` (power-of-two sector/cluster/record sizes, no negatives, checked multiplication), `check_read_range` (sector-aligned, non-empty, <= 64 MiB, inside the volume), `Volume::{open,data,read_at}`, `VolumeError` with a distinct `AccessDenied`. `probe()` returns `Available` only for an openable NTFS volume; `probe_detailed()` gives the reason. Dependency `windows-sys` 0.61 (already in the tree), row added to `docs/engineering.md`.
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

## 2026-10-18 · M4.1 · Claude (platform services, G-014)
- Landed: crate `dirustat-platform` (L1): trait `PlatformServices`, `PlatformError`, `SystemPlatform` (Windows: `trash` for the recycle bin, `explorer.exe /select` without a shell, drives/free space/properties in the isolated `win32.rs`; other OSes: `Unsupported`), `FakePlatform` for tests. `Session::set_platform`/`platform()`, `list.drives` now real by default, commands `platform.reveal`, `platform.properties`, `platform.free_space`, `CommandError::Platform`. ADR 0009.
- Numbers: platform tests 13, engine tests +5.
- Still open: UI hooks (context menu, status bar free space, drive picker), `file.recycle`/delete command (G-015, must use `move_to_recycle_bin` and `docs/safety.md`), `open_default` is `Unsupported` on the real platform, network drives are not listed, the properties window is not covered by an automated test (would open a real window).

## 2026-10-18 · M4.2 · Claude (search, filters, scan exclusions, G-016)
- Landed: `dirustat_scan::Glob`/`Excludes` (hand-written two-pointer wildcard matcher, no backtracking blow-up), `ScanOptions::exclude` (excluded entries are skipped, not descended, not counted; non-empty list bypasses the MFT path), `dirustat_engine::search` (`Filter`, `NameMatcher` substring/wildcard/regex via the `regex` crate with size limit, top-N search with a 10 s budget, session-held hits keyed to the tree revision), commands `search.find`, `search.clear`, optional filters on `tree.children` and `list.largest_files`, `scan.start` param `exclude`, setting `scan.exclude` (no schema bump, serde default).
- Numbers: scan tests +6 (glob 5, exclusion 2 incl. hostile lists), engine tests +17 (search, commands, filters, settings).
- Still open: UI (search box, highlight of `Session::search_hits`, exclusion editor), date filter, exclusions on the MFT fast path, `regex` is declared in `crates/engine/Cargo.toml` directly (not in `[workspace.dependencies]`).

## 2026-10-18 · M4.3 · Claude (destructive file commands, G-015)
- Landed: `file.delete` (recycle via platform), `file.delete_permanent` (confirm + `i_understand_permanent`), `file.move`, `file.delete_preview`, `file.journal`; `engine::protected` (normalized protected-path list incl. env roots, 8.3, `\?\`), `fsops` (stale-tree re-check by kind/size/mtime, canonical location check, link-safe iterative removal), `prune` (tree rebuilt without the subtree, revision bumped), `journal` (cap 10000, optional JSONL). `CommandError::ProtectedPath` (`protected_path`) and `StaleTree` (`stale_tree`).
- Numbers: engine tests +24.
- Still open: UI (context menu, confirmation dialog with the permanent warning), hosts should call `Session::set_journal_file(settings_dir()/journal.jsonl)`, no file id in the model (size+mtime identity), descendants are not re-validated, `file.move` does not insert the destination into the tree, `cleanup.run` not started.

## 2026-10-18 · M4.5 · Claude (user-defined cleanups, G-017)
- Landed: `engine::cleanup` (definitions, validation, selector compiled with the non-backtracking glob, iterative selection with ancestor de-duplication and a 10000-node cap), commands `cleanup.list|save|remove|preview|run` in `cmd/cleanup.rs`, settings key `cleanups` (serde default, lenient per-entry load, sanitized: <= 64, slug ids, no built-in ids), 3 built-in examples that never run by themselves. `cmd/file.rs` now exposes `preflight` (protected + stale + link checks) and `perform` (action + journal) so `cleanup.run` reuses them per node; `prune::without_subtrees` removes many nodes in one pass (one revision bump), so no rescan is needed.
- Numbers: engine tests 144 -> 163.
- Still open: UI (cleanup menu, editor, preview and confirmation dialog), `older_than_days` needs a known mtime (nodes without one never match), no date filter in `search.find`, `empty_dir_contents` recycles direct children only (each child is one bin entry), a run does not re-validate descendants of a selected folder (same as `file.delete`).
