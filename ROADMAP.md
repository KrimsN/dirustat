# ROADMAP: diRUSTat

> Status: 2026-10-13 · revision 5 · stage **M3 (fast MFT), code done, measurement pending (M3.5)**; M1 and M2 done. Update together with `docs/roadmap.md` and `docs/target-app-parity.md` (rules: `docs/progress-docs.md`).

## Summary

| Dimension | Value | Source |
|---|---|---|
| WinDirStat feature parity | 7 / 34 done (20.6 %), 16 partial | `docs/target-app-parity.md` |
| WizTree distinctive features | 0 / 12 done (0 %), 5 partial | `docs/target-app-parity.md` |
| Skeleton crates | 7 + 2 apps + xtask | `docs/architecture.md` |
| Platforms | Windows (primary), Linux/macOS (M6) | `docs/roadmap.md` |
| Scan of 1M files, portable path | warm 1.43 s; first cold-cache scan 125 s (budget ≤ 25 s) | `perf/budgets.toml`, devlog M1.5 |
| Scan of 1M files, MFT | not measured (budget ≤ 3 s); needs an elevated run on a VHDX | `perf/budgets.toml` |
| Tests | 381 passed, 3 ignored (`cargo test --workspace`) | devlog M3.2-M3.6 |

## Milestones

| Milestone | Content | Status |
|---|---|---|
| M0 | Bootstrap: workspace, lints, xtask (layers/ci), documentation, CI | **done** |
| M1 | Portable scan + model + treemap + egui window (tree, treemap, extension list) | **done** (screenshot `docs/img/m1-main.png`; keyboard navigation moved to G-029) |
| M2 | Command registry, control channel, CLI, MCP, screenshots | **done** (drive picker UI, flat list and free-space UI remain, G-011) |
| M3 | NTFS MFT fast path (Windows), administrator rights, fallback | **code done**; exit criterion "measured ≤ 3 s per 1M files" NOT yet met (M3.5 needs an elevated run) |
| M4 | Actions: delete to recycle bin, reveal in file manager, cleanups, search, filters | - |
| M5 | Cushion shading, themes, CSV/JSON export, snapshot comparison, duplicates | - |
| M6 | Linux/macOS, packaging, signed releases | - |
| M7 | Localizations, accessibility, user documentation | - |

## Progress log

- 2026-10-13: M0: repository initialized; crate skeleton with never-crash lints, layers in xtask, CI, documents for agents.
- 2026-10-13: all repository text switched to English (`AGENTS.md` §0).
- 2026-10-13: M1.1-M1.11: model, fixtures, portable scan, panic guard, lossy names ADR 0008, eframe/egui 0.36 shell with tree, treemap and extension panels (90+ tests). Not done: screenshot verification (M2.5), cushion shading, keyboard navigation; parity has 16 partial rows, 0 done.
- 2026-10-13: M2.1-M2.9: command registry, menus and palette, control channel, `ui.screenshot`, CLI, MCP, settings and launch args; 255 tests pass in `cargo test --workspace`. Parity: WinDirStat 7 done, 11 partial; WizTree 0 done, 3 partial. Next: M3.1.
- 2026-10-13: M3.1-M3.6: volume FFI, safe MFT record parser with fuzz targets, MFT reader, tree builder, transparent fallback to the portable scan, fast-path status; never-crash review fixes applied. 381 tests pass, 3 ignored. WizTree row B1 is now partial. Not done: real-volume verification and the 1M-file measurement (M3.5, needs an elevated human run).
