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

# Verify the signature took. rpmsign can exit 0 having signed nothing
# useful if the macro is wrong; an unsigned rpm in a gpgcheck=1 repo
# fails on the user's machine, not here.
for f in "$REPO"/*.rpm; do
  info=$(rpm -qpi "$f" 2>/dev/null)
  echo "$info" | grep -qiE "Signature *:.*Key ID" \
    || { echo "rpm not signed: $f" >&2; exit 1; }
done

createrepo_c "$REPO"
gpg --batch --yes --armor --detach-sign -o "$REPO/repodata/repomd.xml.asc" \
  "$REPO/repodata/repomd.xml"

echo "YUM repo built for channel $CHANNEL:"
find "$REPO" -type f | sort
