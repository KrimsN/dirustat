# UI design

> Status: 2026-10-18 · revision 3 · draft (implementation in M1/M5).

## Layout

```text
┌ menu: File  Edit  View  Cleanup  Help ────────────────────────────────┐
│ toolbar: drive/folder picker · scan/stop · size mode · search           │
├───────────────────────────────┬───────────────────────────────────────┤
│ Directory tree (columns)      │ File-type list (extensions)            │
│ name · % · size · files ·     │ colour · .ext · size · % · files       │
│ folders · modified · attrs    │                                        │
├───────────────────────────────┴───────────────────────────────────────┤
│ Treemap (cushion), highlight of selection / extension                  │
├────────────────────────────────────────────────────────────────────────┤
│ status: path · size · scan progress · free space on disk               │
└────────────────────────────────────────────────────────────────────────┘
```

Panels dock and hide (`view.*` commands); the layout is saved in settings. The window opens in under 300 ms and stays responsive during a scan.

Additional surfaces (M4.4): the **search bar** (`ui.toggle_search`, Ctrl+F) under the top bar with criteria rows and a result list above the status bar; hits are highlighted in the tree (tinted row) and the treemap (warning outline); the **drive picker** (start screen when nothing is scanned, and the Drives menu in the top bar) with fill bars from `list.drives`; the **context menu** of tree rows and treemap tiles (Reveal in file manager, Properties, Copy path; no delete entry yet); the **Preferences** window (`ui.preferences`) with the scan exclusion editor; free space of the scanned volume in the status bar. Evidence: `docs/qa/evidence/ui-m44-*.png`.

## Tokens and themes

- All colours, radii, spacing and font sizes live in `theme::Tokens` (`crates/ui/src/theme.rs`). Hard-coded colours in widgets are forbidden.
- Themes: light, dark, high contrast; switched by the `view.theme` command.
- The extension palette is categorical, distinguishable in both themes and for colour-blind users; a colour is assigned to an extension stably (hash → palette) and the user can override it.
- Icons are drawn in code (vector primitives) or CC0/OSI with a row in `ATTRIBUTION.md`. **Never copy icons from WinDirStat/WizTree.**

## Treemap

- Squarified layout (`dirustat-treemap`), cushion shading (van Wijk) with parameters: brightness, height, scale, light source, grid.
- Nodes smaller than a threshold (about 2 px) are not laid out. Click selects a node; double click zooms; the tooltip shows path, size and percentage of the parent.
- Selection is synchronized between the tree, the treemap and the extension list.

## Accessibility and input

Everything is reachable from the keyboard (shortcuts from the command registry, a command palette on `Ctrl+Shift+P`), UI scaling, contrast at least WCAG AA, screen-reader support as far as egui/AccessKit allow.

Shell-only commands (`ui.*`, `dirustat_ui::UI_COMMANDS`: panel toggles, theme, zoom, palette, quit) sit next to the engine registry; menus, shortcuts and the palette list both. `DirustatApp::ui_state_json` / `apply_ui_patch` back `ui.state` / `ui.set_state` (strict, atomic patch).

## Localization

UI strings live in a catalog `crates/ui/locales/`; the source language is English, other languages are translations. Keys are English identifiers. Strings from other programs are never copied.

## Destructive actions UI (M4.7)

- Delete never happens from a click: every entry (context menu, Edit menu, `Delete`, `Shift+Delete`) runs `file.delete_preview` and opens a modal dialog (`crates/ui/src/delete_dialog.rs`). Only the dialog buttons send `file.delete` / `file.delete_permanent` with `confirm`; this send is UI-only and not a UI command, `ui.set_state` can open or close the dialog but never confirm it.
- Dialog content: name and path, item count, total size, the first paths, a partial-tree warning. Recycle mode says it can be restored; permanent mode has a red border, an irreversible warning and an "I understand this cannot be undone" checkbox (plus one for partial trees). Cancel has the initial focus and `Enter` never confirms a permanent deletion.
- Ids always travel with the session revision. A network (UNC) scan root and the scan root itself disable the entries with a tooltip reason. Errors become friendly toasts or an inline hint (`stale_tree`: scan again, `unsupported`: offer permanent deletion).
- Tools > Cleanups (list, preview, dry run, confirmed run, editor for user cleanups, per-node results) and Tools > Journal (paged `file.journal`) are windows driven by `ui.set_state` fields `cleanups_open`, `cleanups_preview`, `journal_open`.
- Evidence: `docs/qa/evidence/ui-m47-{delete-recycle,delete-permanent,cleanups,journal}-{dark,light}.png`.

## M4.9 UI rules (QA-125..QA-138)

- **Paths are elided in the middle** (`ui::text_fit`): head, "...", tail, and the last path component (the file name) always stays visible; the full path is the hover text. Used by the delete dialog list, the journal and the cleanup preview. Long single-line texts never push buttons out of a window: put the button first in a `right_to_left` layout and truncate the label (exclusion list in Preferences).
- **System font fallback** (`ui::fonts`): at startup the app looks for a CJK-capable font in the OS font folders (Windows: `msyh.ttc`, `YuGothR.ttc`, `malgun.ttf`, `msjh.ttc`, `meiryo.ttc`, `simsun.ttc`; macOS and Linux names too) and registers it as the last fallback of both families. No font file is committed or bundled; a missing or unreadable file leaves the built-in fonts (boxes for CJK). Font files over 64 MiB are ignored.
- **Notices expire**: a status-bar notice disappears after 8 s or when the scan phase changes (`UiState::tick_notice`).
- **Search**: an invalid criterion (regex, size) clears the highlight set, so nothing stale stays on the map. Hits get a two-ring outline (dark and light, chosen by contrast against the tile colour; one ring on tiles below 10 px). A collapsed folder that contains hits shows a count badge ("99+" cap) with a tooltip.
- **Text on bars** (percent in the tree and the file-type list) takes the better of the text and the on-accent colour against the bar fill and the track (`ui::contrast`, WCAG contrast ratio).
- **Extension colours** (`ui::ext_colors`): 60 common extensions get hand-ordered slots on the hue wheel with five lightness/saturation variants (pairwise distance at least 20 in RGB, tested); every other extension is hashed to a continuous hue.
- **Start screen and drive picker** scroll vertically when the window is small or the zoom is high; rows are 220..320 px wide.
- **Command palette** rows clip the label before the right-aligned shortcut (never drawn over it).
- **Cleanups window** reloads when the engine's cleanup definitions change (for example `cleanup.save` over control). A preview or run whose `only_under` folder is outside the scan reads "limited to ..., which is outside the scanned folder" inline, without a toast.
- **Engine refusals** (`delete_dialog::friendly`): `unsupported` for an unaddressable name (lossy, trailing dot or space, reserved character, device name) names the reason and offers nothing; `stale_tree` shows the first difference the engine found (new, removed or changed file) and offers a rescan; `subtree_changed` of a recycle preview is a warning in the dialog; a cleanup preview shows `unchecked` items and a time-budget warning when `budget_exhausted` is set. Explicit path parameters over control must be local drive paths; the UI itself is not restricted.
- **Demo mode**: the default `--demo` tree has no files on disk, so delete is refused up front with an honest reason. `DIRUSTAT_DEMO_DELETABLE=1` scans a disposable fixture folder in the temp directory with the fake platform (private "recycle bin"), removed when the app exits; engine protections are unchanged.
- **Not implemented**: a high-contrast theme (earlier text promised it; G-059).
- Evidence: `docs/qa/evidence/ui-m49-*.png` (dark and light).

## Verification

After UI changes: `ui.screenshot` (or the offscreen example) → PNG → inspect it in both themes at 100 % / 150 % scale. Demo data is synthetic.
