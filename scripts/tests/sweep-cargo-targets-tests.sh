#!/usr/bin/env bash
# Plain-shell tests for scripts/sweep-cargo-targets.sh and the Claude Code hook
# that runs it (scripts/claude-hook-sweep-target.sh). Run directly:
#
#     scripts/tests/sweep-cargo-targets-tests.sh
#
# Fixture target dirs are built with fake artifacts whose mtimes/atimes are set
# by `touch -t`, so every rule is checked without compiling anything. The last
# case builds a real crate (when cargo is installed) to pin the two facts the
# object rule rests on: rustc's `<crate>-<hash>.<cgu>.<tag>.rcgu.o` naming, and
# cargo not rebuilding when those objects disappear.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SWEEP="$ROOT/scripts/sweep-cargo-targets.sh"
HOOK="$ROOT/scripts/claude-hook-sweep-target.sh"
PASS=0
FAIL=0
# Physical path: the sweep reports targets by physical path (/tmp vs /private/tmp).
WORK=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/sweep-tests.XXXXXX")" && pwd -P)
trap 'rm -rf "$WORK"' EXIT

ok() { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n       %s\n' "$1" "$2"; }
assert_exists() { if [ -e "$2" ]; then ok "$1"; else bad "$1" "expected to exist: $2"; fi; }
assert_gone() { if [ -e "$2" ]; then bad "$1" "expected deleted: $2"; else ok "$1"; fi; }
assert_contains() {
  case "$2" in *"$3"*) ok "$1" ;; *) bad "$1" "output lacks '$3':"$'\n'"$2" ;; esac
}
assert_before() {  # assert_before <name> <haystack> <first> <second>
  local rest=${2#*"$3"}
  if [ "$rest" != "$2" ] && [ "${rest#*"$4"}" != "$rest" ]; then ok "$1"
  else bad "$1" "expected '$3' before '$4' in:"$'\n'"$2"; fi
}

# stamp <minutes-ago> -> touch -t timestamp (BSD or GNU date).
stamp() {
  date -v-"$1"M +%Y%m%d%H%M.%S 2>/dev/null || date -d "-$1 minutes" +%Y%m%d%H%M.%S
}
# art <path> <mtime-minutes-ago> [atime-minutes-ago]: a fake artifact.
art() {
  mkdir -p "$(dirname "$1")"
  printf 'x' > "$1"
  touch -m -t "$(stamp "$2")" "$1"
  touch -a -t "$(stamp "${3:-$2}")" "$1"
}
# dir_age <dir> <minutes-ago>: after its contents exist.
dir_age() { touch -m -t "$(stamp "$2")" "$1"; }
mk_target() {
  mkdir -p "$1/debug/deps" "$1/debug/incremental"
  printf 'Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n' \
    > "$1/CACHEDIR.TAG"
  : > "$1/debug/.cargo-lock"
}
# A target exercising every per-target rule.
fixture() {
  local t="$1" d="$1/debug/deps" i="$1/debug/incremental" day=$((3 * 24 * 60))
  mk_target "$t"
  # objects: a live binary rebuilt twice (tagA superseded by tagB)
  art "$d/foo-aaaa" 10
  art "$d/foo-aaaa.c1.tagA.rcgu.o" 120
  art "$d/foo-aaaa.c2.tagA.rcgu.o" 120
  art "$d/foo-aaaa.c1.tagB.rcgu.o" 30
  # objects: a live library's only set, however old
  art "$d/libbar-bbbb.rlib" 600
  art "$d/bar-bbbb.c1.tagC.rcgu.o" 600
  # objects: orphans, one past the guard and one possibly mid-link
  art "$d/gone-cccc.c1.tagD.rcgu.o" 120
  art "$d/new-dddd.c1.tagE.rcgu.o" 5
  # objects: a name that is not rustc's shape is left alone
  art "$d/odd-eeee.rcgu.o" 600
  # binaries: neither built nor run for 3 days, with its companions
  art "$d/old-ffff" "$day"
  art "$d/old-ffff.d" "$day"
  art "$d/old-ffff.c1.tagF.rcgu.o" "$day"
  mkdir -p "$d/old-ffff.dSYM/Contents"
  # binaries: built 3 days ago but run an hour ago
  art "$d/ran-gggg" "$day" 60
  # release binaries are never age-swept
  art "$1/release/deps/rel-kkkk" "$day"
  # libraries are never age-swept
  art "$d/libold-hhhh.rlib" "$day"
  # incremental: stale and fresh
  art "$i/foo-iiii/s-old/dep-graph.bin" "$day"; dir_age "$i/foo-iiii" "$day"
  art "$i/foo-jjjj/s-new/dep-graph.bin" 60; dir_age "$i/foo-jjjj" 60
}
sweep() { "$SWEEP" --min-free-gb 0 "$@" 2>&1; }

echo "per-target rules"
T="$WORK/a/target"; D="$T/debug/deps"; I="$T/debug/incremental"
fixture "$T"
out=$(sweep --scan "$WORK/a")
assert_contains "finds the target by CACHEDIR.TAG" "$out" "1 target dirs"
assert_gone   "superseded object set deleted (1/2)" "$D/foo-aaaa.c1.tagA.rcgu.o"
assert_gone   "superseded object set deleted (2/2)" "$D/foo-aaaa.c2.tagA.rcgu.o"
assert_exists "newest object set of a live binary kept" "$D/foo-aaaa.c1.tagB.rcgu.o"
assert_exists "only object set of a live library kept" "$D/bar-bbbb.c1.tagC.rcgu.o"
assert_gone   "orphan objects past the guard deleted" "$D/gone-cccc.c1.tagD.rcgu.o"
assert_exists "young orphan objects kept (link may be in progress)" "$D/new-dddd.c1.tagE.rcgu.o"
assert_exists "unrecognised object names kept" "$D/odd-eeee.rcgu.o"
assert_gone   "stale binary deleted" "$D/old-ffff"
assert_gone   "stale binary's .d deleted" "$D/old-ffff.d"
assert_gone   "stale binary's objects deleted" "$D/old-ffff.c1.tagF.rcgu.o"
assert_gone   "stale binary's .dSYM deleted" "$D/old-ffff.dSYM"
assert_exists "recently run binary kept" "$D/ran-gggg"
assert_exists "live binary kept" "$D/foo-aaaa"
assert_exists "old library kept" "$D/libold-hhhh.rlib"
assert_exists "old release binary kept" "$T/release/deps/rel-kkkk"
assert_gone   "stale incremental cache deleted" "$I/foo-iiii"
assert_exists "fresh incremental cache kept" "$I/foo-jjjj"
assert_contains "reports objects" "$out" "objects -"
assert_contains "reports binaries" "$out" "binaries -"

echo "--dry-run"
T="$WORK/b/target"; D="$T/debug/deps"
fixture "$T"
out=$(sweep --scan "$WORK/b" --dry-run)
assert_exists "dry run keeps superseded objects" "$D/foo-aaaa.c1.tagA.rcgu.o"
assert_exists "dry run keeps stale binaries" "$D/old-ffff"
assert_exists "dry run keeps stale incremental" "$T/debug/incremental/foo-iiii"
assert_contains "dry run still reports" "$out" "objects -"

if command -v perl >/dev/null 2>&1; then
  echo "a target mid-build"
  T="$WORK/c/target"; D="$T/debug/deps"
  fixture "$T"
  perl -MFcntl=:flock -e 'open(my $f, "<", $ARGV[0]) or die; flock($f, LOCK_EX) or die;
    print "locked\n"; $| = 1; sleep 30' "$T/debug/.cargo-lock" > "$WORK/lock.out" &
  holder=$!
  for _ in 1 2 3 4 5 6 7 8 9 10; do grep -q locked "$WORK/lock.out" 2>/dev/null && break; sleep 0.2; done
  out=$(sweep --scan "$WORK/c")
  kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
  assert_gone   "objects still swept while building" "$D/foo-aaaa.c1.tagA.rcgu.o"
  assert_exists "binaries left alone while building" "$D/old-ffff"
  assert_exists "incremental left alone while building" "$T/debug/incremental/foo-iiii"
  assert_contains "says why" "$out" "building right now"

  if command -v lsof >/dev/null 2>&1; then
    echo "eviction"
    for n in old older recent; do mk_target "$WORK/e/$n/target"; done
    art "$WORK/e/older/target/debug/deps/x-1111" 600
    art "$WORK/e/old/target/debug/deps/x-2222" 300
    art "$WORK/e/recent/target/debug/deps/x-3333" 300 10
    # The least recently used of all, but a program started from it is running.
    busy=""
    if command -v cc >/dev/null 2>&1; then
      mk_target "$WORK/e/busy/target"
      art "$WORK/e/busy/target/debug/deps/x-4444" 900
      printf '#include <unistd.h>\nint main(void) { sleep(60); return 0; }\n' > "$WORK/sleeper.c"
      if cc -o "$WORK/e/busy/target/debug/sleeper" "$WORK/sleeper.c" 2>/dev/null; then
        "$WORK/e/busy/target/debug/sleeper" &
        busy=$!
        sleep 1
      fi
    fi
    out=$("$SWEEP" --scan "$WORK/e" --min-free-gb 0 2>&1)
    assert_exists "--min-free-gb 0 never evicts" "$WORK/e/older/target"
    # No disk has this much free, so every idle target goes, LRU first.
    out=$("$SWEEP" --scan "$WORK/e" --min-free-gb 999999999 --evict-idle-hours 1 2>&1)
    assert_gone   "idle target evicted when disk is low" "$WORK/e/older/target"
    assert_gone   "second idle target evicted when still low" "$WORK/e/old/target"
    assert_before "least recently used goes first" "$out" "evicted $WORK/e/older/target" "evicted $WORK/e/old/target"
    assert_exists "target run 10 minutes ago kept" "$WORK/e/recent/target"
    assert_contains "says why it kept it" "$out" "not evicting"
    assert_exists "eviction leaves the worktree itself" "$WORK/e/old"
    if [ -n "$busy" ]; then
      assert_exists "target a running program was started from is kept" "$WORK/e/busy/target/debug/sleeper"
      assert_contains "says a program is running from it" "$out" "running program was started from $WORK/e/busy/target"
      if kill -0 "$busy" 2>/dev/null; then ok "that program is still running"; else bad "that program is still running" "pid $busy gone"; fi
      kill "$busy" 2>/dev/null; wait "$busy" 2>/dev/null
    else
      echo "  --   no C compiler: skipping the running-program case"
    fi
  else
    echo "(lsof missing: skipping eviction cases)"
  fi
else
  echo "(perl missing: skipping lock and eviction cases)"
fi

echo "--repo finds every worktree"
if command -v git >/dev/null 2>&1; then
  R="$WORK/repo"
  git init -q "$R" && git -C "$R" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init
  git -C "$R" worktree add -q "$WORK/repo-wt" -b sweep-test 2>/dev/null
  mk_target "$R/target"; mk_target "$WORK/repo-wt/target"
  out=$(sweep --repo "$R")
  assert_contains "main checkout and worktree" "$out" "2 target dirs"
fi

echo "--target sweeps only that target"
fixture "$WORK/t1/target"; fixture "$WORK/t2/target"
out=$(sweep --target "$WORK/t1/target")
assert_contains "one target" "$out" "1 target dirs"
assert_gone   "the named target is swept" "$WORK/t1/target/debug/deps/foo-aaaa.c1.tagA.rcgu.o"
assert_exists "other targets are untouched" "$WORK/t2/target/debug/deps/foo-aaaa.c1.tagA.rcgu.o"

if command -v perl >/dev/null 2>&1; then
  echo "Claude Code hook"
  P="$WORK/proj"; D="$P/target/debug/deps"
  fixture "$P/target"
  # hook <bash command>: the PostToolUse JSON Claude Code sends, on stdin.
  hook() {
    printf '{"tool_name":"Bash","tool_input":{"command":%s},"tool_response":{"stdout":"cargo"}}' \
      "$(printf '%s' "$1" | perl -MJSON::PP -0777 -ne 'print JSON::PP->new->allow_nonref->encode($_)')" |
      CLAUDE_PROJECT_DIR="$P" "$HOOK"
  }
  reseed() { art "$D/foo-aaaa.c1.tagA.rcgu.o" 120; }   # a superseded set again
  age_stamp() { touch -m -t "$(stamp 20)" "$P/target/.sweep-hook.stamp"; }
  hook "ls -la"
  assert_exists "ignores commands that run neither cargo nor make" "$D/foo-aaaa.c1.tagA.rcgu.o"
  hook "cargoship deploy"
  assert_exists "ignores words that merely start with cargo" "$D/foo-aaaa.c1.tagA.rcgu.o"
  hook "cd crates/rupu-cli && cargo test -p rupu-cli"; rc=$?
  assert_gone   "sweeps its own target after a cargo command" "$D/foo-aaaa.c1.tagA.rcgu.o"
  assert_exists "keeps the current object set" "$D/foo-aaaa.c1.tagB.rcgu.o"
  assert_exists "logs to target/.sweep.log" "$P/target/.sweep.log"
  if [ "$rc" -eq 0 ]; then ok "exits 0"; else bad "exits 0" "exit $rc"; fi
  reseed; hook "cargo build"
  assert_exists "at most once per interval" "$D/foo-aaaa.c1.tagA.rcgu.o"
  age_stamp; hook "make lint"
  assert_gone   "runs again once the interval has passed, after make too" "$D/foo-aaaa.c1.tagA.rcgu.o"
  reseed; age_stamp; mkdir "$P/target/.sweep-hook.lock"; hook "cargo build"
  assert_exists "one sweep at a time" "$D/foo-aaaa.c1.tagA.rcgu.o"
  touch -m -t "$(stamp 90)" "$P/target/.sweep-hook.lock"; hook "cargo build"
  assert_gone   "a lock left by a killed sweep expires" "$D/foo-aaaa.c1.tagA.rcgu.o"
  assert_gone   "releases its lock" "$P/target/.sweep-hook.lock"
  reseed; age_stamp
  printf 'not json' | CLAUDE_PROJECT_DIR="$P" "$HOOK"; rc=$?
  assert_exists "ignores input it cannot parse" "$D/foo-aaaa.c1.tagA.rcgu.o"
  if [ "$rc" -eq 0 ]; then ok "exits 0 on bad input"; else bad "exits 0 on bad input" "exit $rc"; fi
  printf '{"tool_input":{"command":"cargo build"}}' | CLAUDE_PROJECT_DIR="$WORK/nowhere" "$HOOK"; rc=$?
  if [ "$rc" -eq 0 ]; then ok "exits 0 without a target dir"; else bad "exits 0 without a target dir" "exit $rc"; fi
fi

echo "argument checks"
if "$SWEEP" >/dev/null 2>&1; then bad "refuses to run with nothing to sweep" "exit 0"; else ok "refuses to run with nothing to sweep"; fi
if "$SWEEP" --scan "$WORK" --binary-age-hours x >/dev/null 2>&1; then bad "rejects a non-number" "exit 0"; else ok "rejects a non-number"; fi

if command -v cargo >/dev/null 2>&1; then
  echo "real cargo build"
  C="$WORK/real/probe"
  mkdir -p "$C/src"
  printf '[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n' > "$C/Cargo.toml"
  printf 'pub fn f(x: u64) -> u64 { (0..x).map(|i| i * 3).sum() } // 0\n' > "$C/src/lib.rs"
  printf 'fn main() { println!("{}", probe::f(10)); }\n' > "$C/src/main.rs"
  build() { (cd "$C" && env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR cargo build --offline 2>&1); }
  for v in 1 2 3; do
    sed -e "s|i \\* [0-9]*|i * $v|" -e "s|// [0-9]*|// $v|" "$C/src/lib.rs" > "$C/src/lib.rs.new"
    mv "$C/src/lib.rs.new" "$C/src/lib.rs"
    build >/dev/null
  done
  # Keys (binary or library) still holding more than one object set.
  multi() {
    find "$C/target/debug/deps" -name '*.rcgu.o' | sed -E 's|.*/||' |
      awk -F. 'NF == 5 { print $1, $3 }' | sort -u | awk '{ c[$1]++ } END { for (k in c) if (c[k] > 1) print k }'
  }
  # Only macOS keeps objects in deps/ (split-debuginfo=unpacked); elsewhere
  # there is nothing to sweep and only the no-rebuild checks below apply.
  if [ -n "$(multi)" ]; then ok "rustc left one object set per rebuild (the leak this sweeps)"
  else echo "  --   no loose objects on this platform"; fi
  sweep --scan "$WORK/real" >/dev/null
  tags=$(multi)
  if [ -z "$tags" ]; then ok "one object set per binary/library after the sweep"
  else bad "one object set per binary/library after the sweep" "still several: $tags"; fi
  rebuild=$(build)
  case "$rebuild" in
    *Compiling*) bad "cargo does not rebuild after the sweep" "$rebuild" ;;
    *) ok "cargo does not rebuild after the sweep" ;;
  esac
  printf 'fn main() { println!("{}", probe::f(4)); }\n' > "$C/src/main.rs"
  build >/dev/null
  ran=$("$C/target/debug/probe" 2>&1)
  if [ "$ran" = "18" ]; then ok "an edit after the sweep builds and runs"; else bad "an edit after the sweep builds and runs" "$ran"; fi
else
  echo "(cargo missing: skipping the real-build case)"
fi

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
