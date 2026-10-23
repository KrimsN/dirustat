# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-24 (first pre-release `v0.1.0-alpha.1` prepared; rules in `docs/releasing.md`, notes in `CHANGELOG.md`)
- **Releases:** scheme and admission rules per channel are in `docs/releasing.md`; releases are built by `.github/workflows/release.yml` on a `v*` tag. Next pre-release: bump `Cargo.toml`, add a `CHANGELOG.md` section, tag.
- **Milestone:** **M5 done** (G-018, G-019, G-020, G-066, G-067 closed). M1, M2 and M4 done (M4 with known issues, see `docs/roadmap.md` exit criteria). M3 is code-complete; its exit criterion (1M-file MFT scan within 3 s) was met in 2 of 3 elevated runs on 2026-10-15, the 3.26 s outlier is G-052 and needs another elevated human run.
- **Parity:** WinDirStat 12/34 done (18 partial), WizTree 5/12 done (5 partial) (`docs/target-app-parity.md`, the source of truth for totals).
- **Tests:** see the last `cargo xtask ci`. The wasm32 target is now installed, so `wasm-core` runs locally (`cargo check --target wasm32-unknown-unknown` for `model` and `treemap` was green in M5.6).
- **QA state:**
  - `docs/qa/reports/2026-10-19-m4-code.md` (QA-116..QA-146) and `2026-10-19-m4-ui.md`.
  - `docs/qa/reports/2026-10-23-m4-10-retest.md`: QA-147 (S1) fixed (G-063), QA-148 and QA-149 (S4) fixed (G-064, G-065). The QA-147 fix, G-062, G-064 and G-065 have not been retested independently.
  - **M5 has had no QA pass** (cushion shading, look settings, export, snapshots and comparison, duplicate finder, free-space band).
- **Open items from M5:**
  - PNG export (`export.png`) ignores the session cushion look settings; it is unsupported headless.
  - Native open/save dialogs (`GetSaveFileNameW` and the open counterpart) were not exercised by agents in the running app.
  - The duplicates window, the comparison window and the snapshot UI were not screenshotted or QA'd.
  - Duplicate sibling names resolve to the first match (documented, not disambiguated).
  - MFT backend and 1M-node measurements are missing for the M5 features (duplicates, snapshots, comparison, export).
  - Snapshot comparison is single-threaded.
  - Free space is a band, not a tile of the hierarchy; no "unknown space" item; no separate brightness setting.
- **Open items from M4:**
  - G-059: UI leftovers of QA-138 (menu/shortcut metadata for `platform.reveal`, `platform.properties`, `search.clear`; missing high-contrast theme; low-contrast "Previous scan" overlay).
  - G-060: one extension palette (engine `ext` has 16 colours, UI `ext_colors` has its own).
  - G-016 remainder: date filter, exclusions on the MFT fast path, Preferences screenshot.
  - G-068: ASCII-only case fold in the `fsops` same-directory check; residuals of G-065 (file empty in the listing stays stale until a rescan; MFT backend untouched; one extra open per multi-link file, run `/perf-check` on a WinSxS-heavy scan).
  - Under the session lock still: `file.move`, `list.drives` (queried on the UI thread).
  - ASCII-only case fold in exclusions, search and request-path matching (deliberate after QA-139).
  - Unverified by agents: MFT backend (hardlink marker, `name_lossy`, name identity), the real recycle bin through the app, `platform.reveal` and `platform.properties` windows, a real 1M-file delete in the window.
- **Other open leftovers (not blocking):** G-052 (MFT outlier, then close G-012 and set `enforce = true` in `perf/budgets.toml`; repeat with `cargo run -p dirustat-scan --release --example mft_bench -- Z:\ 3`, elevated, VHDX on an SSD), G-040 (P0, portable scan slowed by hardlink identity), G-045, G-055, G-049/G-050/G-051, G-011, G-025-G-030, G-002, G-031-G-037. Visual re-test of the treemap on maximized `100k-seed1` and `1m-seed1` is still pending (now also with cushion shading).
- **Next task:** M6 (Linux/macOS, packaging, signed releases; G-021, G-022), or a QA pass of M5 via `/qa` first.
- **Known limits:** extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data streams are not counted in sizes, real elevation is M6. Portable scan on Windows is slower (100k fixture 0.04 s -> about 1.1 s) because of hardlink identity (G-040). The devlog entries for M4.1-M4.10 sit at the bottom of `log/devlog.md` (appended out of order), not on top.
- **Blockers:** none (elevated re-measurements need a human run).
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - release verdict for the destructive commands: the retest said NO-GO only because of QA-147, which is now fixed but not retested; the final go is the human's.
  - cleanup (optional): the four stale agent worktrees under `.claude/worktrees/agent-*` and the agent worktrees created in this session can be removed (all their commits are on `main`, some under new hashes); `target/agent-*` directories of finished agents too.
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Each agent uses its own `CARGO_TARGET_DIR=target/agent-<name>`.
