# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-13
- **Milestone:** M1 and M2 done. M3 "Fast MFT" in progress (M3.1 done). M2 delivered the command registry, menus and palette, control channel, `ui.screenshot` (`docs/img/m1-main.png`), CLI, MCP, settings and launch args; 255 tests pass (`cargo test --workspace`).
- **Tree:** green at the last full run reported by the implementing agents (`cargo xtask ci`: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed, G-030)
- **Next task:** M3.2 MFT record parsing in `dirustat-ntfs` (safe code: fixups, attributes `$STANDARD_INFORMATION`/`$FILE_NAME`/`$DATA`, data runs, hostile-input limits) with a fuzz target and a property test. Then M3.3 tree from parent ids, M3.4 fallback.
- **M3.1 result:** `Volume::open/data/read_at` (`crates/ntfs/src/volume.rs`: validated geometry, aligned and capped reads) over `win32.rs` (the only `unsafe`). `probe()` returns `Available` only when the volume opens as NTFS. The real read test `reads_the_ntfs_boot_sector` is `#[ignore]` and needs an elevated shell (`cargo test -p dirustat-ntfs -- --ignored`); run here without admin it returns `AccessDenied` as expected, so reading real bytes is not yet verified.
- **Open leftovers (not blocking M3):** G-011 (drive picker UI, flat list, free-space UI), G-025 (O(n) `tree.children` / `list.largest_files` under the session lock), G-026 (`info --timeout`), G-027 (screenshot encode on the UI thread), G-028 (menus only File/Edit/View), G-029 (tree keyboard navigation), G-030 (wasm check), G-002 (cold-cache scan slowness, `xtask perf`)
- **Blockers:** none
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Parity: WinDirStat 7 done / 11 partial of 34, WizTree 0 done / 3 partial of 12 (see `docs/target-app-parity.md`).
