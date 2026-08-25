#!/usr/bin/env bash
# Package a built rupu.app into the two shippable macOS assets: a DMG
# (drag-to-Applications installer) and a zip (notarization/stapling
# input, also downloadable directly). Writes a .sha256 checksum file
# alongside each, same `shasum -a 256` convention release.yml uses for
# the CLI assets.
#
# This script packages WHATEVER APP IT IS GIVEN — it never signs or
# notarizes anything itself. In the release pipeline the app is signed,
# notarized, and stapled BEFORE this script runs, so the DMG/zip it
# produces already contain the stapled app; the DMG file itself is
# signed/notarized/stapled in a separate CI step AFTER packaging.
#
# Usage:
#   scripts/package-app-dmg.sh <path/to/rupu.app> [output-dir] [version]
#
#   path/to/rupu.app   required; the built .app to package
#   output-dir         default: dist/
#   version             optional, informational only; also read from
#                        RUPU_RELEASE_VERSION if set. Asset names are
#                        platform-canonical and do NOT embed the version
#                        (rupu-app-darwin-arm64.dmg / .zip), matching the
#                        release lane's naming for the CLI assets.
#
# Produces:
#   <output-dir>/rupu-app-darwin-arm64.dmg(.sha256)
#   <output-dir>/rupu-app-darwin-arm64.zip(.sha256)
#
# Linux/Windows: this script no-ops with an explanatory message —
# hdiutil/ditto are macOS-only.
set -euo pipefail

app_path="${1:-}"
out_dir="${2:-dist}"
version="${3:-${RUPU_RELEASE_VERSION:-0.0.0-dev}}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "scripts/package-app-dmg.sh: skipping (non-macOS); hdiutil/ditto are macOS-only."
  exit 0
fi

if [[ -z "$app_path" ]]; then
  echo "scripts/package-app-dmg.sh: usage: scripts/package-app-dmg.sh <path/to/rupu.app> [output-dir] [version]" >&2
  exit 1
fi

if [[ ! -d "$app_path" ]] || [[ "$(basename "$app_path")" != *.app ]]; then
  echo "scripts/package-app-dmg.sh: not a .app bundle: $app_path" >&2
  exit 1
fi

# Resolve to a fully-qualified path (a .app is a directory, so cd into
# its parent rather than the bundle itself) since hdiutil/ditto below
# run from other working directories.
app_parent="$(cd "$(dirname "$app_path")" && pwd)"
app_path="$app_parent/$(basename "$app_path")"

echo "scripts/package-app-dmg.sh: packaging $app_path (version $version)"

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"

dmg_name="rupu-app-darwin-arm64.dmg"
zip_name="rupu-app-darwin-arm64.zip"
dmg_path="$out_dir/$dmg_name"
zip_path="$out_dir/$zip_name"

work="$(mktemp -d -t rupu-package-app)"
cleanup() { rm -rf "$work" 2>/dev/null || true; }
trap cleanup EXIT

# --- DMG -------------------------------------------------------------
# Stage the .app plus an /Applications symlink so a user who opens the
# mounted volume can drag-install in the usual macOS way.
stage="$work/dmg-stage"
mkdir -p "$stage"
ditto "$app_path" "$stage/$(basename "$app_path")"
ln -s /Applications "$stage/Applications"

rm -f "$dmg_path"
hdiutil create \
  -volname "rupu" \
  -srcfolder "$stage" \
  -fs HFS+ \
  -format UDZO \
  -ov \
  "$dmg_path"

# --- zip ---------------------------------------------------------------
# ditto -c -k --keepParent preserves the .app bundle's resource forks /
# extended attributes (a plain `zip` does not), which is required for
# the app's code signature to survive the round trip.
rm -f "$zip_path"
( cd "$(dirname "$app_path")" && ditto -c -k --keepParent "$(basename "$app_path")" "$zip_path" )

# --- checksums -----------------------------------------------------------
( cd "$out_dir" && shasum -a 256 "$dmg_name" > "$dmg_name.sha256" )
( cd "$out_dir" && shasum -a 256 "$zip_name" > "$zip_name.sha256" )

echo "scripts/package-app-dmg.sh: wrote $dmg_path"
echo "scripts/package-app-dmg.sh: wrote $zip_path"
cat "$dmg_path.sha256"
cat "$zip_path.sha256"
