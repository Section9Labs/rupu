#!/usr/bin/env bash
# packaging/build-yum-repo.sh <rpm-dir> <channel> <out-dir>
#
# RPM signing differs from deb: the PACKAGES themselves are signed with
# rpmsign (deb packages are not individually signed; the Release file
# covers them), and repomd.xml gets a detached signature.
set -euo pipefail

RPM_DIR="${1:?usage: build-yum-repo.sh <rpm-dir> <channel> <out-dir>}"
CHANNEL="${2:?usage: build-yum-repo.sh <rpm-dir> <channel> <out-dir>}"
OUT="${3:?usage: build-yum-repo.sh <rpm-dir> <channel> <out-dir>}"
: "${GPG_KEY_ID:?GPG_KEY_ID must be set}"

case "$CHANNEL" in
  beta|stable) ;;
  *) echo "channel must be beta or stable (got: $CHANNEL)" >&2; exit 1 ;;
esac

REPO="$OUT/yum/$CHANNEL"
mkdir -p "$REPO"
cp "$RPM_DIR"/*.rpm "$REPO/"

# rpmsign needs the key name in ~/.rpmmacros
cat > "$HOME/.rpmmacros" <<RPMMACROS
%_signature gpg
%_gpg_name $GPG_KEY_ID
RPMMACROS

rpmsign --addsign "$REPO"/*.rpm

# Verify cryptographically against only the release public key. Human-readable
# rpm -qpi metadata varies between RPM versions and is not a signature check.
verify_dir=$(mktemp -d)
trap 'rm -rf "$verify_dir"' EXIT
gpg --batch --armor --export "$GPG_KEY_ID" > "$verify_dir/release-key.asc"
rpmkeys --dbpath "$verify_dir/rpmdb" --import "$verify_dir/release-key.asc"
for f in "$REPO"/*.rpm; do
  if ! info=$(LC_ALL=C rpmkeys --dbpath "$verify_dir/rpmdb" --checksig --verbose "$f" 2>&1); then
    printf '%s\n' "$info" >&2
    echo "rpm signature verification failed: $f" >&2
    exit 1
  fi
  # Unsigned RPMs can pass digest verification. Require an actual valid signature.
  if ! printf '%s\n' "$info" | grep -qiE 'signature.*: OK$'; then
    printf '%s\n' "$info" >&2
    echo "rpm has no verified signature: $f" >&2
    exit 1
  fi
done

createrepo_c "$REPO"
gpg --batch --yes --armor --detach-sign -o "$REPO/repodata/repomd.xml.asc" \
  "$REPO/repodata/repomd.xml"

echo "YUM repo built for channel $CHANNEL:"
find "$REPO" -type f | sort
