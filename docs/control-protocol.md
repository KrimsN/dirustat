# Control protocol and MCP

> Status: 2026-10-13 · revision 2 · **protocol draft** (implementation in milestone M2). `PROTOCOL_VERSION = 1`.

Goal: any action a human can take is available to an agent; the app can be launched, scan a test folder, run commands and take a screenshot without a mouse or keyboard.

## Transport

- Launch: `dirustat --control <port>`. Listens only on `127.0.0.1:<port>`.
- A window key is generated at start and written to `control-key.local-<port>` (Windows: `%APPDATA%\dirustat\`, Linux: `~/.config/dirustat/`, macOS: `~/Library/Application Support/diRUSTat/`). Every request must carry `key`.
- Format: one JSON line per request, one per response.

```json
{"id":1,"key":"...","method":"engine.execute","params":{"command":"scan.start","params":{"path":"C:\\fixtures\\small"}}}
{"id":1,"ok":true,"result":{"revision":3}}
{"id":2,"ok":false,"error":{"code":"invalid_params","message":"path is empty"}}
```

Stable error codes: `unknown_command`, `invalid_params`, `unsupported`, `confirmation_required`, `not_enabled`, `internal`; transport level: `unknown_method`, `bad_request` (malformed JSON or envelope), `unauthorized` (missing/wrong key), `too_large` (request over 1 MiB closes the connection; response over 8 MiB), `rate_limited` (200 requests/s per connection).

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
| `ui.screenshot` | `path`, `region?` | save a window PNG (offscreen render, no focus stealing) |
| `app.quit` | none | graceful shutdown |

Rules:
- A programmatic call never opens a dialog; anything that needs a dialog in the UI has a parameterized command variant.
- `engine.execute` blocks until fast commands finish; long ones (`scan.start`) return at once and progress is read through `session.state`.
- Responses are size-limited: big trees are returned in pages (`session.tree`).

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
