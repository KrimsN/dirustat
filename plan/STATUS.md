# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-13
- **Milestone:** M1 and M2 done. M3 "Fast MFT" is code-complete EXCEPT M3.5 (measurement on a real volume). Landed: M3.1 volume FFI, M3.2 safe record parser + `MftReader` + fuzz targets, M3.3 tree from MFT records, M3.4 transparent fallback to the portable scan, M3.6 fast-path status / `prefer_fast_path` / admin hint, never-crash review fixes. `cargo test --workspace` = 381 passed, 3 ignored (ntfs 69 + 1 ignored, scan 57 + 1 ignored, engine 76, ui 57).
- **Tree:** green at the last full run reported by the implementing agents (`cargo xtask ci`: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed, G-030)
- **Next task:** M3.5 remainder: a release-build timing of the MFT scan on a VHDX with 1M files (elevated human run; the correctness runs `reads_the_ntfs_boot_sector` and `real_volume_scan_uses_the_mft` already passed on a real `C:`, see devlog). Then M4.
- **M3 result:** the MFT path is covered by tests on synthetic images only; it has never read a real volume. Not measured: MFT scan speed. Known gaps: extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data stream sizes, G-031-G-037 (review deferrals).
- **Open leftovers (not blocking):** G-011 (drive picker UI, flat list, free-space UI), G-025 (O(n) `tree.children` / `list.largest_files` under the session lock), G-026 (`info --timeout`), G-027 (screenshot encode on the UI thread), G-028 (menus only File/Edit/View), G-029 (tree keyboard navigation), G-030 (wasm check), G-002 (cold-cache scan slowness, `xtask perf`), G-031-G-037 (M3 review deferrals)
- **Blockers:** the 1M-file timing requires an elevated human run (agents cannot elevate).
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Parity: WinDirStat 7 done / 11 partial of 34, WizTree 0 done / 5 partial of 12 (see `docs/target-app-parity.md`).
