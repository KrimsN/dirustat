# ADR 0007: All repository text is English; chat uses the user's language

- Status: Accepted · 2026-10-13 · Supersedes the initial Russian-language convention of M0.

## Context

The first M0 revision wrote documentation and comments in Russian. The project is meant to be open source, worked on by agents from different vendors and by outside contributors; a single repository language avoids mixed-language files and keeps tooling and search predictable.

## Decision

- Everything stored in the repository is English: documentation, plans, ADRs, code comments, identifiers, log and error messages, UI source strings, commit messages, branch names, templates and agent/command definitions.
- Agents talk to the user in the user's language (detected from the user's messages), regardless of the repository language.
- Non-English text found in the repository is translated when the file is touched.

## Consequences

- `AGENTS.md` §0 states the rule; `CLAUDE.md` repeats that it overrides global per-user preferences about repository language.
- UI localization uses English as the source language; other languages are translations (`docs/ui-design.md`).
