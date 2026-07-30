#!/usr/bin/env bash
# Plain-shell tests for the release-cadence helper scripts.
#
# Deliberately dependency-free (no bats): these scripts are themselves shell,
# and the repo has no shell test harness to reuse. Run directly:
#
#     scripts/tests/release-cadence-tests.sh
#
# Every case pipes fixture tag data into a script and compares stdout to an
# expected string, so no git repository or network is involved.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PASS=0
FAIL=0

# assert_out <name> <expected> <script> [args...]  -- fixture lines on stdin
assert_out() {
  name="$1"; expected="$2"; shift 2
  actual="$("$@" 2>/dev/null)"
  if [ "$actual" = "$expected" ]; then
    PASS=$((PASS + 1))
    printf '  ok   %s\n' "$name"
  else
    FAIL=$((FAIL + 1))
    printf '  FAIL %s\n       expected: %s\n       actual:   %s\n' \
      "$name" "$expected" "$actual"
  fi
}

# assert_fails <name> <script> [args...]  -- fixture lines on stdin
assert_fails() {
  name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    FAIL=$((FAIL + 1))
    printf '  FAIL %s\n       expected non-zero exit, got 0\n' "$name"
  else
    PASS=$((PASS + 1))
    printf '  ok   %s\n' "$name"
  fi
}

echo "next-beta-version.sh"
NEXT="$ROOT/scripts/next-beta-version.sh"

# A base with no betas yet starts at 1, not 0.
assert_out "first beta on a fresh base" "v0.71.0-beta.1" \
  sh -c "printf 'v0.70.0-beta.1\nv0.70.0\n' | '$NEXT' 0.71.0"

# The bug this whole test file exists to catch: lexical sort puts beta.9
# above beta.10, so a naive `sort | tail -1` regresses to beta.2 forever.
# This test discriminates: with both 9 and 10 present, lexical would pick 9,
# numeric picks 10.
assert_out "beta.9 vs beta.10 compares numerically, not lexically" "v0.71.0-beta.11" \
  sh -c "printf 'v0.71.0-beta.9\nv0.71.0-beta.10\n' | '$NEXT' 0.71.0"

# Betas belonging to a different base must not influence the counter.
assert_out "counter resets after a base bump" "v0.72.0-beta.1" \
  sh -c "printf 'v0.71.0-beta.1\nv0.71.0-beta.2\nv0.71.0\n' | '$NEXT' 0.72.0"

# The legacy suffix-less form counts as beta 1 of that base, so the next is 2.
assert_out "legacy bare -beta tag counts as 1" "v0.71.0-beta.2" \
  sh -c "printf 'v0.71.0-beta\n' | '$NEXT' 0.71.0"

# Unrelated refs must be ignored rather than parsed as a counter.
assert_out "ignores rolling and unrelated tags" "v0.71.0-beta.1" \
  sh -c "printf 'latest-beta\nlatest-stable\nnightly\n' | '$NEXT' 0.71.0"

# Leading zeros in counters must be handled without octal-interpretation crashes.
assert_out "leading-zero counter is incremented correctly" "v0.71.0-beta.10" \
  sh -c "printf 'v0.71.0-beta.09\n' | '$NEXT' 0.71.0"

# Base version validation must be strict: exactly three dot-separated components.
assert_fails "rejects base with trailing suffix" \
  sh -c "printf '' | '$NEXT' '0.71.0-beta.5'"

assert_fails "rejects base with extra component" \
  sh -c "printf '' | '$NEXT' '0.71.0.1'"

assert_fails "rejects base with non-numeric suffix" \
  sh -c "printf '' | '$NEXT' '0.71.0extra'"

assert_fails "rejects a malformed base version" \
  sh -c "printf '' | '$NEXT' 'not-a-version'"

assert_fails "rejects a missing base version" \
  sh -c "printf '' | '$NEXT'"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
