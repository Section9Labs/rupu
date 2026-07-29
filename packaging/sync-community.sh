#!/usr/bin/env bash
# packaging/sync-community.sh <version> <sha-dir>
#
# Rewrites the version and checksums in the three community package
# definitions from the sidecars the release just published. Hand-editing
# these is how they go stale, and a stale hash surfaces to the user as a
# checksum mismatch they cannot diagnose.
set -euo pipefail

VERSION="${1:?usage: sync-community.sh <version> <sha-dir>}"
SHADIR="${2:?usage: sync-community.sh <version> <sha-dir>}"

sha_for() {
  local asset="$1" f="$SHADIR/$1.sha256"
  test -f "$f" || { echo "missing sidecar: $f" >&2; exit 1; }
  # Sidecar format is "<hex>  <asset-name>"
  awk '{print $1}' "$f"
}

X64=$(sha_for rupu-linux-x64)
ARM64=$(sha_for rupu-linux-arm64)
DARWIN=$(sha_for rupu-darwin-arm64)

for v in "$X64" "$ARM64" "$DARWIN"; do
  [ "${#v}" -eq 64 ] || { echo "not a sha256: $v" >&2; exit 1; }
done

for f in flake.nix packaging/aur/PKGBUILD packaging/homebrew/rupu.rb; do
  test -f "$f" || { echo "missing: $f" >&2; exit 1; }
  # Substitution order is load-bearing: REPLACED_BY_CI is a prefix of the
  # three suffixed tokens below, so it MUST be replaced last. Reversing
  # the order would corrupt every hash (the bare-token substitution would
  # fire first and mangle the suffixed tokens mid-string).
  sed -i.bak \
    -e "s/REPLACED_BY_CI_LINUX_X64/$X64/g" \
    -e "s/REPLACED_BY_CI_LINUX_ARM64/$ARM64/g" \
    -e "s/REPLACED_BY_CI_DARWIN_ARM64/$DARWIN/g" \
    -e "s/REPLACED_BY_CI/$VERSION/g" \
    "$f"
  rm -f "$f.bak"
done

# Nothing may still carry a placeholder — a leftover means a rename broke
# the substitution and the definition would ship unusable.
if grep -rn "REPLACED_BY_CI" flake.nix packaging/aur/PKGBUILD packaging/homebrew/rupu.rb; then
  echo "placeholders remain after substitution" >&2
  exit 1
fi

echo "synced community definitions to $VERSION"
