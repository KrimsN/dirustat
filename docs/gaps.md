# Gaps: the ranked work list

> Status: 2026-10-13 · revision 11 · M3 code landed (G-012 and G-013 reduced to real-volume verification, measurement and extras; G-031-G-037 added from the M3 review); M2 landed; G-005, G-006, G-008, G-009, G-010 closed; G-011 reduced; G-025-G-030 added. Take items top to bottom within the current milestone (`plan/STATUS.md`). When you close one, move it to "Closed" with a date and update the parity documents.

Priority: **P0** infrastructure and reliability, **P1** core parity, **P2** remaining parity, **P3** beyond parity.

## Open

| ID | P | Milestone | Description | Done when |
|---|---|---|---|---|
| G-038 | P0 | QA | **QA-2, QA-101 (S2).** Treemap `max_tiles` cap is spent LIFO, so the smallest dirs are expanded first and the largest stay empty; holes grow with the window size (the reported resize bug). Fix: spend the cap largest-first / breadth-first, or aggregate the remainder | invariant test: with a cap hit, no top-level dir is left without children while smaller ones are expanded; maximized `100k-seed1` and `1m-seed1` screenshots show no holes |
| G-039 | P0 | QA | **QA-1 (S2).** `layout_tree` drops children below `min_size`, marks the parent `leaf = false`, and `map_panel` paints it as a frame only, leaving background holes that depend on the viewport | every directory rect is fully covered (fill the parent or add an "other" tile); property test on area coverage at several bounds |
| G-040 | P0 | QA | **QA-3 (S2).** Windows portable scan counts hardlinks N times (`link_id_of` returns `None` outside unix) | fixture with 3 hardlinks counts the size once; parity with the MFT backend; works under `unsafe_code = forbid` (safe wrapper or `dirustat-ntfs`) |
| G-041 | P0 | QA | **QA-4 (S2/S1).** Control-channel UNC guard is bypassed by `\??\UNC\...` and runs only for `scan.start` | every path-taking command refuses network paths in every spelling; dispatcher tests |
| G-042 | P1 | QA | **QA-102 (S3).** Treemap relayout runs on the UI thread on every resize step (about 2.4 s on 1M nodes) | layout off the UI thread or debounced, previous layout reused; resize on `1m-seed1` does not block frames |
| G-043 | P1 | QA | **QA-103 (S3).** File types panel ignores the size mode (`ext_panel.rs:109` uses `SizeMode::default()`) | logical/allocated switch changes the panel; screenshot test |
| G-044 | P1 | QA | **QA-104 (S3).** File types panel: extension and Files columns have zero width, size clipped, sort glyph is a tofu box | default width honoured, columns readable, glyph renders; screenshot |
| G-045 | P1 | QA | **QA-5 (S3, static).** Portable and MFT backends disagree on allocated size (resident files 0 vs 4096, sparse, compressed) | same fixture gives the same allocated totals on both backends, or the difference is documented; elevated parity test |
| G-046 | P1 | QA | **QA-106 (S3).** Treemap shrinks to a strip at narrow widths or 150% zoom; panels vanish without a hint | treemap keeps a minimum share, side panels collapse first; screenshots at 700x500 and 1000x700 with zoom 150 |
| G-047 | P1 | QA | **QA-105 (S3).** `ui.state` and `ui.screenshot` time out (10 s / 40 s) while the window is minimized; docs say 60 s | `ui.state` answers while minimized or fails fast with a clear error; docs match |
| G-048 | P2 | QA | **QA-6 (S3).** Keyless local client can exhaust connection slots and the global rate limit of the control server | unauthenticated peers do not consume authenticated capacity; load test |
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
| G-038 | P1 | M3 | MFT scan outlier: 3.26 s run with ~2.7 s unexplained inside `collect` (parse 44 ms, disk wait 325 ms); suspects: reader backpressure or thread stalls, `NO_BUFFERING` handle fallback, antivirus, rayon contention | repeat 10+ runs with finer phase timers (consumer blocked-on-send, per-chunk latency histogram); budget met in all runs |
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
