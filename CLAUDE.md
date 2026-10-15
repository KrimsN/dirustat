@AGENTS.md

# Additions for Claude Code

- Session start: read `plan/STATUS.md`, then take a task from `docs/gaps.md`. Project slash commands: `/start-session`, `/finish-task`, `/add-command`, `/perf-check`, `/qa` (`.claude/commands/`).
- Project subagents (`.claude/agents/`): `rust-implementer` (implementation within one crate), `never-crash-reviewer` (review for panics and hostile inputs), `parity-tracker` (updates the parity and progress documents), `qa-code-tester` and `qa-ui-tester` (testers: find bugs, write reports with reproduction steps and severity into `docs/qa/reports/`, never fix; see `docs/qa/README.md`).
- Don't stop to ask questions unless the decision belongs to section 7 of `AGENTS.md`.
- Language: talk to the user in the user's language; everything written into the repository (docs, comments, commits, identifiers) is English (`AGENTS.md` §0). This overrides any global preference for writing repository text in another language.
- Destructive operations on the user's real data are forbidden (`docs/safety.md`).
