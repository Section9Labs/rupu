#!/usr/bin/env bash
# scripts/shipped-paths.sh  — keep only the changed paths a release ships.
#
# Reads repository paths (one per line, e.g. `git diff --name-only`) on stdin
# and writes back the ones that can change what a release publishes, in input
# order. Empty output means the batch is documentation only, and
# release-beta.yml skips the cut rather than spend a full CI run on it.
#
# PURE BY DESIGN: this never calls git. See scripts/tests/release-cadence-tests.sh.
#
# Deliberately an allowlist of what does NOT ship, kept tiny. A path missing
# from it costs one unneeded beta; a path wrongly on it silently skips a
# release that had real changes. So only two shapes are dropped:
#
#   docs/**        — except docs/pages/**, the published site and the apt/yum
#                    keyring that CI's keyring check guards.
#   <root>/*.md    — CLAUDE.md, TODO.md, README.md, ISSUES.md, ...
#
# A `.md` suffix alone is NOT documentation: crates/rupu-cli/templates/**/*.md
# are compiled into the binary with include_str!.
set -euo pipefail

# `|| [ -n "$path" ]` keeps an unterminated final line.
while IFS= read -r path || [ -n "$path" ]; do
  case "$path" in
    '')           continue ;;
    docs/pages/*) ;;
    docs/*)       continue ;;
    */*)          ;;
    *.md)         continue ;;
  esac
  printf '%s\n' "$path"
done
