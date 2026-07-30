#!/usr/bin/env bash
# scripts/next-beta-version.sh <base-version>  — emit the next beta tag.
#
# Reads candidate tag names (one per line) on stdin and writes a single tag
# such as `v0.71.0-beta.4` to stdout.
#
# PURE BY DESIGN: this never calls git. The caller supplies the tag list, which
# is what makes it testable against fixtures with no repository present. See
# scripts/tests/release-cadence-tests.sh.
#
# The counter is DERIVED, never stored — there is no state file to drift out of
# sync, and deleting a tag simply lowers the max.
set -euo pipefail

base="${1:-}"
# Base version must be exactly three dot-separated numeric components.
bad=0
case "$base" in
  *[!0-9.]*|*..*|.*|*.) bad=1 ;;
  *.*.*.*)              bad=1 ;;
  *.*.*)                bad=0 ;;
  *)                    bad=1 ;;
esac
if [ "$bad" -eq 1 ]; then
  echo "usage: $0 <base-version>  (e.g. 0.71.0)" >&2
  exit 2
fi

# Highest existing counter for THIS base. `-beta` with no suffix is the legacy
# single-tag form and counts as 1.
#
# The numeric comparison is load-bearing: a lexical sort ranks "9" above "10",
# which would pin the counter just below its true max forever.
max=0
while IFS= read -r tag; do
  case "$tag" in
    "v$base-beta")    n=1 ;;
    "v$base-beta."*)  n="${tag##*-beta.}" ;;
    *)                continue ;;
  esac
  # Guard against a hand-made tag like `-beta.x`: non-numeric is not a counter.
  case "$n" in
    ''|*[!0-9]*) continue ;;
  esac
  # Use base-10 explicit arithmetic to avoid octal interpretation of leading zeros.
  n=$((10#$n))
  if [ "$n" -gt "$max" ]; then
    max="$n"
  fi
done

echo "v$base-beta.$((max + 1))"
