# Releasing rupu

Releases are cut by GitHub Actions, on a schedule. Nothing is built or
uploaded from a laptop.

The CLI is published by **`.github/workflows/release.yml`**, triggered by
pushing a `v*` tag. It builds every platform, signs and notarizes the macOS
binary, publishes the GitHub release and the `.deb`/`.rpm` + apt/yum repos,
and (for **stable** tags only) syncs the Nix flake, the AUR `PKGBUILD` and the
Homebrew formula.

rupu.app has its own lane, **`release-app.yml`**, which is triggered by
`release.yml` *completing successfully* rather than by the tag itself. It
attaches `rupu-app-darwin-arm64.dmg`/`.zip` to the release `release.yml` just
made — and only when something under `apps/rupu-macos/` (or the packaging
script, the `Makefile`, or the lane itself) changed since the newest release
*on the same channel* that carries an app. A CLI-only change therefore never
builds, notarizes, or can be failed by, the app; and a release with no
`rupu-app-*` asset simply means the app did not change — the rolling tag still
carries the newest bundle. The app's own version string lags the CLI's between
app changes; its minimum-CLI-version gate is what actually has to be right.

Two scheduled workflows do nothing but decide which tag to push:

| Workflow | Schedule | What it does |
| --- | --- | --- |
| `release-beta.yml` | daily, 07:37 UTC | pushes `v<base>-beta.N` for today's `main` |
| `release-stable.yml` | Sundays, 09:17 UTC | re-tags a soaked beta's commit as `v<X.Y.Z>`, then bumps `main`'s base version |

So the normal flow is: merge a PR → tomorrow morning a beta ships → after it
has soaked two days, a Sunday promotes exactly that build to stable →
Homebrew/AUR/Nix follow within the same run.

## Why both schedulers push over SSH

Both workflows push their tag with the `MAIN_PUSH_KEY` deploy key, **not** the
ambient `GITHUB_TOKEN`. This is not optional and must not be "simplified":

> Events triggered by the `GITHUB_TOKEN` will not create a new workflow run,
> with the following exceptions: `workflow_dispatch` and `repository_dispatch`.

`release.yml` triggers on `push: tags`. A tag pushed with the ambient token
therefore appears in the repository and publishes **nothing**, while the job
that pushed it goes green. If `MAIN_PUSH_KEY` is missing, both workflows fail
loudly rather than push a tag that would silently do nothing.

`MAIN_PUSH_KEY` is a deploy key with write access to this repository, and it
is also the only actor allowed to bypass the ruleset protecting `main` (see
`.github/rulesets/README.md`) — GitHub rejects the Actions integration as a
bypass actor, so a deploy key is the only credential that can express it.

## The daily beta

`release-beta.yml` cuts `v<base>-beta.N`, where `<base>` is `Cargo.toml`'s
workspace version and `N` is derived from the existing tag list by
`scripts/next-beta-version.sh` (never stored, so deleting a tag simply lowers
the maximum). The tag is **annotated** — `release-stable.yml`'s soak window
reads the tag's own creation date, which a lightweight tag does not have.

It skips — logging which condition fired and exiting **0**, because a skipped
day is a normal day — when any of:

1. **`main` has not moved** since the last beta of this base. Bypassable with
   the `force` input, for re-cutting after a botched upload.
2. **`Cargo.toml`'s version is not newer than the newest stable tag.** A
   `v0.71.0-beta.N` sorts below the shipped `v0.71.0` and would walk
   `rupu update` backwards on the beta channel. Not bypassable; the cure is
   the version bump, which `release-stable.yml` owns.
3. **`ci.yml` has not passed for this exact HEAD commit.** The check is by
   head SHA, not "the newest run on `main`" — `ci.yml` uses
   `cancel-in-progress`, so the newest run on the branch is frequently
   `cancelled` and can belong to an older commit. An unanswerable query
   (permissions, rate limit) counts as not-green. Not bypassable.

## The weekly stable promotion

`release-stable.yml` picks the newest beta that has existed for at least
`SOAK_DAYS` (2) via `scripts/pick-promotable-beta.sh`, and re-tags **that
beta's commit** as `vX.Y.Z`. Stable is therefore byte-identical in content to
a beta people have already been running for two days; it is never a fresh
build of `main`'s head.

Guards, in order:

1. **Soak** — nothing old enough is a normal week: log and exit 0.
2. **Backward guard** — a version not newer than the newest stable tag is
   skipped rather than published, so `latest-stable` can never move backwards.
   This runs *before* the ancestry check on purpose: a stale beta left by a
   rebase is both backwards and unreachable, and failing on it would turn
   every subsequent Sunday red with no way to self-heal.
3. **Ancestry** — a beta whose commit is no longer reachable from `main`
   (force-push, rebase) is a hard **failure**. Its version is newer than
   anything shipped, so skipping it would stall the stable channel silently.

### The version bump on `main`

After promoting, the workflow pushes one commit to `main`: `make bump
VERSION=<next>`. This is the only commit any automation may push to `main`.

The target is `max(current Cargo.toml version, minor-bump of the version being
promoted)` — never lower than what `Cargo.toml` already holds, so a human who
has already bumped `main` further ahead is not overwritten backwards.

The bump also runs on weeks where **nothing** was promoted, whenever `main`'s
base is not ahead of the newest stable tag. That is deliberate and load-
bearing: in that state the daily beta's condition 2 skips every morning, and
nothing else in the repo will ever write `Cargo.toml` again. Gating the bump
solely on "a promotion happened" makes the very first run — and any single
failed bump — a permanent, all-green deadlock in which nothing is ever
published again.

## Manual escape hatches

All of these are `gh workflow run`; none of them build anything locally.

```bash
# Cut a beta right now, even if main has not moved.
# (Conditions 2 and 3 — version stall and red CI — still apply.)
gh workflow run release-beta.yml -f force=true

# Promote the newest soaked beta right now, off-schedule.
gh workflow run release-stable.yml

# Ship a same-day stable: promote one specific beta, ignoring the soak window.
# The backward guard and the ancestry check STILL apply.
gh workflow run release-stable.yml -f tag=v0.72.0-beta.3

# Same, but only relax the soak window rather than naming a tag.
gh workflow run release-stable.yml -f soak_days=0

# Re-publish an existing tag (a failed upload, a runner flake). CLI only —
# a dispatch runs from main, so release-app.yml cannot learn the tag from it.
gh workflow run release.yml -f tag=v0.72.0

# Build and attach rupu.app to an existing tag's release. Always builds
# (the change gate only applies to the automatic path).
gh workflow run release-app.yml -f tag=v0.72.0

# Dry-run the app lane against an existing tag: build, sign, notarize and
# staple exactly as a release would, publish nothing.
gh workflow run release-app.yml -f tag=v0.72.0 -f app_dry_run=true
```

A bad value for `tag` or `soak_days` is operator error and fails the run
loudly — unlike the scheduled skip conditions, which exit 0.

To ship a hotfix as stable the same day: merge the fix, wait for `ci.yml`,
`gh workflow run release-beta.yml -f force=true`, then once that beta has
published, `gh workflow run release-stable.yml -f tag=<that beta>`.

## Bumping the version by hand

```bash
make bump VERSION=X.Y.Z    # rewrites Cargo.toml + Cargo.lock, commits
```

Push it on a branch and merge via PR like any other change. The next morning's
beta picks the new base up automatically. Note that `make cp-web` must have
run if the embedded CP UI changed — `release.yml` builds it in the `web` job,
so this only matters for local testing.

## The helper scripts

`scripts/next-beta-version.sh` and `scripts/pick-promotable-beta.sh` are pure:
they never call `git` or `gh`, they read their tag list on stdin, and `now` is
a parameter. That is what makes them testable without a repository.
`scripts/tests/release-cadence-tests.sh` runs on every PR via `ci.yml`'s
`release-scripts` job — these scripts decide what gets published unattended,
so a counter regression must not reach `main`.

## Secrets

Set under repo Settings → Secrets and variables → Actions.

**Release (`release.yml`, `release-beta.yml`, `release-stable.yml`):**

- `MAIN_PUSH_KEY` — deploy key with write access to **this** repository.
  Required by both schedulers (tag pushes) and by `release.yml`'s `community`
  job (the definitions commit to `main`). Absent → those jobs fail; a stale
  `flake.nix` on `main` means `nix run github:Section9Labs/rupu` installs the
  wrong version, and a wrong install is worse than a missing one.
- `APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD` — Developer ID Application
  cert for signing the macOS binaries.
- `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`, `APPLE_API_KEY_BASE64` —
  App Store Connect API key for `notarytool`. The CLI binary has no
  `stapler` step — bare command-line binaries cannot be stapled, so its
  ticket is served online and Gatekeeper checks it on first run. rupu.app
  IS stapled (`release-app.yml`'s `macos-app` job staples both the `.app`
  bundle and the DMG after notarization; app bundles and disk images accept
  tickets). Both workflows read the same Apple secrets.
- `GPG_PRIVATE_KEY`, `GPG_KEY_ID` — signs the apt/yum repository metadata.
- `AUR_SSH_PRIVATE_KEY`, `AUR_USERNAME`, `AUR_EMAIL` — deploy key and commit
  identity for the `rupuaur` AUR account. `.SRCINFO` is regenerated inside a
  throwaway `archlinux:latest` container (`makepkg --printsrcinfo` only runs
  on Arch — it is never hand-written).
- `TAP_SSH_PRIVATE_KEY` — deploy key with write access to
  `Section9Labs/homebrew-tap`. A cross-repo push always needs its own
  credential; the ambient `GITHUB_TOKEN` is scoped to this repository
  whatever the tap's visibility. If absent, that step logs and exits 0 — a
  formula one version behind is a nuisance, a red release blocks every other
  asset for everyone.

Deploy keys rather than PATs throughout, deliberately: each writes to exactly
one repository, none expire, and revoking one touches no other account.

**Nightly live-API tests (`nightly-live-tests.yml`):**

- `RUPU_LIVE_ANTHROPIC_KEY`
- `RUPU_LIVE_OPENAI_KEY`
- `RUPU_LIVE_GEMINI_KEY`
- `RUPU_LIVE_COPILOT_TOKEN`
- `RUPU_LIVE_GITHUB_TOKEN` — PAT, scopes: repo + read:user + read:org
- `RUPU_LIVE_GITLAB_TOKEN` — PAT, scopes: api + read_user + read_repository

## Smoking a release

Assets are bare binaries (`rupu-darwin-arm64`, `rupu-linux-x64`,
`rupu-linux-arm64`), the macOS app (`rupu-app-darwin-arm64.dmg` and `.zip` —
signed, notarized, stapled), plus `.deb`/`.rpm` packages, each with a
`.sha256`. The app assets arrive a few minutes after the CLI ones, from
`release-app.yml`, and only on releases where the app changed (check that
workflow's run for the "unchanged since" notice if they are absent). When
they are there, smoke them too: download the DMG, `hdiutil attach` it, and
`spctl -a -t exec -vv` the mounted `rupu.app` — Gatekeeper must accept it
with `source=Notarized Developer ID`:

```bash
TAG=v0.72.0
curl -fsSL -o /tmp/rupu \
  "https://github.com/Section9Labs/rupu/releases/download/${TAG}/rupu-darwin-arm64"
chmod +x /tmp/rupu
/tmp/rupu --version
```

`release.yml` also publishes a rolling `latest-stable` / `latest-beta` release
alongside the versioned one, so a download URL can be pinned to "newest".
`rupu update` itself resolves by semver plus the release's `prerelease` flag,
so that flag — derived from the `-beta` tag suffix — is the part that has to
be right.

Or `rupu update`, which follows the configured `[update].channel`
(`stable` by default, `beta` for the daily builds).
