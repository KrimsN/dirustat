# ADR 0005: Layers and the agent development workflow

- Status: Accepted · 2026-10-13

## Context

Development is done by agents (Claude Code, Codex, etc.), possibly in parallel and with interrupted sessions. Strict rails are needed so the code stays consistent without constant human involvement.

## Decision

- **Layers L0-L4 plus apps**, dependencies only downward, checked by `cargo xtask layers`. The layer registry is `xtask/src/layers.rs`.
- **One entry point for agents: `AGENTS.md`** (Claude Code gets it through `CLAUDE.md`). Work state is `plan/STATUS.md`; details are `plan/execution-plan.md`; the journal is `log/devlog.md`.
- **Quality gate** is `cargo xtask ci`: fmt, clippy `-D warnings`, tests, layers, wasm check of L0. The same set runs in GitHub Actions.
- **"Never crash" lints** at workspace level (deny for unwrap/expect/panic/indexing); `unsafe` is forbidden except in `dirustat-ntfs`.
- **Parallel agents:** separate target dirs and branches, new commands in new modules, atomic manifests.
- **Decisions** are ADRs; **progress** is living documents with numbers (`docs/progress-docs.md`).
- **Autonomy:** an agent does not stop to ask questions unless the decision is on the "human decides" list (`AGENTS.md` §7).

## Consequences

- Any agent can pick up work from `plan/STATUS.md` and `log/devlog.md`.
- Layer violations and panics are caught mechanically, not in review.
