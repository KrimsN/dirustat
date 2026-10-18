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

## Verification

After UI changes: `ui.screenshot` (or the offscreen example) → PNG → inspect it in both themes at 100 % / 150 % scale. Demo data is synthetic.
