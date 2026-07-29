#!/usr/bin/env bash
# packaging/test-sync-community.sh
#
# Regression test for the defect that made community publishing work exactly
# once: sync-community.sh used to substitute placeholders in the committed
# files, so after the first stable release there were no placeholders left,
# every later run was a silent no-op, and every guard still passed.
#
# The test runs the sync TWICE against a scratch tree with different versions
# and different hashes and asserts run 2's output reflects run 2's values.
# It never touches the checkout: everything is rendered under a temp root.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SYNC="$HERE/sync-community.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

OUTS=(flake.nix packaging/aur/PKGBUILD packaging/homebrew/rupu.rb)

fail() { echo "FAIL: $*" >&2; exit 1; }

# <dir> <x64> <arm64> <darwin>
write_sidecars() {
  mkdir -p "$1"
  printf '%s  rupu-linux-x64\n'    "$2" > "$1/rupu-linux-x64.sha256"
  printf '%s  rupu-linux-arm64\n'  "$3" > "$1/rupu-linux-arm64.sha256"
  printf '%s  rupu-darwin-arm64\n' "$4" > "$1/rupu-darwin-arm64.sha256"
}

hex() { printf "$1%.0s" $(seq 64); }

V1=9.9.1; X1=$(hex a); A1=$(hex b); D1=$(hex c)
V2=9.9.2; X2=$(hex d); A2=$(hex e); D2=$(hex f)

ROOT="$TMP/root"
mkdir -p "$ROOT"

echo "=== RUN 1 ($V1) ==="
write_sidecars "$TMP/sha1" "$X1" "$A1" "$D1"
"$SYNC" "$V1" "$TMP/sha1" "$ROOT"
for f in "${OUTS[@]}"; do
  grep -q "$V1" "$ROOT/$f" || fail "run 1: $f missing version $V1"
done
grep -q "$X1" "$ROOT/flake.nix" || fail "run 1: flake.nix missing the x64 hash"

echo "=== RUN 2 ($V2) ==="
write_sidecars "$TMP/sha2" "$X2" "$A2" "$D2"
"$SYNC" "$V2" "$TMP/sha2" "$ROOT"

# The whole point: run 2's values are present and run 1's are GONE.
for f in "${OUTS[@]}"; do
  grep -q "$V2" "$ROOT/$f" || fail "run 2: $f did not pick up version $V2 (stale sync)"
  if grep -q "$V1" "$ROOT/$f"; then
    fail "run 2: $f still carries version $V1 (sync is not repeatable)"
  fi
  for stale in "$X1" "$A1" "$D1"; do
    if grep -q "$stale" "$ROOT/$f"; then
      fail "run 2: $f still carries a run-1 hash (sync is not repeatable)"
    fi
  done
done

# Per-platform correspondence, checked independently of the script's own
# assertions so a bug in those cannot hide a swap.
grep -Eq "\"x86_64-linux\"[[:space:]]*=[[:space:]]*\"$X2\";"   "$ROOT/flake.nix" || fail "flake: x64 hash not on x86_64-linux"
grep -Eq "\"aarch64-linux\"[[:space:]]*=[[:space:]]*\"$A2\";"  "$ROOT/flake.nix" || fail "flake: arm64 hash not on aarch64-linux"
grep -Eq "\"aarch64-darwin\"[[:space:]]*=[[:space:]]*\"$D2\";" "$ROOT/flake.nix" || fail "flake: darwin hash not on aarch64-darwin"
grep -q "sha256sums_x86_64=('$X2')"  "$ROOT/packaging/aur/PKGBUILD" || fail "PKGBUILD: x64 hash not on sha256sums_x86_64"
grep -q "sha256sums_aarch64=('$A2')" "$ROOT/packaging/aur/PKGBUILD" || fail "PKGBUILD: arm64 hash not on sha256sums_aarch64"

pairs=$(awk '
  /url "/       { if (match($0, /rupu-[a-z0-9-]+"/)) asset = substr($0, RSTART, RLENGTH - 1) }
  /^ *sha256 "/ { h = $2; gsub(/"/, "", h); print asset "=" h }
' "$ROOT/packaging/homebrew/rupu.rb" | sort)
want=$(printf '%s\n' "rupu-darwin-arm64=$D2" "rupu-linux-x64=$X2" "rupu-linux-arm64=$A2" | sort)
[ "$pairs" = "$want" ] || fail "formula: url/sha256 pairing wrong:
got:
$pairs
want:
$want"

# The generated files must announce that they are generated.
for f in "${OUTS[@]}"; do
  head -3 "$ROOT/$f" | grep -q "GENERATED" || fail "$f: missing the generated-file header"
done

echo "PASS: sync-community.sh is idempotent and repeatable, and every hash lands on its own platform"
