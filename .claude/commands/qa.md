---
description: Run QA: code and UI testers, merged bug report with reproduction steps and severity
argument-hint: <area: all | code | ui | scan | mft | control | cli | treemap | crate or command id>
---

Run a QA pass for the area `$ARGUMENTS` (default `all`). You are the QA lead: you coordinate and verify, you do not fix bugs.

1. Orient: read `docs/qa/README.md`, `plan/STATUS.md`, `git log -5 --oneline`; note the commit and whether the tree is clean. A red build is itself the first finding.
2. Plan the scope: code-level targets go to `qa-code-tester`, running-app targets to `qa-ui-tester`. For `all`, start both in parallel with disjoint target dirs (`target/agent-qa-code`, `target/agent-qa-ui`) and disjoint report files. Give each agent the area, the commit and recent changes worth attacking (`git diff` of the last milestone).
3. Optionally run `never-crash-reviewer` on the diff of recent commits for static panic and hostile-input findings, and fold its output into the code report.
4. Collect the reports from `docs/qa/reports/`. Reproduce the top S1/S2 findings yourself once; mark what you cannot reproduce as `cannot-reproduce` and merge duplicates.
5. Write `docs/qa/reports/<YYYY-MM-DD>-summary.md`: bugs by severity, release verdict (GO / GO with known issues / NO-GO) with the deciding bugs, coverage gaps, what to test next.
6. Add confirmed S1-S3 bugs to `docs/gaps.md` as ranked tasks (S1/S2 on top), keeping the file's format. Do not touch source code. Commit only if asked.
7. Reply to the user in their language: counts per severity, top bugs with one-line reproduction, verdict, report paths.

Safety: synthetic fixtures only, never the user's real data, no destructive commands outside temp fixtures (`docs/safety.md`).
