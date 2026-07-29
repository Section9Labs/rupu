#!/usr/bin/env bash
# packaging/build-apt-repo.sh <deb-dir> <channel> <out-dir>
#
# Builds a signed flat-ish APT repository. Beta and stable are separate
# SUITES in one repository, so a user who opted into beta never silently
# receives stable.
#
# Only the CURRENT version is indexed; older versions stay downloadable
# from their GitHub release assets. See the spec's "Why the index carries
# only the latest version".
set -euo pipefail

DEB_DIR="${1:?usage: build-apt-repo.sh <deb-dir> <channel> <out-dir>}"
CHANNEL="${2:?usage: build-apt-repo.sh <deb-dir> <channel> <out-dir>}"
OUT="${3:?usage: build-apt-repo.sh <deb-dir> <channel> <out-dir>}"
: "${GPG_KEY_ID:?GPG_KEY_ID must be set}"

case "$CHANNEL" in
  beta|stable) ;;
  *) echo "channel must be beta or stable (got: $CHANNEL)" >&2; exit 1 ;;
esac

# Per-channel pool. A pool shared between channels would make
# dpkg-scanpackages index beta's .deb files into stable's Packages, and
# would accumulate every version ever published — both wrong.
POOL="$OUT/apt/pool/$CHANNEL"
DIST="$OUT/apt/dists/$CHANNEL/main"
mkdir -p "$POOL" "$DIST/binary-amd64" "$DIST/binary-arm64"

cp "$DEB_DIR"/*.deb "$POOL/"

for arch in amd64 arm64; do
  ( cd "$OUT/apt" && dpkg-scanpackages --arch "$arch" "pool/$CHANNEL" /dev/null ) \
    > "$DIST/binary-$arch/Packages"
  gzip -9 -k -f "$DIST/binary-$arch/Packages"
  # A Packages file with no entries means the pool did not contain a .deb
  # for this architecture — publishing that silently gives users an
  # "unable to locate package" they cannot diagnose.
  test -s "$DIST/binary-$arch/Packages" \
    || { echo "no .deb found for $arch" >&2; exit 1; }
done

apt-ftparchive \
  -o "APT::FTPArchive::Release::Origin=Section9Labs" \
  -o "APT::FTPArchive::Release::Label=rupu" \
  -o "APT::FTPArchive::Release::Suite=$CHANNEL" \
  -o "APT::FTPArchive::Release::Codename=$CHANNEL" \
  -o "APT::FTPArchive::Release::Architectures=amd64 arm64" \
  -o "APT::FTPArchive::Release::Components=main" \
  release "$OUT/apt/dists/$CHANNEL" > "$OUT/apt/dists/$CHANNEL/Release"

gpg --batch --yes --local-user "$GPG_KEY_ID" --armor --detach-sign \
  -o "$OUT/apt/dists/$CHANNEL/Release.gpg" "$OUT/apt/dists/$CHANNEL/Release"
gpg --batch --yes --local-user "$GPG_KEY_ID" --clearsign \
  -o "$OUT/apt/dists/$CHANNEL/InRelease" "$OUT/apt/dists/$CHANNEL/Release"

echo "APT repo built for channel $CHANNEL:"
find "$OUT/apt" -type f | sort
