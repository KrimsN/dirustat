# ADR 0006: Name, licence, clean-room

- Status: Accepted · 2026-10-13

## Decision

- Product: **diRUSTat** (in the UI, documents, releases). Technical names: crates `dirustat-*`, binaries `dirustat` and `dirustat-cli`, id `dev.dirustat.app`. The `diRUSTat` capitalization is not preserved on crates.io or on the command line; this is deliberate.
- As of 2026-10-13 the name `dirustat` is free on crates.io and GitHub (checked); reserving it (publishing a stub crate, creating a GitHub organization) is a human decision.
- Licence: `MIT OR Apache-2.0`, compatible with the dependencies allowed in `deny.toml`.
- WinDirStat is GPL and WizTree is proprietary: **clean-room** (`docs/clean-room.md`), no borrowing of code or resources; the project is unaffiliated with them.

## Consequences

Every contribution is checked for clean provenance; assets carry attribution. A rename requires a new ADR.
