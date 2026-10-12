# Roadmap

> Status: 2026-10-13 · revision 2 · **Current focus: M1** (portable scan + model + treemap + window).

| Milestone | Result | Main gaps | Estimate |
|---|---|---|---|
| **M0** Bootstrap | Workspace, lints, xtask, CI, documents, agent instructions | G-000 | **done 2026-10-13** |
| **M1** Core and first window | Parallel scan → model → treemap + tree + extensions in eframe | G-001…G-007, G-024 | 2-3 weeks |
| **M2** Commands and control | Command registry, control, CLI, MCP, screenshots, settings | G-008…G-011 | 2 weeks |
| **M3** Fast MFT | `dirustat-ntfs`, fallback, administrator rights | G-012, G-013 | 2-3 weeks |
| **M4** Actions | Recycle bin, file manager, cleanups, search, filters, safe deletion | G-014…G-017 | 2 weeks |
| **M5** Visual polish and data | Cushion, export, snapshots, comparison, duplicates | G-018…G-020 | 2-3 weeks |
| **M6** Cross-platform and release | Linux/macOS, packaging, signed releases | G-021, G-022 | 2 weeks |
| **M7** Product | Localizations, accessibility, user documentation | G-023 | 2 weeks |

## Exit criteria

- **M1:** `cargo run -p dirustat` opens a window, a scan of a test folder is shown as a tree and treemap, selection is synchronized, the UI does not block, `cargo xtask ci` is green, a screenshot is attached to the devlog, a scan of 1M synthetic files is measured.
- **M2:** every command is callable from the UI, CLI, control and MCP; the hostile test is green; an agent takes a screenshot through `ui.screenshot`.
- **M3:** a 1M-file MFT scan takes ≤ 3 s (measured in the devlog); without rights there is a transparent fallback; MFT parser fuzzing finds no crashes.
- **M4:** destructive commands satisfy `docs/safety.md`, with refusal tests.
- **M5:** cushion shading is within the frame budget; snapshots are saved and compared.
- **M6:** CI is green on Windows/Linux/macOS and a release builds automatically.
- **M7:** localizations and an accessibility checklist.

## Revision history

- 2026-10-13 · r1: milestones M0-M7.
- 2026-10-13 · r2: translated to English.
