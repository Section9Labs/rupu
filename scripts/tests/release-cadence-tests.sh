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

echo
echo "pick-promotable-beta.sh"
PICK="$ROOT/scripts/pick-promotable-beta.sh"

# NOW is a fixed clock so ages are exact. 2026-07-30T00:00:00Z.
NOW=1785369600
DAY=86400

# Two days' soak: a beta committed 3 days ago qualifies.
assert_out "promotes a beta past the soak window" "v0.71.0-beta.3" \
  sh -c "printf 'v0.71.0-beta.3\t%d\n' \$(( $NOW - 3 * $DAY )) | '$PICK' 2 $NOW"

# One committed 1 day ago does not — this is the guarantee that a stable was
# always available as a beta for a real day first.
assert_out "refuses a beta inside the soak window" "" \
  sh -c "printf 'v0.71.0-beta.4\t%d\n' \$(( $NOW - 1 * $DAY )) | '$PICK' 2 $NOW"

# Among eligible betas, the NEWEST wins — and numerically, so beta.10 beats
# beta.9 (the same lexical-sort trap as Task 1).
assert_out "picks the highest eligible counter numerically" "v0.71.0-beta.10" \
  sh -c "printf 'v0.71.0-beta.9\t%d\nv0.71.0-beta.10\t%d\n' \
    \$(( $NOW - 5 * $DAY )) \$(( $NOW - 4 * $DAY )) | '$PICK' 2 $NOW"

# A newer-but-unsoaked beta must not mask an older eligible one.
assert_out "falls back to the newest SOAKED beta" "v0.71.0-beta.2" \
  sh -c "printf 'v0.71.0-beta.2\t%d\nv0.71.0-beta.3\t%d\n' \
    \$(( $NOW - 4 * $DAY )) \$(( $NOW - 1 * $DAY )) | '$PICK' 2 $NOW"

# Nothing to promote is a normal Sunday, not an error.
assert_out "empty input yields no output" "" \
  sh -c "printf '' | '$PICK' 2 $NOW"

# A higher base version wins over a lower one regardless of counter.
assert_out "prefers the higher base version" "v0.72.0-beta.1" \
  sh -c "printf 'v0.71.0-beta.9\t%d\nv0.72.0-beta.1\t%d\n' \
    \$(( $NOW - 6 * $DAY )) \$(( $NOW - 3 * $DAY )) | '$PICK' 2 $NOW"

assert_fails "rejects a non-numeric soak window" \
  sh -c "printf '' | '$PICK' two $NOW"

# Unterminated final line handling: read returns non-zero at EOF when the line
# has no trailing newline, so it would be silently dropped without the fix.
assert_out "single qualifying line without trailing newline yields its tag" "v0.71.0-beta.5" \
  sh -c "printf 'v0.71.0-beta.5\t%d' \$(( $NOW - 3 * $DAY )) | '$PICK' 2 $NOW"

# When the newer beta is last and unterminated, it must still be picked.
assert_out "newer unterminated line at EOF is correctly picked" "v0.71.0-beta.10" \
  sh -c "printf 'v0.71.0-beta.9\t%d\nv0.71.0-beta.10\t%d' \
    \$(( $NOW - 5 * $DAY )) \$(( $NOW - 4 * $DAY )) | '$PICK' 2 $NOW"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
