# ROADMAP: diRUSTat

> Status: 2026-10-13 · revision 2 · stage **M0 (bootstrap)**. Update together with `docs/roadmap.md` and `docs/target-app-parity.md` (rules: `docs/progress-docs.md`).

## Summary

| Dimension | Value | Source |
|---|---|---|
| WinDirStat feature parity | 0 / 34 (0 %) | `docs/target-app-parity.md` |
| WizTree distinctive features | 0 / 12 (0 %) | `docs/target-app-parity.md` |
| Skeleton crates | 7 + 2 apps + xtask | `docs/architecture.md` |
| Platforms | Windows (primary), Linux/macOS (M6) | `docs/roadmap.md` |
| Scan of 1M files, portable path | not measured (budget ≤ 25 s) | `perf/budgets.toml` |
| Scan of 1M files, MFT | not measured (budget ≤ 3 s) | `perf/budgets.toml` |

## Milestones

| Milestone | Content | Status |
|---|---|---|
| M0 | Bootstrap: workspace, lints, xtask (layers/ci), documentation, CI | **done** |
| M1 | Portable scan + model + treemap + egui window (tree, treemap, extension list) | next |
| M2 | Command registry, control channel, CLI, MCP, screenshots | - |
| M3 | NTFS MFT fast path (Windows), administrator rights, fallback | - |
| M4 | Actions: delete to recycle bin, reveal in file manager, cleanups, search, filters | - |
| M5 | Cushion shading, themes, CSV/JSON export, snapshot comparison, duplicates | - |
| M6 | Linux/macOS, packaging, signed releases | - |
| M7 | Localizations, accessibility, user documentation | - |

## Progress log

- 2026-10-13: M0: repository initialized; crate skeleton with never-crash lints, layers in xtask, CI, documents for agents.
- 2026-10-13: all repository text switched to English (`AGENTS.md` §0).
