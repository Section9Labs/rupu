#!/usr/bin/env bash
# packaging/build-packages.sh <version> <nfpm-arch>
#
# Stages an already-built rupu binary plus its generated completions and man
# page, then emits a .deb and an .rpm. The binary is NEVER rebuilt here.
#
# Completions and the man page are architecture-independent — they are
# generated once from the x64 binary by the caller and reused for both
# architectures, because an x86_64 runner cannot execute the arm64 binary.
set -euo pipefail

VERSION="${1:?usage: build-packages.sh <version> <amd64|arm64>}"
PKG_ARCH="${2:?usage: build-packages.sh <version> <amd64|arm64>}"

case "$PKG_ARCH" in
  amd64|arm64) ;;
  *) echo "arch must be amd64 or arm64 (got: $PKG_ARCH)" >&2; exit 1 ;;
esac

test -x dist/rupu                        || { echo "dist/rupu missing or not executable" >&2; exit 1; }
test -f dist/rupu.1.gz                   || { echo "dist/rupu.1.gz missing" >&2; exit 1; }
test -f dist/completions/rupu.bash       || { echo "dist/completions/rupu.bash missing" >&2; exit 1; }
test -f dist/completions/_rupu           || { echo "dist/completions/_rupu missing" >&2; exit 1; }
test -f dist/completions/rupu.fish       || { echo "dist/completions/rupu.fish missing" >&2; exit 1; }

export PKG_VERSION="$VERSION"
export PKG_ARCH

echo "→ building packages for $PKG_ARCH at $PKG_VERSION"
nfpm package --config packaging/nfpm.yaml --packager deb --target dist/
nfpm package --config packaging/nfpm.yaml --packager rpm --target dist/

echo "→ produced:"
ls -la dist/*.deb dist/*.rpm
