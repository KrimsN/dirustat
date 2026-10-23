# Changelog

Format: [Keep a Changelog](https://keepachangelog.com). Versioning and channels: [docs/releasing.md](docs/releasing.md).

## [0.1.0-alpha.1] - 2026-10-24

First public pre-release. **Alpha: expect rough edges. Do not rely on it for deleting data you cannot restore.**

### Added

- Native desktop app (egui/eframe on wgpu): directory tree, squarified treemap with cushion shading, file-type list with colours, logical/allocated size modes, light and dark themes, command palette.
- Scanning: parallel portable walk and an NTFS MFT fast path on Windows (needs an elevated process; 1M-file scan measured at 0.31 / 0.74 / 3.26 s in three runs on a VHDX).
- Search and filters, cleanups, delete to the recycle bin with confirmation and preview, reveal in the file manager, properties, native folder picker.
- Snapshots (`.dirsnap`) and snapshot comparison, duplicate finder, export, free-space band.
- Every action is a command: JSON control channel, `dirustat-cli` (`scan`, `info`, `run`, `commands`, `mcp`) and an MCP server.
- Never-crash rules: no panics in non-test code, hostile filesystem and MFT input yields errors; last-resort panic guard around command dispatch and scans.

### Known issues

- Unsigned Windows binaries: SmartScreen shows a warning. The GUI opens next to a console window.
- Only the Windows build is published. Linux builds in CI but has not been exercised by hand; macOS is not covered.
- Destructive commands (recycle-bin delete, permanent delete, move): the last safety fix (QA-147, G-063, and the G-062 lock changes) has regression tests but no independent retest; the recycle bin through the running app was verified only on fixtures. Treat delete as experimental and prefer the recycle bin (the default).
- The M5 features (cushion shading, export, snapshots, comparison, duplicates, free-space band) have had no QA pass; their windows were not screenshotted; PNG export ignores the cushion look settings.
- MFT backend: extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data streams are not counted, the 1M-file scan has one outlier above the 3 s budget (G-052). The portable scan on Windows is slower than planned because of hardlink identity (G-040).
- Parity with WinDirStat: 12 of 34 features done, 18 partial; with WizTree: 5 of 12 done, 5 partial (`docs/target-app-parity.md`).
- Name matching in exclusions, search and path requests folds ASCII case only.

Full list: `plan/STATUS.md`, `docs/gaps.md`, `docs/qa/reports/`.
