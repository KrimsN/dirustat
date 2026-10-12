# Development: build, test, run, debug

> Status: 2026-10-13 · revision 2.

## Tooling

- Stable Rust, pinned by `rust-toolchain.toml` (clippy, rustfmt). MSRV is `rust-version` in the root `Cargo.toml`.
- Windows: MSVC build tools. For the L0 wasm check: `rustup target add wasm32-unknown-unknown`.
- Optional: `cargo-nextest` (fast tests), `cargo-deny` (licences), `cargo-fuzz` (parser fuzzing).

## Main commands

```sh
cargo build --workspace
cargo run -p dirustat                         # the app
cargo run -p dirustat-cli -- <subcommand>     # CLI
cargo test -p <crate> --lib --tests           # fast loop (skips doctests)
cargo nextest run                             # parallel across binaries (if installed)
cargo xtask layers                            # dependency layers
cargo xtask ci                                # fmt + clippy -D warnings + test + wasm-core + layers
cargo deny check                              # licences, sources, banned dependencies
```

## Parallel agents and target dirs

`CARGO_TARGET_DIR=target/agent-<name>`. Delete the directories of finished agents.

## Fixtures

Scanner tests run only on **synthetic trees in temporary directories** (`tempfile`), never on a user's real data. Large trees (1M files) are produced by `cargo xtask fixtures` (arrives in M1; the generator is deterministic by seed). MFT corpora are NTFS images with synthetic content built by an `xtask` script and never committed (`corpus/` is in `.gitignore`).

## Running and driving the app (from M2)

```sh
cargo run --release -p dirustat -- --control 7981 --scan <test-folder>
```

Control channel: JSON lines on `127.0.0.1:7981`; the window key lives in `%APPDATA%\dirustat\control-key.local-7981`. Methods and examples: `docs/control-protocol.md`. Screenshot: `ui.screenshot` → PNG → look at it.

Headless CLI:

```sh
cargo run -p dirustat-cli -- scan <folder> --json out.json
cargo run -p dirustat-cli -- run --scan <folder> --cmd export.csv={"path":"out.csv"}
cargo run -p dirustat-cli -- commands            # list commands with parameter docs
cargo run -p dirustat-cli -- mcp                 # MCP server (stdio)
```

## Debugging

- Logs: `RUST_LOG=dirustat=debug` (the `tracing` crate, added in M1).
- A panic in a test: `RUST_BACKTRACE=1`. A panic outside tests is a never-crash bug; add a regression test.
- Profiling: `cargo xtask perf` (M1+) writes numbers to `perf/`; compare with `perf/budgets.toml`.
- Windows: use `Start-Process -Verb RunAs` to exercise the MFT path with administrator rights; tests that need admin are `#[ignore]` and run explicitly (`-- --ignored`).

## Shell notes (Windows)

The repository is developed on Windows. In bash-like shells long heredocs can break; use the file-writing tool instead. Build paths with `std::path`, never by string concatenation; tests must not depend on the path separator.
