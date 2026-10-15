# STATUS

> A snapshot for hand-off between agent sessions. Overwrite at the end of every session (it is not a journal; the journal is `log/devlog.md`).

- **Updated:** 2026-10-15
- **Milestone:** M1 and M2 done. M3 "Fast MFT" is code-complete EXCEPT M3.5 (measurement on a real volume). Landed: M3.1 volume FFI, M3.2 safe record parser + `MftReader` + fuzz targets, M3.3 tree from MFT records, M3.4 transparent fallback to the portable scan, M3.6 fast-path status / `prefer_fast_path` / admin hint, never-crash review fixes. `cargo test --workspace` = 381 passed, 3 ignored (ntfs 69 + 1 ignored, scan 57 + 1 ignored, engine 76, ui 57).
- **Tree:** green at the last full run reported by the implementing agents (`cargo xtask ci`: fmt, clippy `-D warnings`, tests, layers; wasm-core is skipped when the target is not installed, G-030)
- **Next task:** G-038 (explain the 3.26 s MFT outlier; budget 3 s met in 2 of 3 runs on NVMe: 0.31 s, 0.74 s), then close G-012 and set `enforce = true` in `perf/budgets.toml`; then M4 (platform services, destructive commands per `docs/safety.md`). Repeat measurements with: `cargo run -p dirustat-scan --release --example mft_bench -- Z:\ 3` (elevated; the fixture VHDX must live on an SSD, an HDD-backed Storage Space measures the disk).
- **M3 result:** MFT path verified on a real NTFS volume (elevated): correct trees, 1M nodes in 0.31-3.26 s on NVMe (phase breakdown in devlog 2026-10-15). Known gaps: extension records (`$ATTRIBUTE_LIST`) are not merged, alternate data streams are not counted in sizes, real elevation is M6.
- **Open leftovers (not blocking):** G-011 (drive picker UI, flat list, free-space UI), G-025 (O(n) `tree.children` / `list.largest_files` under the session lock), G-026 (`info --timeout`), G-027 (screenshot encode on the UI thread), G-028 (menus only File/Edit/View), G-029 (tree keyboard navigation), G-030 (wasm check), G-002 (cold-cache scan slowness, `xtask perf`), G-031-G-037 (M3 review deferrals)
- **Blockers:** none (elevated re-measurements need a human run).
- **Waiting for a human decision:**
  - reserving the name `dirustat` on crates.io / a GitHub organization;
  - publishing the repository and choosing its owner
- **Notes:** environment is Windows 11, Rust 1.96. In bash-like shells long heredocs break; write files with the file-writing tool. Repository text is English (`AGENTS.md` §0); talk to the user in their language. Parity: WinDirStat 7 done / 11 partial of 34, WizTree 0 done / 5 partial of 12 (see `docs/target-app-parity.md`).
