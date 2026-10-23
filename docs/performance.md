# Performance

> Status: 2026-10-23 · revision 4. Budgets are in `perf/budgets.toml`; the two cushion shading rows are measured and `enforce = true`, the others stay `enforce = false` until measured.

## Targets

| Scenario | Budget | Notes |
|---|---|---|
| Scan 1M files, MFT (NTFS, admin) | ≤ 3 s | WizTree is the reference |
| Scan 1M files, portable, SSD | ≤ 25 s | parallel `read_dir`, rayon |
| Treemap layout, 1M nodes | ≤ 150 ms | lazy: lay out only the levels visible at pixel scale |
| Treemap frame, 1M nodes | ≤ 8 ms | no layout per frame, cache by revision |
| First paint after scan start | ≤ 300 ms | the UI stays responsive during a scan |
| Memory | ≤ ~100 bytes/node (+ name) | compact `Node`, no extra `String`s |

## How to measure

- Release builds only. Reference trees come from `cargo xtask fixtures` (deterministic by seed) with 100k / 1M / 5M nodes; for MFT a manual procedure on a test NTFS volume (a VHDX with synthetic data).
- `cargo xtask perf --quick` is a quick pass; `cargo xtask perf --update-baseline` publishes a full run to `perf/baseline.json`.
- Record before/after in `log/devlog.md` (numbers, machine, configuration).
- When a budget is met, set `enforce = true` in `perf/budgets.toml` so CI catches regressions.

## Hot paths and techniques

1. **Scan:** minimize syscalls per file (get the size together with `read_dir`: `FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH` on Windows), batch writes into the arena, no locks on the hot path (per-thread buffers merged at the end).
2. **MFT:** sequential reads in large blocks, parallel record parsing, build paths from parent ids without recursion; fixup verification is mandatory.
3. **Aggregation:** one reverse pass over the arena (already so).
4. **Treemap:** limit layout depth by minimum rectangle size (do not lay out below 1-2 pixels), cache by (revision, size, focus), render as a single mesh/texture with cushion shading, rebuild only when the cache changes.
5. **Tree/list UI:** row virtualization (draw only visible rows), sort in the background.
6. **Memory:** intern extensions, names as `Box<str>`; cluster counts as `u32` units where that saves space.

## Cushion shading (G-018, M5.6)

- `cargo run --release -p dirustat-treemap --example cushion_bench` builds synthetic trees of 100,101 and 1,000,101 nodes (dense ids, skewed weights, 3 levels) and prints layout, surface accumulation, per-pixel shading and the number of pixels no leaf covers.
- Measured 2026-10-23 (Windows 11, release, single thread): 1080p with the default 50,000-tile cap 11.4-11.9 ms for both the 100k and the 1M tree (5.3-5.6 ns/px, surfaces 0.3 ms), so the cost follows the pixels and the tile cap, not the node count; 200,000-tile cap 11.7 ms (100k) / 13.5 ms (1M, 143,705 tiles); 4K 44-45 ms. Layout of the 1M tree at the 50,000-tile cap: 15-17 ms (100k: 5.5 ms). The renderer adds a gap closing pass over the image, 1.3 ms at 1080p. Budget 25 ms (rows `treemap_cushion_shading_100k_nodes_1080p` and `treemap_cushion_shading_1m_nodes_1080p`, both enforced).
- The job runs once per layout on a background thread; a frame only paints the cached texture. It is cancelled early when the revision or the look changes.
- Not measured: a real 1M-file scan in the window, the 4K case against a budget (44 ms is within the 300 ms settle time but has no row).

## Duplicate finder (`duplicates.*`)

- Cost is dominated by disk reads, so the pipeline reads as little as possible: only files that share a logical size are opened, then only their first and last 64 KiB, and the whole file only when those collide (files up to 128 KiB are read once, whole). A tree with no equal sizes never touches the disk.
- Memory: 16 bytes per eligible file for the size sort plus the ids of the candidate runs; one 256 KiB buffer per worker (at most 4 workers); paths are built on demand from the tree (no stored path strings) and only the requested number of groups (default 1000) is kept.
- The job runs on its own thread and never holds the session lock; the UI polls `duplicates.status` once per frame. Cancel is checked per file and per buffer, so it takes effect within one 256 KiB read per worker.
- File identity (for unflagged hardlinks) is queried only for members of confirmed groups, not for every candidate.
- Not measured yet at 1M files or on a spinning disk (several workers can hurt a hard disk; lower `MAX_WORKERS` in `crates/engine/src/duplicates.rs` if so).

## Anti-patterns

Scanning/aggregating/laying out on the UI thread; cloning the tree per frame; a `String` path per node (build paths on demand); sorting the whole tree for a top-N (use partial selection).
