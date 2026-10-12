---
description: Start a diRUSTat development session: orient and pick a task
---

Start a work session following the project protocol:

1. Read `AGENTS.md`, `plan/STATUS.md` and two or three recent entries of `log/devlog.md`.
2. Run `cargo xtask ci`. If it is red, find out whether the cause is yours and fix the tree first.
3. Pick the next task: from STATUS, otherwise the top unblocked item of `docs/gaps.md` in the current milestone (`docs/roadmap.md` › Current focus).
4. Briefly tell the user, in the user's language: milestone, task, a 3-5 step plan, which documents you will touch. Do not wait for confirmation unless the decision belongs to section 7 of `AGENTS.md`.
5. Work in small steps: code + test → fmt → clippy → test → commit.

Repository text is English; talk to the user in the user's language.

User arguments (if any): $ARGUMENTS
