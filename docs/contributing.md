# Contributing

> Status: 2026-10-13 · revision 2.

- **Language of code:** Rust only. No JS/TS. If a web build appears, it uses `wasm-bindgen` with no hand-written JS.
- **Language of text:** everything in the repository is English (docs, comments, messages, commits, names). Conversation with the user happens in the user's language (`AGENTS.md` §0).
- **Licence:** contributions are `MIT OR Apache-2.0` (`LICENSE-MIT`, `LICENSE-APACHE`).
- **Clean-room:** do not copy code, icons, strings or resources from WinDirStat (GPL), WizTree or other programs. `docs/clean-room.md`.
- **Never crash:** `docs/never-crash.md`. No `unwrap`/`expect`/`panic!`/indexing/`unsafe` in non-test code (exception: `dirustat-ntfs`).
- **Commands, not handlers:** a new feature is a command in `engine` with tests; the UI calls it.
- **Layers:** `cargo xtask layers` must pass; register new crates in `xtask/src/layers.rs`.
- **Data safety:** `docs/safety.md` for anything that writes to disk.

## Commits and branches

- Branch: `agent/<name>/<task>` or `feat/<topic>`. Changes reach `main` via PR with green CI.
- Commit message: short English imperative with a task id: `M1.3: add scan progress channel`.
- Commit after every finished arc of work that builds. Never commit a broken tree.
- Do not use `--no-verify` and do not rewrite shared history.
- A co-author trailer for the agent is added as configured by the tool (`Co-Authored-By: ...`).

## Style

- `cargo fmt` (`rustfmt.toml`: `max_width = 120`). Clippy warnings are errors (`-D warnings`).
- Comments explain **why**, not **what**. No narrative comments such as "now we do X". Doc comments (`///`) on public API.
- Keep the public API minimal: private fields, invariants held inside the type.
- Keep functions short; over ~60 lines is a reason to split. Nesting of `match`/`if` no deeper than three levels.
- Names: `snake_case` functions, `UpperCamelCase` types, command ids `group.action`.

## Checklist: add a command

1. Choose an id (`group.action`) and add it to the menu catalog `ui/src/menu_catalog.rs` (if it has a menu item/shortcut).
2. Create or extend the module `engine/src/cmd/<area>.rs`: a `CommandSpec` with label, menu path, shortcut, `params_doc`, `enabled`, `run`.
3. Validate parameters: ranges, empty values, wrong types give `CommandError::InvalidParams`, never a panic.
4. Tests: the success case; "edge parameters give Err"; for destructive commands, dry-run and refusal without confirmation.
5. Register `specs()` in the registry (`engine/src/cmd/mod.rs`), a small surgical edit.
6. `cargo test -p dirustat-engine` (includes `hostile_params_never_panic`).
7. If the command moves parity, update `docs/target-app-parity.md` and `docs/gaps.md` (date, numbers).
8. Append to `log/devlog.md`.

## Checklist: add a crate

Directory `crates/<name>`, package `dirustat-<name>`, package fields from `workspace.package`, `[lints] workspace = true`, an entry in `[workspace.dependencies]` of the root `Cargo.toml`, a layer in `xtask/src/layers.rs`, a row in `docs/architecture.md`. Create the manifest atomically (`AGENTS.md` §6).

## Assets

Icons and images are original (drawn in code, preferably), CC0/public domain, or OSI-compatible. Every file gets a row in `ATTRIBUTION.md` (path, title, author, source, licence) in the same commit. Fonts are never added to the repository: use system fonts and, if needed, an external fonts repository as an optional build input.

## Pull requests

Description: what and why, a link to the task in `docs/gaps.md`, the `cargo xtask ci` result, a screenshot for UI changes, the progress documents touched. Template: `.github/PULL_REQUEST_TEMPLATE.md`.
