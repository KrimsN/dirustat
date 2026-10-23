# ROADMAP: diRUSTat

> Status: 2026-10-23 · revision 8 · stage **M5 (visual polish and data) in progress: M5.1 cushion shading landed**; M4 done with known issues; M1, M2 done; M3 code done, measurement pending (M3.5). Update together with `docs/roadmap.md` and `docs/target-app-parity.md` (rules: `docs/progress-docs.md`).

## Summary

| Dimension | Value | Source |
|---|---|---|
| WinDirStat feature parity | 11 / 34 done (32.4 %), 17 partial | `docs/target-app-parity.md` |
| WizTree distinctive features | 2 / 12 done (17 %), 4 partial | `docs/target-app-parity.md` |
| Skeleton crates | 8 + 2 apps + xtask | `docs/architecture.md` |
| Platforms | Windows (primary), Linux/macOS (M6) | `docs/roadmap.md` |
| Scan of 1M files, portable path | warm 1.43 s; first cold-cache scan 125 s (budget ≤ 25 s) | `perf/budgets.toml`, devlog M1.5 |
| Scan of 1M files, MFT | 0.31 / 0.74 / 3.26 s over 3 elevated release runs on a VHDX (budget ≤ 3 s, one outlier: G-052) | devlog 2026-10-15, `perf/budgets.toml` |
| Tests | 750 passed (`cargo xtask ci` green, 2026-10-23; wasm-core skipped locally) | devlog 2026-10-23 |
| Cushion shading, 100k nodes, 1080p | about 12 ms per layout on a background thread; 4K 43.5 ms (budget 25 ms, `enforce = false`) | `perf/budgets.toml`, devlog 2026-10-23 |

## Milestones

| Milestone | Content | Status |
|---|---|---|
| M0 | Bootstrap: workspace, lints, xtask (layers/ci), documentation, CI | **done** |
| M1 | Portable scan + model + treemap + egui window (tree, treemap, extension list) | **done** (screenshot `docs/img/m1-main.png`; keyboard navigation moved to G-029) |
| M2 | Command registry, control channel, CLI, MCP, screenshots | **done** (drive picker UI, flat list and free-space UI remain, G-011) |
| M3 | NTFS MFT fast path (Windows), administrator rights, fallback | **code done**; exit criterion "measured ≤ 3 s per 1M files" NOT yet met (M3.5 needs an elevated run) |
| M4 | Actions: delete to recycle bin, reveal in file manager, cleanups, search, filters | **done 2026-10-19 with known issues**: lock-free deletes in every host since 2026-10-23 (G-062); M4.10 fixes retested 2026-10-23, the S1 it found (QA-147) and both S4 (G-064, G-065) are fixed; MFT backend and real recycle bin unverified by agents |
| M5 | Cushion shading, themes, CSV/JSON export, snapshot comparison, duplicates | **in progress** since 2026-10-23: M5.1 cushion shading done (G-018 first half); M5.2-M5.6 open (`plan/execution-plan.md`) |
| M6 | Linux/macOS, packaging, signed releases | - |
| M7 | Localizations, accessibility, user documentation | - |

## Progress log

- 2026-10-13: M0: repository initialized; crate skeleton with never-crash lints, layers in xtask, CI, documents for agents.
- 2026-10-13: all repository text switched to English (`AGENTS.md` §0).
- 2026-10-13: M1.1-M1.11: model, fixtures, portable scan, panic guard, lossy names ADR 0008, eframe/egui 0.36 shell with tree, treemap and extension panels (90+ tests). Not done: screenshot verification (M2.5), cushion shading, keyboard navigation; parity has 16 partial rows, 0 done.
- 2026-10-13: M2.1-M2.9: command registry, menus and palette, control channel, `ui.screenshot`, CLI, MCP, settings and launch args; 255 tests pass in `cargo test --workspace`. Parity: WinDirStat 7 done, 11 partial; WizTree 0 done, 3 partial. Next: M3.1.
- 2026-10-13: M3.1-M3.6: volume FFI, safe MFT record parser with fuzz targets, MFT reader, tree builder, transparent fallback to the portable scan, fast-path status; never-crash review fixes applied. 381 tests pass, 3 ignored. WizTree row B1 is now partial. Not done: real-volume verification and the 1M-file measurement (M3.5, needs an elevated human run).
- 2026-10-19: M4.1-M4.10: platform services, search, filters, exclusions, destructive commands with safety checks, cleanups, UI for all of them, QA fixes QA-116..QA-146. Parity: WinDirStat 11 done, 16 partial; WizTree 2 done, 4 partial. The test count in the Summary table was not re-measured for this update.
- 2026-10-23: M4.11 (G-062): deletes and cleanups run without the session lock in every host, the app runs them on a worker thread; M4.10 independent retest found QA-147 (S1, symlinked network scan root), fixed the same day; 707 tests pass. Parity unchanged.
- 2026-10-23: M5.1 (G-018 first half): cushion shading with the `treemap.cushion` toggle and a background renderer; QA quick fixes G-064, G-065, G-053, G-054 closed. 750 tests pass. Parity: WinDirStat 11 done, 17 partial (row 14 cushion shading is now partial); WizTree unchanged.
