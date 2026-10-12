# Dev log

New entries go on top. Format: `docs/progress-docs.md`.

## 2026-10-13 · M0 · Claude (language switch)
- Landed: all repository text translated to English; `AGENTS.md` §0 now says repository text is English and chat uses the user's language (ADR 0007). Code comments, Cargo descriptions, agent/command definitions, templates and docs updated.
- Verified: `cargo xtask ci` is green after the change.
- Still open: same as the M0 entry below.

## 2026-10-13 · M0 · Claude
- Landed: repository diRUSTat initialized. Workspace (7 crates + 2 apps + xtask), "never crash" lints (deny unwrap/expect/panic/indexing, `unsafe` forbidden except in `dirustat-ntfs`), `cargo xtask layers|ci`, GitHub Actions CI, `deny.toml`, `perf/budgets.toml`. Agent documentation: `AGENTS.md`, `CLAUDE.md`, `docs/*`, ADRs 0001-0006, `plan/*`. Claude Code configuration: `.claude/commands`, `.claude/agents`.
- Basis of the approach: studied the storytold repositories (photocraft, wordcraft, craft-libs): AGENTS.md/CLAUDE.md, layers with xtask, "everything is a command", control channel/MCP, never-crash, devlog and living progress documents. The private `craftrules` repository is unavailable; its standards were reconstructed from links in public files.
- Numbers: tests: model 3, treemap 2, scan/engine/control/ui/ntfs 1-2 each, xtask 1; parity 0/34 and 0/12.
- Verified: `cargo xtask ci` is green (wasm-core skipped locally: target not installed).
- Still open: M1.1 (model), M1 dependencies, a human decision about reserving the name `dirustat`.
