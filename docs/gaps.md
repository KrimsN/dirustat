# Gaps: the ranked work list

> Status: 2026-10-18 · revision 13 · QA fixes landed: 8 QA gaps closed (G-038, G-039, G-041, G-043, G-044, G-046, G-047, G-048); G-040, G-042, G-045 partial; G-052 (MFT outlier, renumbered) and G-053-G-055 open; M3 code landed (G-012 and G-013 reduced to real-volume verification, measurement and extras; G-031-G-037 added from the M3 review). Take items top to bottom within the current milestone (`plan/STATUS.md`). When you close one, move it to "Closed" with a date and update the parity documents.

Priority: **P0** infrastructure and reliability, **P1** core parity, **P2** remaining parity, **P3** beyond parity.

## Open

| ID | P | Milestone | Description | Done when |
|---|---|---|---|---|
| G-040 | P0 | QA | **QA-3 (S2).** Windows portable scan counts hardlinks N times (`link_id_of` returns `None` outside unix). Fixed by `dirustat_ntfs::file_identity`; remaining: cost, every non-empty file is opened once (100k fixture, release, warm: 0.04 s -> about 1.1 s), so a cheaper source is wanted (directory enumeration by handle with file ids, or an opt-out option) | fixture with 3 hardlinks counts the size once; parity with the MFT backend; works under `unsafe_code = forbid` (safe wrapper or `dirustat-ntfs`) |
| G-042 | P1 | QA | **QA-102 (S3).** Treemap relayout runs on the UI thread on every resize step (about 2.4 s on 1M nodes) | layout off the UI thread or debounced, previous layout reused; resize on `1m-seed1` does not block frames. **Mitigated (2026-10-15):** `Session::map_layout` lays out for coarsely quantized bounds (6-12 % steps) and only rescales the cached tiles to the exact bounds, so a drag recomputes once per step, not per pixel. **Remaining:** a step crossing (and the first layout after a scan) still blocks the UI thread for the full layout time; compute on a background thread and keep painting the previous layout meanwhile |
| G-045 | P1 | QA | **QA-5 (S3, static).** Portable and MFT backends disagree on allocated size (resident files 0 vs 4096, sparse, compressed). Policy now documented (scan crate docs, `docs/architecture.md`): MFT exact, portable an upper bound. Left: portable needs a per-file `GetCompressedFileSize` wrapper in `dirustat-ntfs` (verify what it returns for resident files; costs another file open, see G-040 timings), plus sparse files with `compression_unit == 0` (G-033) | same fixture gives the same allocated totals on both backends; elevated parity test on a VHDX with resident, sparse and compressed files |
| G-049 | P2 | QA | **QA-110 (S4).** `ui.state` does not round-trip through `ui.set_state`; shell `ui.*` commands are not drivable over control | documented way to run every `UI_COMMANDS` entry or a list of read-only fields |
| G-050 | P2 | QA | **QA-111 (S4).** UI layout differs from `docs/ui-design.md` (drive picker, search, folders/attrs columns, path and free space in the status bar, Largest files menu entry) | each item lands or is struck from the design doc |
| G-051 | P3 | QA | **QA-114 (S4, unconfirmed).** Clicking the same tree header twice in a row did not toggle the sort direction (maybe a synthetic-mouse artifact) | confirmed or dismissed with a physical mouse |
| G-053 | P2 | QA | **QA-112 (S4), remainder.** The scan root is normalized and `tree.path` joins with one separator (fixed), but the treemap tooltip still joins path segments with `/` | tooltip path uses the same join as `tree.path`; unit test |
| G-054 | P2 | QA | **QA-113 (S4), part (b).** The "too small" hint landed (part a), but `select.node` issued right after a scan finishes is still cleared when the scan completes (engine side) | `select.node` survives scan finish, or returns a clear error; engine test |
| G-055 | P3 | QA | **QA-115 (S5), skipped items.** The empty-folder message and the check-mark bullet landed; the root row clipped under the header and the "No data" text next to a root row were not addressed | each item fixed or dismissed after a visual check; screenshot |
| G-002 | P0 | M1 | Portable parallel scan in `scan` (rayon, progress, cancellation, hardlinks, reparse points; errors do not abort the walk) | integration tests on fixtures; budget documented. M1.4 `scan::start` and M1.5 fixture test + measurement landed; open: cold-cache scan slowness (125 s for 1M), `xtask perf` |
| G-011 | P1 | M2 | Remainder of M2.9: drive picker UI, flat-list panel, free-space UI (settings, launch args and the engine side of top-N, paged children and the drives provider landed). Real drive enumeration and free space move to M4/G-014 | parity rows A12, B3, B4, B10 become done |
| G-025 | P1 | M2 | `tree.children` and `list.largest_files` are O(n) under the session lock | cache by `Session::revision()`; measured on 1M nodes |
| G-026 | P2 | M2 | `dirustat-cli info` has no `--timeout` | flag added, test with a stalled server |
| G-027 | P2 | M2 | `ui.screenshot` PNG encoding runs on the UI thread | encoding moved off the UI thread, frame time unaffected |
| G-028 | P2 | M2 | Menus cover only File/Edit/View | remaining groups from the registry appear in the menu bar |
| G-029 | P1 | M1 | Keyboard navigation in the tree panel | arrow/Home/End/Enter navigation with tests |
| G-030 | P2 | M0 | `wasm-core` check is skipped locally (target `wasm32-unknown-unknown` not installed) | `cargo xtask ci` runs the wasm check on this machine |
| G-012 | P1 | M3 | `dirustat-ntfs`: Win32 FFI (isolated unsafe), MFT reading, record parsing, fixups, data runs | fuzz target, round-trip, budget ≤ 3 s per 1M. **Done (2026-10-13):** volume FFI (M3.1), safe record parser, fixups, data runs, `MftReader`, proptests, fuzz targets (M3.2), tree from MFT records (M3.3), review fixes. **Remaining:** verification on a real volume (elevated `cargo test -p dirustat-ntfs -- --ignored` and `cargo test -p dirustat-scan -- --ignored`), budget ≤ 3 s per 1M measured (M3.5, VHDX with 1M files), merging extension records (`$ATTRIBUTE_LIST`), a fuzz target for the whole `MftReader` image (see G-036), alternate data stream sizes |
| G-013 | P1 | M3 | Transparent fallback MFT → portable scan, administrator rights request | tests + manual check. **Done (2026-10-13):** fallback MFT → portable with `fallback_reason`, `scan.fast_path_status`, status-bar hint (M3.4, M3.6). **Remaining:** real elevation (manifest / `runas` relaunch) in M6 packaging; manual check on a real non-elevated run |
| G-031 | P2 | M3 | A late I/O failure during the MFT pass wastes the whole pass | per-chunk corrupt marking: an unreadable chunk marks its records as errors and the pass continues; regression test |
| G-032 | P2 | M3 | The orphan pass can create duplicate nodes when parent links form a cycle | each record attached at most once; test with a cyclic parent fixture |
| G-033 | P2 | M3 | Allocated size of sparse data is wrong when `compression_unit == 0` | sparse files report allocated size from data runs; test with a sparse image |
| G-034 | P2 | M3 | `link_names` is O(n²) on records with many hard-link names | linear or bounded implementation; measured on a hostile record |
| G-035 | P2 | M3 | Progress is frozen during the tree build phase after the MFT read | progress counters advance while building; test |
| G-036 | P2 | M3 | No fuzz target for the whole MFT image (`MftReader` + builder) | `crates/ntfs/fuzz` target added and run without crashes |
| G-037 | P3 | M3 | The UI does not show `fallback_reason` (only the elevation hint) | reason visible in the status bar or about window; screenshot |
| G-052 | P1 | M3 | MFT scan outlier: 3.26 s run with ~2.7 s unexplained inside `collect` (parse 44 ms, disk wait 325 ms); suspects: reader backpressure or thread stalls, `NO_BUFFERING` handle fallback, antivirus, rayon contention | repeat 10+ runs with finer phase timers (consumer blocked-on-send, per-chunk latency histogram); budget met in all runs |
| G-014 | P1 | M4 | Platform services: recycle bin, file manager, properties, free space | `PlatformServices` + Windows implementation |
| G-015 | P0 | M4 | Destructive commands per `docs/safety.md` (confirm, dry-run, protected paths, journal) | tests for refusal without confirm and for protected paths |
| G-016 | P2 | M4 | Search (substring/wildcard/regex), filters, scan exclusions | tests + hostile patterns |
| G-017 | P2 | M4 | User-defined cleanups | commands + tests |
| G-018 | P2 | M5 | Cushion shading and treemap parameters | screenshots, frame budget |
| G-019 | P2 | M5 | CSV/JSON/PNG export, scan snapshots (save/load), comparison | round-trip tests |
| G-020 | P3 | M5 | Duplicate finder | correct on fixtures |
| G-021 | P2 | M6 | Linux/macOS: portable scan, platform services, CI matrix | CI green on three OSes |
| G-022 | P2 | M6 | Packaging (MSI/portable zip/AppImage/dmg), signed releases, `release.yml` | release artifact |
| G-023 | P3 | M7 | Localizations, AccessKit, user documentation | accessibility checklist |

## Closed

| ID | Date | What |
|---|---|---|
| G-000 | 2026-10-13 | M0: workspace, lints, xtask layers/ci, documentation, CI |
| G-001 | 2026-10-13 | M1.1: model with logical/allocated sizes (`SizeMode`), `NodeData`, file/dir/error counters, mtime, attributes |
| G-004 | 2026-10-13 | M1.2: rayon, serde, serde_json, tracing, tempfile, proptest added; `cargo deny check` passes |
| G-003 | 2026-10-13 | M1.3: `cargo xtask fixtures` (seeded splitmix64 plan, exact node count, hardlinks, symlink cycle, empty/unicode/long-path cases) |
| G-007 | 2026-10-13 | M1.6: `engine::guard` (`catch_unwind`) around `Session::run` and `Session::finish_scan`; `CommandError::Panicked`; scan thread panic becomes `CommandError::Scan` |
| G-005 | 2026-10-13 | eframe/wgpu window with tree, extension list, status bar; screenshot `docs/img/m1-main.png` via `ui.screenshot` (keyboard navigation moved to G-029, menus to G-028) |
| G-006 | 2026-10-13 | Treemap rendering, highlight, click selection, selection sync; screenshot `docs/img/m1-main.png` (cushion shading stays in G-018) |
| G-008 | 2026-10-13 | M2.1, M2.2: `CommandSpec` registry, menus and command palette (Ctrl+Shift+P) built from the registry, `ui.*` shell commands |
| G-009 | 2026-10-13 | M2.3: `hostile_params_never_panic` over the registry |
| G-010 | 2026-10-13 | M2.4-M2.7: control channel (TCP 127.0.0.1, key file, limits), `ui.screenshot`, CLI (`commands`/`scan`/`info`/`run`/`mcp`), MCP stdio server and `mcp --connect` |
| G-024 | 2026-10-13 | M1.7: ADR 0008; names stored lossy (U+FFFD) with `NodeData::name_lossy`; `name_from_wide`, `Tree::add_node_from_wide` |
| G-038 | 2026-10-18 | QA-2/QA-101: treemap tiles expanded largest-first so the `max_tiles` cap cuts the smallest (commit "QA-2"). Maximized 100k/1M screenshot re-test still pending (see `plan/STATUS.md`) |
| G-039 | 2026-10-18 | QA-1: children too small to draw are merged into a remainder tile so every parent stays covered (commit "QA-1"). Screenshot re-test pending |
| G-041 | 2026-10-18 | QA-4: UNC guard covers every spelling for every path-taking control command (commit "QA-4") |
| G-043 | 2026-10-18 | QA-103: file types panel follows the session size mode |
| G-044 | 2026-10-18 | QA-104: readable file types columns, drawn sort arrow, minimum panel width |
| G-046 | 2026-10-18 | QA-106: treemap keeps a minimum share, side panels get width ranges and collapse first |
| G-047 | 2026-10-18 | QA-105: fail fast when the window does not render, stale `ui.state`, docs aligned |
| G-048 | 2026-10-18 | QA-6: separate pre-auth slots and rate budget from authenticated clients |

## Revision history

- 2026-10-13 · r1: initial list.
- 2026-10-13 · r2: translated to English.
- 2026-10-13 · r3: G-001 closed (M1.1).
- 2026-10-13 · r4: G-004 closed (M1.2).
- 2026-10-13 · r5: G-003 closed (M1.3).
- 2026-10-13 · r6: G-007 closed (M1.6).
- 2026-10-13 · r7: G-024 closed (M1.7); G-005/G-006 partially advanced (see devlog M1.8-M1.11).
- 2026-10-13 · r8: G-005/G-006 progress notes updated after M1.9-M1.11 panels; both stay open pending a screenshot.
- 2026-10-13 · r9: after M2, closed G-005, G-006, G-008, G-009, G-010; G-011 narrowed to the UI remainder; added G-025-G-030 from review follow-ups.
- 2026-10-13 · r10: M3.1 landed; G-012 stays open (progress note).
- 2026-10-13 · r11: M3.2-M3.6 and review fixes landed; G-012/G-013 narrowed (both stay open); added G-031-G-037 (review deferrals).
- 2026-10-15 · r12: QA pass (`docs/qa/reports/2026-10-15-summary.md`): added G-038-G-048 from confirmed S2/S3 bugs (S2 on top).
- 2026-10-15 · r12: M3.5 measurement recorded; G-038 added.
- 2026-10-18 · r13: 20 QA fix commits landed. Closed G-038 (QA-2), G-039 (QA-1), G-041 (QA-4), G-043 (QA-103), G-044 (QA-104), G-046 (QA-106), G-047 (QA-105), G-048 (QA-6). Kept open with notes: G-040, G-042, G-045. Duplicate id fixed: the M3.5 MFT outlier row is now G-052 (was a second G-038); references updated in `plan/STATUS.md`, `log/devlog.md`, `perf/budgets.toml`. Added G-053 (QA-112 tooltip), G-054 (QA-113b), G-055 (QA-115 skipped items). QA-7, QA-8, QA-9 were fixed in the same batch and had no gap rows; QA-107, QA-108, QA-109 are fixed too (no gap rows).
