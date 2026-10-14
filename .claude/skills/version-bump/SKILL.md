---
name: version-bump
description: Bump the workspace version after every fix or feature in everythux (feature = minor, fix = build/patch), keep Cargo.lock and ROADMAP.md in step, and commit it. Use right after finishing and verifying a change, before pushing or opening a PR, or when asked to bump, release or tag a version.
---

# Bump the version after each change

Every fix and feature that lands gets a version bump, in the same branch as the change.
The version lives in one place: `[workspace.package] version` in the root `Cargo.toml`. Every crate
inherits it.

## 1. Pick the bump

| Change | Bump | Example |
|--------|------|---------|
| New feature, new setting, new UI capability, config schema change | **minor**, patch resets to 0 | `0.5.2` → `0.6.0` |
| Bug fix, crash fix, perf fix, UI correction | **build** (the third number) | `0.5.1` → `0.5.2` |
| Docs, comments, tests only, refactor with no behaviour change, CI | **none** | |

- Mixed change (a feature and a fix together): the feature wins, so bump the minor.
- The major stays at 0 until the user says otherwise. Never bump it on your own.
- Start from the version on the base branch (`git show main:Cargo.toml`), not from the branch. If the
  branch already carries a bump, recompute from the base so one branch means one bump of the highest
  kind. A feature after an earlier fix bump on the same branch changes `0.5.2` to `0.6.0`, not `0.6.1`.

## 2. Apply it

1. Edit `version` in the root `Cargo.toml`.
2. Run `cargo check --workspace` so `Cargo.lock` picks up the new crate versions. Never edit
   `Cargo.lock` by hand.
3. In `ROADMAP.md`, add or update the line in the "Implementation status" section for that version
   (`- 0.6.0: <what the change does, one line>`). Tick the checkbox of any roadmap task you finished.
4. Mention the user-visible change in `README.md` if it has one (a key, a menu entry, a setting).

## 3. Commit

- Run the gate first (see `rust-verify`). The bump is the last commit before pushing.
- Own commit, subject `Bump the version to X.Y.Z`, with a short body naming the change that caused it.
- Files in the commit: `Cargo.toml`, `Cargo.lock`, `ROADMAP.md`.
- Do **not** create a git tag or a GitHub release. The release workflow is tag-driven, so tagging
  happens only when the user asks, and after the PR has merged.

## 4. Checks

- `git diff --stat` shows only the three files above.
- `cargo metadata --no-deps --format-version 1 | grep -o '"version":"[^"]*"' | sort -u` shows one
  version for the whole workspace.
