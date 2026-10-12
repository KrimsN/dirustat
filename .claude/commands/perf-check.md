---
description: Measure a hot path's performance and compare with the budgets
argument-hint: <scenario: scan | mft | treemap | ui>
---

Check the performance of the scenario `$ARGUMENTS`:

1. Read `docs/performance.md` and `perf/budgets.toml`.
2. Build in release. Use synthetic fixtures (`cargo xtask fixtures` once it exists), never the user's real data.
3. Take 3 measurements and record the median, the machine and the configuration.
4. Compare with the budget. If a budget is met for the first time, set `enforce = true`. If not, find the bottleneck by profiling, not by guessing.
5. Record before/after numbers in `log/devlog.md`; to publish a baseline run `cargo xtask perf --update-baseline`.
