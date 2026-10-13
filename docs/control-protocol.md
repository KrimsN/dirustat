# Control protocol and MCP

> Status: 2026-10-13 · revision 2 · **protocol draft** (implementation in milestone M2). `PROTOCOL_VERSION = 1`.

Goal: any action a human can take is available to an agent; the app can be launched, scan a test folder, run commands and take a screenshot without a mouse or keyboard.

## Transport

- Launch: `dirustat --control <port>`. Listens only on `127.0.0.1:<port>`.
- A window key is generated at start and written to `control-key.local-<port>` (Windows: `%APPDATA%\dirustat\`, Linux: `~/.config/dirustat/`, macOS: `~/Library/Application Support/diRUSTat/`). Every request must carry `key`. The key comes from the OS random generator (`getrandom`); if that fails the server does not start (fail closed). A stale key file is removed and the new one created exclusively (mode 0600 on Unix); it is removed again on shutdown.
- Format: one JSON line per request, one per response.

```json
{"id":1,"key":"...","method":"engine.execute","params":{"command":"scan.start","params":{"path":"C:\\fixtures\\small"}}}
{"id":1,"ok":true,"result":{"revision":3}}
{"id":2,"ok":false,"error":{"code":"invalid_params","message":"path is empty"}}
```

Stable error codes: `unknown_command`, `invalid_params`, `unsupported`, `confirmation_required`, `not_enabled`, `internal`; transport level: `unknown_method`, `bad_request` (malformed JSON or envelope), `unauthorized` (missing/wrong key), `too_large` (request over 1 MiB closes the connection; response over 8 MiB), `rate_limited` (200 requests/s per connection and 1000 requests/s over all connections). A connection that has not sent a request with the right key within 5 s is closed (idle and wrong-key peers free their slot); an authenticated connection may idle for 60 s.

Implementation (`dirustat-control`): `Dispatcher` (one line in, one line out), `ControlServer::start(port, key_dir, session, bridge)`, `UiBridge` for `ui.*`/`app.quit` (headless: `unsupported`). `session.state` accepts `path` as a JSON pointer (`/selection`). `session.tree` takes `node` (id), `depth` (max 8), `limit` (max 1000), `offset` (pages the children of `node`), `mode` (`allocated`|`logical`); children are sorted by size, largest first.

## Methods

| Method | Parameters | Result |
|---|---|---|
| `engine.commands` | none | list of commands: id, label, menu, shortcut, params_doc, destructive |
| `engine.execute` | `command`, `params` | command result; destructive ones require `confirm: true` |
| `session.state` | `path?` | session state as JSON (top-level tree, selection, progress, revision) |
| `session.tree` | `node?`, `depth?`, `limit?` | subtree with sizes (paginated, never the whole tree) |
| `ui.state` | none | serialized UI state (serde) |
| `ui.set_state` | `patch` | change UI state (panels, sorting, zoom) |
| `ui.screenshot` | `path`, `region?`, `overwrite?` | save a window PNG. An existing file is an error unless `overwrite: true`; missing parent directories are never created; the file is written to a temp file and renamed; captures above 64 megapixels are refused; a request with no capture within 60 s fails with a timeout |
| `app.quit` | none | graceful shutdown |

Rules:
- A programmatic call never opens a dialog; anything that needs a dialog in the UI has a parameterized command variant.
- `engine.execute` blocks until fast commands finish; long ones (`scan.start`) return at once and progress is read through `session.state`.
- Responses are size-limited: big trees are returned in pages (`session.tree`). One `session.tree` call examines at most 50 000 nodes; past that the answer has `truncated: true` and `visit_capped: true` (`total_children` is then a lower bound).
- `engine.execute` of `scan.start` with a UNC path (two leading slashes or backslashes, including the extended UNC form) is refused with `invalid_params` over control. `scan.start` returns at once: a missing or non-directory root is reported when the scan is polled (`session.poll`/`session.state`) as `invalid_params`.
- `engine.execute` of `scan.fast_path_status` (`{volume?}`, default the system drive) probes a volume read-only and returns `{volume, available, reason, needs_admin, preferred}`; `reason` is one of `available`, `not_windows`, `access_denied`, `not_ntfs`, `not_found`, `invalid_name`, `malformed`, `io_error`. A bad volume name is a `reason`, not an error. It never elevates (ADR 0002). Setting `scan.prefer_fast_path` (bool, default true) is changed with `settings.set`.
- `ui.relaunch_elevated_help` (UI command, run through the bridge) only toggles an explanation window (`ui.state`: `elevation_help_open`); `ui.state.elevation_hint` holds the volume whose scan root lacked administrator rights. No elevation is performed; a manifest or `runas` relaunch is left for packaging (M6).
- `ControlClient` drops its connection after any I/O error or timeout (the next call reconnects) and rejects a response whose `id` differs from the request.

## MCP

`dirustat-cli mcp` is an MCP server (stdio); `dirustat-cli mcp --connect 127.0.0.1:<port>` bridges to a running app. Every registry command is published as a tool with a JSON parameter schema; destructive ones are marked `destructive: true` and require `confirm`. Resources: `dirustat://session/state`, `dirustat://session/tree`.

## CLI equivalents

```sh
dirustat-cli commands
dirustat-cli scan <folder> --json out.json
dirustat-cli run --scan <folder> --cmd 'export.csv={"path":"out.csv"}'
```

## Command conventions

- Id: `group.action` (`scan.start`, `scan.cancel`, `view.zoom_in`, `select.node`, `file.delete`, `export.csv`, `search.find`).
- Groups: `scan`, `view`, `select`, `tree`, `treemap`, `ext`, `file`, `cleanup`, `search`, `filter`, `export`, `snapshot`, `settings`, `ui` (shell only).
- Parameters are a JSON object; every field is documented in `params_doc`; unknown fields are an `invalid_params` error.

## Channel security

Loopback only, mandatory key, request size limit, rate limit; destructive commands follow `docs/safety.md`. The channel is off by default.
