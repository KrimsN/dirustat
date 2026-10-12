# Progress documents

> Status: 2026-10-13 · revision 2.

Living documents: `ROADMAP.md`, `docs/roadmap.md`, `docs/target-app-parity.md`, `docs/gaps.md`, `plan/STATUS.md`, `log/devlog.md`.

## Rules

1. **A dated header.** Every living document has a line `> Status: <date> · revision N · <summary>`. When you change it, update the date, bump the revision and fix the summary.
2. **Revision history.** At the end of the document a list of `- <date> · rN: what changed`.
3. **Numbers are measured, not estimated.** Parity is counted from `done` rows in `docs/target-app-parity.md`; performance from measurements in `perf/` and the devlog. Never write "approximately".
4. **When you close a gap, update everything related:** `docs/gaps.md` (move to "Closed"), parity rows, totals in `ROADMAP.md`, `plan/STATUS.md`, an entry in `log/devlog.md`.
5. **Do not diverge.** Numbers in `ROADMAP.md` must match `docs/target-app-parity.md`.
6. **Devlog entry format:**

```markdown
## 2026-10-14 · M1.2 · <agent>
- Landed: ...
- Numbers: scan of 100k files, 1.9 s (release, SSD)
- Verified: cargo xtask ci is green
- Still open: ...
```

7. **`plan/STATUS.md`** is not a journal but a snapshot of "where we are now": milestone, active task, next step, blockers; rewritten at the end of every session.
