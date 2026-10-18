# User data safety

> Status: 2026-10-18 · revision 3. Mandatory for humans and agents.

## Principles

1. **Scan means read-only.** No scanning, statistics, search or export command modifies user files. Export writes only to the explicitly given path.
2. **Destructive commands** (`file.delete`, `file.move`, `cleanup.run`) require:
   - a `confirm: true` parameter (without it: `Err(ConfirmationRequired)` plus a description of what would happen);
   - `dry_run: true` returns the affected paths and total size and changes nothing;
   - **recycle bin by default** (Windows Recycle Bin / `trash` on other OSes); permanent deletion is a separate command `file.delete_permanent` with its own confirmation and a UI warning;
   - refusal for protected paths: volume root, `Windows`, `Program Files`, `ProgramData`, an entire user profile, system volumes reached through a reparse point.
3. **Re-check before acting.** Before deleting, the path is re-validated (exists, same file id as in the tree, did not leave the scan root). A stale tree must never delete the wrong thing.
4. **Journal.** Every destructive action is written to a session journal (path, size, time, mode) for recovery from the bin and for debugging.

## Cleanups (`cleanup.*`, G-017)

- A cleanup is a saved selector plus an action (`recycle`, `permanent`, `empty_dir_contents`, `reveal`). **Nothing runs automatically**: not at startup, not after a scan, not the 3 built-in examples (`temporary-files`, `empty-folders`, `build-output-folders`). Only an explicit `cleanup.run` with `confirm: true` acts; `cleanup.preview` and `dry_run` change nothing.
- `cleanup.run` reuses the `file.*` pipeline per node, with no weaker variant: protected-path refusal (lexical and real location), the stale-tree re-check, recycle bin by default through the platform trait, permanent deletion only with `i_understand_permanent: true`, and one journal entry per node (`op: "cleanup.run"`). Refused nodes are reported with `status` and are never touched.
- Extra guards: the scan root is never selected; a node inside an already selected node is not selected (ancestors only); at most 10000 nodes per run (`truncated`); `only_under` must be an absolute path inside the scan root without `.`/`..`; selectors need at least one real criterion; `empty` folders are re-checked on disk right before acting; a run stops on a missing recycle bin or after 5 consecutive platform failures.
- The tree is pruned once at the end of a run (one revision bump, ids invalid); if pruning fails `rescan_recommended` is set. Agents test cleanups only on `tempfile` trees with `FakePlatform`.

## Rules for agents

- **Never** run destructive commands on the user's real directories (`C:\Users\...`, Documents, Desktop, external drives). Test only on directories created by the test in a temp folder (`tempfile`) or by `cargo xtask fixtures`.
- Scanning real directories for measurements is allowed only in read-only mode; never save the contents of someone's paths into the repository, devlog, screenshots or fixtures. Report aggregates and synthetic names.
- `ui.screenshot` over control never overwrites an existing file unless `overwrite: true`, never creates directories and refuses to scan UNC paths (see `docs/control-protocol.md`); settings and CLI exports are written via temp file plus rename and a settings file from a newer version is never overwritten.
- Never enable automatic deletion "by default" in the UI, CLI or MCP. Destructive MCP tools are marked `destructive: true` and require `confirm`.
- Before any action that cannot be undone (permanent deletion, formatting, writing to system directories), stop and ask the human.

## Privileges

- The fast MFT path needs administrator rights. The app never elevates silently: it shows a request and, if declined, falls back to the portable path. Tests that need admin are `#[ignore]`.
- The app sends no file data over the network. There is no telemetry. Any network feature is a separate human decision and an ADR.

## Privacy in logs

User paths appear in logs only at `debug` level and not in release builds by default. Screenshots and reports meant for publication use synthetic data.

## Implementation (revision 3)

Commands: `file.delete` (recycle bin), `file.delete_permanent` (needs `confirm` and `i_understand_permanent`; UIs must warn first), `file.move` (rename, never overwrites), read-only `file.delete_preview` and `file.journal`. Details and result shapes: `docs/control-protocol.md`.

- Pre-flight for all of them: the target resolves to a node of the current tree; `engine::protected` refuses the volume root, the scan root, `Windows` (and everything below), `Program Files`, `Program Files (x86)`, `ProgramData`, `Users`, an entire user profile, 8.3 spellings and the env-configured roots (`SystemRoot`, `ProgramFiles*`, `ProgramData`, `USERPROFILE`), lexically and again on the canonical location (a link or junction on the way is refused); the object on disk must match the tree (kind, size, mtime; the model stores no file id) or `stale_tree`.
- Links and junctions are never followed; permanent deletion removes the link itself, recycling a link is refused.
- After success the node is cut out of the session tree (arena ids change, revision bumped, selection cleared). Limitation: only the top object is re-validated against the disk, not every descendant.
- Journal: in memory (newest 10000) and optionally appended to a JSONL file chosen by the host (`Session::set_journal_file`); write errors never fail a command. Paths are in the journal by design and never logged.
