---
description: Add a command to the engine registry following the project checklist
argument-hint: <command id, e.g. scan.cancel>
---

Add the command `$ARGUMENTS` strictly following the "add a command" checklist in `docs/contributing.md`:

1. Check `docs/target-app-parity.md` and `docs/control-protocol.md`: the id and group must follow the conventions.
2. Create a module `crates/engine/src/cmd/<area>.rs` (or extend an existing one) with `specs()`; validate parameters, return `CommandError`, no panics and no indexing.
3. A destructive command? Follow `docs/safety.md`: `confirm`, `dry_run`, protected paths, journal, refusal tests.
4. Tests: success, edge/junk parameters give `Err`; include the command in the hostile test.
5. If it has a menu item/shortcut, add it to `crates/ui/src/menu_catalog.rs`.
6. `cargo test -p dirustat-engine`, `cargo xtask ci`.
7. Update parity/gaps and the devlog.
