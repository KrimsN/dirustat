# Parity with WinDirStat and WizTree

> Status: 2026-10-23 · revision 14 · WinDirStat done 11/34 (partial 18), WizTree done 3/12 (partial 5). Behaviour was determined from user documentation and use of the programs as black boxes (`docs/clean-room.md`).

Statuses: `missing` · `partial` · `done`. When you change a row, update the date, recount the totals in the header and in `ROADMAP.md`, and extend the revision history. Identifiers are command ids (`docs/control-protocol.md`).

## A. WinDirStat (34)

| # | Feature | Command / area | Milestone | Status |
|---|---|---|---|---|
| 1 | Scan a chosen folder | `scan.start` | M1 | done 2026-10-13 (registered command + path bar + Scan button, non-blocking; CLI `scan`; screenshot `docs/img/m1-main.png`) |
| 2 | Choose drives (all local / specific) | `scan.start_drives` | M1 | missing |
| 3 | Cancel a scan | `scan.cancel` | M1 | done 2026-10-13 (registered command + Stop button; cancellation tests) |
| 4 | Rescan / refresh | `scan.rescan` | M1 | partial (Scan on the same path; no dedicated rescan command) |
| 5 | Progress and estimate display | status bar | M1 | partial (status bar progress; no estimate) |
| 6 | Scan exclusions (masks, paths) | `scan.start` param `exclude`, setting `scan.exclude` | M4 | partial 2026-10-18 (name and path wildcards, tested on tempfile trees; exclusion editor in Preferences (Edit > Preferences) with the fast-path hint, headless tests, window not yet checked in a screenshot; MFT fast path is bypassed when a list is set) |
| 7 | Expand/collapse the tree | `tree.expand` / `tree.collapse` | M1 | partial (mouse only, virtualized; no keyboard navigation; not commands yet) |
| 8 | Sort by column | `tree.sort` | M1 | partial (sortable headers; not a command yet) |
| 9 | Columns: name, share (bar), %, size, items, files, folders, modified, attributes | `tree.columns` | M1 | partial (name, size, % of parent, files, mtime; no share bar, items, folders, attributes) |
| 10 | Selection sync tree ↔ treemap ↔ extensions | `select.node` | M1 | done 2026-10-13 (registered `select.node` / `select.clear`; tree, map and extension list synced; screenshot `docs/img/m1-main.png`) |
| 11 | Size mode: logical / on disk | `view.size_mode` | M1 | done 2026-10-13 (registered command, toggle in the UI, `--size-mode` launch flag, persisted in settings; tests) |
| 12 | "Free space" and "unknown" items | `view.free_space` | M2 | missing |
| 13 | Squarified treemap | `treemap.render` | M1 | done 2026-10-13 (layout invariants + extension-coloured rendering, hover tooltip; flat, cushion is row 14; screenshot `docs/img/m1-main.png`) |
| 14 | Cushion shading | `treemap.cushion` | M5 | partial 2026-10-23 (M5.1: van Wijk cushion shading in `crates/treemap/src/cushion.rs`, background renderer `crates/ui/src/cushion_view.rs` cached by revision/size/theme, command `treemap.cushion` / View > Cushion shading, setting `treemap.cushion` default on; treemap tests 10 -> 22; 100k nodes at 1080p about 12 ms per layout on a background thread, 4K 43.5 ms; checked on the synthetic demo screenshot only; budget `treemap_cushion_shading_100k_nodes_1080p` 25 ms not enforced, not re-measured on 1M nodes, about 12 of 2.07M pixels at 1080p show the background colour, wasm32 check not run locally; G-018) |
| 15 | Treemap options (grid, brightness, height, scale, light) | `treemap.look`, `treemap.grid` | M5 | partial 2026-10-23 (M5.2: `treemap.look` sets height, scale, ambient, diffuse and light direction (validated, persisted as `treemap.look`), `treemap.grid` draws tile edge lines, Preferences "Treemap" section with live sliders and View > Grid lines; engine tests +8, ui tests +14; checked on the synthetic demo screenshots `docs/img/m52-prefs-*.png` only; no separate brightness setting, `show_free_space` is still a placeholder; G-018) |
| 16 | Zoom into/out of a directory | `view.zoom_in` / `view.zoom_out` | M2 | missing |
| 17 | Highlight the selection on the treemap | `treemap.highlight` | M1 | partial (selection synced to the map; not visually verified) |
| 18 | Select by clicking the treemap | `treemap.pick` | M1 | partial (click select via `treemap::hit_test`; not a command yet) |
| 19 | Show/hide the treemap | `view.toggle_treemap` | M1 | missing |
| 20 | File-type list (extension, size, count, %) | `ext.list` | M1 | partial (swatch, count, size, percent, selection; not a command yet; no screenshot) |
| 21 | File-type colours | `ext.colors` | M1 | done 2026-10-13 (deterministic colours in `engine::ext`; used by map and list; tests; screenshot `docs/img/m1-main.png`) |
| 22 | Highlight an extension's files on the treemap | `ext.highlight` | M2 | partial (selected-extension highlight on the map landed early in M1.10; not a command yet) |
| 23 | Sort the extension list | `ext.sort` | M1 | partial (sortable columns; not a command yet) |
| 24 | Open a file | `file.open` | M4 | missing |
| 25 | Reveal in file manager | `platform.reveal` | M4 | partial 2026-10-19 (engine command with path validation and tests via `FakePlatform`; context menu entry and Ctrl+Shift+R; the real Explorer call was never exercised by agents and there is no screenshot of the menu) |
| 26 | Delete to recycle bin | `file.delete` | M4 | done 2026-10-19 (M4.7: confirmation dialog from the context menu, Edit menu and Delete; preview first, never from a click; toast with count and size; screenshots `docs/qa/evidence/ui-m47-delete-*.png`) |
| 27 | Delete permanently | `file.delete_permanent` | M4 | done 2026-10-19 (M4.7: red irreversible warning, "I understand" checkbox, extra one for partial trees, Enter never confirms; screenshots `ui-m47-delete-permanent-*.png`) |
| 28 | Copy path | `ui.copy_path` (shell command) | M4 | partial 2026-10-19 (context menu on tree rows and treemap tiles, headless tests; no engine command, no screenshot) |
| 29 | User-defined cleanups | `cleanup.list` / `cleanup.save` / `cleanup.remove` / `cleanup.preview` / `cleanup.run` | M4 | done 2026-10-19 (M4.7: Tools > Cleanups window with preview, dry run, confirmed run, editor and per-node results; Tools > Journal; screenshots `ui-m47-cleanups-*.png`, `ui-m47-journal-*.png`) |
| 30 | File/folder properties | `platform.properties` | M4 | partial 2026-10-19 (engine command with tests via `FakePlatform`; context menu entry and Alt+Enter; the real Windows properties window was never opened by agents, no screenshot) |
| 31 | Search by name | `search.find` | M4 | done 2026-10-18 (same feature as B5: Ctrl+F search bar, substring/wildcard/regex, tree and treemap highlight; budgeted and hostile-pattern tests; screenshots `docs/qa/evidence/ui-m44-search-*.png`) |
| 32 | Filter by name/size/date | `search.find`, filters on `tree.children` / `list.largest_files` | M4 | partial 2026-10-18 (name, size and extension filters in the search bar (Ctrl+F) with tree and treemap highlight, screenshots `docs/qa/evidence/ui-m44-search-*.png`; no date filter) |
| 33 | Persist settings | `settings.get` / `settings.set` / `settings.reset` | M2 | done 2026-10-13 (serde settings, atomic save, corrupt-file handling; tests) |
| 34 | Launch from the command line with a path | `dirustat --scan <path>` | M2 | partial (`LaunchArgs`: `--scan`, `--demo`, `--size-mode`, `--screenshot`, `--control`; no bare positional path) |

## B. Distinctive WizTree features (12)

| # | Feature | Command / area | Milestone | Status |
|---|---|---|---|---|
| 1 | Scan through the MFT | `scan.start` (backend `mft`) | M3 | partial 2026-10-13 (safe record parser, MFT reader and tree builder with transparent fallback, covered by tests on synthetic images; not verified on a real volume, no extension-record merging, budget ≤ 3 s per 1M not measured) |
| 2 | Ask for administrator rights and fall back to a normal scan | `scan.elevate` | M3 | partial 2026-10-13 (transparent fallback MFT → portable with a reason in `session.state`; awareness only: `scan.fast_path_status`, status-bar hint, `ui.relaunch_elevated_help`; real elevation left for M6 packaging) |
| 3 | Flat list of all files | `view.file_list` | M2 | missing |
| 4 | Top largest files | `list.largest_files` | M2 | partial (engine command with tests, callable from CLI/control/MCP; no UI panel; O(n) per call, see G-025) |
| 5 | Search with wildcard/regex | `search.find` (mode) | M4 | done 2026-10-18 (search bar with substring/wildcard/regex, case and path toggles, size and extension filters, result list, tree and treemap highlight, inline errors; screenshots `docs/qa/evidence/ui-m44-search-dark.png`, `ui-m44-search-light.png`) |
| 6 | CSV export | `export.csv` | M5 | done 2026-10-23 (CSV and JSON, streamed, limits, formula-safe cells, atomic write, File > Export; native save dialog not exercised by agents) |
| 7 | Treemap image export | `export.png` | M5 | partial 2026-10-23 (cushion-shaded PNG of a subtree at a chosen size, desktop app only; default cushion look, checked on synthetic data only) |
| 8 | Save/load scan snapshots | `snapshot.save` / `snapshot.load` | M5 | missing |
| 9 | Compare snapshots | `snapshot.compare` | M5 | missing |
| 10 | Drive picker with fill bars | `list.drives` | M2 | done 2026-10-18 (start-screen picker and Drives menu with fill bars from `list.drives`, click starts `scan.start`; free space of the scanned volume in the status bar via `platform.free_space`; screenshots `docs/qa/evidence/ui-m44-start-dark.png`, `ui-m44-start-light.png`) |
| 11 | Scan non-NTFS / network paths | `scan.start` (backend `portable`) | M1 | partial (portable scan shown in the window, `scan.start` is a command; no network-path check) |
| 12 | Portable mode (no install, settings alongside) | `settings.portable` | M6 | missing |

## C. Beyond parity (not counted in percentages)

Duplicate file finder (size → partial hash → full hash), change history between snapshots with a chart, dark/high-contrast themes, a command palette, agent control (control/MCP), CLI export, cleanup templates, "what can be deleted" hints (caches, temp files).

## D. Metrics

| Metric | Value |
|---|---|
| WinDirStat, `done` | 11 / 34 (partial 18) |
| WizTree, `done` | 3 / 12 (partial 5) |
| MFT scan speed (1M files) | 0.31 / 0.74 / 3.26 s over 3 elevated release runs on a VHDX (budget ≤ 3 s, one outlier: G-052; devlog 2026-10-15) |

## Revision history

- 2026-10-13 · r1: initial matrix.
- 2026-10-13 · r2: translated to English.
- 2026-10-13 · r3: after M1.7-M1.11, rows A1, A3, A4, A5, A7-A10, A13, A17, A18, A20-A23, B11 moved to `partial`; none is `done` because there is no screenshot verification (`ui.screenshot` arrives in M2.5), the actions are not commands yet (M2.2), and cushion shading, keyboard navigation and some columns are missing.
- 2026-10-13 · r4: after M2, screenshot `docs/img/m1-main.png` exists and actions are registered commands. A1, A3, A10, A11, A13, A21, A33 moved to `done`; A34, B4, B10 moved to `partial`. Other partial rows stay partial (no commands for tree/ext actions, no keyboard navigation, no cushion, no UI for largest files/drives).
- 2026-10-13 · r5: after M3.2-M3.6, B1 moved to `partial` (parser, tree builder and fallback tested on synthetic images only). Recounted from rows: WizTree partial rows are B1, B2, B4, B10, B11 = 5 (the previous header said 3, which missed B2 and B4/B10 bookkeeping). WinDirStat unchanged (7 done, 11 partial).
- 2026-10-18 · r6: G-016 landed in the engine. A6, A32 and B5 moved to `missing` -> `partial` (commands and tests only, no UI). Recounted: WinDirStat partial 13, WizTree partial 6.
- 2026-10-18 · r7: G-015 landed in the engine. A26 and A27 moved to `partial` (commands and tests only, no UI). WinDirStat partial 15.
- 2026-10-18 · r8: G-017 landed in the engine. A29 moved to `partial` (commands and tests only, no UI). WinDirStat partial 16.
- 2026-10-18 · r9: M4.4 UI part 1. B5 (search UI) and B10 (drive picker) moved to `done` with screenshots; A6 and A32 stay `partial` (exclusion editor and search bar landed; no date filter, editor not yet screenshotted). Recounted from rows: WizTree done 2, partial 4 (B1, B2, B4, B11); WinDirStat unchanged.
- 2026-10-19 · r10: M4.7 UI part 2. Rows 26 (delete to recycle bin), 27 (delete permanently) and 29 (user-defined cleanups) moved `partial` -> `done` with screenshots. Recounted: WinDirStat done 10/34, partial 13.
- 2026-10-19 · r11: M4 code complete (M4.1-M4.10). Rows 25 (reveal), 28 (copy path) and 30 (properties) moved `missing` -> `partial` (commands, UI hooks and tests; real shell calls never run by agents, no screenshots). Row 31 (search by name) moved `missing` -> `done` (same feature and evidence as B5). Rows 26, 27, 29 stay `done`; M4.8 and M4.10 safety fixes (QA-116..QA-146) do not change row status. Fixed the stale Metrics table (it still said 7/34). Recounted from rows: WinDirStat done 11/34 (A1, A3, A10, A11, A13, A21, A26, A27, A29, A31, A33), partial 16 (A4-A9, A17, A18, A20, A22, A23, A25, A28, A30, A32, A34); WizTree unchanged (done 2: B5, B10; partial 4: B1, B2, B4, B11).
- 2026-10-23 · r12: M5.1 (G-018 first half). Row 14 (cushion shading) moved `missing` -> `partial`: implemented and tested, but checked visually on the synthetic demo only, budget not enforced, 1M nodes not re-measured. Row 15 stays `missing` (note added). The G-064, G-065, G-053, G-054 fixes do not change any row. Recounted from rows: WinDirStat done 11/34 (unchanged), partial 17 (A4-A9, A14, A17, A18, A20, A22, A23, A25, A28, A30, A32, A34); WizTree unchanged (done 2, partial 4). Metrics: the MFT scan speed row now quotes the 2026-10-15 measurement, as `ROADMAP.md` already did.
- 2026-10-23 · r13: M5.2 (G-018 second half). Row 15 moved `missing` -> `partial` (look constants and grid user-settable, UI and tests, demo screenshots only). Recounted: WinDirStat done 11/34, partial 18. G-066 and G-067 closed (no parity row).
- 2026-10-23 · r13: M5.3 (G-019 first half). B6 (CSV export) `missing` -> `done`, B7 (treemap image export) `missing` -> `partial`. Recounted: WizTree done 3, partial 5.
