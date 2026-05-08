#!/usr/bin/env bash
# scripts/gh-build.sh — publish the local release binary to GitHub
# under TWO releases:
#
#   1. `latest-build`            — rolling tag, force-moved on every
#                                  run. Stable URL for "give me the
#                                  freshest local build."
#   2. `v<X.Y.Z>-build`          — versioned tag, derived from the
#                                  workspace `[workspace.package].version`
#                                  in Cargo.toml. Stable per-version
#                                  reference. Re-running at the same
#                                  Cargo version overwrites the same
#                                  versioned release; bumping via
#                                  `make bump` creates a new one.
#
# Both publish the SAME binary + SHA-256 sidecar. Use `latest-build`
# for "always current" links, `v<X.Y.Z>-build` for "pin to this
# specific build" references in chat / runbooks / etc.
#
# Pre-condition: target/release/rupu has just been built and signed
# (the Makefile's `gh-build` target runs `release` first, which does
# both via `make release` → `cargo build --release` + `scripts/sign-dev.sh`).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

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
VERSIONED_TAG="v${WORKSPACE_VERSION}-build"

# Warn loud if the working tree is dirty — the binary may not match HEAD.
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "warning: working tree is dirty; the published binary may not match \`$SHA_SHORT\`." >&2
fi

echo "→ Hashing binary..."
shasum -a 256 "$BIN" | tee "$BIN.sha256"
BIN_SHA="$(awk '{print $1}' "$BIN.sha256")"

# Common notes body, reused for both release upserts.
NOTES_ROLLING="$(cat <<EOF
Rolling local build of rupu — the tag floats; do not link to it from
the CHANGELOG or version columns. Use the tagged \`v0.x.y-cli\`
releases for stable references.

Built locally from \`${BRANCH}\` @ \`${SHA_SHORT}\` (\`${SHA_FULL}\`).
Workspace version: \`${WORKSPACE_VERSION}\`.

Asset: \`${ASSET_NAME}\`
SHA-256: \`${BIN_SHA}\`
EOF
)"

NOTES_VERSIONED="$(cat <<EOF
Local build of rupu pinned to workspace version \`${WORKSPACE_VERSION}\`.
This tag is overwritten if you re-run \`make gh-build\` at the same
Cargo version; bump via \`make bump VERSION=<new>\` to start a new
versioned release. Use this URL when you want a stable per-version
reference; use \`latest-build\` when you want the freshest local
build regardless of version.

Built locally from \`${BRANCH}\` @ \`${SHA_SHORT}\` (\`${SHA_FULL}\`).

Asset: \`${ASSET_NAME}\`
SHA-256: \`${BIN_SHA}\`
EOF
)"

# Upsert + upload to a single release tag. Used for both the rolling
# `latest-build` tag and the versioned `v<X.Y.Z>-build` tag — same
# binary, different semantics on tag movement (rolling vs pinned).
publish_release() {
  local tag="$1"
  local title="$2"
  local notes="$3"
  local rolling="$4"  # "rolling" or "versioned" — controls force-move semantics

  echo "→ ${tag}: tagging HEAD ${SHA_SHORT}..."
  if [[ "$rolling" == "rolling" ]]; then
    # Rolling tag: force-move every run. Don't use --force-with-lease;
    # tag semantics for it differ from branches and would block the
    # legitimate move on a stale lease.
    git tag -f "$tag"
    git push --force origin "$tag"
  else
    # Versioned tag: re-create only if absent, otherwise force-move so
    # re-runs at the same Cargo version still publish the latest binary
    # under the same versioned URL. Bumping via `make bump` creates a
    # new tag for the new version.
    git tag -f "$tag"
    git push --force origin "$tag"
  fi

  if gh release view "$tag" >/dev/null 2>&1; then
    echo "→ Release \`${tag}\` exists — refreshing notes..."
    gh release edit "$tag" --notes "$notes" --prerelease >/dev/null
  else
    echo "→ Creating prerelease \`${tag}\`..."
    gh release create "$tag" \
      --prerelease \
      --title "$title" \
      --notes "$notes" >/dev/null
  fi

  echo "→ Uploading $ASSET_NAME → $tag..."
  gh release upload "$tag" "$BIN#$ASSET_NAME" --clobber >/dev/null
  gh release upload "$tag" "$BIN.sha256" --clobber >/dev/null
}

publish_release "latest-build" "rolling local build" "$NOTES_ROLLING" "rolling"
publish_release "$VERSIONED_TAG" "rupu ${VERSIONED_TAG}" "$NOTES_VERSIONED" "versioned"

LATEST_URL="$(gh release view latest-build --json url --jq '.url')"
VERSIONED_URL="$(gh release view "$VERSIONED_TAG" --json url --jq '.url')"
echo ""
echo "→ Rolling:    $LATEST_URL"
echo "→ Versioned:  $VERSIONED_URL"
