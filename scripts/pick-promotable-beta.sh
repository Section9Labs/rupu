#!/usr/bin/env bash
# scripts/pick-promotable-beta.sh <soak-days> <now-unixtime>
#
# Reads `<tag><TAB><commit-unixtime>` lines on stdin and writes the beta tag
# that should be promoted to stable — or nothing at all (exit 0) when none
# qualifies, which is a normal week, not an error.
#
# PURE BY DESIGN: never calls git, and takes `now` as an argument rather than
# reading the clock, so the tests are deterministic. See
# scripts/tests/release-cadence-tests.sh.
#
# "Newest" means highest base version, then highest counter — both compared
# NUMERICALLY. A lexical sort would rank beta.9 above beta.10.
set -euo pipefail

soak_days="${1:-}"
now="${2:-}"
case "$soak_days" in ''|*[!0-9]*) echo "usage: $0 <soak-days> <now-unixtime>" >&2; exit 2 ;; esac
case "$now"       in ''|*[!0-9]*) echo "usage: $0 <soak-days> <now-unixtime>" >&2; exit 2 ;; esac

cutoff=$(( now - soak_days * 86400 ))

best_tag=""
best_major=-1; best_minor=-1; best_patch=-1; best_counter=-1

# The `|| [ -n "${tag:-}" ]` ensures a final unterminated line is processed.
# `read` returns non-zero at EOF when the line has no trailing newline, so the
# loop body would never run for that record and it would be silently dropped —
# a critical bug on a release path.
while IFS="$(printf '\t')" read -r tag ts || [ -n "${tag:-}" ]; do
  [ -n "${tag:-}" ] || continue
  case "$ts" in ''|*[!0-9]*) continue ;; esac
  # Too fresh: it has not soaked. This is the check that guarantees a stable
  # was installable as a beta for a real day before it shipped.
  [ "$ts" -le "$cutoff" ] || continue

  # Split `v<major>.<minor>.<patch>-beta[.N]`; anything else is not a beta.
  case "$tag" in
    v*.*.*-beta|v*.*.*-beta.*) ;;
    *) continue ;;
  esac
  rest="${tag#v}"
  version="${rest%%-beta*}"
  case "$tag" in
    *-beta) counter=1 ;;
    *)      counter="${tag##*-beta.}" ;;
  esac
  case "$counter" in ''|*[!0-9]*) continue ;; esac

  major="${version%%.*}"
  minor="${version#*.}"; minor="${minor%%.*}"
  patch="${version##*.}"
  case "$major$minor$patch" in ''|*[!0-9]*) continue ;; esac

  newer=0
  if   [ "$major" -gt "$best_major" ]; then newer=1
  elif [ "$major" -eq "$best_major" ]; then
    if   [ "$minor" -gt "$best_minor" ]; then newer=1
    elif [ "$minor" -eq "$best_minor" ]; then
      if   [ "$patch" -gt "$best_patch" ]; then newer=1
      elif [ "$patch" -eq "$best_patch" ] && [ "$counter" -gt "$best_counter" ]; then newer=1
      fi
    fi
  fi

  if [ "$newer" -eq 1 ]; then
    best_tag="$tag"; best_major="$major"; best_minor="$minor"
    best_patch="$patch"; best_counter="$counter"
  fi
done

[ -n "$best_tag" ] && echo "$best_tag"
exit 0
