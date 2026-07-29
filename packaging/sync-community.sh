#!/usr/bin/env bash
# packaging/sync-community.sh <version> <sha-dir> [root]
#
# Regenerates the three community package definitions from the templates in
# packaging/templates/, filling in the version and the checksums from the
# sidecars the release just published:
#
#   packaging/templates/flake.nix.in -> <root>/flake.nix
#   packaging/templates/PKGBUILD.in  -> <root>/packaging/aur/PKGBUILD
#   packaging/templates/rupu.rb.in   -> <root>/packaging/homebrew/rupu.rb
#
# ALWAYS regenerating from the templates is what makes this repeatable. An
# earlier version substituted in place, over the committed files: that worked
# exactly once — after the first stable release the placeholders were gone,
# every subsequent run was a silent no-op, and Nix/AUR/Homebrew users stayed
# pinned to the first version forever with no error anywhere. The templates
# are now the only place the REPLACED_BY_CI* tokens live, so a run always
# starts from a file that still has them.
#
# Hand-editing the generated files is the other way they go stale, and a
# stale hash surfaces to the user as a checksum mismatch they cannot
# diagnose — hence the generated-file header each template carries.
#
# <root> defaults to the repo this script lives in. Pass a scratch directory
# to render without touching the checkout (see packaging/test-sync-community.sh).
set -euo pipefail

VERSION="${1:?usage: sync-community.sh <version> <sha-dir> [root]}"
SHADIR="${2:?usage: sync-community.sh <version> <sha-dir> [root]}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATES="$HERE/templates"
ROOT="${3:-$(cd "$HERE/.." && pwd)}"

# The version is substituted with sed, and it is stamped into URLs and a
# pkgver. Anything outside this set is either a sed-injection or an invalid
# pkgver, and both are better caught here than in a user's package manager.
case "$VERSION" in
  *[!0-9A-Za-z.+~_-]* | "")
    echo "not a usable version string: '$VERSION'" >&2; exit 1 ;;
esac

sha_for() {
  local f="$SHADIR/$1.sha256"
  test -f "$f" || { echo "missing sidecar: $f" >&2; exit 1; }
  # Sidecar format is "<hex>  <asset-name>"
  awk '{print $1}' "$f"
}

X64=$(sha_for rupu-linux-x64)
ARM64=$(sha_for rupu-linux-arm64)
DARWIN=$(sha_for rupu-darwin-arm64)

# Exactly 64 lowercase hex characters, nothing else. The per-field assertions
# below compare against these values, so validating them once here is what
# makes "the field holds 64 hex characters" true by construction.
for v in "$X64" "$ARM64" "$DARWIN"; do
  if [ "${#v}" -ne 64 ] || [ -n "${v//[0-9a-f]/}" ]; then
    echo "not a sha256: '$v'" >&2; exit 1
  fi
done

fail() {
  echo "::error::$*" >&2
  exit 1
}

render() {
  local tpl="$TEMPLATES/$1" out="$ROOT/$2"
  test -f "$tpl" || fail "missing template: $tpl"
  mkdir -p "$(dirname "$out")"
  # Substitution order is load-bearing: REPLACED_BY_CI is a prefix of the
  # three suffixed tokens, so it MUST be replaced last. If it fired first it
  # would consume the prefix of every hash token and leave e.g.
  # `0.71.0_LINUX_X64` in a sha256 field — which contains no REPLACED_BY_CI,
  # so the placeholder grep below would NOT catch it. The per-field
  # assertions in check_* are what actually catch that; the grep only catches
  # a token that was never substituted at all (a rename).
  sed -e "s/REPLACED_BY_CI_LINUX_X64/$X64/g" \
      -e "s/REPLACED_BY_CI_LINUX_ARM64/$ARM64/g" \
      -e "s/REPLACED_BY_CI_DARWIN_ARM64/$DARWIN/g" \
      -e "s/REPLACED_BY_CI/$VERSION/g" \
      "$tpl" > "$out.tmp"
  mv "$out.tmp" "$out"

  if grep -n "REPLACED_BY_CI" "$out"; then
    fail "$2: placeholders remain after substitution"
  fi
}

# Version is regex-escaped for the assertions: `.` in "0.71.0" would
# otherwise match any character.
ESCVER=$(printf '%s' "$VERSION" | sed 's/[][^$.*+?(){}|\\]/\\&/g')

# Each assertion pins a specific hash to the specific platform field that must
# carry it. A hash landing in the wrong field still passes a "is it 64 hex
# chars" check and is undiagnosable for the user who hits it.
assert() {
  local file="$1" what="$2" pattern="$3"
  grep -Eq -- "$pattern" "$ROOT/$file" || {
    echo "--- $file ---" >&2
    cat "$ROOT/$file" >&2
    fail "$file: $what does not hold the expected value"
  }
}

check_flake() {
  assert flake.nix "version" "^ *version = \"$ESCVER\";\$"
  assert flake.nix "x86_64-linux sha256" "\"x86_64-linux\"[[:space:]]*=[[:space:]]*\"$X64\";"
  assert flake.nix "aarch64-linux sha256" "\"aarch64-linux\"[[:space:]]*=[[:space:]]*\"$ARM64\";"
  assert flake.nix "aarch64-darwin sha256" "\"aarch64-darwin\"[[:space:]]*=[[:space:]]*\"$DARWIN\";"
}

check_pkgbuild() {
  local f=packaging/aur/PKGBUILD
  assert "$f" "pkgver" "^pkgver=$ESCVER\$"
  assert "$f" "sha256sums_x86_64" "^sha256sums_x86_64=\('$X64'\)\$"
  assert "$f" "sha256sums_aarch64" "^sha256sums_aarch64=\('$ARM64'\)\$"
}

check_formula() {
  local f="$ROOT/packaging/homebrew/rupu.rb"
  assert packaging/homebrew/rupu.rb "version" "^ *version \"$ESCVER\"\$"
  # Pair every `url` with the `sha256` that follows it, so the check is about
  # correspondence rather than about line numbers: the formula selects its
  # asset by block (on_macos/on_arm, on_linux/on_intel, ...) and the hash must
  # belong to the asset named in the url directly above it.
  local got want
  got=$(awk '
    /url "/       { if (match($0, /rupu-[a-z0-9-]+"/)) asset = substr($0, RSTART, RLENGTH - 1) }
    /^ *sha256 "/ { h = $2; gsub(/"/, "", h); print asset "=" h }
  ' "$f" | sort)
  want=$(printf '%s\n' \
    "rupu-darwin-arm64=$DARWIN" \
    "rupu-linux-x64=$X64" \
    "rupu-linux-arm64=$ARM64" | sort)
  if [ "$got" != "$want" ]; then
    echo "--- url/sha256 pairs in packaging/homebrew/rupu.rb ---" >&2
    echo "got:"  >&2; printf '%s\n' "$got"  >&2
    echo "want:" >&2; printf '%s\n' "$want" >&2
    fail "packaging/homebrew/rupu.rb: a hash landed on the wrong asset"
  fi
}

render flake.nix.in flake.nix
render PKGBUILD.in  packaging/aur/PKGBUILD
render rupu.rb.in   packaging/homebrew/rupu.rb

check_flake
check_pkgbuild
check_formula

echo "synced community definitions to $VERSION (root: $ROOT)"
