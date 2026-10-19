# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-19 (M4 code complete: M4.1-M4.10, UI M4.9)
- **Milestone:** M1, M2 and M4 done (M4 with known issues, see `docs/roadmap.md` exit criteria). M3 is code-complete; its exit criterion (1M-file MFT scan within 3 s) is not met because the measurement needs an elevated human run (G-052). Next milestone: M5 (cushion shading, export, snapshots, duplicates).
- **Parity:** WinDirStat 11/34 done (16 partial), WizTree 2/12 done (4 partial) (`docs/target-app-parity.md`).
- **QA state:** `docs/qa/reports/2026-10-19-m4-code.md` (QA-116..QA-146, all fixed, regression tests shown to fail without their fixes) and `2026-10-19-m4-ui.md`. The M4.10 fixes (QA-139..QA-146) have not been retested independently.
- **Open items from M4:**
  - G-059: UI leftovers of QA-138 (menu/shortcut metadata for `platform.reveal`, `platform.properties`, `search.clear`; missing high-contrast theme; low-contrast "Previous scan" overlay).
  - G-060: one extension palette (engine `ext` has 16 colours, UI `ext_colors` has its own).
  - G-061 / G-062: permanent delete no longer holds the session lock for `execute_split` hosts (control, MCP, bridge), but the UI must move to `Session::execute_split` on a worker thread for `file.delete_permanent` and `platform.free_space`, and `cleanup.run` with permanent actions needs the same split. Residual of QA-144 (113.6 s lock hold measured for a 1M-file folder before the split).
  - ASCII-only case fold in exclusions, search and request-path matching (non-ASCII case differences are not folded; deliberate after QA-139).
  - Unverified by agents: MFT backend (hardlink marker, `name_lossy`, name identity), the real recycle bin, `platform.reveal` and `platform.properties` windows, real mapped network drives (drive-type classification is tested with a fake provider only). Identity checks are name-based (no file id in the model).
  - G-016 remainder: date filter, exclusions on the MFT fast path, Preferences screenshot.
- **Other open leftovers (not blocking):** G-052 (MFT outlier, then close G-012 and set `enforce = true` in `perf/budgets.toml`; repeat with `cargo run -p dirustat-scan --release --example mft_bench -- Z:\ 3`, elevated, VHDX on an SSD), G-040, G-045, G-053, G-054, G-055, G-049/G-050/G-051, G-011, G-025-G-030, G-002, G-031-G-037. Visual re-test of the treemap on maximized `100k-seed1` and `1m-seed1` is still pending.
- **Next task:** the M4 leftovers above (G-062 first), then the top unblocked M5 row of `docs/gaps.md` (G-018 cushion shading, G-019 export and snapshots).
- **Known limits:** extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data streams are not counted in sizes, real elevation is M6. Portable scan on Windows is slower (100k fixture 0.04 s -> about 1.1 s) because of hardlink identity (G-040). `log/devlog.md` has no entries for M4.1-M4.3, M4.5-M4.10 yet; the report and `git log` hold the facts.
- **Blockers:** none (elevated re-measurements need a human run).
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - release verdict: the destructive commands passed the retest with known issues; the final go for shipping them is the human's.
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Test counts were not re-measured by this documentation update.
