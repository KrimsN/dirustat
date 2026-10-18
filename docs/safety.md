# User data safety

> Status: 2026-10-18 · revision 4. Mandatory for humans and agents.

## Principles

1. **Scan means read-only.** No scanning, statistics, search or export command modifies user files. Export writes only to the explicitly given path.
2. **Destructive commands** (`file.delete`, `file.move`, `cleanup.run`) require:
   - a `confirm: true` parameter (without it: `Err(ConfirmationRequired)` plus a description of what would happen);
   - `dry_run: true` returns the affected paths and total size and changes nothing;
   - **recycle bin by default** (Windows Recycle Bin / `trash` on other OSes); permanent deletion is a separate command `file.delete_permanent` with its own confirmation and a UI warning;
   - refusal for protected paths (exact list in "Implementation" below): volume root, system directories and their subtrees, an entire user profile, system volumes reached through a reparse point, network scan roots.
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

## Implementation (revision 4)

Commands: `file.delete` (recycle bin), `file.delete_permanent` (needs `confirm` and `i_understand_permanent`; UIs must warn first), `file.move` (no-replace rename), read-only `file.delete_preview` and `file.journal`. Details and result shapes: `docs/control-protocol.md`.

- **Addressing.** By `path`, or by `id` plus the `revision` at which the id was read; a different current revision is `stale_tree` with no side effects (arena ids change on every prune, so a bare id could name another file).
- **Pre-flight** for all of them: the target resolves to a node of the current tree; `engine::protected` refuses (lexically and again on the canonical location; a link or junction on the way is refused):
  - the volume root and the scan root;
  - the whole subtree of `Windows`, `Recovery`, `Boot`, `EFI`, `$Recycle.Bin`, `System Volume Information`, and of the env-configured `SystemRoot`/`windir`;
  - `Program Files`, `Program Files (x86)`, `ProgramData`, `Users`, `Documents and Settings`, `PUBLIC`, `ALLUSERSPROFILE` themselves **and their direct children** (`C:\Program Files\<product>`, `C:\ProgramData\<vendor>`); deeper levels are allowed;
  - an entire user profile (`C:\Users\<name>`), and inside a profile `AppData` with its first level (`Local`, `Roaming`, `LocalLow`) and `NTUSER.DAT*` (also for the env `USERPROFILE`); `AppData\Local\Temp` and deeper stay allowed;
  - 8.3 spellings of those names, trailing dots and spaces of components (Windows ignores them), `.`/`..`;
  - administrative UNC shares: `\host\C$\...` is checked as `C:\...`, `\host\ADMIN$` as the Windows directory, other `$` shares are refused.
- **Network scan roots.** If the scan root is a UNC path (`\server\share`, `\?\UNC\...`), every destructive command and non-dry `cleanup.run` is refused with `protected_path`; so is a UNC move destination. Scan a local drive path to delete.
- The object on disk must match the tree (kind, size, mtime; the model stores no file id) or `stale_tree`. The check runs again immediately before the action.
- **Partial trees.** A scan with exclusions, or a subtree with read errors, omits entries that exist on disk. Reports say `partial_tree: true`; permanently deleting a folder of such a tree needs `i_understand_partial: true` on top of `i_understand_permanent`.
- **Recycle bin.** Only fixed local disks are recycled (`GetDriveTypeW`); network, removable and unknown drives return `unsupported` and the item is untouched (the error suggests `file.delete_permanent`). After a recycle the original path must be gone, otherwise the command fails and the tree is not pruned. Not covered: the shell may still delete permanently an item larger than the bin allows on a fixed disk; the journal says `recycle` in that case.
- **Move.** `MoveFileExW` without `REPLACE_EXISTING` (platform `rename_no_replace`; `unsupported` where the platform has no such primitive), so a destination created after our check is never overwritten. Destination names with `:` (alternate data streams), trailing dots or spaces, or reserved characters are refused.
- **Links and junctions** are never followed; permanent deletion removes the link itself, recycling a link is refused. Folders are removed with `std::fs::remove_dir_all`, which does not follow links inside the tree.
- **Residual race (accepted).** Between the last check (symlink metadata, canonical location) and the removal another process can still replace a path component by a link; only the top object is re-validated, not every descendant. This is inherent to path-based deletion without holding a directory handle; the checks are repeated immediately before the action to keep the window small.
- **Failed permanent removal.** A removal that fails half-way leaves the tree stale: the session sets `rescan_recommended` (`session.state`), and a node that is gone after the failure is pruned.
- After success the node is cut out of the session tree (arena ids change, revision bumped, selection cleared).
- Journal: in memory (newest 10000) and optionally appended to a JSONL file chosen by the host (`Session::set_journal_file`); write errors never fail a command. Paths are in the journal by design and never logged.

## Revision history

- 2026-10-19 · r4: M4.6 never-crash review fixes (revision-checked ids, UNC, subtree protection, partial trees, recycle-bin and no-replace move hardening).
