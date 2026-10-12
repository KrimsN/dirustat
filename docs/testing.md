# Testing

> Status: 2026-10-13 · revision 2.

## Pyramid

| Level | What | Where |
|---|---|---|
| Unit | Model, layout, parsers, size arithmetic | `#[cfg(test)] mod tests` in the crate |
| Invariant/property | "area sum = bounds area", "siblings do not overlap", "total = own + sum of children", "parser never panics on arbitrary bytes" | `proptest` (dev dependency, M1) |
| Integration | Scan a synthetic tree in `tempfile`, compare with the expectation | `crates/scan/tests/` |
| Command | Every command: success, edge parameters give `Err`, the hostile test | `crates/engine/src/tests*.rs` |
| Fuzz | MFT record parsing, filters, snapshot import | `crates/*/fuzz/` (`cargo fuzz`) |
| UI snapshots | Offscreen egui render to PNG, compare with a reference within tolerance | `crates/ui/examples/snapshot` (M1) |
| Performance | Measurements against `perf/budgets.toml` | `cargo xtask perf` |

## Rules

1. **Tests are the gate**: a change without tests is not accepted. A crash fix comes with a regression test.
2. **No real user data.** Fixtures are synthetic, deterministic (seeded), in temporary directories, cleaned up automatically.
3. **Platform independence**: do not rely on the path separator, `read_dir` order, name case or admin rights. Platform tests are gated by `cfg`; tests needing privileges are `#[ignore]` with an explanation.
4. **Scan reference**: the test tree is built by a generator with known expected sizes; compare `Tree` with the expectation (names, sizes, file counts), including hardlinks, a symlink cycle, an inaccessible directory, empty directories, unicode and very long paths.
5. **Layout**: check invariants (area sum, bounds, no overlaps within tolerance, monotonicity by weight), degenerate inputs (empty, zeros, NaN, huge weights) and determinism.
6. **MFT parser**: round-trip on synthetic records, broken records (truncated, bad fixup, cyclic attributes), fuzzing; a corpus of real images is never committed.
7. **Hostile commands**: the test walks the registry and feeds junk parameters; it must stay green.
8. **Screenshots**: after UI changes take a PNG and look at it; compare only on synthetic data.

## Running

```sh
cargo test -p <crate>                        # one crate
cargo test --workspace --locked              # everything
cargo test -p dirustat-engine -- --ignored   # heavy/privileged
cargo fuzz run mft_record                    # fuzzing (from the crate directory)
```
