# diRUSTat

**diRUSTat** = **Dir** + **Rust** + **Stat**: directory statistics, written in Rust (and a nod to WinDirStat).

A fast disk-space analyzer in pure Rust: "where did my space go?" in the style of WinDirStat, at WizTree speed.

> Status: **alpha** ([0.1.0-alpha.1](CHANGELOG.md)). M1, M2, M4 and M5 done, M3 (NTFS MFT fast path) code-complete. Working desktop app (egui/eframe on wgpu): directory tree, treemap with cushion shading, file-type list, scan, search and filters, cleanups, delete to the recycle bin, snapshots, duplicate finder, command palette, JSON control channel, CLI and MCP server. Next: M6 (Linux/macOS, packaging, signed releases). Details: [ROADMAP.md](ROADMAP.md), [plan/STATUS.md](plan/STATUS.md), release rules: [docs/releasing.md](docs/releasing.md).

## Install (alpha, Windows x86_64)

Download `dirustat-<version>-windows-x86_64.zip` from [Releases](https://github.com/KrimsN/dirustat/releases), check it against the `.sha256` file and unpack. The binaries are unsigned, so SmartScreen may warn. Run `dirustat.exe` (add `--demo` to try it on synthetic data); run it elevated to use the fast NTFS MFT scan. This is an alpha: read the "Known issues" of the release before deleting anything.

![diRUSTat main window: directory tree, treemap and file-type list (synthetic demo data)](docs/img/main-dark.png)

## Goals

- **Feature parity with WinDirStat:** directory tree, treemap with cushion shading, file-type list with colours, cleanups, search, filters.
- **WizTree-class speed:** direct NTFS MFT reading (fast path, Windows) and a parallel portable walk for other filesystems.
- **Native, no shells:** egui/eframe on wgpu. No Tauri/Electron/webview, no JS/TS.
- **Every feature available to agents:** all actions are commands; JSON control channel, CLI and MCP server.
- **Never crashes:** no `unwrap`/`panic`/`unsafe` (except the isolated MFT crate); hostile input yields errors.
- **Data safety:** scanning is read-only; deletion goes through the recycle bin and requires confirmation.

## Quick start

```sh
cargo run --release -p dirustat -- --demo   # desktop app on synthetic demo data
cargo run -p dirustat-cli             # headless CLI
cargo xtask ci                        # full quality gate
```

Requires stable Rust (see `rust-toolchain.toml`). Details: [docs/development.md](docs/development.md).

## For agents and contributors

Start with [AGENTS.md](AGENTS.md) (Claude Code reads `CLAUDE.md`, which imports `AGENTS.md`), then `plan/STATUS.md`. Architecture: [docs/architecture.md](docs/architecture.md); rules: [docs/contributing.md](docs/contributing.md).

## License

MIT OR Apache-2.0 ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)). WinDirStat and WizTree are trademarks of their owners; this project is unaffiliated and implemented clean-room ([docs/clean-room.md](docs/clean-room.md)).
