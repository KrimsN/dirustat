# Parity with WinDirStat and WizTree

> Status: 2026-10-13 · revision 4 · WinDirStat done 7/34 (partial 11), WizTree done 0/12 (partial 3). Behaviour was determined from user documentation and use of the programs as black boxes (`docs/clean-room.md`).

Statuses: `missing` · `partial` · `done`. When you change a row, update the date, recount the totals in the header and in `ROADMAP.md`, and extend the revision history. Identifiers are command ids (`docs/control-protocol.md`).

## A. WinDirStat (34)

| # | Feature | Command / area | Milestone | Status |
|---|---|---|---|---|
| 1 | Scan a chosen folder | `scan.start` | M1 | done 2026-10-13 (registered command + path bar + Scan button, non-blocking; CLI `scan`; screenshot `docs/img/m1-main.png`) |
| 2 | Choose drives (all local / specific) | `scan.start_drives` | M1 | missing |
| 3 | Cancel a scan | `scan.cancel` | M1 | done 2026-10-13 (registered command + Stop button; cancellation tests) |
| 4 | Rescan / refresh | `scan.rescan` | M1 | partial (Scan on the same path; no dedicated rescan command) |
| 5 | Progress and estimate display | status bar | M1 | partial (status bar progress; no estimate) |
| 6 | Scan exclusions (masks, paths) | `scan.exclude` | M4 | missing |
| 7 | Expand/collapse the tree | `tree.expand` / `tree.collapse` | M1 | partial (mouse only, virtualized; no keyboard navigation; not commands yet) |
| 8 | Sort by column | `tree.sort` | M1 | partial (sortable headers; not a command yet) |
| 9 | Columns: name, share (bar), %, size, items, files, folders, modified, attributes | `tree.columns` | M1 | partial (name, size, % of parent, files, mtime; no share bar, items, folders, attributes) |
| 10 | Selection sync tree ↔ treemap ↔ extensions | `select.node` | M1 | done 2026-10-13 (registered `select.node` / `select.clear`; tree, map and extension list synced; screenshot `docs/img/m1-main.png`) |
| 11 | Size mode: logical / on disk | `view.size_mode` | M1 | done 2026-10-13 (registered command, toggle in the UI, `--size-mode` launch flag, persisted in settings; tests) |
| 12 | "Free space" and "unknown" items | `view.free_space` | M2 | missing |
| 13 | Squarified treemap | `treemap.render` | M1 | done 2026-10-13 (layout invariants + extension-coloured rendering, hover tooltip; flat, cushion is row 14; screenshot `docs/img/m1-main.png`) |
| 14 | Cushion shading | `treemap.cushion` | M5 | missing |
| 15 | Treemap options (grid, brightness, height, scale, light) | `treemap.options` | M5 | missing |
| 16 | Zoom into/out of a directory | `view.zoom_in` / `view.zoom_out` | M2 | missing |
| 17 | Highlight the selection on the treemap | `treemap.highlight` | M1 | partial (selection synced to the map; not visually verified) |
| 18 | Select by clicking the treemap | `treemap.pick` | M1 | partial (click select via `treemap::hit_test`; not a command yet) |
| 19 | Show/hide the treemap | `view.toggle_treemap` | M1 | missing |
| 20 | File-type list (extension, size, count, %) | `ext.list` | M1 | partial (swatch, count, size, percent, selection; not a command yet; no screenshot) |
| 21 | File-type colours | `ext.colors` | M1 | done 2026-10-13 (deterministic colours in `engine::ext`; used by map and list; tests; screenshot `docs/img/m1-main.png`) |
| 22 | Highlight an extension's files on the treemap | `ext.highlight` | M2 | partial (selected-extension highlight on the map landed early in M1.10; not a command yet) |
| 23 | Sort the extension list | `ext.sort` | M1 | partial (sortable columns; not a command yet) |
| 24 | Open a file | `file.open` | M4 | missing |
| 25 | Reveal in file manager | `file.reveal` | M4 | missing |
| 26 | Delete to recycle bin | `file.delete` | M4 | missing |
| 27 | Delete permanently | `file.delete_permanent` | M4 | missing |
| 28 | Copy path | `file.copy_path` | M4 | missing |
| 29 | User-defined cleanups | `cleanup.run` | M4 | missing |
| 30 | File/folder properties | `file.properties` | M4 | missing |
| 31 | Search by name | `search.find` | M4 | missing |
| 32 | Filter by name/size/date | `filter.set` | M4 | missing |
| 33 | Persist settings | `settings.get` / `settings.set` / `settings.reset` | M2 | done 2026-10-13 (serde settings, atomic save, corrupt-file handling; tests) |
| 34 | Launch from the command line with a path | `dirustat --scan <path>` | M2 | partial (`LaunchArgs`: `--scan`, `--demo`, `--size-mode`, `--screenshot`, `--control`; no bare positional path) |

## B. Distinctive WizTree features (12)

| # | Feature | Command / area | Milestone | Status |
|---|---|---|---|---|
| 1 | Scan through the MFT | `scan.start` (backend `mft`) | M3 | missing |
| 2 | Ask for administrator rights and fall back to a normal scan | `scan.elevate` | M3 | partial 2026-10-13 (awareness only: `scan.fast_path_status`, status-bar hint, `ui.relaunch_elevated_help`; real elevation left for M6 packaging) |
| 3 | Flat list of all files | `view.file_list` | M2 | missing |
| 4 | Top largest files | `list.largest_files` | M2 | partial (engine command with tests, callable from CLI/control/MCP; no UI panel; O(n) per call, see G-025) |
| 5 | Search with wildcard/regex | `search.find` (mode) | M4 | missing |
| 6 | CSV export | `export.csv` | M5 | missing |
| 7 | Treemap image export | `export.image` | M5 | missing |
| 8 | Save/load scan snapshots | `snapshot.save` / `snapshot.load` | M5 | missing |
| 9 | Compare snapshots | `snapshot.compare` | M5 | missing |
| 10 | Drive picker with fill bars | `list.drives` | M2 | partial (engine drives provider, default empty; real Win32 enumeration and free space need platform services, M4/G-014; no picker UI) |
| 11 | Scan non-NTFS / network paths | `scan.start` (backend `portable`) | M1 | partial (portable scan shown in the window, `scan.start` is a command; no network-path check) |
| 12 | Portable mode (no install, settings alongside) | `settings.portable` | M6 | missing |

## C. Beyond parity (not counted in percentages)

Duplicate file finder (size → partial hash → full hash), change history between snapshots with a chart, dark/high-contrast themes, a command palette, agent control (control/MCP), CLI export, cleanup templates, "what can be deleted" hints (caches, temp files).

## D. Metrics

| Metric | Value |
|---|---|
| WinDirStat, `done` | 7 / 34 (partial 11) |
| WizTree, `done` | 0 / 12 (partial 3) |
| MFT scan speed (1M files) | not measured |

## Revision history

- 2026-10-13 · r1: initial matrix.
- 2026-10-13 · r2: translated to English.
- 2026-10-13 · r3: after M1.7-M1.11, rows A1, A3, A4, A5, A7-A10, A13, A17, A18, A20-A23, B11 moved to `partial`; none is `done` because there is no screenshot verification (`ui.screenshot` arrives in M2.5), the actions are not commands yet (M2.2), and cushion shading, keyboard navigation and some columns are missing.
- 2026-10-13 · r4: after M2, screenshot `docs/img/m1-main.png` exists and actions are registered commands. A1, A3, A10, A11, A13, A21, A33 moved to `done`; A34, B4, B10 moved to `partial`. Other partial rows stay partial (no commands for tree/ext actions, no keyboard navigation, no cushion, no UI for largest files/drives).
