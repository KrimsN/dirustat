# Execution plan

> The detailed plan for agents. Tasks are numbered `M<milestone>.<n>` and reference gaps from `docs/gaps.md`.

## Autonomous operation protocol

1. **Start.** Read `AGENTS.md`, `plan/STATUS.md` and the recent entries of `log/devlog.md`. Make sure the tree is green: `cargo xtask ci`. If it is red, fix it first (unless it is red because of someone else's unfinished edit: wait and retry).
2. **Pick a task.** The next one in STATUS; if it is blocked, the next by `docs/gaps.md` in the same milestone.
3. **Work in small steps.** Each step: code + test → `cargo fmt` → clippy → test → commit (`M1.2: ...`). Do not accumulate large unverified changes.
4. **Decisions.** If an architectural decision is needed that no ADR covers, make it yourself, write a short new ADR and move on. Ask the human only about section 7 of `AGENTS.md`.
5. **UI verification.** After interface changes take a screenshot and look at it (`docs/ui-design.md`).
6. **Finish.** `cargo xtask ci` → update parity/gaps/ROADMAP if numbers moved → entry in `log/devlog.md` → rewrite `plan/STATUS.md` → commit.
7. **Session interruption.** Allowed at any moment: rules 3 and 6 guarantee the next agent continues from STATUS and the devlog.
8. **Data safety.** No destructive operations on real data (`docs/safety.md`).
9. **Language.** Repository text is English; talk to the user in the user's language (`AGENTS.md` §0).

## M1: Core and first window

| Task | Gap | Content |
|---|---|---|
| M1.1 | G-001 | Model: logical/allocated size, `files`/`dirs` counters, mtime, attributes, access-error flag; tests, saturation |
| M1.2 | G-004 | Add dependencies (`rayon`, `serde`, `serde_json`, `tracing`; dev: `tempfile`, `proptest`); update the table in `docs/engineering.md`; `cargo deny check` |
| M1.3 | G-003 | `cargo xtask fixtures`: deterministic tree generator (seed; 100k/1M/5M), hardlinks/symlink cycle/empty folders/unicode/long paths |
| M1.4 | G-002 | Portable scan: `scan::start(root, opts) -> ScanHandle` (progress, cancellation, errors on nodes, hardlinks, reparse points, iterative walk) |
| M1.5 | G-002 | Scan integration tests on fixtures; measure 100k/1M, record in the devlog; fill `perf/budgets.toml` with the measured values |
| M1.6 | G-007 | Last-resort guard (`catch_unwind`) around scans and commands |
| M1.7 | G-024 | ADR: storing names with invalid UTF-16 (lossy + flag) |
| M1.8 | G-005 | eframe window: app skeleton, status bar, scan launch, theme from `Tokens` |
| M1.9 | G-005 | Directory tree with columns, virtualization, sorting |
| M1.10 | G-006 | Treemap: layout cached by revision, rendering, hit-testing, highlight, selection sync |
| M1.11 | G-005 | Extension list: aggregation, colours, sorting |
| M1.12 | - | Screenshot, devlog, parity update (rows A1, A3, A4, A5, A7-A11, A13, A17-A21, A23, B11) |

## M2: Commands and control (brief)

M2.1 `CommandSpec` and registry (`engine/src/cmd/*`); M2.2 move M1 actions into commands; M2.3 hostile test; M2.4 control channel and key; M2.5 offscreen `ui.screenshot`; M2.6 CLI (`scan/info/run/commands`); M2.7 MCP; M2.8 settings (serde) and launch arguments; M2.9 drive picker, flat list, top-N, zoom, free space.

## M3: Fast MFT (brief)

M3.1 Win32 FFI module (open the volume, read blocks), the only unsafe; M3.2 MFT record parsing (fixups, attributes, data runs) + fuzzing; M3.3 build the tree from parent ids; M3.4 fallback when rights are missing; M3.5 measure on a VHDX with 1M files; M3.6 administrator rights request in the UI.

## M4

M4.1-M4.11 are described in `log/devlog.md` (platform services, search and filters, destructive commands, cleanups, UI, QA fixes, lock-free deletes).

## M5: Visual polish and data

- **M5.1** (done 2026-10-23, G-018 first half): cushion shading (van Wijk algorithm, `crates/treemap/src/cushion.rs`), background renderer `crates/ui/src/cushion_view.rs`, command `treemap.cushion` (View > Cushion shading), setting `treemap.cushion`, budget row `treemap_cushion_shading_100k_nodes_1080p` (`enforce = false`).
- **M5.2** (G-018 remainder): cushion parameters (height, scale, ambient, diffuse, light) in settings, Preferences UI, `treemap.grid`.
- **M5.3** (G-019): CSV/JSON/PNG export, including the PNG of the shaded treemap reusing `cushion_view::render`.
- **M5.4** (G-019): scan snapshots save/load and comparison.
- **M5.5** (G-020): duplicate finder.
- **M5.6** (G-018): enforce the cushion budget, re-measure on 1M nodes, fill sub-pixel slivers, run the wasm32 check.

## M6-M7

See `docs/roadmap.md` and `docs/gaps.md`; task detail is added to this file when a milestone is entered.
