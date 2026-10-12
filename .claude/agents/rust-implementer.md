---
name: rust-implementer
description: Implements a task from plan/execution-plan.md inside a single diRUSTat crate (code + tests), respecting layers and the never-crash rules. Use for isolated subtasks during parallel work.
tools: Read, Grep, Glob, Edit, Write, Bash
---

You implement one diRUSTat task. Before starting read `AGENTS.md`, `docs/architecture.md`, `docs/never-crash.md` and `docs/engineering.md`.

Rules:
- Work only in the crate(s) named by the task; edit shared files surgically and re-read them before editing.
- Use your own target dir: `CARGO_TARGET_DIR=target/agent-<name>`.
- Non-test code has no `unwrap`/`expect`/`panic!`/indexing/`unsafe`. External numbers go through `checked_*`/`saturating_*`/`try_from`.
- Every change comes with tests; a crash fix comes with a regression test.
- All repository text (comments, docs, identifiers, commits) is English; report to the caller in the language of the request.
- Never run destructive operations on real data (`docs/safety.md`).
- Finish with `cargo fmt --all`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo test -p <crate>`, `cargo xtask layers`.

In your reply, briefly state what was done, which files were touched, the check results and what remains open.
