# ADR 0003: Everything is a command

- Status: Accepted · 2026-10-13

## Context

Every human action must be available to an agent (Claude, Codex, MCP clients), testable without a GUI, and must not diverge between the UI and the CLI.

## Decision

Every user-visible behaviour is a `CommandSpec` in `dirustat-engine`: id (`group.action`), label, menu path, shortcut, parameter docs, `enabled`, `run`. UI, CLI, control channel and MCP invoke commands by id. A programmatic call never opens a dialog. Shell-only view state (panels, zoom) lives in separate `ui.*` commands in `dirustat-ui`. Destructive commands are marked and require `confirm` (`docs/safety.md`).

## Consequences

- One registry gives menus, a command palette, `dirustat-cli commands` and MCP schemas for free.
- The hostile test walks the registry and guarantees no panics.
- Every new feature starts with a command and tests, not a button.
