# Gaps: the ranked work list

> Status: 2026-10-13 · revision 7. Take items top to bottom within the current milestone (`plan/STATUS.md`). When you close one, move it to "Closed" with a date and update the parity documents.

Priority: **P0** infrastructure and reliability, **P1** core parity, **P2** remaining parity, **P3** beyond parity.

## Open

| ID | P | Milestone | Description | Done when |
|---|---|---|---|---|
| G-002 | P0 | M1 | Portable parallel scan in `scan` (rayon, progress, cancellation, hardlinks, reparse points; errors do not abort the walk) | integration tests on fixtures; budget documented. M1.4 `scan::start` and M1.5 fixture test + measurement landed; open: cold-cache scan slowness (125 s for 1M), `xtask perf` |
| G-005 | P1 | M1 | eframe/wgpu window: tree, extension list, status bar | screenshot in `log/devlog.md` |
| G-006 | P1 | M1 | Treemap: layout rendering, highlight, click selection, selection sync | layout invariants + screenshot |
| G-008 | P1 | M2 | Command registry (`CommandSpec`), menu catalog, command palette | `engine.commands` lists everything |
| G-009 | P0 | M2 | `hostile_params_never_panic` over the whole registry | test is green |
| G-010 | P1 | M2 | Control channel (TCP/key), `ui.screenshot`, CLI, MCP | the protocol in `docs/control-protocol.md` works |
| G-011 | P1 | M2 | Settings (serde), command-line arguments, drive picker, flat list/top-N | parity rows become done |
| G-012 | P1 | M3 | `dirustat-ntfs`: Win32 FFI (isolated unsafe), MFT reading, record parsing, fixups, data runs | fuzz target, round-trip, budget ≤ 3 s per 1M |
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
| G-024 | 2026-10-13 | M1.7: ADR 0008; names stored lossy (U+FFFD) with `NodeData::name_lossy`; `name_from_wide`, `Tree::add_node_from_wide` |

## Revision history

- 2026-10-13 · r1: initial list.
- 2026-10-13 · r2: translated to English.
- 2026-10-13 · r3: G-001 closed (M1.1).
- 2026-10-13 · r4: G-004 closed (M1.2).
- 2026-10-13 · r5: G-003 closed (M1.3).
- 2026-10-13 · r6: G-007 closed (M1.6).
- 2026-10-13 · r7: G-024 closed (M1.7); G-005/G-006 partially advanced (see devlog M1.8-M1.11).
