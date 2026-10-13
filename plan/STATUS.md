# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-13
- **Milestone:** M1 and M2 done. Starting M3 "Fast MFT". M2 delivered the command registry, menus and palette, control channel, `ui.screenshot` (`docs/img/m1-main.png`), CLI, MCP, settings and launch args; 255 tests pass (`cargo test --workspace`).
- **Tree:** green at the last full run reported by the implementing agents (`cargo xtask ci`: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed, G-030)
- **Next task:** M3.1 Win32 FFI module in `dirustat-ntfs` (isolated unsafe, ADR 0002; G-012). Then MFT record parsing, fixups, data runs, fuzz target.
- **Open leftovers (not blocking M3):** G-011 (drive picker UI, flat list, free-space UI), G-025 (O(n) `tree.children` / `list.largest_files` under the session lock), G-026 (`info --timeout`), G-027 (screenshot encode on the UI thread), G-028 (menus only File/Edit/View), G-029 (tree keyboard navigation), G-030 (wasm check), G-002 (cold-cache scan slowness, `xtask perf`)
- **Blockers:** none
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Parity: WinDirStat 7 done / 11 partial of 34, WizTree 0 done / 3 partial of 12 (see `docs/target-app-parity.md`).
