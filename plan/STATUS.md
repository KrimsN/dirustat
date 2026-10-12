# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-13
- **Milestone:** M1 "Core and first window" is not started (M0 is complete)
- **Tree:** green (`cargo xtask ci` passes: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed)
- **Next task:** M1.1 (see `plan/execution-plan.md`): extend the model with logical/allocated size, counters, mtime, attributes, error flag (G-001)
- **Blockers:** none
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - confirming the M1 dependency set (egui/eframe, rayon, serde, tracing).
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language.
