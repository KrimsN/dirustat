# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-23 (M5.1 cushion shading; QA quick fixes G-064, G-065, G-053, G-054)
- **Milestone:** **M5 in progress** (M5.1 done; M5.2-M5.6 in `plan/execution-plan.md`). M1, M2 and M4 done (M4 with known issues, see `docs/roadmap.md` exit criteria). M3 is code-complete; its exit criterion (1M-file MFT scan within 3 s) was met in 2 of 3 elevated runs on 2026-10-15, the 3.26 s outlier is G-052 and needs another elevated human run.
- **Parity:** WinDirStat 11/34 done (17 partial), WizTree 2/12 done (4 partial) (`docs/target-app-parity.md`; row 14 cushion shading is now partial, totals match the rows).
- **Tests:** `cargo xtask ci` green on 2026-10-23 (750 tests); `wasm-core` skipped locally (the wasm32 target is not installed; GitHub CI runs it), so `cargo check --target wasm32-unknown-unknown` for `model` and `treemap` (the new `cushion.rs`) has not been run on this machine.
- **QA state:**
  - `docs/qa/reports/2026-10-19-m4-code.md` (QA-116..QA-146) and `2026-10-19-m4-ui.md`.
  - `docs/qa/reports/2026-10-23-m4-10-retest.md`: independent retest of QA-139..QA-146: 6 fixed, QA-144 and QA-145 partial (QA-144 completed by G-062, QA-145 bypass = QA-147). QA-147 (S1) fixed (G-063), QA-148 and QA-149 (S4) fixed (G-064, G-065, commit fdc325e). The QA-147 fix, G-062, G-064 and G-065 have not been retested independently. The M5.1 cushion shading has had no QA pass.
- **Open items from M4:**
  - G-059: UI leftovers of QA-138 (menu/shortcut metadata for `platform.reveal`, `platform.properties`, `search.clear`; missing high-contrast theme; low-contrast "Previous scan" overlay).
  - G-060: one extension palette (engine `ext` has 16 colours, UI `ext_colors` has its own).
  - G-016 remainder: date filter, exclusions on the MFT fast path, Preferences screenshot.
  - G-068: ASCII-only case fold in the `fsops` same-directory check (hand-typed non-ASCII case mismatch is refused); residuals of G-065 (file empty in the listing stays stale until a rescan; MFT backend untouched; one extra open per multi-link file, run `/perf-check` on a WinSxS-heavy scan).
  - Under the session lock still: `file.move`, `list.drives` (drive list in the UI, queried on the UI thread).
  - ASCII-only case fold in exclusions, search and request-path matching (deliberate after QA-139).
  - Unverified by agents: MFT backend (hardlink marker, `name_lossy`, name identity), the real recycle bin through the app, `platform.reveal` and `platform.properties` windows, a real 1M-file delete in the window.
- **Open items from M5.1 (G-018 remainder, M5.2 and M5.6):** cushion look constants (height, scale, ambient, diffuse, light) are not user-settable; `treemap.grid` and `show_free_space` are placeholders; about 12 of 2.07M pixels at 1080p show the background colour (non-leaf slivers); budget row `treemap_cushion_shading_100k_nodes_1080p` (25 ms) has `enforce = false` and was not measured on 1M nodes; visual check was done on the synthetic demo screenshot only. G-066: `ColorCache` in `map_panel.rs` has no theme in its key (stale tile colours after a theme switch). G-067: the UI does not show `selection_lost`; duplicate sibling names resolve to the first match.
- **Other open leftovers (not blocking):** G-052 (MFT outlier, then close G-012 and set `enforce = true` in `perf/budgets.toml`; repeat with `cargo run -p dirustat-scan --release --example mft_bench -- Z:\ 3`, elevated, VHDX on an SSD), G-040 (P0, portable scan slowed by hardlink identity), G-045, G-055, G-049/G-050/G-051, G-011, G-025-G-030, G-002, G-031-G-037. Visual re-test of the treemap on maximized `100k-seed1` and `1m-seed1` is still pending (now also with cushion shading).
- **Next task:** M5.2 (cushion parameters in settings, Preferences UI, `treemap.grid`; G-018) or M5.3 (CSV/JSON/PNG export, G-019), then M5.4 snapshots, M5.5 duplicates, M5.6 enforce the cushion budget. G-066 and G-067 are quick wins.
- **Known limits:** extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data streams are not counted in sizes, real elevation is M6. Portable scan on Windows is slower (100k fixture 0.04 s -> about 1.1 s) because of hardlink identity (G-040); G-065 adds one open per multi-link file. The devlog entries for M4.1-M4.10 sit at the bottom of `log/devlog.md` (appended out of order), not on top.
- **Blockers:** none (elevated re-measurements need a human run).
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - release verdict for the destructive commands: the retest said NO-GO only because of QA-147, which is now fixed but not retested; the final go is the human's.
  - cleanup (optional): the four stale agent worktrees under `.claude/worktrees/agent-*` and the agent worktrees created in this session can be removed (all their commits are on `main`, some under new hashes); `target/agent-*` directories of finished agents too.
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Each agent uses its own `CARGO_TARGET_DIR=target/agent-<name>`.
