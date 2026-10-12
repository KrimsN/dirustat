## What and why

<!-- Brief. Link the gap from docs/gaps.md (e.g. G-002) and the milestone. -->

## Verification

- [ ] `cargo xtask ci` is green (fmt, clippy -D warnings, test, layers, wasm-core)
- [ ] New code has no `unwrap`/`expect`/`panic!`/indexing/`unsafe` (except `dirustat-ntfs`)
- [ ] Tests added; for a crash fix, a regression test
- [ ] For new commands: the checklist in `docs/contributing.md`, hostile test
- [ ] Destructive operations follow `docs/safety.md`; tests do not touch real data
- [ ] UI changes: screenshot attached (both themes)
- [ ] Performance: before/after numbers in `log/devlog.md` (if a hot path was touched)
- [ ] Clean-room: no code/resources from WinDirStat/WizTree; assets listed in `ATTRIBUTION.md`
- [ ] Progress documents updated (`docs/progress-docs.md`): parity, gaps, ROADMAP, devlog, STATUS
- [ ] All text in the change is English
