# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-13
- **Milestone:** M1 "Core and first window" in progress (M1.1-M1.4 done)
- **Tree:** green (`cargo xtask ci` passes: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed)
- **Next task:** M1.5 (see `plan/execution-plan.md`): scan integration tests on `cargo xtask fixtures` trees, measure 100k/1M, fill `perf/budgets.toml` (G-002)
- **Blockers:** none
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - (M1 dependency set confirmed 2026-10-13)
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language.
