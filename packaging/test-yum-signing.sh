#!/usr/bin/env bash
# Real RPM signing smoke test, using a disposable key and tiny package.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
export GNUPGHOME="$TMP/gnupg"
mkdir -m 700 "$GNUPGHOME"
export GPG_KEY_ID='Rupu packaging test <packaging-test@example.invalid>'
gpg --batch --pinentry-mode loopback --passphrase '' --quick-generate-key "$GPG_KEY_ID" rsa2048 sign 0
mkdir -p "$TMP/build" "$TMP/input" "$TMP/bin"
cat > "$TMP/fixture.spec" <<'SPEC'
Name: rupu-signature-test
Version: 1
Release: 1
Summary: Disposable signing fixture
License: MIT
BuildArch: noarch
%description
Disposable signing fixture.
%install
mkdir -p %{buildroot}/usr/share/rupu-signature-test
printf 'fixture\n' > %{buildroot}/usr/share/rupu-signature-test/fixture
%files
/usr/share/rupu-signature-test/fixture
SPEC
rpmbuild --define "_topdir $TMP/build" -bb "$TMP/fixture.spec"
cp "$TMP/build/RPMS/noarch/"*.rpm "$TMP/input/"
bash "$HERE/build-yum-repo.sh" "$TMP/input" beta "$TMP/signed"
gpg --verify "$TMP/signed/yum/beta/repodata/repomd.xml.asc" "$TMP/signed/yum/beta/repodata/repomd.xml"
# A successful signing process that leaves the input unsigned must fail closed.
printf '#!/usr/bin/env bash\nexit 0\n' > "$TMP/bin/rpmsign"
chmod +x "$TMP/bin/rpmsign"
if PATH="$TMP/bin:$PATH" bash "$HERE/build-yum-repo.sh" "$TMP/input" beta "$TMP/unsigned" > "$TMP/unsigned.log" 2>&1; then
  echo 'FAIL: accepted an unsigned RPM' >&2
  exit 1
fi
grep -q 'rpm has no verified signature' "$TMP/unsigned.log"
echo 'PASS: signed RPM verified; unsigned RPM rejected'
