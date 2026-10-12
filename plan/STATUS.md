# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-13
- **Milestone:** M1 "Core and first window" in progress (M1.1-M1.8 done; M1.10/M1.11 logic done, rendering pending)
- **Tree:** green (`cargo xtask ci` passes: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed)
- **Next task:** M1.9 tree panel (`ui/src/tree_panel.rs`), then render map (`map_panel.rs`) and extension list (`ext_panel.rs`), then M1.12 screenshot + parity. Open: human decision on `deny.toml` (Ubuntu-font-1.0 for egui default fonts); cold-cache scan slowness, `xtask perf` (G-002)
- **Blockers:** none
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - (M1 dependency set confirmed 2026-10-13)
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language.
