#!/usr/bin/env bash
# scripts/shipped-paths.sh  — keep only the changed paths a release ships.
#
# Reads repository paths (one per line, e.g. `git diff --name-only`) on stdin
# and writes back the ones that can change what a release publishes, in input
# order. Empty output means the batch changed no product — only docs, CI,
# release scripts or tests — and release-beta.yml skips the cut rather than
# publish a beta identical to the last one. A fix to the release machinery
# that does need re-publishing is what release-beta.yml's `force` input is for.
#
# PURE BY DESIGN: this never calls git. See scripts/tests/release-cadence-tests.sh.
#
# Deliberately an allowlist of what does NOT ship. A path missing from it
# costs one unneeded beta; a path wrongly on it silently skips a release that
# had real changes. So only these shapes are dropped:
#
#   docs/**                 — including docs/pages/, which GitHub Pages deploys
#                             on merge; no release publishes it.
#   <root>/*.md             — CLAUDE.md, TODO.md, README.md, ISSUES.md, ...
#   .github/**              — CI, release workflows, rulesets.
#   scripts/**              — release and dev helper scripts.
#   crates/<crate>/tests/** — integration tests, never linked into a binary.
#
# A `.md` suffix alone is NOT documentation: crates/rupu-cli/templates/**/*.md
# are compiled into the binary with include_str!. Nor is every `tests/`
# directory a test: one under src/ is compiled into its crate.
set -euo pipefail

# `|| [ -n "$path" ]` keeps an unterminated final line.
while IFS= read -r path || [ -n "$path" ]; do
  case "$path" in
    ''|docs/*|.github/*|scripts/*) continue ;;
    crates/*/tests/*)
      # `*` in a case pattern also matches `/`, so `crates/*/tests/*` alone
      # would match crates/x/src/tests/y.rs. Require tests/ to be the
      # directory directly under the crate.
      rest="${path#crates/}"
      crate="${rest%%/*}"
      case "${rest#"$crate"/}" in
        tests/*) continue ;;
      esac
      ;;
    */*) ;;
    *.md) continue ;;
  esac
  printf '%s\n' "$path"
done
