# The "never crash" standard

> Status: 2026-10-13 · revision 2. Outranks any feature work.

diRUSTat reads other people's disks: huge, damaged, changing under its feet. A crash in the middle of a scan or a delete destroys trust. Any bad input yields **an error that can be shown and handled**.

## Rules

1. **No panics in non-test code:** `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!` are forbidden by lints (`deny`). An unfinished function returns `Unsupported`.
2. **No indexing:** `v[i]` and `s[a..b]` are forbidden (`indexing_slicing = deny`). Use `get()`, iterators, `split_at_checked`; slice strings only at char boundaries.
3. **No `unsafe`** except in the two isolated crates `dirustat-ntfs` (ADR 0002) and `dirustat-platform` (ADR 0009, only `win32.rs`): a block needs a `// SAFETY:` proof; parsing is safe code.
4. **Numbers are hostile:** `checked_add/mul`, `saturating_*`, `try_from`; check division by zero and NaN/inf; cap allocation sizes (name length in an MFT record, attribute count, read buffer size).
5. **Bound recursion** or replace it with iteration; filesystem cycles (junctions, symlinks, hardlinked directories) must not loop (seen-set by file id, depth limit).
6. **Filesystem errors are data, not exceptions:** access denied, file vanished, path too long, invalid names are marked on the node/in the report and the walk continues.
7. **Don't cascade:** handle poisoned mutexes, treat `JoinHandle::join` as a `Result`, a cancelled thread must not take down the UI.
8. **Last-resort guard:** the shell wraps command dispatch and scanning in `catch_unwind`, turns a panic into an error and keeps the session. Profiles keep `panic = "unwind"`.
9. **Prove it:** every crash fix gets a minimal regression test that failed before the fix.
10. **Hostile command test:** `engine::tests::hostile_params_never_panic` calls every command with junk parameters (empty, huge, negative, NaN, unicode, invalid paths).

## Recipes

```rust
// bad: v[i], as, unwrap
// good:
let first = items.get(i).ok_or(ScanError::BadIndex)?;
let len = usize::try_from(raw_len).map_err(|_| ScanError::TooLarge)?;
let total = a.checked_add(b).ok_or(ScanError::Overflow)?;
let Some(name) = parse_name(buf) else { return Err(ScanError::BadName) };
```

## What to check in review

- Every `?` at the filesystem/MFT/command-parameter boundary converts to a typed error.
- Every allocation sized from external input is capped by a constant with a "why this limit" comment.
- Every loop over MFT data has an upper bound on iterations.
- A fuzz target for each parser (MFT record, USN, filters/patterns) and a property test "does not panic on arbitrary bytes".
