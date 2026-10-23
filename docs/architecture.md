# diRUSTat architecture

> Status: 2026-10-13 · revision 2 · M0 skeleton.

## 1. Principles

1. **Layers.** Every crate belongs to a layer and depends only on lower ones. Checked by `cargo xtask layers`.
2. **The engine/UI seam.** All logic lives in `engine` behind the command registry. The UI is a thin client: it reads `Session` and sends commands.
3. **Data apart from presentation.** `model` and `treemap` are pure data/geometry with no I/O and no GUI; they also compile to wasm.
4. **One path for humans and agents.** UI, CLI, control channel and MCP are equal clients of one command registry.
5. **Scanning is read-only.** Any write to disk is a separate destructive command governed by `docs/safety.md`.

## 2. Layers and crates

| Layer | Crate | Responsibility | Status |
|---|---|---|---|
| L0 | `dirustat-model` | Arena file tree (`Tree`, `NodeId`, `Node`), iterative saturating size aggregation | skeleton + tests |
| L0 | `dirustat-treemap` | Squarified layout (`squarify`, `layout_tree`), hit-testing, cushion surfaces and shading (`cushion`) | skeleton + tests |
| L0 | `dirustat-ntfs` | NTFS MFT reading and parsing; the only `unsafe` zone (Win32 FFI) | stub |
| L1 | `dirustat-scan` | Filesystem walk to `Tree`: portable parallel and fast MFT paths; progress and cancellation | stub |
| L2 | `dirustat-engine` | `Session`, command registry, extension statistics, search, filters, snapshots, actions | skeleton |
| L3 | `dirustat-control` | JSON control channel (TCP 127.0.0.1 + key), MCP server | skeleton |
| L4 | `dirustat-ui` | egui shell: tree, treemap, extension list, themes, menus | skeleton |
| apps | `dirustat`, `dirustat-cli` | Binaries | banner |

Dependencies (arrow = "depends on"): `ui → control → engine → scan → ntfs`, `engine → model, treemap`, `scan → model`. Only `ui` and `dirustat` may pull in egui/eframe/winit/rfd/wgpu.

## 3. Data model

- `Tree` is a flat `Vec<Node>`; `NodeId` is a `u32`. A node holds a name (`Box<str>`), `NodeKind` (`Dir`/`File`/`Link`), `own_size`, `total_size`, parent, first/last child and next sibling. A parent is always added before its children, so aggregation is a reverse pass without recursion.
- **Sizes.** The base field is `own_size`; in M1 the model gains a *logical* / *allocated* pair (clusters, sparse files, compression) and `files`/`dirs`/`mtime`/attribute counters. The display mode is a setting (rule 3 in `AGENTS.md`).
- **Names.** Stored as UTF-8; Windows names with invalid UTF-16 are encoded without panicking (lossy + flag), details in an ADR when M1 lands.
- **Immutable after a scan.** A scan builds the tree in the background and publishes revisioned snapshots; the UI works with an `Arc` of the latest revision. Caches (layout, statistics) are keyed by revision.

## 4. Scanning

```text
scan::start(root, options) -> ScanHandle { progress: Receiver<Progress>, cancel(), join() -> Result<Tree> }
```

- Portable path: rayon pool, work-stealing across directories, `read_dir` + metadata; hardlinks keyed by (volume, file id), reparse points not followed, access errors recorded as flagged nodes and the walk goes on.
- Fast path (Windows/NTFS): `dirustat-ntfs` reads the MFT and yields flat records (id, parent id, name, sizes, flags); `scan` builds a `Tree` from them. Needs administrator rights; without them it falls back to the portable path transparently.
- Progress and cancellation use channels; the UI thread never blocks. A scan can be cancelled at any time; a partial tree is valid.

## 5. Commands

`CommandSpec { id, label, menu_path, shortcut, params_doc, enabled(&Session), run(&mut Session, Params) -> Result<Value, CommandError> }`.

- An id is `group.action` in snake_case: `scan.start`, `scan.cancel`, `view.zoom_in`, `file.delete`, `export.csv`.
- The registry is assembled from `engine/src/cmd/*.rs` (one module per area; each exports `specs()`).
- Menus and the command palette are built from the registry. The menu/shortcut catalog is `ui/src/menu_catalog.rs`: an item with an exact id goes live automatically.
- The `hostile_params_never_panic` test runs every command with junk parameters and must stay green.

## 6. UI

egui/eframe on wgpu. Panels: directory tree (columns), treemap, extension list, status bar (progress, free space). The treemap is drawn with a mesh/tile cache; layout is recomputed only when the revision, size or focus changes. State (`ui::state`, serde) is read and written by the control channel. Design: `docs/ui-design.md`.

## 7. Agent control

See `docs/control-protocol.md`: `engine.execute`, `engine.commands`, `session.state`, `ui.screenshot`. The same set is available through the CLI (`dirustat-cli run`) and MCP (`dirustat-cli mcp`).

## 8. Platforms

- Windows 10/11 is primary (MFT, recycle bin, Explorer, context menu).
- Linux/macOS get the portable scan (M6); platform actions sit behind a `PlatformServices` trait in `engine`.
- Web: L0 only (layout demo). Scanning a disk in a browser is not a goal.

## 9. Glossary

- **MFT**: Master File Table, the table of all NTFS files; reading it whole gives a very fast scan.
- **Logical size**: the file's data size; **allocated size**: space taken on disk (clusters). The MFT backend reports exact allocation (resident files 0, sparse/compressed = stored size); the portable backend estimates it as logical rounded up to the cluster size, so its allocated total is an upper bound (policy table in the `dirustat-scan` crate docs, remaining work G-045).
- **Treemap**: a map of nested rectangles with area proportional to size; **squarified**: an algorithm that minimizes elongation; **cushion**: van Wijk's cushion shading that reveals hierarchy.
- **Cleanup**: a user-defined action on the selected file/directory (open, delete, run a command).
- **Revision**: a monotonic number of the tree state; the cache key.
