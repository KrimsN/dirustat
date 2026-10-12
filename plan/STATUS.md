# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-13
- **Milestone:** M1 "Core and first window" in progress (M1.1-M1.11 landed: eframe/egui 0.36 shell, tree, treemap and extension panels; 90+ tests)
- **Tree:** green (`cargo xtask ci` passes: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed)
- **Next task:** M2.1 command registry (`CommandSpec`, `engine/src/cmd/*`). Remaining M1 leftovers: screenshot verification (M1.12) once `ui.screenshot` exists (M2.5), keyboard navigation in the tree, cushion shading (M5), scan using `add_node_from_wide`, cold-cache scan slowness and `xtask perf` (G-002)
- **Blockers:** none
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner;
  - (M1 dependency set confirmed 2026-10-13; `deny.toml` BSL-1.0 / Ubuntu-font-1.0 approved)
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Parity: 0 done, 16 partial (see `docs/target-app-parity.md`); menus are placeholders until M2.2.
