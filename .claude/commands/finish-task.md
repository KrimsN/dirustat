---
description: Finish a task: quality gate, progress documents, devlog, STATUS
---

Finish the current task:

1. `cargo fmt --all`, then `cargo xtask ci`. If anything is red, fix the cause; do not disable rules or weaken lints.
2. If commands were added or changed, run `hostile_params_never_panic` (once it exists) and check the "add a command" checklist in `docs/contributing.md`.
3. If a hot path changed, measure it (`docs/performance.md`) and record the numbers.
4. If the UI changed, take a screenshot and look at it.
5. Update the progress documents per `docs/progress-docs.md`: `docs/gaps.md`, `docs/target-app-parity.md`, `ROADMAP.md` (the numbers must match), `docs/roadmap.md`.
6. Append an entry to `log/devlog.md` (landed, numbers, verified, still open).
7. Rewrite `plan/STATUS.md`.
8. Commit with a short English message that carries the task id (`M1.2: ...`).
9. Report to the user in the user's language: what was done, what was verified, what is left.
