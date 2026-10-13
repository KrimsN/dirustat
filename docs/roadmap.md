# Roadmap

> Status: 2026-10-13 · revision 6 · **Current focus: M3 wrap-up** (fast MFT). M1 and M2 are done; M3 code is done (M3.1-M3.4, M3.6, review fixes) but the exit criterion "measured ≤ 3 s per 1M files" is NOT met: M3.5 needs an elevated human run on a VHDX. Next after that: M4.

| Milestone | Result | Main gaps | Estimate |
|---|---|---|---|
| **M0** Bootstrap | Workspace, lints, xtask, CI, documents, agent instructions | G-000 | **done 2026-10-13** |
| **M1** Core and first window | Parallel scan → model → treemap + tree + extensions in eframe | G-001…G-007, G-024 | **done 2026-10-13** (leftovers: G-029, G-002 cold cache) |
| **M2** Commands and control | Command registry, control, CLI, MCP, screenshots, settings | G-008…G-011 | **done 2026-10-13** (leftover UI: G-011; follow-ups G-025…G-028) |
| **M3** Fast MFT | `dirustat-ntfs`, fallback, administrator rights | G-012, G-013 | **code done 2026-10-13**; measurement (M3.5) pending |
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
- 2026-10-13 · r3: current focus updated after M1.7-M1.11 (M1 exit criterion "screenshot in devlog" still open).
- 2026-10-13 · r4: M1 and M2 marked done; current focus moved to M3.
- 2026-10-13 · r5: M3.1 done; next M3.2.
- 2026-10-13 · r6: M3 code done (M3.2-M3.4, M3.6, review fixes); exit criterion (≤ 3 s per 1M, real volume) still open until M3.5 is measured.
