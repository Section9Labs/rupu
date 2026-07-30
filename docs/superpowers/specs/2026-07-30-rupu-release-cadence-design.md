# Scheduled release cadence — design

**Date:** 2026-07-30
**Status:** Implemented — PR #572. See "As-built corrections" below.
**Surface:** `.github/workflows/`, `scripts/`, `Makefile`

## Problem

Releases are cut by hand. `make gh-beta` / `make gh-stable` build on the
maintainer's Mac and publish through `scripts/gh-build.sh`, so a release only
happens when someone remembers to run it — and there is no cadence anyone can
rely on. Work that lands on `main` can sit unreleased indefinitely.

The publishing machinery already exists: `release.yml` builds and publishes
every platform from a pushed `v*` tag, deriving the channel from the tag
suffix. What is missing is anything that *drives* it.

## Decision

Two scheduled workflows drive the existing `release.yml`:

- **beta — daily**, skipped when there is nothing new or when `main` is red.
- **stable — weekly**, promoting a beta that has already soaked.

Neither builds anything. Both compute and push a tag; `release.yml` does all
the platform work.

## §1 — The two workflows

### `release-beta.yml` — daily

Cron `37 7 * * *`. Deliberately off the `:00` mark and clear of
`nightly-live-tests.yml`'s 08:00 UTC slot.

Publishes only when **all three** hold:

1. `main` has moved since the commit behind the newest `v<base>-beta.*` tag.
2. The base version in `Cargo.toml` is **greater than** the newest stable tag.
   This is the stall-guard: after stable `v0.71.0` ships, a `v0.71.0-beta.4`
   would sort *below* it under semver and walk `rupu update` backwards on the
   beta channel.
3. The latest CI run on `main` concluded `success`.

Any condition false → log which one and `exit 0`. A skipped day is a normal
outcome, not a failure.

Condition 3 is the cheapest safety property available: without it a red `main`
ships a broken beta every morning, and it costs one API call.

The workflow also exposes `workflow_dispatch` with a boolean `force` input.
`force: true` bypasses **condition 1 only** — re-releasing an unchanged `main`
is a legitimate manual act (a botched upload, a signing fix). Conditions 2 and
3 are never bypassable: condition 2 produces a version that sorts backwards,
and condition 3 knowingly ships a broken build. Without this input the manual
escape hatch in §4 would refuse on exactly the day you need it.

### `release-stable.yml` — weekly

Cron `17 9 * * 0` (Sunday). Selects the newest beta tag whose commit is **at
least 2 days old**, re-tags that exact commit as `v<X.Y.Z>`, and lets
`release.yml` publish.

The soak window means a Thursday beta can promote on Sunday but Saturday's
cannot — there has always been a real day of use behind a stable. No eligible
beta → skip the week and log it.

Promotion re-tags the **same commit**, so a stable is always a build that
already shipped as a beta. Artifacts are rebuilt (and freshly signed) from that
SHA rather than copied.

`release.yml` already sets `cancel-in-progress: false`, so a promotion cannot
be half-published.

## §2 — Version arithmetic

`Cargo.toml` remains the single source of the base `X.Y.Z`.

**The beta counter is derived from tags, never stored.** List
`v<base>-beta.*`, take the maximum `N`, publish `N+1`. Nothing to keep in
sync, and a deleted tag self-heals. Numeric prerelease identifiers compare
numerically under semver, so `0.71.0-beta.10 > 0.71.0-beta.9 > 0.71.0-beta.2`,
and all of them sort below `0.71.0`.

After a successful stable promotion, `release-stable.yml` pushes one commit
directly to `main`:

```
chore(release): bump to 0.72.0
```

**Minor, not patch.** rupu ships features weekly and is pre-1.0, so
`0.71.0 → 0.72.0` reads correctly; a patch bump would imply a bugfix-only
week.

### Deliberate carve-out from the never-commit-to-main rule

The project convention is that every change goes through a PR. This bump is an
explicit, approved exception: it is a single mechanical line written by
`github-actions[bot]` immediately after a release it just published, and
routing it through a PR would stall the beta channel until someone merged it.
The exception is scoped to exactly this commit — nothing else in CI may push to
`main`.

**`main` IS protected**, by a repository *ruleset* (not legacy branch
protection — the legacy API returns 404 for it, which misled the original
draft of this spec). GitHub rejects the Actions integration as a ruleset bypass
actor (HTTP 422), so the only credential that can push is a `DeployKey`. Both
the bump commit AND every tag push therefore go over SSH using
`secrets.MAIN_PUSH_KEY`, exactly as `release.yml`'s community-definitions job
already does. A missing secret fails the job loudly.

## §3 — `release.yml` change

Its tag matcher requires the string to *end* in `-beta`, so `v0.71.0-beta.4`
falls through to the error branch today. One case arm widens to accept an
optional `.N`:

```sh
v[0-9]*.[0-9]*.[0-9]*-beta|v[0-9]*.[0-9]*.[0-9]*-beta.[0-9]*)
```

The bare `-beta` form keeps working, so pre-existing tags and manual
`workflow_dispatch` runs are unaffected.

## §4 — Retiring the local publish path

`make gh-beta` and `make gh-stable` build on the maintainer's Mac and publish
via `scripts/gh-build.sh` — a **macOS-only** release, where `release.yml`
publishes every platform. Two publish paths producing different artifact sets
is a drift hazard, and the local one is the path muscle memory currently
reaches for.

Both targets are removed. `make release` stays for local dev builds. The
replacement for a manual publish is:

```
gh workflow run release-beta.yml
```

which produces the same result through the same code path as the scheduled
run.

`scripts/gh-build.sh` is deleted along with the targets — nothing else calls
it.

## §5 — Testing

The interesting logic is version arithmetic and skip conditions, all pure shell
over `git tag` output. It is extracted out of the workflow YAML so it can be
tested without dispatching a workflow:

- `scripts/next-beta-version.sh` — given a base version, emit the next
  `-beta.N`.
- `scripts/pick-promotable-beta.sh` — given a soak window, emit the tag to
  promote, or nothing.

Cases each must cover:

- first beta on a fresh base (`beta.1`, not `beta.0`)
- `beta.9` → `beta.10`, **not** `beta.2` (string vs numeric sort — the bug this
  test exists to catch)
- counter resets after a base bump
- no beta old enough to promote → empty output, exit 0
- the stall-guard: base version equal to the shipped stable → refuse
- a beta tag pointing at a commit no longer on `main` (force-push / rebase) →
  refuse rather than promote unreachable code

The workflows themselves stay thin enough to review by eye.

## Open calls recorded

Settled with rationale, flagged as adjustable:

- **2-day soak.** Arbitrary. Three days would mean only Mon–Thu betas ever
  promote.
- **Minor bump.** See §2.
- **§4 retirement.** Changes an existing habit; the escape hatch becomes
  `gh workflow run`.

## Out of scope

- Changing what `release.yml` builds or which platforms it targets.
- Release notes generation or changelog automation.
- Notarization (`scripts/notarize-release.sh`) — unchanged, still invoked by
  `release.yml` where it already is.

## As-built corrections

Found during implementation and review; all are in PR #572.

1. **Tag pushes must use the deploy key, not the ambient token.** A tag pushed
   with `GITHUB_TOKEN` does **not** trigger another workflow — GitHub exempts
   only `workflow_dispatch` and `repository_dispatch`. The original design would
   have pushed tags that `release.yml` never saw: betas accumulating with no
   releases, every run green, nothing ever published. Both workflows now push
   over SSH with `MAIN_PUSH_KEY`.
2. **`main` is ruleset-protected** — see §2 above. The spec originally asserted
   the opposite.
3. **The bump must be self-healing.** Gating it solely on "a promotion happened"
   deadlocks permanently: with `Cargo.toml` equal to the shipped stable, the beta
   guard skips daily and the promoter's backward guard skips weekly, and nothing
   can ever write `Cargo.toml` again. The bump now also runs whenever the base is
   not greater than the newest stable tag.
4. **The stall-guard compares versions, not equality**, and the bump target is
   `max(current base, minor-bump(promoted))` — deriving it from the promoted
   version alone could rewrite `Cargo.toml` backwards.
5. **A promotion must move forward.** Without a guard, today's tag state would
   have promoted `v0.70.1` as stable and force-moved `latest-stable` *below* the
   already-shipped `v0.71.0`.
6. **Beta tags are annotated.** The soak window reads `creatordate`, which on a
   lightweight tag is the *commit* date — so a 3-day-old commit tagged this
   morning could be promoted 100 minutes later. Annotating makes `creatordate`
   the tag's own age, which is what the window is about.
7. **The CI-green check matches HEAD's SHA**, not merely the most recent run;
   `ci.yml` cancels in progress, so an older commit's success could otherwise
   green-light an untested HEAD.
8. **`release-stable.yml` takes `tag` and `soak_days` dispatch inputs**, so a
   same-day stable is possible. Neither bypasses the backward guard or the
   ancestry check.
9. The bump commit message is `make bump`'s existing
   `release: bump workspace to vX.Y.Z`, not the `chore(release):` form §2
   originally specified.

### Before the cadence can start

`Cargo.toml` is `0.71.0` and `v0.71.0` is already released. The self-healing
bump (correction 3) resolves this on the first stable run, but the fastest path
is to land a `make bump VERSION=0.72.0` on `main`. Until the base exceeds the
newest stable tag, the beta cron correctly skips every morning.

**Do not trust the first green run.** After the first `gh workflow run
release-beta.yml`, confirm a `release.yml` run actually started for the pushed
tag — that is the one behaviour no local test can prove.
