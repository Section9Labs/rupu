#!/usr/bin/env bash
# scripts/gh-build.sh <beta|stable> — publish the local release binary to
# GitHub under a channel:
#
#   beta:   prerelease channel. Publishes both a rolling `latest-beta` tag
#           (force-moved every run) and a versioned `v<X.Y.Z>-beta` tag,
#           derived from the workspace `[workspace.package].version` in
#           Cargo.toml. Marked `--prerelease` on GitHub.
#
#   stable: full release channel. Publishes both a rolling `latest-stable`
#           tag and a versioned `v<X.Y.Z>` tag (no `-beta` suffix). This is
#           NOT marked `--prerelease` — it's a real release.
#
# Both channels publish the SAME asset shape: `rupu-<os>-<arch>` + a
# `.sha256` sidecar. Use the rolling tag for "always current on this
# channel" links, the versioned tag to pin to a specific release.
#
# `rupu update` (crates/rupu-update) resolves the configured
# `[update].channel` against these releases via the GitHub API by
# semver + the `prerelease` flag on each release — it does not depend on
# the rolling tag names, but the channel naming here (`beta`/`stable`)
# must keep matching `rupu_update::model::Channel`.
#
# Pre-condition: target/release/rupu has just been built — with
# RUPU_RELEASE_CHANNEL/RUPU_RELEASE_VERSION set in the environment so the
# binary embeds its own channel/version (see
# crates/rupu-cli/src/build_info.rs) — and signed via scripts/sign-dev.sh.
# The Makefile's `gh-beta` / `gh-stable` targets do all three steps in
# order; don't invoke this script directly unless you've done the same.
#
# Deprecated: `make gh-build` is now an alias for `make gh-beta` (betas
# used to be tagged `-build`; that convention is retired in favor of the
# explicit `beta`/`stable` channel names).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

CHANNEL="${1:?usage: gh-build.sh <beta|stable>}"

# Per-channel tag/flag derivation. CREATE_PRE_FLAG / EDIT_PRE_FLAG are
# intentionally left unquoted at their call sites below (word-splitting
# an optional flag) rather than kept in a bash array, so this keeps
# working under the bash 3.2 that ships as macOS's default `bash`
# (arrays expanded with "${arr[@]}" under `set -u` misbehave on empty
# arrays pre-4.4).
case "$CHANNEL" in
  beta)
    TAG_SUFFIX="-beta"
    ROLLING_TAG="latest-beta"
    CREATE_PRE_FLAG="--prerelease"
    EDIT_PRE_FLAG="--prerelease"
    ;;
  stable)
    TAG_SUFFIX=""
    ROLLING_TAG="latest-stable"
    CREATE_PRE_FLAG=""
    EDIT_PRE_FLAG="--prerelease=false"
    ;;
  *)
    echo "channel must be 'beta' or 'stable' (got: $CHANNEL)" >&2
    exit 1
    ;;
esac

BIN="target/release/rupu"
if [[ ! -x "$BIN" ]]; then
  echo "$BIN missing or not executable — run \`make release\` first." >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "gh CLI not installed — \`brew install gh\` then \`gh auth login\`." >&2
  exit 1
fi

# `gh auth status` exits 0 only when authenticated to the active host.
if ! gh auth status >/dev/null 2>&1; then
  echo "gh CLI not authenticated — run \`gh auth login\`." >&2
  exit 1
fi

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
ASSET_NAME="rupu-${OS}-${ARCH}"

SHA_FULL="$(git rev-parse HEAD)"
SHA_SHORT="$(git rev-parse --short HEAD)"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"

# Read the workspace version from Cargo.toml. The grep is anchored to
# `^version = "..."` which the workspace `[workspace.package]` block
# is the only owner of (per-crate `Cargo.toml`s use `version.workspace
# = true`). If we ever stop satisfying that invariant the assertion
# below catches it before we publish a wrongly-tagged release.
WORKSPACE_VERSION="$(grep -E '^version = "[0-9]+\.[0-9]+\.[0-9]+' Cargo.toml | head -n1 | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+[^"]*)".*/\1/')"
if [[ -z "$WORKSPACE_VERSION" ]]; then
  echo "could not parse workspace version from Cargo.toml — expected a line like 'version = \"X.Y.Z\"'" >&2
  exit 1
fi
VERSIONED_TAG="v${WORKSPACE_VERSION}${TAG_SUFFIX}"

# Warn loud if the working tree is dirty — the binary may not match HEAD.
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "warning: working tree is dirty; the published binary may not match \`$SHA_SHORT\`." >&2
fi

echo "→ Hashing binary..."
shasum -a 256 "$BIN" | tee "$BIN.sha256"
BIN_SHA="$(awk '{print $1}' "$BIN.sha256")"

# Common notes body, reused for both release upserts.
NOTES_ROLLING="$(cat <<EOF
Rolling ${CHANNEL} build of rupu — the tag floats; do not link to it from
the CHANGELOG or version columns. Use the tagged \`${VERSIONED_TAG}\`
release for stable references.

Built locally from \`${BRANCH}\` @ \`${SHA_SHORT}\` (\`${SHA_FULL}\`).
Workspace version: \`${WORKSPACE_VERSION}\`.
Channel: \`${CHANNEL}\`.

Asset: \`${ASSET_NAME}\`
SHA-256: \`${BIN_SHA}\`
EOF
)"

NOTES_VERSIONED="$(cat <<EOF
${CHANNEL} release of rupu pinned to workspace version \`${WORKSPACE_VERSION}\`.
This tag is overwritten if you re-run \`make gh-${CHANNEL}\` at the same
Cargo version; bump via \`make bump VERSION=<new>\` to start a new
versioned release. Use this URL when you want a stable per-version
reference; use \`${ROLLING_TAG}\` when you want the freshest ${CHANNEL}
build regardless of version.

Built locally from \`${BRANCH}\` @ \`${SHA_SHORT}\` (\`${SHA_FULL}\`).
Channel: \`${CHANNEL}\`.

Asset: \`${ASSET_NAME}\`
SHA-256: \`${BIN_SHA}\`
EOF
)"

# Upsert + upload to a single release tag. Used for both the rolling
# tag and the versioned tag — same binary, different semantics on tag
# movement (rolling vs pinned).
publish_release() {
  local tag="$1"
  local title="$2"
  local notes="$3"

  echo "→ ${tag}: tagging HEAD ${SHA_SHORT}..."
  # Force-move: the rolling tag floats every run by definition; the
  # versioned tag re-creates only if absent, otherwise force-moves so
  # re-runs at the same Cargo version still publish the latest binary
  # under the same versioned URL. Bumping via `make bump` creates a new
  # tag for the new version.
  git tag -f "$tag"
  git push --force origin "$tag"

  if gh release view "$tag" >/dev/null 2>&1; then
    echo "→ Release \`${tag}\` exists — refreshing notes..."
    # shellcheck disable=SC2086
    gh release edit "$tag" --notes "$notes" $EDIT_PRE_FLAG >/dev/null
  else
    echo "→ Creating ${CHANNEL} release \`${tag}\`..."
    # shellcheck disable=SC2086
    gh release create "$tag" $CREATE_PRE_FLAG \
      --title "$title" \
      --notes "$notes" >/dev/null
  fi

  echo "→ Uploading $ASSET_NAME → $tag..."
  gh release upload "$tag" "$BIN#$ASSET_NAME" --clobber >/dev/null
  gh release upload "$tag" "$BIN.sha256" --clobber >/dev/null
}

publish_release "$ROLLING_TAG" "rolling ${CHANNEL} build" "$NOTES_ROLLING"
publish_release "$VERSIONED_TAG" "rupu ${VERSIONED_TAG}" "$NOTES_VERSIONED"

LATEST_URL="$(gh release view "$ROLLING_TAG" --json url --jq '.url')"
VERSIONED_URL="$(gh release view "$VERSIONED_TAG" --json url --jq '.url')"
echo ""
echo "→ Rolling:    $LATEST_URL"
echo "→ Versioned:  $VERSIONED_URL"
