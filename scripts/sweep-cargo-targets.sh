#!/usr/bin/env bash
# Keep cargo target dirs from filling the disk.
#
# Cargo never garbage-collects a target dir, and three things make rupu's grow
# by tens of GB a day per worktree (measured 2026-10-01, cargo/rustc 1.97 on
# macOS — one worktree reached 221 GB, 88 GB of it loose .o files):
#
#  1. Leaked debug objects. macOS debug builds default to
#     split-debuginfo=unpacked: a binary's debug map points at the .o files
#     rustc leaves in deps/. rustc names them per invocation
#     (`<crate>-<hash>.<cgu>.<tag>.rcgu.o`), so every rebuild of a crate adds
#     a full new set and none is ever removed — one test binary had 12,288.
#  2. Binary variants. `cargo test -p X`, `--workspace`, clippy, a toolchain
#     bump or a lockfile change each unify features differently, so every
#     downstream test binary (~100 MB, statically linked) is rebuilt under a
#     new hash and the old one stays.
#  3. One target/ per worktree, and many worktrees.
#
# What it removes, cheapest first:
#
#  objects      .o sets superseded by a newer build of the same binary or
#               library. Cargo does not track .o files, so this never causes a
#               rebuild; the current build's set is always kept, so backtraces
#               keep their line numbers.
#  binaries     deps/ executables neither built nor run for --binary-age-hours
#               (their .d, .o and .dSYM go with them). Cargo recompiles one
#               only if it is needed again.
#  incremental  incremental caches not compiled into for
#               --incremental-age-hours (costs one non-incremental compile of
#               that crate if it comes back).
#  evict        whole target dirs, least recently used first, while the disk
#               holding them has less than --min-free-gb free and they have
#               been idle for --evict-idle-hours (the next build is cold).
#
# binaries/incremental/evict run only while holding the target's cargo build
# locks, so a cargo started meanwhile waits ("Blocking waiting for file lock")
# instead of failing, and a target that is mid-build is left for the next run.
#
# Usage:
#   scripts/sweep-cargo-targets.sh --repo DIR [--scan DIR]... [options]
#   scripts/sweep-cargo-targets.sh --install --repo DIR [--scan DIR]... [options]
#   scripts/sweep-cargo-targets.sh --uninstall
#
#   --repo DIR                 sweep target/ of every worktree of this git repo
#   --scan DIR                 also sweep every cargo target dir under DIR
#                              (found by cargo's CACHEDIR.TAG); repeatable
#   --binary-age-hours N       default 24
#   --incremental-age-hours N  default 48
#   --min-free-gb N            default 200; 0 never evicts
#   --evict-idle-hours N       default 1
#   --dry-run                  report what would go, delete nothing
#   --log FILE                 append output to FILE (rotated at 1 MB)
#   --install                  macOS: copy this script out of the repo and run
#                              it hourly from a launchd agent with the other
#                              options given
#   --uninstall                remove that launchd agent
#
# `make sweep-targets` runs it once; `make sweep-targets-install` installs it.
set -uo pipefail

LABEL="dev.rupu.sweep-cargo-targets"
INSTALL_DIR="$HOME/Library/Application Support/rupu"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG_DEFAULT="$HOME/Library/Logs/rupu-sweep-cargo-targets.log"

# Objects with no binary or library left are only deleted once they are this
# old, so a first link that has written its objects but not yet its binary is
# never touched.
ORPHAN_GUARD_SECS=3600

if stat -f %m / >/dev/null 2>&1 && ! stat --version >/dev/null 2>&1; then
  STAT=(stat -f '%m %a %z %N')   # BSD: mtime atime size name
else
  STAT=(stat -c '%Y %X %s %n')   # GNU
fi

die() { echo "sweep-cargo-targets: $*" >&2; exit 2; }
gb() { awk -v k="$1" 'BEGIN { printf "%.1f GB", k / 1048576 }'; }
free_kb() { df -Pk "$1" 2>/dev/null | awk 'NR == 2 { print $4 }'; }
is_int() { case "$1" in ''|*[!0-9]*) return 1 ;; esac; }

# deps/ and incremental/ dirs of a target: <t>/<profile>/… and
# <t>/<triple>/<profile>/….
profile_dirs() { find "$1" -mindepth 2 -maxdepth 3 -type d -name "$2" 2>/dev/null; }

# ---------------------------------------------------------------- objects --

# Prints "<kb> <files>" freed (or that would be).
sweep_objects() {
  local deps="$1" now list kb
  now=$(date +%s)
  list=$(mktemp "${TMPDIR:-/tmp}/sweep-objects.XXXXXX")
  (
    cd "$deps" || exit 0
    # Live artifacts: executables (no extension) and libraries.
    find . -maxdepth 1 -type f \( ! -name '*.*' -o -name 'lib*.rlib' -o -name 'lib*.dylib' -o -name 'lib*.so' \) |
      sed -e 's|^\./||' -e 's|^lib\(.*\)\.rlib$|\1|' -e 's|^lib\(.*\)\.dylib$|\1|' \
        -e 's|^lib\(.*\)\.so$|\1|' -e 's/^/L /'
    find . -maxdepth 1 -type f -name '*.rcgu.o' -print0 | xargs -0 "${STAT[@]}" 2>/dev/null |
      sed 's/^/O /'
  ) | awk -v now="$now" -v guard="$ORPHAN_GUARD_SECS" -v out="$list" '
    $1 == "L" { live[$2] = 1; next }
    $1 == "O" {
      name = $5; sub(/^\.\//, "", name)
      if (name !~ /^[A-Za-z0-9_.+-]+$/) next
      # <crate>-<hash>.<cgu>.<tag>.rcgu.o; anything else is not ours to judge.
      if (split(name, p, ".") != 5) next
      n++; nm[n] = name; key[n] = p[1]; tag[n] = p[3]; mt[n] = $2; sz[n] = $4
      g = p[1] SUBSEP p[3]
      if ($2 > gmax[g]) gmax[g] = $2
    }
    END {
      for (g in gmax) {
        split(g, kt, SUBSEP)
        if (gmax[g] > best[kt[1]]) { best[kt[1]] = gmax[g]; newest[kt[1]] = kt[2] }
      }
      for (i = 1; i <= n; i++) {
        if (key[i] in live) { if (tag[i] == newest[key[i]]) continue }
        else if (now - mt[i] < guard) continue
        print nm[i] > out
        bytes += sz[i]; files++
      }
      printf "%d %d\n", bytes / 1024, files
    }'
  if [ -s "$list" ] && [ -z "${SWEEP_DRY_RUN:-}" ]; then
    (cd "$deps" && xargs rm -f < "$list")
  fi
  rm -f "$list"
}

# ------------------------------------------------- binaries / incremental --

# Runs inside the target's locks (see with_locks). Prints one summary line.
sweep_aged() {
  local t="$1" deps inc bins_kb=0 bins=0 inc_kb=0 incs=0 x kb paths
  local bmin=$((SWEEP_BINARY_AGE_HOURS * 60)) imin=$((SWEEP_INCREMENTAL_AGE_HOURS * 60))
  while IFS= read -r deps; do
    # Release builds carry no loose objects and few variants, and their binary
    # (`make release`) is slow to relink and often goes days between builds.
    case "$deps" in */release/deps) continue ;; esac
    while IFS= read -r x; do
      x=${x#./}
      case "$x" in *[!A-Za-z0-9_+-]*|'') continue ;; esac
      paths=$(cd "$deps" && ls -d "$x" "$x.d" "$x".*.rcgu.o "$x.dSYM" 2>/dev/null)
      kb=$(cd "$deps" && printf '%s\n' "$paths" | xargs du -sk 2>/dev/null | awk '{ s += $1 } END { print s + 0 }')
      bins_kb=$((bins_kb + kb)); bins=$((bins + 1))
      [ -z "${SWEEP_DRY_RUN:-}" ] && (cd "$deps" && printf '%s\n' "$paths" | xargs rm -rf)
    done < <(cd "$deps" && find . -maxdepth 1 -type f ! -name '*.*' -mmin +"$bmin" -amin +"$bmin")
  done < <(profile_dirs "$t" deps)
  while IFS= read -r inc; do
    while IFS= read -r x; do
      kb=$(du -sk "$x" 2>/dev/null | awk '{ print $1 + 0 }')
      inc_kb=$((inc_kb + kb)); incs=$((incs + 1))
      [ -z "${SWEEP_DRY_RUN:-}" ] && rm -rf "$x"
    done < <(find "$inc" -mindepth 1 -maxdepth 1 -type d -mmin +"$imin")
  done < <(profile_dirs "$t" incremental)
  if [ "$bins" -gt 0 ] || [ "$incs" -gt 0 ]; then
    echo "  $t: binaries -$(gb "$bins_kb") ($bins), incremental -$(gb "$inc_kb") ($incs)"
  fi
}

# Renames the target aside first so a cargo that starts afterwards gets a
# fresh dir rather than one being deleted under it.
evict() {
  local t="$1" trash="$1.sweep-evicting.$$"
  grep -q 'created by cargo' "$t/CACHEDIR.TAG" 2>/dev/null || return 1
  mv "$t" "$trash" && rm -rf "$trash"
}

# with_locks <target> <cmd…>: run cmd holding every cargo build lock in the
# target. Exits 75 without running it if a build holds one.
with_locks() {
  local t="$1" locks=()
  shift
  while IFS= read -r l; do locks+=("$l"); done < <(
    find "$t" -mindepth 2 -maxdepth 3 -type f \
      \( -name .cargo-lock -o -name .cargo-build-lock -o -name .cargo-artifact-lock \) 2>/dev/null)
  perl -MFcntl=:flock -e '
    my @fh;
    while (@ARGV && $ARGV[0] ne "--") {
      my $p = shift @ARGV;
      open(my $f, "<", $p) or next;
      flock($f, LOCK_EX | LOCK_NB) or exit 75;
      push @fh, $f;
    }
    shift @ARGV;
    my $rc = system @ARGV;
    exit($rc == -1 ? 127 : $rc >> 8);
  ' ${locks[@]+"${locks[@]}"} -- "$@"
}

# Newest build (mtime of any .d or binary) or test run (atime of a binary).
last_used() {
  local deps
  profile_dirs "$1" deps | while IFS= read -r deps; do
    (cd "$deps" && find . -maxdepth 1 -type f \( -name '*.d' -o ! -name '*.*' \) -print0 |
      xargs -0 "${STAT[@]}" 2>/dev/null)
  done | awk '{ if ($1 > m) m = $1; if ($2 > m) m = $2 } END { print m + 0 }'
}

# ---------------------------------------------------------------- install --

xml_escape() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

install_agent() {
  [ "$(uname)" = Darwin ] || die "--install sets up a launchd agent and is macOS-only"
  local args=("$@") dest="$INSTALL_DIR/sweep-cargo-targets.sh" a
  mkdir -p "$INSTALL_DIR" "$(dirname "$PLIST")" "$(dirname "$LOG_DEFAULT")"
  cp "$0" "$dest" && chmod 755 "$dest"
  {
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$(xml_escape "$dest")</string>
EOF
    for a in ${args[@]+"${args[@]}"} --log "$LOG_DEFAULT"; do
      echo "    <string>$(xml_escape "$a")</string>"
    done
    cat <<EOF
  </array>
  <key>StartInterval</key><integer>3600</integer>
  <key>RunAtLoad</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin</string>
  </dict>
</dict>
</plist>
EOF
  } > "$PLIST"
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
  launchctl bootstrap "gui/$(id -u)" "$PLIST" || die "launchctl bootstrap failed"
  echo "installed $LABEL: runs hourly (and now); log: $LOG_DEFAULT"
  echo "  script: $dest"
  echo "  args:   ${args[*]-}"
}

uninstall_agent() {
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
  rm -f "$PLIST" "$INSTALL_DIR/sweep-cargo-targets.sh"
  echo "uninstalled $LABEL"
}

# ------------------------------------------------------------------- main --

# Internal: re-entered under with_locks, which can only run a command.
if [ "${1:-}" = "--_locked" ]; then
  case "$2" in
    aged) sweep_aged "$3" ;;
    evict) evict "$3" ;;
  esac
  exit $?
fi

REPO="" SCANS=() INSTALL="" UNINSTALL="" LOG="" PASS=()
export SWEEP_BINARY_AGE_HOURS=24 SWEEP_INCREMENTAL_AGE_HOURS=48 SWEEP_DRY_RUN=""
MIN_FREE_GB=200 EVICT_IDLE_HOURS=1
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="${2:?}"; PASS+=("$1" "$2"); shift ;;
    --scan) SCANS+=("${2:?}"); PASS+=("$1" "$2"); shift ;;
    --binary-age-hours) SWEEP_BINARY_AGE_HOURS="${2:?}"; PASS+=("$1" "$2"); shift ;;
    --incremental-age-hours) SWEEP_INCREMENTAL_AGE_HOURS="${2:?}"; PASS+=("$1" "$2"); shift ;;
    --min-free-gb) MIN_FREE_GB="${2:?}"; PASS+=("$1" "$2"); shift ;;
    --evict-idle-hours) EVICT_IDLE_HOURS="${2:?}"; PASS+=("$1" "$2"); shift ;;
    --dry-run) SWEEP_DRY_RUN=1 ;;
    --log) LOG="${2:?}"; shift ;;
    --install) INSTALL=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,/^set -uo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done
for n in "$SWEEP_BINARY_AGE_HOURS" "$SWEEP_INCREMENTAL_AGE_HOURS" "$MIN_FREE_GB" "$EVICT_IDLE_HOURS"; do
  is_int "$n" || die "expected a whole number, got '$n'"
done

if [ -n "$UNINSTALL" ]; then uninstall_agent; exit 0; fi
[ -n "$REPO" ] || [ ${#SCANS[@]} -gt 0 ] || die "nothing to sweep: pass --repo and/or --scan (see --help)"
if [ -n "$INSTALL" ]; then
  # The agent must outlive whichever worktree installed it: point it at the
  # main checkout, not this worktree.
  if [ -n "$REPO" ]; then
    common=$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) ||
      die "--repo $REPO is not a git repository"
    main=$(dirname "$common")
    for i in "${!PASS[@]}"; do
      [ "${PASS[$i]}" = "--repo" ] && PASS[$((i + 1))]="$main"
    done
  fi
  install_agent ${PASS[@]+"${PASS[@]}"}
  exit $?
fi

if [ -n "$LOG" ]; then
  mkdir -p "$(dirname "$LOG")"
  if [ -f "$LOG" ] && [ "$(wc -c < "$LOG")" -gt 1048576 ]; then mv -f "$LOG" "$LOG.1"; fi
  exec >> "$LOG" 2>&1
fi
command -v perl >/dev/null 2>&1 ||
  echo "warning: perl not found; only superseded objects can be swept (binaries, incremental and eviction need cargo's locks)"

SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
TARGETS=$(
  if [ -n "$REPO" ]; then
    git -C "$REPO" worktree list --porcelain 2>/dev/null | sed -n 's/^worktree //p' |
      while IFS= read -r wt; do
        [ -f "$wt/target/CACHEDIR.TAG" ] && echo "$wt/target"
      done
  fi
  for root in ${SCANS[@]+"${SCANS[@]}"}; do
    [ -d "$root" ] || continue
    find "$root" -maxdepth 8 \( -name deps -o -name incremental -o -name .fingerprint \
      -o -name node_modules -o -name .git \) -prune -o -type f -name CACHEDIR.TAG -print 2>/dev/null |
      while IFS= read -r tag; do
        grep -q 'created by cargo' "$tag" 2>/dev/null && dirname "$tag"
      done
  done | sort -u
)

echo "$(date '+%Y-%m-%d %H:%M:%S') sweep${SWEEP_DRY_RUN:+ (dry run)}: $(printf '%s\n' "$TARGETS" | grep -c .) target dirs"
[ -n "$TARGETS" ] || exit 0

while IFS= read -r t; do
  [ -d "$t" ] || continue
  obj_kb=0 objs=0
  while IFS= read -r deps; do
    read -r kb n < <(sweep_objects "$deps")
    obj_kb=$((obj_kb + kb)); objs=$((objs + n))
  done < <(profile_dirs "$t" deps)
  [ "$objs" -gt 0 ] && echo "  $t: objects -$(gb "$obj_kb") ($objs)"
  if command -v perl >/dev/null 2>&1; then
    with_locks "$t" "$BASH" "$SELF" --_locked aged "$t"
    [ $? -eq 75 ] && echo "  $t: building right now; binaries/incremental left for the next run"
  fi
done <<< "$TARGETS"

# Eviction: least recently used first, until every disk is above the floor.
floor_kb=$((MIN_FREE_GB * 1048576))
low_disk=$(printf '%s\n' "$TARGETS" | while IFS= read -r t; do
  [ -d "$t" ] && [ "$(free_kb "$t")" -lt "$floor_kb" ] && { echo 1; break; }
done)
if [ -n "$low_disk" ] && command -v perl >/dev/null 2>&1; then
  now=$(date +%s)
  credit=0  # dry run: what earlier "evictions" would have freed
  while IFS=' ' read -r used t; do
    [ -d "$t" ] || continue
    free=$(( $(free_kb "$t") + credit ))
    [ "$free" -ge "$floor_kb" ] && continue
    idle=$(( (now - used) / 3600 ))
    if [ $((now - used)) -lt $((EVICT_IDLE_HOURS * 3600)) ]; then
      echo "  low disk ($(gb "$free") free < $MIN_FREE_GB GB) but $t was used ${idle}h ago; not evicting"
      continue
    fi
    kb=$(du -sk "$t" 2>/dev/null | awk '{ print $1 + 0 }')
    if [ -n "$SWEEP_DRY_RUN" ]; then
      credit=$((credit + kb))
    else
      with_locks "$t" "$BASH" "$SELF" --_locked evict "$t"
      rc=$?
      if [ $rc -eq 75 ]; then echo "  $t: building right now; not evicted"; continue; fi
      if [ $rc -ne 0 ]; then echo "  $t: could not evict (exit $rc)"; continue; fi
    fi
    echo "  evicted $t (-$(gb "$kb"), idle ${idle}h; low disk: $(gb "$free") free < $MIN_FREE_GB GB)"
  done < <(printf '%s\n' "$TARGETS" | while IFS= read -r t; do
             [ -d "$t" ] && echo "$(last_used "$t") $t"
           done | sort -n)
fi

printf '%s\n' "$TARGETS" | head -1 | while IFS= read -r t; do
  echo "  free: $(gb "$(free_kb "$(dirname "$t")")")"
done
