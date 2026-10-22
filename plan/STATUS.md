# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-23 (M4.11: G-062 closed; M4.10 retested; QA-147 S1 fixed)
- **Milestone:** M1, M2 and M4 done (M4 with known issues, see `docs/roadmap.md` exit criteria). M3 is code-complete; its exit criterion (1M-file MFT scan within 3 s) was met in 2 of 3 elevated runs on 2026-10-15, the 3.26 s outlier is G-052 and needs another elevated human run. Next milestone: M5 (cushion shading, export, snapshots, duplicates).
- **Parity:** WinDirStat 11/34 done (16 partial), WizTree 2/12 done (4 partial) (`docs/target-app-parity.md`; recounted by an audit subagent on 2026-10-23, totals match the rows).
- **Tests:** `cargo xtask ci` green on 2026-10-23 (707 tests); `wasm-core` skipped locally (the wasm32 target is not installed; GitHub CI runs it).
- **QA state:**
  - `docs/qa/reports/2026-10-19-m4-code.md` (QA-116..QA-146) and `2026-10-19-m4-ui.md`.
  - `docs/qa/reports/2026-10-23-m4-10-retest.md`: independent retest of QA-139..QA-146: 6 fixed, QA-144 and QA-145 partial (QA-144 completed by G-062, QA-145 bypass = QA-147). New QA-147 (S1) **fixed** (G-063), QA-148 and QA-149 (S4) open as G-064 and G-065. The QA-147 fix and G-062 have not been retested independently.
- **Open items from M4:**
  - G-064 (QA-148): whitespace-only names cannot be deleted; `\n`/`\t` move destination gives `internal`. Fails safe.
  - G-065 (QA-149): hardlinked file changed through another name: stale name refused once, stale size.
  - G-059: UI leftovers of QA-138 (menu/shortcut metadata for `platform.reveal`, `platform.properties`, `search.clear`; missing high-contrast theme; low-contrast "Previous scan" overlay).
  - G-060: one extension palette (engine `ext` has 16 colours, UI `ext_colors` has its own).
  - G-016 remainder: date filter, exclusions on the MFT fast path, Preferences screenshot.
  - Under the session lock still: `file.move`, `list.drives` (drive list in the UI, queried on the UI thread).
  - ASCII-only case fold in exclusions, search and request-path matching (deliberate after QA-139).
  - Unverified by agents: MFT backend (hardlink marker, `name_lossy`, name identity), the real recycle bin through the app, `platform.reveal` and `platform.properties` windows, a real 1M-file delete in the window.
- **Other open leftovers (not blocking):** G-052 (MFT outlier, then close G-012 and set `enforce = true` in `perf/budgets.toml`; repeat with `cargo run -p dirustat-scan --release --example mft_bench -- Z:\ 3`, elevated, VHDX on an SSD), G-040 (P0, portable scan slowed by hardlink identity), G-045, G-053, G-054, G-055, G-049/G-050/G-051, G-011, G-025-G-030, G-002, G-031-G-037. Visual re-test of the treemap on maximized `100k-seed1` and `1m-seed1` is still pending.
- **Next task:** G-064 and G-065 (small, safety-adjacent), then the top unblocked M5 row of `docs/gaps.md` (G-018 cushion shading, G-019 export and snapshots). M5 has no task ids in `plan/execution-plan.md` yet: add them when entering it. Quick wins also available: G-053, G-054.
- **Known limits:** extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data streams are not counted in sizes, real elevation is M6. Portable scan on Windows is slower (100k fixture 0.04 s -> about 1.1 s) because of hardlink identity (G-040). The devlog entries for M4.1-M4.10 sit at the bottom of `log/devlog.md` (appended out of order), not on top.
- **Blockers:** none (elevated re-measurements need a human run).
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - release verdict for the destructive commands: the retest said NO-GO only because of QA-147, which is now fixed but not retested; the final go is the human's.
  - the four stale agent worktrees under `.claude/worktrees/agent-*` (their commits are all on `main` under new hashes) can be removed.
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Each agent uses its own `CARGO_TARGET_DIR=target/agent-<name>`.
