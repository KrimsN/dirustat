# Gaps: the ranked work list

> Status: 2026-10-13 · revision 10 · M3.1 landed (G-012 partly: volume FFI done, record parsing open); M2 landed; G-005, G-006, G-008, G-009, G-010 closed; G-011 reduced; G-025-G-030 added. Take items top to bottom within the current milestone (`plan/STATUS.md`). When you close one, move it to "Closed" with a date and update the parity documents.

Priority: **P0** infrastructure and reliability, **P1** core parity, **P2** remaining parity, **P3** beyond parity.

## Open

| ID | P | Milestone | Description | Done when |
|---|---|---|---|---|
| G-002 | P0 | M1 | Portable parallel scan in `scan` (rayon, progress, cancellation, hardlinks, reparse points; errors do not abort the walk) | integration tests on fixtures; budget documented. M1.4 `scan::start` and M1.5 fixture test + measurement landed; open: cold-cache scan slowness (125 s for 1M), `xtask perf` |
| G-011 | P1 | M2 | Remainder of M2.9: drive picker UI, flat-list panel, free-space UI (settings, launch args and the engine side of top-N, paged children and the drives provider landed). Real drive enumeration and free space move to M4/G-014 | parity rows A12, B3, B4, B10 become done |
| G-025 | P1 | M2 | `tree.children` and `list.largest_files` are O(n) under the session lock | cache by `Session::revision()`; measured on 1M nodes |
| G-026 | P2 | M2 | `dirustat-cli info` has no `--timeout` | flag added, test with a stalled server |
| G-027 | P2 | M2 | `ui.screenshot` PNG encoding runs on the UI thread | encoding moved off the UI thread, frame time unaffected |
| G-028 | P2 | M2 | Menus cover only File/Edit/View | remaining groups from the registry appear in the menu bar |
| G-029 | P1 | M1 | Keyboard navigation in the tree panel | arrow/Home/End/Enter navigation with tests |
| G-030 | P2 | M0 | `wasm-core` check is skipped locally (target `wasm32-unknown-unknown` not installed) | `cargo xtask ci` runs the wasm check on this machine |
| G-012 | P1 | M3 | `dirustat-ntfs`: Win32 FFI (isolated unsafe), MFT reading, record parsing, fixups, data runs | fuzz target, round-trip, budget ≤ 3 s per 1M. **Progress:** M3.1 volume FFI + geometry validation done; M3.2 safe record parsing, fixups, data runs, `MftReader`, proptests and `crates/ntfs/fuzz` targets done (2026-10-13); open: real-volume MFT read with extension records (`$ATTRIBUTE_LIST`), 1M-record budget measurement |
| G-013 | P1 | M3 | Transparent fallback MFT → portable scan, administrator rights request | tests + manual check |
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
