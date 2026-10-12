# AGENTS.md: guide for AI agents and contributors

diRUSTat is an open-source, native disk-space analyzer in the spirit of WinDirStat and WizTree, written in **pure Rust** (no JavaScript/TypeScript). **No Tauri, Electron or webview shells**: the desktop app is native egui/eframe on wgpu. Goal: feature parity with WinDirStat, WizTree-class speed (reading the NTFS MFT), and every feature drivable by agents. Development is done by agents (Claude Code, Codex, etc.); this file is their main entry point. Read it fully, then `plan/STATUS.md`.

The product name in user-facing text (UI, window titles, About, installers, release names, documentation prose) is always written **diRUSTat**. Machine names are lowercase: crates `dirustat-*`, binaries `dirustat`/`dirustat-cli`, id `dev.dirustat.app`.

The approach is borrowed from [storytold](https://github.com/storytold) (photocraft, wordcraft, craft-libs): layers enforced by xtask, "everything is a command", app control via a control channel/MCP, strict never-crash, and living progress docs plus a dev log for handing work between agents.

## 0. Language

- **All text in the repository is English**: documentation (`*.md`), plans and task descriptions, code comments (`//`, `///`, `//!`), identifiers, log and error messages, UI strings, commit messages, branch names, file names, ADRs, issue/PR templates, agent and command definitions.
- **Communication with the user is in the user's language** (chat replies, questions, summaries, status reports). Detect it from the user's messages and keep to it; if the user switches language, follow.
- Quoted user-supplied text and proper names keep their original form. Product terms (MFT, treemap, arena, backpressure) are used as-is.
- Never put non-English prose into the repository, even if the conversation is in another language. If a file in the repo contains non-English text, translate it when you touch it.
- Commit messages: short, imperative, English, with the task id (`M1.3: add scan progress channel`).

## 1. Orientation (5 minutes)

| Read | Why |
|---|---|
| `plan/STATUS.md` | Current milestone, next task, blockers. **Start every session here** |
| `plan/execution-plan.md` | Autonomous operation protocol and task order |
| `docs/architecture.md` | Crate map, layers, engine/UI seam, data model, glossary |
| `docs/development.md` | Build, test, run, drive the app, debugging |
| `docs/contributing.md` | Rules: clean-room, tests, layers, style, commits; the "add a command" checklist |
| `docs/never-crash.md` | The never-crash standard (outranks features) |
| `docs/safety.md` | Rules for destructive operations (deleting user files) |
| `docs/engineering.md` | Code conventions: errors, concurrency, dependencies, comments |
| `docs/testing.md` | Test pyramid, fixtures, fuzzing, property tests |
| `docs/performance.md` | Budgets, how to measure, hot paths |
| `docs/control-protocol.md` | JSON control channel and MCP: how agents drive the app and take screenshots |
| `docs/ui-design.md` | Tokens, themes, widgets, visual verification |
| `docs/target-app-parity.md` | Parity with WinDirStat/WizTree: feature matrix with command ids |
| `docs/gaps.md` | Ranked work list: tasks are picked from here |
| `docs/roadmap.md`, `ROADMAP.md` | Milestones M0–M7, current focus |
| `docs/adr/` | Accepted architecture decisions. Do not revisit without a new ADR |
| `log/devlog.md` | Journal: what landed, numbers, what is still open |

## 2. Workspace map

```text
crates/
  model  treemap  ntfs     L0 foundation: arena file tree, squarified layout, NTFS MFT parsing
  scan                     L1 filesystem walk -> model::Tree (portable + fast path via ntfs)
  engine                   L2 Session + command registry (every action is a command)
  control                  L3 control channel (JSON/TCP) and MCP server
  ui                       L4 egui shell (thin, everything goes through engine)
apps/
  dirustat                 desktop app (eframe/wgpu)
  dirustat-cli             headless CLI: scan / info / run / commands / mcp
xtask/                     cargo xtask layers | ci | (parity | perf | scorecard | fixtures as they appear)
```

**Layering is enforced** by `cargo xtask layers`. A crate depends only on lower layers. Nothing below `ui` knows about egui/eframe/winit/rfd/wgpu; `dirustat-cli` does not depend on `ui`. Register a new crate in `xtask/src/layers.rs`.

## 3. Golden rules

### Never crash (outranks feature work)

Users trust the app with their disks; a crash during a scan or a delete is unacceptable. A broken MFT record, a file name with invalid UTF-16, a 32k-long path, a directory that vanishes mid-walk, access denied, size overflow, hostile command parameters: all of these yield **an error that can be handled, not a panic**. Details: `docs/never-crash.md`.

- **Non-test code never panics.** No `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, no `[i]`/`[a..b]` indexing (workspace lints are `deny`). Return the crate's error type and propagate with `?`; use `ok_or(..)?`, `let .. else`, `get()`. The only exception is a provably infallible literal: `#[allow(clippy::expect_used)]` plus `.expect("why it can't fail")`.
- **`unsafe` is forbidden** (`unsafe_code = "forbid"` in the workspace). The one exception is the isolated crate `dirustat-ntfs` (Win32 FFI for reading the volume, ADR 0002): `unsafe_code = "deny"`, every block has `// SAFETY:`, `undocumented_unsafe_blocks = "deny"`, record parsing is safe code plus fuzzing. Do not add `unsafe` anywhere else.
- **Numbers from the filesystem and the MFT are hostile.** Sizes, lengths, offsets and record counts go through `checked_*`/`saturating_*`, `try_from` instead of `as`, allocations are capped, guard division by zero and NaN/inf. Tree sizes are summed with saturation.
- **Bound recursion** (depth limits, seen-sets): directories can be cyclic (junctions, bind mounts, hardlinked directories). Tree walks are iterative.
- **Don't cascade.** Handle poisoned mutexes (`lock().unwrap_or_else(PoisonError::into_inner)`), treat thread joins as `Result`s. One unreadable folder does not abort a scan: it is marked with an error and the walk continues.
- **Last-resort guard.** The shell catches a panic around command dispatch and scanning, reports it as an error and keeps the session. This is a safety net, not a licence to panic. Keep `panic = "unwind"`.
- **Prove it.** Every crash fix comes with a minimal regression test that failed before the fix.

### Other rules

1. **Everything is a command.** New user-visible behaviour = a command in `engine` with id, label, menu path, shortcut, params doc, `enabled` and `run`, plus tests. UI, CLI, control and MCP all invoke commands by id. Pure view state (zoom, panels, layout) belongs to the shell (`ui::UI_COMMANDS`). A programmatic command call **never** opens a dialog. Checklist: `docs/contributing.md`.
2. **Read-only by default.** Scanning changes nothing on disk. Destructive commands (delete, move) follow `docs/safety.md`: an explicit confirmation parameter, recycle bin by default, dry-run, and **agents must never run them on the user's real data**, only on temporary fixtures.
3. **Size on disk is not file size.** The model distinguishes logical size and allocated size (clusters, sparse files, NTFS compression; hardlinks are not counted twice; reparse points are not followed). Never substitute one for the other silently; the display mode is a setting.
4. **Clean-room.** WinDirStat (GPL) and WizTree (proprietary) are studied for behaviour, public documentation and observation only. **Never copy their code, icons, localization strings or resources.** In particular, do not read WinDirStat's source to port logic from it. Algorithms come from papers (squarified treemaps, van Wijk cushion treemaps) and public NTFS specifications. Third-party assets need a compatible licence, the licence file next to them and a row in `ATTRIBUTION.md` in the same change. Never commit font files. See `docs/clean-room.md`.
5. **Tests are the gate.** Every change comes with tests. Scans run on synthetic trees in `tempfile` directories; parsers get round-trip, property and fuzz tests; layout gets invariants (area sums, no overlaps, proportionality).
6. **The UI is thin and data-driven.** UI state is serde-serializable so the control channel can read and drive it. Colours and radii come from `theme::Tokens`, never hard-coded.
7. **Verify UI changes visually.** Render offscreen / take a screenshot via control (`ui.screenshot`) and look at the PNG. Demo data is synthetic; never use the contents of a user's real disk in screenshots or fixtures.
8. **Performance is a feature.** Measure hot paths in release on 1M+ node trees; budgets live in `perf/budgets.toml`. Never rescan or recompute the whole tree per frame: cache by revision. Parallelism via rayon/channels; the UI thread never blocks.
9. **Don't break wasm for L0.** `model` and `treemap` must pass `cargo check --target wasm32-unknown-unknown` (pure data/geometry, usable for a web demo). The rest is desktop.
10. **Keep dependencies minimal.** A new dependency gets a row in `docs/engineering.md` › Dependencies (why, licence, alternative) and must pass `cargo deny check`. tauri/wry/electron/webview are banned.

## 4. Picking work

Priorities: important infrastructure first, then low-hanging parity, then the long tail.

0. Read `plan/STATUS.md`, then `docs/roadmap.md` › **Current focus**.
1. `docs/gaps.md` is the ranked list. Take the top unblocked task of the milestone named in STATUS.
2. `docs/target-app-parity.md` is the WinDirStat/WizTree feature matrix; when you close an item, update its row (status, date) and recount the totals.
3. `log/devlog.md`: the "Still open" bullets of recent entries.
4. When your work moves a number or closes a gap, update the matching documents, bump their status line and revision history (`docs/progress-docs.md`).

## 5. Before you finish a task

```sh
cargo fmt --all
cargo clippy -p <touched crates> --all-targets -- -D warnings
cargo test -p <touched crates>
cargo xtask layers
cargo xtask ci                  # full gate: fmt, clippy, test, layers, wasm-core
```

Then append a terse entry to `log/devlog.md` (what landed, numbers, what is still open) and update `plan/STATUS.md`. Sessions end abruptly (context limits, crashes); the dev log plus a green tree is how the next agent picks up. **Keep the tree building at every step.** Commit after every arc of work that builds, with a task id (`M1.3: add scan progress channel`).

## 6. Parallel agents

- Use your own target dir: `CARGO_TARGET_DIR=target/agent-<name>` (avoids the Cargo build lock). Edit only the files you own. Each target dir is several GB; delete `target/agent-*` of finished agents.
- Shared files (`engine/src/lib.rs`, the `specs()` list in the command registry, `ui/src/menus.rs`, `state.rs`) get small, surgical edits; re-read before editing.
- Put new commands in a **new module** (`engine/src/cmd/<area>.rs` with a `specs()` function) rather than growing a shared file.
- If someone else's in-progress edit breaks the build, wait and retry; don't fix their files.
- Keep every `Cargo.toml` valid at all times: the `crates/*` glob means one broken manifest breaks everyone's build. Create or rewrite manifests atomically: write to a temp file outside `crates/`, then move it into place.
- Git: each agent uses its own branch/worktree (`agent/<name>/<task>`); changes reach `main` via PR/review. Never `git push --force`, `reset --hard` or rewrite someone else's commits.

## 7. What the human decides (ask, don't guess)

Licensing and publishing (crates.io, GitHub, releases), secrets and signing, adding dependencies with copyleft licences, changing the public control protocol after a release, any action on the user's real data. Decide everything else yourself and record decisions in an ADR/devlog. Details: `plan/execution-plan.md` › Protocol.

## 8. Where things are tracked

- `plan/STATUS.md`: current state for hand-off between sessions (update at the end of each one).
- `plan/execution-plan.md`: detailed milestone and task plan (M1.1, M1.2, ...).
- `docs/roadmap.md`: milestones and current focus; `ROADMAP.md`: one-page summary with numbers.
- `docs/target-app-parity.md`, `docs/gaps.md`: where we stand against WinDirStat/WizTree.
- `docs/adr/`: decisions; `log/devlog.md`: journal.
- `perf/budgets.toml`: performance budgets.
