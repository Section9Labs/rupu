#!/usr/bin/env bash
# scripts/gh-build.sh — publish the local release binary to a fixed
# `latest-build` GitHub release that floats over time.
#
# Pre-condition: target/release/rupu has just been built and signed
# (the Makefile's `gh-build` target runs `release` first, which does
# both via `make release` → `cargo build --release` + `scripts/sign-dev.sh`).
#
# What this script does:
#   1. Re-confirms the binary exists and computes a SHA-256 sidecar.
#   2. Force-moves the lightweight tag `latest-build` to the current
#      HEAD (locally and on origin).
#   3. Creates the prerelease `latest-build` if it doesn't exist; if
#      it does, leaves the release in place and just refreshes assets.
#   4. Uploads the binary (named `rupu-<os>-<arch>`) and `.sha256`
#      sidecar via `gh release upload --clobber`, so each invocation
#      replaces the prior asset at the same URL.
#   5. Edits the release notes with the source branch + SHA + bin
#      hash so anyone landing on the release page knows what they're
#      looking at.
#
# Why a rolling tag: the tag is a sharable URL anyone can curl to
# grab the latest local build, but it's NOT a real version tag —
# CHANGELOG and the v0.x.y release line stay clean.

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

# Warn loud if the working tree is dirty — the binary may not match HEAD.
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "warning: working tree is dirty; the published binary may not match \`$SHA_SHORT\`." >&2
fi

echo "→ Hashing binary..."
shasum -a 256 "$BIN" | tee "$BIN.sha256"
BIN_SHA="$(awk '{print $1}' "$BIN.sha256")"

echo "→ Moving rolling tag \`latest-build\` → ${SHA_SHORT}..."
git tag -f latest-build
# Force-push is correct here: latest-build is an explicitly rolling tag.
# Don't use --force-with-lease — its semantics for tags differ from
# branches and would block the legitimate move on a stale lease.
git push --force origin latest-build

# Body is fed to both `release create` (initial) and `release edit`
# (subsequent runs) so the message stays in sync with whatever we
# just published.
NOTES="$(cat <<EOF
Rolling local build of rupu — the tag floats; do not link to it from
the CHANGELOG or version columns. Use the tagged \`v0.x.y-cli\`
releases for stable references.

Built locally from \`${BRANCH}\` @ \`${SHA_SHORT}\` (\`${SHA_FULL}\`).

Asset: \`${ASSET_NAME}\`
SHA-256: \`${BIN_SHA}\`
EOF
)"

if gh release view latest-build >/dev/null 2>&1; then
  echo "→ Release \`latest-build\` exists — refreshing assets + notes..."
  gh release edit latest-build --notes "$NOTES" --prerelease
else
  echo "→ Creating prerelease \`latest-build\`..."
  gh release create latest-build \
    --prerelease \
    --title "rolling local build" \
    --notes "$NOTES"
fi

echo "→ Uploading $ASSET_NAME..."
gh release upload latest-build "$BIN#$ASSET_NAME" --clobber
gh release upload latest-build "$BIN.sha256" --clobber

URL="$(gh release view latest-build --json url --jq '.url')"
echo ""
echo "→ Done: $URL"
