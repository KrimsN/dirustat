# User data safety

> Status: 2026-10-13 · revision 2. Mandatory for humans and agents.

## Principles

1. **Scan means read-only.** No scanning, statistics, search or export command modifies user files. Export writes only to the explicitly given path.
2. **Destructive commands** (`file.delete`, `file.move`, `cleanup.run`) require:
   - a `confirm: true` parameter (without it: `Err(ConfirmationRequired)` plus a description of what would happen);
   - `dry_run: true` returns the affected paths and total size and changes nothing;
   - **recycle bin by default** (Windows Recycle Bin / `trash` on other OSes); permanent deletion is a separate command `file.delete_permanent` with its own confirmation and a UI warning;
   - refusal for protected paths: volume root, `Windows`, `Program Files`, `ProgramData`, an entire user profile, system volumes reached through a reparse point.
3. **Re-check before acting.** Before deleting, the path is re-validated (exists, same file id as in the tree, did not leave the scan root). A stale tree must never delete the wrong thing.
4. **Journal.** Every destructive action is written to a session journal (path, size, time, mode) for recovery from the bin and for debugging.

## Rules for agents

- **Never** run destructive commands on the user's real directories (`C:\Users\...`, Documents, Desktop, external drives). Test only on directories created by the test in a temp folder (`tempfile`) or by `cargo xtask fixtures`.
- Scanning real directories for measurements is allowed only in read-only mode; never save the contents of someone's paths into the repository, devlog, screenshots or fixtures. Report aggregates and synthetic names.
- Never enable automatic deletion "by default" in the UI, CLI or MCP. Destructive MCP tools are marked `destructive: true` and require `confirm`.
- Before any action that cannot be undone (permanent deletion, formatting, writing to system directories), stop and ask the human.

## Privileges

- The fast MFT path needs administrator rights. The app never elevates silently: it shows a request and, if declined, falls back to the portable path. Tests that need admin are `#[ignore]`.
- The app sends no file data over the network. There is no telemetry. Any network feature is a separate human decision and an ADR.

## Privacy in logs

User paths appear in logs only at `debug` level and not in release builds by default. Screenshots and reports meant for publication use synthetic data.
