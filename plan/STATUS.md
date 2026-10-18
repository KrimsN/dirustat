# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-18
- **Milestone:** M1 and M2 done. M3 "Fast MFT" is code-complete except the M3.5 outlier (G-052). A QA pass (2026-10-15) found 23 bugs; 20 fix commits landed on main on 2026-10-18 (treemap QA-2/QA-1/QA-102, scan QA-3/QA-5, control QA-4/6/7/8/9/112, ui QA-103..109/113/115). Closed gaps: G-038, G-039, G-041, G-043, G-044, G-046, G-047, G-048.
- **Tree:** `cargo xtask ci` reported green by the fixing agents (fmt, clippy `-D warnings`, tests, layers; wasm-core skipped when the target is not installed, G-030).
- **Next task:** (1) visual re-test of the treemap on a maximized window with `100k-seed1` and `1m-seed1` (compare with `docs/qa/evidence/ui-100k-1000.png`; also check scan dimming and the "too small" hint); (2) remainders of G-042 (background layout) and G-040 (hardlink identity cost); (3) G-052 (explain the 3.26 s MFT outlier; budget 3 s met in 2 of 3 runs on NVMe), then close G-012 and set `enforce = true` in `perf/budgets.toml`; (4) M4 (platform services, destructive commands per `docs/safety.md`). Repeat MFT measurements with: `cargo run -p dirustat-scan --release --example mft_bench -- Z:\ 3` (elevated; the fixture VHDX must live on an SSD).
- **Known limits:** extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data streams are not counted in sizes, real elevation is M6. Portable scan on Windows is slower (100k fixture 0.04 s -> about 1.1 s) because of hardlink identity (G-040).
- **Open leftovers (not blocking):** G-045 (allocated size parity), G-053 (tooltip path separator), G-054 (`select.node` cleared after scan), G-055 (QA-115 polish), G-049/G-050/G-051 (QA S4), G-011, G-025-G-030, G-002, G-031-G-037.
- **Blockers:** none (elevated re-measurements need a human run).
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - release verdict from `docs/qa/reports/2026-10-15-summary.md` was NO-GO; the treemap, hardlink and UNC fixes landed, the visual re-test is pending.
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Parity numbers are unchanged by this batch (see `docs/target-app-parity.md`).
