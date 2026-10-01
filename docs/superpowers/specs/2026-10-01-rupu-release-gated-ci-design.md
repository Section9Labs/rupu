# Release-gated CI — design

Status: approved 2026-10-01 (matt). Implemented in the same PR as this spec.

## Problem

Every change paid for CI twice before it could ship, and paid it whether or
not anything shipped:

| Step (runs since 2026-09-28) | Avg wall clock | Runs |
|---|---|---|
| `ci.yml` on the PR — every push, any file, plans included | 11 min | 31 (+5 cancelled) |
| `ci.yml` on `main` after merge — `release-beta.yml` refused to tag without it | 12 min | 13 (+6 cancelled) |
| `release.yml` | 17 min | 6 |

With roughly one beta per PR (14 betas of 0.81.0; 7 on 2026-10-01 alone),
that was ~23 minutes of CI per change, the same suite twice over nearly the
same tree, before the release build even started.

## Decision

CI is a **release gate**, not a merge gate. The full suite runs once per
release, against the exact commit being tagged.

### Pull requests

- `main`'s ruleset keeps `pull_request`, `deletion`, `non_fast_forward`; the
  `required_status_checks` rule is removed. PRs merge as soon as they are
  ready. Local verification remains the expectation before merging.
- `ci.yml`'s `pull_request` trigger is path-filtered to what can break CI or
  the release machinery itself. A `scope` job maps paths to jobs:

  | Paths | Jobs |
  |---|---|
  | `.github/workflows/**`, `docker/**`, `rust-toolchain.toml`, `.cargo/**` | everything (musl build ×2, test, lint, packaging, scripts, keyring) |
  | `packaging/**`, `flake.nix` | community package definitions |
  | `scripts/**` | release cadence scripts |
  | `docs/pages/rupu-archive-keyring.asc` | keyring is public-only |

  The keyring check must stay on PRs: GitHub Pages publishes the file the
  moment it lands on `main`, before any release.
- Opt-in full run on any branch: `gh workflow run ci.yml --ref <branch>`
  (reports on the branch's head commit, so it shows on the PR). Chosen over a
  label because a `paths`-filtered `pull_request` trigger cannot also fire on
  a label for a PR whose paths don't match.

### Release

`release-beta.yml` becomes three jobs, all on the run's `github.sha`:

1. **decide** — stall guard (unchanged), then "something shipped changed":
   find the nearest beta tag in history (`git describe --match
   'v*-beta*'`); skip if HEAD is that commit or if every changed path is
   documentation per `scripts/shipped-paths.sh` (`docs/` except
   `docs/pages/`, and root-level `*.md`; a `.md` under `crates/` is compiled
   in, so the suffix alone never counts). `force` bypasses this; it never
   bypasses the stall guard or CI. Skips are written to the step summary.
2. **full CI** — `uses: ./.github/workflows/ci.yml` (`workflow_call`).
   Replaces the old "is there a green ci.yml run for this SHA" query, and with
   it the failure mode where dispatching before CI finished reported success
   while cutting nothing.
3. **push the tag** — only if every CI job succeeded, on `github.sha`
   explicitly, so what was tested is what is tagged even if `main` moved.

A `dry_run` input runs decide + full CI and never tags; it is the only mode
allowed off `main`, which makes the whole gate testable on a branch.

`ci.yml` has no `push` trigger. The test step runs `--no-fail-fast`, since a
red release over a batch must name every failing binary at once.

### Caches

A called workflow inherits the caller's ref, so the release gate's CI runs on
`refs/heads/main`, satisfies the existing `save-if: main`, and writes the
`musl-release-*` / `darwin-release` caches minutes before `release.yml`'s tag
build restores them. The #684 warmer design is unchanged; only its trigger
moved from "every push to main" to "every release".

## Rejected: a `release` branch fed by release PRs

A `main` → `release` PR would run CI on the PR, but GitHub scopes a PR run's
caches to the PR ref, and tag runs can only read default-branch caches. The
release build would then either build cold (33–45 min, the pre-#684 numbers)
or need CI to run again on the push to `release`, which is the duplicate run this
change exists to remove. It also adds merge commits and a second branch for
every session to reason about. The beta tags already mark "last released"
(`git log <last beta>..main` is the next batch).

## Trade-offs accepted

- `main` can be red between releases; fixes go forward. A session branching
  from a red `main` may see failures that are not its own.
- Interactions between parallel PRs surface at release time, not merge time.
- CI's clippy is the pinned 1.95; local Homebrew rustc is newer and misses
  some lints, which now fail a release rather than a PR. Fixing the local
  toolchain is a separate item.

## Verification

- `scripts/tests/release-cadence-tests.sh` covers `shipped-paths.sh`.
- The decide step was exercised against real history: unchanged `main`
  (skip), docs-only commit (skip), code commit (`go`, next counter), `force`
  on unchanged (`go`), and a 609-path batch (the `printf | head` SIGPIPE
  abort under `pipefail` it would have hit is fixed).
- This PR touches `.github/workflows/`, so its own PR run exercises `scope`
  and the full suite; `release-beta.yml -f dry_run=true` on the branch
  exercises the `workflow_call` wiring before merge.
- After merge: the `main` ruleset is re-applied from
  `.github/rulesets/main.json` (drops the required checks), and the next beta
  is cut through the new gate.
