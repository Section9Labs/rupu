#!/usr/bin/env bash
# Sign the just-built rupu binary with the Developer ID Application
# cert so the macOS keychain treats successive builds as the same code
# identity (no "Always Allow" re-prompt every rebuild).
#
# Looks up the identity once and caches its SHA-1 hash so this runs
# instantly on subsequent invocations. Override identity selection by
# setting RUPU_SIGNING_IDENTITY to the cert's SHA-1 or full
# "Common Name" string.
#
# Usage:
#   scripts/sign-dev.sh                     # signs target/debug/rupu
#   scripts/sign-dev.sh release             # signs target/release/rupu
#   scripts/sign-dev.sh debug path/to/bin   # signs a specific binary
#
# Linux/Windows: this script no-ops with an explanatory message — the
# keychain prompt issue is macOS-specific.
set -euo pipefail

profile="${1:-debug}"
binary="${2:-target/${profile}/rupu}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "scripts/sign-dev.sh: skipping (non-macOS); the keychain re-prompt issue is macOS-specific."
  exit 0
fi

if [[ ! -x "$binary" ]]; then
  echo "scripts/sign-dev.sh: binary not found at $binary" >&2
  echo "  build it first with: cargo build${profile:+ --$([ "$profile" = "release" ] && echo release || echo profile=dev)}" >&2
  exit 1
fi

identity="${RUPU_SIGNING_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  # Prefer "Developer ID Application" (will pass notarization). Fall
  # back to the first available code-signing identity.
  identity=$(security find-identity -v -p codesigning 2>/dev/null \
    | awk '/Developer ID Application/ { print $2; exit }')
  if [[ -z "$identity" ]]; then
    identity=$(security find-identity -v -p codesigning 2>/dev/null \
      | awk 'NR==1 { print $2 }')
  fi
fi

if [[ -z "$identity" ]]; then
  cat >&2 <<'NOIDENT'
scripts/sign-dev.sh: no code-signing identity found.

You have two options:

  1. (Recommended) Use a real Developer ID Application cert. Run:
       security find-identity -v -p codesigning
     If you don't see one, request one from your Apple Developer
     account at https://developer.apple.com/account/resources/certificates.

  2. Generate a local self-signed cert called `rupu-dev`:
     - Open Keychain Access
     - Menu: Certificate Assistant → Create a Certificate...
     - Name: rupu-dev
     - Identity Type: Self Signed Root
     - Certificate Type: Code Signing
     - Save it in your `login` keychain
     - Re-run scripts/sign-dev.sh

Once a cert is in place, click "Always Allow" on the FIRST keychain
prompt after signing — subsequent builds reuse the trust because the
code identity is now stable.
NOIDENT
  exit 1
fi

# Sign with the hardened runtime + the identity. -f replaces any prior
# signature; -s names the identity (SHA-1 hash works); --options runtime
# is required for notarization eligibility (and harmless for dev).
codesign --force --sign "$identity" --options runtime "$binary" 2>&1

# Verify the signature is intact.
codesign --verify --verbose=2 "$binary" 2>&1 | tail -2

echo "scripts/sign-dev.sh: signed $binary with identity $identity"
