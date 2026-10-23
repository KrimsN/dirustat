# Performance

> Status: 2026-10-13 · revision 3. Budgets are in `perf/budgets.toml`; `enforce = false` for now because nothing is measured yet.

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

## Duplicate finder (`duplicates.*`)

- Cost is dominated by disk reads, so the pipeline reads as little as possible: only files that share a logical size are opened, then only their first and last 64 KiB, and the whole file only when those collide (files up to 128 KiB are read once, whole). A tree with no equal sizes never touches the disk.
- Memory: 16 bytes per eligible file for the size sort plus the ids of the candidate runs; one 256 KiB buffer per worker (at most 4 workers); paths are built on demand from the tree (no stored path strings) and only the requested number of groups (default 1000) is kept.
- The job runs on its own thread and never holds the session lock; the UI polls `duplicates.status` once per frame. Cancel is checked per file and per buffer, so it takes effect within one 256 KiB read per worker.
- File identity (for unflagged hardlinks) is queried only for members of confirmed groups, not for every candidate.
- Not measured yet at 1M files or on a spinning disk (several workers can hurt a hard disk; lower `MAX_WORKERS` in `crates/engine/src/duplicates.rs` if so).

## Anti-patterns

Scanning/aggregating/laying out on the UI thread; cloning the tree per frame; a `String` path per node (build paths on demand); sorting the whole tree for a top-N (use partial selection).
