# Control protocol and MCP

> Status: 2026-10-19 · revision 7 · **protocol draft** (implementation in milestone M2). `PROTOCOL_VERSION = 1`.

Goal: any action a human can take is available to an agent; the app can be launched, scan a test folder, run commands and take a screenshot without a mouse or keyboard.

## Transport

- Launch: `dirustat --control <port>`. Listens only on `127.0.0.1:<port>`.
- A window key is generated at start and written to `control-key.local-<port>` (Windows: `%APPDATA%\dirustat\`, Linux: `~/.config/dirustat/`, macOS: `~/Library/Application Support/diRUSTat/`). Every request must carry `key`. The key comes from the OS random generator (`getrandom`); if that fails the server does not start (fail closed). A stale key file is removed and the new one created exclusively (mode 0600 on Unix); it is removed again on shutdown.
- Format: one JSON line per request, one per response. The request `id` is an integer or a string (a float such as `1.5` is `bad_request`); it is echoed back.

```json
{"id":1,"key":"...","method":"engine.execute","params":{"command":"scan.start","params":{"path":"C:\\fixtures\\small"}}}
{"id":1,"ok":true,"result":{"revision":3}}
{"id":2,"ok":false,"error":{"code":"invalid_params","message":"path is empty"}}
```

Stable error codes: `unknown_command`, `invalid_params`, `unsupported`, `confirmation_required`, `not_enabled`, `protected_path` (a destructive command refused a protected path), `stale_tree` (the tree no longer matches the disk; nothing was changed, rescan), `internal`; transport level: `unknown_method`, `bad_request` (malformed JSON or envelope), `unauthorized` (missing/wrong key), `too_large` (request over 1 MiB closes the connection; response over 8 MiB), `rate_limited` (200 requests/s per connection and 1000 requests/s over all connections). A connection that has not sent a request with the right key within 2 s is closed (idle and wrong-key peers free their slot). Unauthenticated connections have their own budget of 8 slots (the oldest is evicted when it is full) and 50 failed requests/s over all peers, a connection past that is closed; none of this touches the 16 authenticated connections or the 1000 requests/s authenticated budget. An authenticated connection may idle for 60 s.

Implementation (`dirustat-control`): `Dispatcher` (one line in, one line out), `ControlServer::start(port, key_dir, session, bridge)`, `UiBridge` for `ui.*`/`app.quit` (headless: `unsupported`). `session.state` accepts `path` as a JSON pointer (`/selection`). `session.tree` takes `node` (id), `depth` (max 8), `limit` (max 1000), `offset` (pages the children of `node`), `mode` (`allocated`|`logical`); children are sorted by size, largest first.

## Methods

| Method | Parameters | Result |
|---|---|---|
| `engine.commands` | none | list of commands: id, label, menu, shortcut, params_doc, destructive |
| `engine.execute` | `command`, `params` | command result; destructive ones require `confirm: true` |
| `session.state` | `path?` | session state as JSON (top-level tree, selection, progress, revision) |
| `session.tree` | `node?`, `depth?`, `limit?` (1..), `offset?`, `mode?` | subtree with sizes (paginated, never the whole tree); `limit` 0 is `invalid_params` |
| `ui.state` | none | serialized UI state (serde) |
| `ui.set_state` | `patch` | change UI state (panels, sorting, zoom, `cleanups_open`, `journal_open`, `cleanups_preview` (cleanup id, read-only preview), `delete_dialog` = `recycle`/`permanent`/`close` opens the confirmation dialog for the selection but never confirms it) |
| `ui.screenshot` | `path`, `region?`, `overwrite?` | save a window PNG. An existing file is an error unless `overwrite: true`, and even then only an existing PNG (checked by its signature) is replaced, never another kind of file; UNC paths and system locations (Windows, Program Files, ProgramData, `/etc`, `/usr`, ...) are refused; missing parent directories are never created; the file is written to a temp file and renamed; captures above 64 megapixels are refused; a request the window does not pick up within 5 s (for example it is minimized) fails with a `timeout` error asking to restore it; once accepted, no capture within 30 s fails with a timeout (at most 40 s per call). `ui.state` of a window that does not render is answered from the last published state with `"stale": true` |
| `app.quit` | none | graceful shutdown |

Rules:
- A programmatic call never opens a dialog; anything that needs a dialog in the UI has a parameterized command variant.
- `engine.execute` of `file.delete_permanent` and `platform.free_space` runs the slow part (the removal, the OS volume query) with the session unlocked (QA-143, QA-144), so `session.state` and other reads answer meanwhile. While a removal runs, `session.state` has `removal_in_progress: true` and destructive commands and `scan.*` are `not_enabled` ("a permanent deletion is still running"). A volume query that did not return within 3 s keeps one helper thread busy; further `platform.free_space` calls fail at once with `internal` ("an earlier query of a volume has not returned yet") until it finishes.
- `engine.execute` blocks until fast commands finish; long ones (`scan.start`) return at once and progress is read through `session.state`.
- Responses are size-limited: big trees are returned in pages (`session.tree`). One `session.tree` call examines at most 50 000 nodes; past that the answer has `truncated: true` and `visit_capped: true` (`total_children` is then a lower bound).
- **Path parameters over control.** In `engine.execute`, every parameter named `path`, `paths`, `destination`, `only_under` or `root` (at any depth, so also `selector.only_under` of `cleanup.save`) must be an absolute local drive path (`X:\...`, `X:/...`, `\\?\X:\...`); UNC and network spellings, device namespaces (`\\.\`, `\\?\GLOBALROOT\`, `\\?\Volume{`, `\??\`), relative and drive-relative paths are `invalid_params` at once, before any I/O (QA-117). Free text (`query`, `exclude` patterns, names) is never inspected, so `\\d+` or `a//b` are fine (QA-122). The CLI and the UI are not restricted by this rule. `scan.start` returns at once: a missing or non-directory root is reported when the scan is polled (`session.poll`/`session.state`) as `invalid_params`.
- `engine.execute` of `scan.fast_path_status` (`{volume?}`, default the system drive) probes a volume read-only and returns `{volume, available, reason, needs_admin, preferred}`; `reason` is one of `available`, `not_windows`, `access_denied`, `not_ntfs`, `not_found`, `invalid_name`, `malformed`, `io_error`. A bad volume name is a `reason`, not an error. It never elevates (ADR 0002). Setting `scan.prefer_fast_path` (bool, default true) is changed with `settings.set`.
- `ui.relaunch_elevated_help` (UI command, run through the bridge) only toggles an explanation window (`ui.state`: `elevation_help_open`); `ui.state.elevation_hint` holds the volume whose scan root lacked administrator rights. No elevation is performed; a manifest or `runas` relaunch is left for packaging (M6).
- `engine.execute` of `search.find` (read-only) takes `{query?, mode?, case_sensitive?, match_path?, min_size?, max_size?, extensions?, kind?, limit?, size_mode?}`: `query` is at most 512 characters, no NUL, empty means no name test; `mode` is `substring` (default), `wildcard` (`*` any run, `?` one character) or `regex` (linear-time engine with a 1 MiB compiled-size limit; invalid or too large patterns are `invalid_params`); matching is case-insensitive unless `case_sensitive: true`; `match_path` tests the full path instead of the name (entries deeper than 2048 levels cannot be tested; `skipped_deep` counts them, non-zero means the result may be incomplete); `extensions` (at most 64, case and leading dot ignored) never match directories; `kind` is `file` (links included), `dir` or `any`; `limit` is 1..=10000 (default 100); `size_mode` is `logical` or `allocated` (default: session mode). Directory size is the subtree total. Returns `{total_matched, truncated, budget_exhausted, skipped_deep, revision, items: [{id, name, kind, path, size}]}` largest first (ties by id); the search stops after 10 s with `budget_exhausted: true`. The ids are held by the session as the highlight set (`Session::search_hits`) for the tree `revision` they were made on and disappear when the tree is replaced; `search.clear` (no params) drops them and returns `{cleared}`.
- `tree.children` and `list.largest_files` accept the optional filters `extensions` (directories never match), `min_size`, `max_size` and `name` (case-insensitive wildcard); `tree.children` then reports the filtered `total`.
- `scan.start` accepts `exclude`: at most 256 wildcard patterns of at most 1024 characters (no NUL, not empty). A pattern without `/` or `\` is matched against each entry name, one with a separator against the whole path (separators normalized to `/`); matching is case-insensitive on Windows. Matches are skipped entirely: excluded directories are not descended and nothing in them is counted; they are not errors. `exclude` replaces the persistent setting `scan.exclude` (`settings.set`, same limits, default `[]`) for that scan, `[]` means no exclusions. A non-empty list makes the scan use the portable walk instead of the MFT fast path.
- `session.state` scan fields: `progress` is `null` when no scan runs, else `{dirs, files, bytes, errors, backend, fallback_reason}`. `backend` is `portable` or `mft`; `fallback_reason` is a string when the fast path was tried and the scan fell back to the portable walk, otherwise `null`. `last_scan` is `null` until a scan finishes, then `{backend, fallback_reason}` of the most recent finished scan.
- `ControlClient` drops its connection after any I/O error or timeout (the next call reconnects) and rejects a response whose `id` differs from the request.

## MCP

`dirustat-cli mcp` is an MCP server (stdio); `dirustat-cli mcp --connect 127.0.0.1:<port>` bridges to a running app. Every registry command is published as a tool with a JSON parameter schema; destructive ones are marked `destructive: true` and require `confirm`. Resources: `dirustat://session/state`, `dirustat://session/tree`.

## CLI equivalents

```sh
dirustat-cli commands
dirustat-cli scan <folder> --json out.json
dirustat-cli run --scan <folder> --cmd 'select.node={"id":0}'
```

## Command conventions

- Id: `group.action` (`scan.start`, `scan.cancel`, `view.zoom_in`, `select.node`; `search.find`, `search.clear`; `file.delete`, `file.delete_permanent`, `file.move`, `file.delete_preview`, `file.journal`; `cleanup.list`, `cleanup.save`, `cleanup.remove`, `cleanup.preview`, `cleanup.run`; planned, not implemented yet: `export.csv`).
- Groups: `scan`, `view`, `select`, `tree`, `treemap`, `ext`, `file`, `cleanup`, `search`, `filter`, `export`, `snapshot`, `settings`, `ui` (shell only).
- Parameters are a JSON object; every field is documented in `params_doc`; unknown fields are an `invalid_params` error.

## Destructive file commands (`docs/safety.md`)

- **Addressing.** `path` (absolute, resolved exactly: no trimming, no Unicode folding, ASCII case folding only on Windows; no match is `invalid_params`, several matches `stale_tree`) or `id` **together with the required `revision`** (u64): the `revision` returned next to the id by `tree.children`, `list.largest_files`, `search.find`, `cleanup.preview` or `session.state`. `id` without `revision` is `invalid_params`; a `revision` other than the session's current one is `stale_tree` (for `path` addressing too) and nothing happens, because arena ids change on every prune. Applies to `file.delete`, `file.delete_permanent`, `file.move`, `file.delete_preview`. `cleanup.run` addresses cleanups by string id and needs no node ids.
- **Partial tree.** Reports (dry run, preview, result) carry `partial_tree: bool`: the scan used exclusions, or the subtree contains read errors, so entries that exist on disk may be missing from the tree. `file.delete_permanent` of a folder with `partial_tree: true` needs `i_understand_partial: true` in addition to `i_understand_permanent` (otherwise `confirmation_required`); `cleanup.run` takes the same flag and reports held-back nodes with status `needs_confirmation`. `session.state` shows `partial_scan` and `rescan_recommended` (a permanent removal failed half-way).
- **Unaddressable names and drifting folders.** A node whose name (or an ancestor's) is lossy (`name_lossy`), ends with a dot or space, contains `:`, a reserved character or a C0 control (DEL and C1 controls are allowed), or is a Windows device name, is refused with `unsupported` ("cannot be addressed by a unique path"); so is a path that opens an object with a different name (Win32 aliases). `file.delete_permanent`, `file.move` and permanent/`move` cleanups of a folder re-read the real subtree and answer `stale_tree` naming the first difference (new, removed or changed file at any depth; two entries with names identical after conversion to text; more than 2,000,000 entries or 30 s also refuses). Recycle mode keeps the light check and reports `subtree_changed: string|null` in the report. `file.move` destinations that are device names (`CON`, `COM1`, `aux.txt`, ...) are `invalid_params`.
- **Local scan roots only.** When the scan root is not a lettered local path (a UNC share `\\host\share`, `\\?\UNC\...`, a device namespace or a relative root) every destructive command (`file.delete`, `file.delete_permanent`, `file.move`, `cleanup.run` without `dry_run`) is refused with `protected_path`; a network or device move destination is refused too. `platform.free_space`, `platform.reveal` and `platform.properties` with an explicit `path` need a local drive path (`invalid_params`); `platform.free_space` answers an `internal` error if the volume does not respond within 3 s. Administrative shares (`\\host\C$\Windows`) are treated as the local volume by the protected-path table.
- Listings (`tree.children`, `list.largest_files`) also return `revision` and `budget_exhausted` (the 10 s name-filter budget ran out and the list is partial).

- `file.delete_preview {id+revision|path, mode?: "recycle"|"permanent"}` (read-only, no confirm) and the dry run of the destructive commands return the report `{dry_run, mode, path, kind, count, files, dirs, links, size, size_logical, size_allocated, paths (<= 200, target first), truncated, partial_tree, subtree_changed}`.
- `file.delete {id+revision|path, dry_run?, confirm: true}` sends to the recycle bin; `file.delete_permanent {id+revision|path, dry_run?, i_understand_permanent: true, i_understand_partial?, confirm: true}` deletes for good (a UI must show a warning first; links are removed themselves, never followed); `file.move {id+revision|path, destination, dry_run?, confirm: true}` renames with a no-replace primitive (`MoveFileExW` without `REPLACE_EXISTING`; unsupported where the platform has none). The registry demands `confirm: true` even for `dry_run`.
- Pre-flight, in order: the target must resolve to a node of the current tree (`invalid_params` otherwise); protected paths are refused with `protected_path` (volume root, the scan root, `Windows` and everything below it, `Program Files`, `Program Files (x86)`, `ProgramData`, `Users`, an entire user profile, 8.3 spellings, env-configured roots, and the real location when a link or junction is on the way); the object on disk must still match the tree (kind, file size, modification time), otherwise `stale_tree` and nothing changes. Links are never recycled (`invalid_params`; use `file.delete_permanent`).
- On success the result adds `{done: true, tree_updated, revision, ids_invalidated}`: the node's subtree is cut out of the session tree, the revision is bumped, arena ids change and the selection is cleared. `file.move` adds `{destination, rescan_recommended}` (the destination is not added to the tree).
- `file.journal {limit?, offset?}` lists the session journal newest first: `{total, offset, entries: [{time, op, path, destination, size, mode, ok, error}]}`, at most 10000 entries. Hosts may mirror it to a JSONL file with `Session::set_journal_file`.

## Cleanups (`cleanup.*`, `docs/safety.md`)

- Definition: `{id (slug [a-z0-9_-], <= 64), name (1..=128 chars), action: "recycle"|"permanent"|"empty_dir_contents"|"reveal", selector: {names?: [wildcard, <= 64, <= 256 chars, no separators], extensions?: [<= 64], min_size?, max_size?, kind?: "file"|"dir"|"any", only_under?: absolute path in the scan root, older_than_days?: 1..=36500 (needs a known mtime), empty?: bool (folders without files; needs kind "dir")}}`. At least one of names / extensions / min_size / max_size / older_than_days / empty is required. Stored in settings key `cleanups` (at most 64; invalid entries are dropped on load).
- `cleanup.list {}` -> `{max, cleanups: [{id, name, action, selector, builtin}]}` (3 built-ins first; they cannot be edited or removed and never run by themselves).
- `cleanup.save {definition}` -> `{saved, replaced, cleanups}`; `cleanup.remove {id}` -> `{removed, cleanups}`. Both only change settings (the shell persists them). Unknown fields, built-in ids, bad limits: `invalid_params`.
- `cleanup.preview {id}` (read-only) -> `{id, name, action, revision, count, size, truncated, budget_exhausted (5 s budget; partial list, unchecked items have status "unchecked"), items: [{id, name, kind, path, size, status: "ok"|"protected"|"stale"|"invalid", error?}]}`, largest first, at most 10000. For `empty_dir_contents` the items are the entries inside the selected folders.
- `cleanup.run {id, dry_run?, i_understand_permanent?, confirm: true}` -> `{id, action, dry_run, count, size, truncated, budget_exhausted, deleted, deleted_size, failed, not_attempted, stopped, results: [{id, path, ok, status: "ok"|"done"|"protected"|"stale"|"invalid"|"failed", error?}], done, tree_updated, rescan_recommended, revision, ids_invalidated}`. Not enabled without a scanned tree. `reveal` shows only the largest match. Ids are invalid after a run that deleted something.

## Channel security

Loopback only, mandatory key, request size limit, rate limit; destructive commands follow `docs/safety.md`. The channel is off by default.

## Revision history

- 2026-10-19 · r7: M4.10: exact path addressing, `removal_in_progress`, lock-free permanent removal and `platform.free_space`, `skipped_deep`, drive-type classification.
- 2026-10-13 · r3: documented `session.state` fields `progress.backend`, `progress.fallback_reason`, `last_scan{backend,fallback_reason}` (M3.4). Earlier revisions (r1, r2) predate this section.
- 2026-10-19 · r6: positive local-path validator for path-typed params, `subtree_changed`, subtree verification, unaddressable names, `budget_exhausted` on cleanup preview/run (M4.8).
- 2026-10-19 · r5: `id` addressing of destructive commands requires `revision`; `partial_tree` and `i_understand_partial`; UNC scan roots refused; `revision`/`budget_exhausted` on listings (M4.6).
- 2026-10-18 · r4: documented `search.find`, `search.clear`, the filters of `tree.children`/`list.largest_files`, and `scan.start` `exclude` / setting `scan.exclude` (M4.2, G-016).
