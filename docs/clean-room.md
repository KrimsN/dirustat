# Clean-room

> Status: 2026-10-13 · revision 2.

diRUSTat is an independent implementation of the ideas "disk map" and "fast MFT scan". WinDirStat is distributed under the GPL and WizTree is proprietary; their names are trademarks of their owners and this project is unaffiliated with them.

## Allowed

- Use the programs as a black box: run them on synthetic data, record behaviour, take screenshots **of the program window only** (never the whole desktop) into the local directory `plan/refs/` (gitignored, never committed).
- Read public documentation, help, articles and specifications: squarified treemaps (Bruls, Huizing, van Wijk), cushion treemaps (van Wijk, van de Wetering), NTFS documentation (MFT structure, `$STANDARD_INFORMATION`, `$FILE_NAME`, `$DATA` attributes, fixups, data runs), Win32 API (MSDN).
- Reuse **function names and user scenarios** (views, menus, commands) to the extent needed for parity.

## Forbidden

- Read, copy, rewrite or "get inspired line by line" by the source of WinDirStat or other GPL projects; port algorithms and data structures from them. If you happen to see such code, tell the human and do not use it.
- Copy icons, cursors, images, localization strings, resources, help text or extension colour schemes.
- Disassemble or analyze binaries and resources of other programs.
- Commit files produced by other programs or their screenshots.

## Assets

All assets are original, CC0/PD or OSI-compatible, with the licence file next to them and a row in `ATTRIBUTION.md`. Fonts are never added to the repository.

## When in doubt

Stop and ask the human. It is better to write from scratch from a public specification than to risk the project's licence.
