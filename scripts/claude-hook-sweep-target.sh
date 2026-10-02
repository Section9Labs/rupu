#!/usr/bin/env bash
# Claude Code hook (.claude/settings.json, PostToolUse/PostToolUseFailure on
# Bash, async): after a session runs cargo or make, sweep that session's own
# target/ with scripts/sweep-cargo-targets.sh — superseded debug objects, stale
# test binaries and incremental caches. Never evicts a whole target and never
# touches another worktree's.
#
# Reads the hook's JSON on stdin (only tool_input.command is used). Runs at
# most once per SWEEP_HOOK_INTERVAL_MIN minutes (default 15) per target, one at
# a time, logs to target/.sweep.log, and always exits 0 so it can never fail
# the tool call that triggered it.

root="${CLAUDE_PROJECT_DIR:-$PWD}"
t="$root/target"
interval="${SWEEP_HOOK_INTERVAL_MIN:-15}"

is_build=$(perl -MJSON::PP -0777 -ne '
  my $j = eval { decode_json($_) } or exit;
  my $c = $j->{tool_input}{command} // "";
  print 1 if $c =~ /(?:^|[\s;&|(])(?:cargo|make)(?:\s|$)/;
' 2>/dev/null)
[ "$is_build" = 1 ] || exit 0
grep -q 'created by cargo' "$t/CACHEDIR.TAG" 2>/dev/null || exit 0

stamp="$t/.sweep-hook.stamp"
if [ -f "$stamp" ] && [ -z "$(find "$stamp" -mmin +"$interval" 2>/dev/null)" ]; then
  exit 0
fi
# One sweep at a time; a lock left by a killed sweep expires after an hour.
lock="$t/.sweep-hook.lock"
find "$lock" -maxdepth 0 -type d -mmin +60 -exec rmdir {} \; 2>/dev/null
mkdir "$lock" 2>/dev/null || exit 0
trap 'rmdir "$lock" 2>/dev/null' EXIT
touch "$stamp"

"$(dirname "$0")/sweep-cargo-targets.sh" --target "$t" --min-free-gb 0 --log "$t/.sweep.log"
exit 0
