# Releasing

> Status: 2026-10-24 · revision 1 · versioning scheme, release channels, defect admission rules, release procedure.

## 1. Versioning

diRUSTat uses [Semantic Versioning 2.0.0](https://semver.org) with pre-release identifiers. One version for the whole workspace: `[workspace.package].version` in the root `Cargo.toml`; every crate inherits it.

```text
0.1.0-alpha.1  ->  0.1.0-alpha.2  ->  0.1.0-beta.1  ->  0.1.0-rc.1  ->  0.1.0  ->  0.1.1 / 0.2.0 ...  ->  1.0.0
```

- **Git tag** is `v<version>` (`v0.1.0-alpha.1`). The tag must equal the `Cargo.toml` version; the release workflow refuses a mismatch.
- **Pre-release counter** (`alpha.N`, `beta.N`, `rc.N`) starts at 1 and only grows within one `MAJOR.MINOR.PATCH` core. Moving to the next channel resets it to 1.
- **While `0.y.z`** the public surface (control protocol, CLI flags, `.dirsnap` and settings formats) may change between minors. Breaking changes are listed in `CHANGELOG.md` under "Changed". From `1.0.0` they require a major bump.
- `MINOR` is bumped when a milestone adds user-visible features; `PATCH` for fixes only. `0.1.0` is the first release that satisfies the "stable" column of §2.
- Version `0.0.1` was the internal development placeholder and is never published.
- Crates are not published to crates.io yet (a human decision, `AGENTS.md` §7); the GitHub Release is the only distribution channel.

## 2. Channels and what may ship in each

Defect severities S1-S5 are defined in `docs/qa/README.md`. "Known" means filed in `docs/qa/reports/`, `docs/gaps.md` or the release notes.

| | **alpha** | **beta** | **rc** | **stable** |
|---|---|---|---|---|
| Purpose | Early feedback; features may be incomplete | Feature set frozen for the minor; hardening | Candidate for stable; only blocker fixes | Recommended for everyone |
| GitHub flag | pre-release | pre-release | pre-release | latest |
| Open **S1** | Never. A fixed S1 that was not independently retested may ship only if it has a regression test and is named in the notes | Never | Never | Never |
| Open **S2** | Allowed if listed in "Known issues" and not on the scan or delete path | Not on any core scenario; listed | Never | Never |
| Open **S3** | Allowed | Allowed if listed, with a workaround | Only with owner approval, listed | Only with owner approval, listed |
| **S4 / S5** | Allowed | Allowed | Allowed | Allowed |
| Unverified areas (no QA pass, no real-data run) | Allowed, must be listed | Only the ones not on a core scenario | Never | Never |
| Performance budgets (`perf/budgets.toml`) | May miss; measured numbers are stated | May miss by at most 2x; listed | Must meet, or a documented waiver | Must meet |
| Missing features / parity gaps | Allowed | Allowed for the frozen set only | Never new gaps | Listed in parity doc |
| Breaking changes to formats or protocol | Allowed | Allowed with a migration note | Never | Only in a major |
| Platform coverage | Windows build is the supported artifact; Linux builds in CI but is not shipped | Same | Same unless M6 is done | Per platform matrix in the notes |
| Code signing, installer, icon | Not required | Not required | Recommended | Required for the Windows build (M6) |
| CI | `cargo xtask ci` green on the tagged commit | same | same | same |

**Never-crash and data-safety rules (`docs/never-crash.md`, `docs/safety.md`) are not relaxed in any channel.** The table only governs which known defects and gaps may remain, never whether a rule may be broken knowingly.

Always acceptable in every channel (not defects): unsigned binaries that trigger SmartScreen in pre-releases; a console window next to the GUI while `--help`/control output needs it.

## 3. Pre-release checklist

1. `plan/STATUS.md` and `docs/qa/reports/` reviewed: no open S1; S2/S3 items fit the channel (§2).
2. `cargo xtask ci` is green locally and the `CI` workflow is green on the commit to be tagged.
3. Version bumped in the root `Cargo.toml` (`Cargo.lock` updated, committed).
4. `CHANGELOG.md` has a section `## [<version>] - <date>` with Added / Changed / Fixed / Known issues.
5. Smoke test of the release build: `dirustat-cli --help`, a scan of a temp fixture, `dirustat --demo --screenshot <png> --exit-after-screenshot`.
6. `plan/STATUS.md`, `ROADMAP.md`, `log/devlog.md` updated; this document's revision history extended if the rules changed.

## 4. Publishing

Releases are produced by `.github/workflows/release.yml`, triggered by pushing a tag `v*`:

```sh
git tag -a v0.1.0-alpha.1 -m "diRUSTat 0.1.0-alpha.1"
git push origin v0.1.0-alpha.1
```

The workflow checks that the tag matches `Cargo.toml`, builds the release binaries on Windows with `--locked`, smoke-tests them, packs `dirustat-<version>-windows-x86_64.zip` (binaries, licences, README, CHANGELOG) with a `.sha256` file, and creates the GitHub Release with the notes taken from the matching `CHANGELOG.md` section. A version with a `-` suffix is marked as a pre-release automatically.

A published tag is never moved or deleted. A bad release is superseded by the next counter (`alpha.2`) and its notes get a warning line.

## Revision history

- 2026-10-24 · r1: first version (alpha scheme, channel admission table, workflow).
