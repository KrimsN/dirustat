# ROADMAP: diRUSTat

> Status: 2026-10-13 · revision 3 · stage **M1 (core and first window), in progress**. Update together with `docs/roadmap.md` and `docs/target-app-parity.md` (rules: `docs/progress-docs.md`).

## Summary

| Dimension | Value | Source |
|---|---|---|
| WinDirStat feature parity | 0 / 34 done (0 %), 15 partial | `docs/target-app-parity.md` |
| WizTree distinctive features | 0 / 12 done (0 %), 1 partial | `docs/target-app-parity.md` |
| Skeleton crates | 7 + 2 apps + xtask | `docs/architecture.md` |
| Platforms | Windows (primary), Linux/macOS (M6) | `docs/roadmap.md` |
| Scan of 1M files, portable path | warm 1.43 s; first cold-cache scan 125 s (budget ≤ 25 s) | `perf/budgets.toml`, devlog M1.5 |
| Scan of 1M files, MFT | not measured (budget ≤ 3 s) | `perf/budgets.toml` |

## Milestones

| Milestone | Content | Status |
|---|---|---|
| M0 | Bootstrap: workspace, lints, xtask (layers/ci), documentation, CI | **done** |
| M1 | Portable scan + model + treemap + egui window (tree, treemap, extension list) | **in progress** (M1.1-M1.11 landed; screenshot verification pending) |
| M2 | Command registry, control channel, CLI, MCP, screenshots | - |
| M3 | NTFS MFT fast path (Windows), administrator rights, fallback | - |
| M4 | Actions: delete to recycle bin, reveal in file manager, cleanups, search, filters | - |
| M5 | Cushion shading, themes, CSV/JSON export, snapshot comparison, duplicates | - |
| M6 | Linux/macOS, packaging, signed releases | - |
| M7 | Localizations, accessibility, user documentation | - |

## Progress log

- 2026-10-13: M0: repository initialized; crate skeleton with never-crash lints, layers in xtask, CI, documents for agents.
- 2026-10-13: all repository text switched to English (`AGENTS.md` §0).
- 2026-10-13: M1.1-M1.11: model, fixtures, portable scan, panic guard, lossy names ADR 0008, eframe/egui 0.36 shell with tree, treemap and extension panels (90+ tests). Not done: screenshot verification (M2.5), cushion shading, keyboard navigation; parity has 16 partial rows, 0 done.
