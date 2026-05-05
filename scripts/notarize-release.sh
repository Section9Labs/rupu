#!/usr/bin/env bash
# Submit a signed `target/release/rupu` to Apple's notary service so
# Gatekeeper accepts it on first run for downstream users. Run this
# AFTER scripts/sign-dev.sh release (which signs with --options runtime).
#
# Prereqs (one-time):
#   xcrun notarytool store-credentials rupu \
#     --apple-id <your@apple.id> \
#     --team-id 995PCLM9KH
# (You'll be prompted for an app-specific password from
#  appleid.apple.com → Sign-In and Security → App-Specific Passwords.)
#
# Usage:
#   scripts/notarize-release.sh [path/to/rupu]    # default: target/release/rupu
#
# Exits non-zero if the binary isn't signed with hardened runtime, or
# if notarization is rejected. On success, prints the submission ID
# and the final status. The binary's signature carries the ticket
# online; no `stapler` step is needed for bare binaries (stapler only
# applies to .app, .pkg, .dmg).
set -euo pipefail

binary="${1:-target/release/rupu}"
profile="${RUPU_NOTARY_PROFILE:-rupu}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "scripts/notarize-release.sh: macOS-only — skipping."
  exit 0
fi

if [[ ! -x "$binary" ]]; then
  echo "scripts/notarize-release.sh: binary not found at $binary" >&2
  exit 1
fi

# Confirm hardened runtime is set (notarization requires it).
# Capture full output first to dodge a `set -o pipefail` + `grep -q`
# race where grep closes the pipe early and codesign exits with
# SIGPIPE, making the whole pipeline non-zero.
codesign_out="$(codesign --display --verbose=2 "$binary" 2>&1 || true)"
if ! grep -q "flags=.*runtime" <<<"$codesign_out"; then
  cat >&2 <<EOF
scripts/notarize-release.sh: $binary lacks hardened runtime.
Re-sign first:
  scripts/sign-dev.sh release
EOF
  exit 1
fi

# notarytool requires a .zip (tar.gz isn't accepted). Build one with
# just the binary at the archive root.
zipfile=$(mktemp -t rupu-notary).zip
cleanup() { rm -f "$zipfile" 2>/dev/null || true; }
trap cleanup EXIT

work=$(mktemp -d -t rupu-notary)
cp "$binary" "$work/rupu"
( cd "$work" && zip -q "$zipfile" rupu )
rm -rf "$work"

echo "scripts/notarize-release.sh: submitting $binary to notarytool (profile: $profile)…"
xcrun notarytool submit "$zipfile" \
  --keychain-profile "$profile" \
  --wait \
  --output-format plist \
  > /tmp/notary-submission.plist

submission_id=$(plutil -extract id raw /tmp/notary-submission.plist 2>/dev/null || true)
status=$(plutil -extract status raw /tmp/notary-submission.plist 2>/dev/null || true)

echo "submission id: ${submission_id:-?}"
echo "status:        ${status:-?}"

if [[ "$status" != "Accepted" ]]; then
  echo "scripts/notarize-release.sh: notarization failed (status=$status). Fetching log…" >&2
  xcrun notarytool log "$submission_id" --keychain-profile "$profile" >&2 || true
  exit 1
fi

echo "scripts/notarize-release.sh: $binary is notarized."
