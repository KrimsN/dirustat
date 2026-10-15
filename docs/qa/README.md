# QA: bug reports and severity

> Status: 2026-10-15 · revision 1.

Testing is done by two project agents (`.claude/agents/`) and driven by the `/qa` command:

| Agent | Scope |
|---|---|
| `qa-code-tester` | Code and engine level: hostile inputs, command registry, scan backends, CLI, control protocol, tests, never-crash and safety rules |
| `qa-ui-tester` | The running app: egui UI driven through the control channel, screenshots, UI state, visual and usability defects |

Testers **never fix** anything and never change source files. They only write bug reports. Fixing is a separate task picked from the report.

## Where reports go

`docs/qa/reports/<YYYY-MM-DD>-<area>.md`, one file per test run, using the template below. Every bug gets an id `QA-<n>`; continue the numbering from the latest report (`grep -rh "^### QA-" docs/qa/reports` finds the last one). Reports are committed; confirmed bugs that should be worked on are also added to `docs/gaps.md`. Evidence (small PNGs of synthetic data only) goes to `docs/qa/evidence/`.

## Severity (impact on the project)

| Level | Name | Meaning | Release impact |
|---|---|---|---|
| S1 | Blocker | Crash/panic or hang, data loss, destructive action on the wrong target, a safety or never-crash rule broken, a security hole in the control channel | Blocks any release |
| S2 | Critical | Core scenario broken or wrong numbers (sizes, totals, treemap), scan result incorrect or silently incomplete, fast path returns different data than the portable walk | Blocks a release |
| S3 | Major | Feature does not work as documented, bad error handling, serious perf budget miss, a workaround exists | Fix before release; may ship as a known issue if the owner agrees |
| S4 | Minor | Cosmetic or rare-case defect, small mismatch between docs and behaviour | Does not block |
| S5 | Trivial | Typos, polish, suggestions | Does not block |

Also give **Likelihood** (Always / Often / Sometimes / Rare / Once) and **Scope** (all users / Windows only / admin only / large volumes only / ...). Severity is the damage when it happens; likelihood and scope say how often. A rare S1 stays S1.

Release verdict at the end of each report: `GO`, `GO with known issues` or `NO-GO`, naming the bugs that decide it.

## Bug template

```markdown
### QA-<n>: <short title>

- **Severity:** S1-S5 · **Likelihood:** ... · **Scope:** ...
- **Area:** crate / command id / UI panel · **Found by:** code | ui
- **Build:** commit hash, profile (debug/release), OS, elevated or not
- **Status:** new | confirmed | cannot-reproduce

**Steps to reproduce**
1. ...
2. ...

**Expected:** ...
**Actual:** ... (error text, backtrace, screenshot path)
**Reproduces:** n/n runs. Minimal repro command or test: `...`
**Impact:** why this severity, what it means for the release
**Suspected cause / pointer:** `file:line` (optional, marked as a guess)
**Evidence:** log excerpt, JSON response, PNG path
```

## Report skeleton

```markdown
# QA report <date> · <area>

- Commit, OS, profile, tester agent, what was covered, what was NOT covered
- Summary table: id, severity, title
- Bugs (template above), sorted by severity
- Verdict: GO / GO with known issues / NO-GO
```

## Revision history

- 2026-10-15 · r1: introduced QA agents, `/qa` command, severity scale and templates.
