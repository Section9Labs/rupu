# Scheduled Release Cadence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish a beta every day that `main` moves and a stable every week, by driving the existing `release.yml` from two scheduled workflows.

**Architecture:** Neither new workflow builds anything — each computes a tag and pushes it, and the existing `release.yml` does all platform work. The version arithmetic lives in two **pure shell scripts that read tag data from stdin** rather than calling `git` themselves, which is what makes them testable against fixtures without a repository. A plain-shell test harness (no new dependency) runs them in CI.

**Tech Stack:** GitHub Actions, POSIX shell, `gh` CLI.

## Spec

`docs/superpowers/specs/2026-07-30-rupu-release-cadence-design.md`

## Global Constraints

- **Every script is POSIX `sh`-compatible and runs under `set -euo pipefail`.** These run unattended; a silent failure publishes a wrong tag.
- **The scripts never invoke `git` or `gh`.** They read tag data on stdin and write one line to stdout. The workflow supplies the data. This is the whole reason they can be tested.
- **A skipped run is a success.** Both workflows `exit 0` when there is nothing to do, and log which condition stopped them.
- **Only `release-stable.yml` may push to `main`, and only the bump commit.** This is the narrowly-scoped carve-out from the never-commit-to-main rule (spec §2). Nothing else in CI pushes to `main`.
- **Never run `cargo fmt` in any form** — including `cargo fmt -- <path>`, which resolves the whole workspace. Not needed in this plan (no Rust changes), but it applies if you touch any.
- Cron times stay off the `:00` mark and clear of `nightly-live-tests.yml`'s 08:00 UTC slot.

## File structure

| File | Responsibility |
|------|----------------|
| `scripts/next-beta-version.sh` | **Create.** Pure. Given a base version and beta tags on stdin, emit the next `v<base>-beta.N`. |
| `scripts/pick-promotable-beta.sh` | **Create.** Pure. Given a soak window and `tag<TAB>unixtime` lines on stdin, emit the tag to promote, or nothing. |
| `scripts/tests/release-cadence-tests.sh` | **Create.** Plain-shell harness + cases for both scripts. No new dependency. |
| `.github/workflows/release-beta.yml` | **Create.** Daily cron + `workflow_dispatch(force)`. Evaluates the three conditions, pushes the beta tag. |
| `.github/workflows/release-stable.yml` | **Create.** Weekly cron. Promotes a soaked beta, then pushes the bump commit to `main`. |
| `.github/workflows/release.yml` | **Modify.** Widen the tag matcher to accept `-beta.N`. |
| `.github/workflows/ci.yml` | **Modify.** Run the shell tests. |
| `Makefile` | **Modify.** Delete `gh-beta`, `gh-stable`, `gh-build`. |
| `scripts/gh-build.sh` | **Delete.** Nothing else calls it. |

---

### Task 1: `next-beta-version.sh`

**Files:**
- Create: `scripts/next-beta-version.sh`
- Create: `scripts/tests/release-cadence-tests.sh`

**Interfaces:**
- Produces: `scripts/next-beta-version.sh <base-version>`, reading candidate tag names (one per line) on stdin, writing one tag (e.g. `v0.71.0-beta.4`) to stdout. Exits non-zero with a message on stderr if `<base-version>` is malformed.

- [ ] **Step 1: Write the failing tests**

Create `scripts/tests/release-cadence-tests.sh`:

```sh
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
assert_out "beta.9 goes to beta.10, not beta.2" "v0.71.0-beta.10" \
  sh -c "printf 'v0.71.0-beta.8\nv0.71.0-beta.9\n' | '$NEXT' 0.71.0"

# Betas belonging to a different base must not influence the counter.
assert_out "counter resets after a base bump" "v0.72.0-beta.1" \
  sh -c "printf 'v0.71.0-beta.1\nv0.71.0-beta.2\nv0.71.0\n' | '$NEXT' 0.72.0"

# The legacy suffix-less form counts as beta 1 of that base, so the next is 2.
assert_out "legacy bare -beta tag counts as 1" "v0.71.0-beta.2" \
  sh -c "printf 'v0.71.0-beta\n' | '$NEXT' 0.71.0"

# Unrelated refs must be ignored rather than parsed as a counter.
assert_out "ignores rolling and unrelated tags" "v0.71.0-beta.1" \
  sh -c "printf 'latest-beta\nlatest-stable\nnightly\n' | '$NEXT' 0.71.0"

assert_fails "rejects a malformed base version" \
  sh -c "printf '' | '$NEXT' 'not-a-version'"

assert_fails "rejects a missing base version" \
  sh -c "printf '' | '$NEXT'"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
chmod +x scripts/tests/release-cadence-tests.sh
scripts/tests/release-cadence-tests.sh
```

Expected: every case FAILs — `scripts/next-beta-version.sh` does not exist.

- [ ] **Step 3: Implement the script**

Create `scripts/next-beta-version.sh`:

```sh
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
case "$base" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "usage: $0 <base-version>  (e.g. 0.71.0)" >&2; exit 2 ;;
esac

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
  if [ "$n" -gt "$max" ]; then
    max="$n"
  fi
done

echo "v$base-beta.$((max + 1))"
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
chmod +x scripts/next-beta-version.sh
scripts/tests/release-cadence-tests.sh
```

Expected: `7 passed, 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add scripts/next-beta-version.sh scripts/tests/release-cadence-tests.sh
git commit -m "feat(release): derive the next beta counter from existing tags"
```

---

### Task 2: `pick-promotable-beta.sh`

**Files:**
- Create: `scripts/pick-promotable-beta.sh`
- Modify: `scripts/tests/release-cadence-tests.sh` (append a second section)

**Interfaces:**
- Consumes: the `assert_out` / `assert_fails` helpers from Task 1's harness.
- Produces: `scripts/pick-promotable-beta.sh <soak-days> <now-unixtime>`, reading `<tag><TAB><commit-unixtime>` lines on stdin, writing the tag to promote to stdout — or **nothing at all**, exit 0, when none qualifies. `<now-unixtime>` is a parameter rather than read from the clock so the tests are deterministic.

- [ ] **Step 1: Write the failing tests**

Append to `scripts/tests/release-cadence-tests.sh`, immediately before the final `printf`/`[ "$FAIL" -eq 0 ]` lines:

```sh
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
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
scripts/tests/release-cadence-tests.sh
```

Expected: Task 1's 7 cases still pass; the 7 new cases FAIL — `scripts/pick-promotable-beta.sh` does not exist.

- [ ] **Step 3: Implement the script**

Create `scripts/pick-promotable-beta.sh`:

```sh
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

while IFS="$(printf '\t')" read -r tag ts; do
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
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
chmod +x scripts/pick-promotable-beta.sh
scripts/tests/release-cadence-tests.sh
```

Expected: `14 passed, 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add scripts/pick-promotable-beta.sh scripts/tests/release-cadence-tests.sh
git commit -m "feat(release): pick the newest soaked beta for promotion"
```

---

### Task 3: Widen `release.yml`'s tag matcher, and gate the scripts in CI

**Files:**
- Modify: `.github/workflows/release.yml` (the `meta` job's `case` block, ~line 43)
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `scripts/tests/release-cadence-tests.sh` (Tasks 1–2).
- Produces: `release.yml` accepts `v<X.Y.Z>-beta.<N>` as a beta tag; CI runs the shell tests on every PR.

- [ ] **Step 1: Widen the matcher**

In `.github/workflows/release.yml`, the `meta` job's `case "$tag" in` block currently opens with:

```sh
            v[0-9]*.[0-9]*.[0-9]*-beta)
```

Replace that single line with:

```sh
            v[0-9]*.[0-9]*.[0-9]*-beta|v[0-9]*.[0-9]*.[0-9]*-beta.[0-9]*)
```

The bare `-beta` alternative is kept deliberately: pre-existing tags and manual
`workflow_dispatch` re-publishes of them must keep resolving to the beta
channel.

- [ ] **Step 2: Verify the matcher by hand**

The `case` logic is a pure shell fragment, so exercise it directly rather than by dispatching a workflow:

```bash
for tag in v0.71.0-beta v0.71.0-beta.4 v0.71.0 v0.71.0-beta.x vfoo; do
  case "$tag" in
    v[0-9]*.[0-9]*.[0-9]*-beta|v[0-9]*.[0-9]*.[0-9]*-beta.[0-9]*) echo "$tag -> beta" ;;
    v[0-9]*.[0-9]*.[0-9]*) echo "$tag -> stable" ;;
    *) echo "$tag -> REJECTED" ;;
  esac
done
```

Expected exactly:

```
v0.71.0-beta -> beta
v0.71.0-beta.4 -> beta
v0.71.0 -> stable
v0.71.0-beta.x -> REJECTED
vfoo -> REJECTED
```

- [ ] **Step 3: Run the shell tests in CI**

In `.github/workflows/ci.yml`, add a job alongside the existing ones. Match the
surrounding style (the file already has a `runs-on: ubuntu-latest` +
`timeout-minutes` + `actions/checkout@v5` shape):

```yaml
  release-scripts:
    name: release cadence scripts
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@v5
      # These scripts decide what gets published unattended every morning. A
      # counter regression here silently republishes an old version, so they
      # are gated on every PR rather than trusted.
      - run: scripts/tests/release-cadence-tests.sh
```

- [ ] **Step 4: Verify the tests still pass locally**

```bash
scripts/tests/release-cadence-tests.sh
```

Expected: `14 passed, 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml .github/workflows/ci.yml
git commit -m "feat(release): accept -beta.N tags and gate the cadence scripts in CI"
```

---

### Task 4: `release-beta.yml`

**Files:**
- Create: `.github/workflows/release-beta.yml`

**Interfaces:**
- Consumes: `scripts/next-beta-version.sh` (Task 1); `release.yml`'s widened matcher (Task 3).
- Produces: a pushed `v<base>-beta.N` tag, which triggers `release.yml`.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release-beta.yml`:

```yaml
name: release-beta

# Cuts a beta every day that `main` actually moved. Publishing itself is
# `release.yml`'s job — this workflow only decides on a tag and pushes it.
on:
  schedule:
    # 07:37 UTC. Off the :00 mark (every scheduled workflow on GitHub piles
    # onto it) and clear of nightly-live-tests.yml's 08:00 slot.
    - cron: "37 7 * * *"
  workflow_dispatch:
    inputs:
      force:
        description: "Release even if main has not moved since the last beta"
        type: boolean
        default: false

concurrency:
  group: release-beta
  # Two beta cuts racing would compute the same counter and one would fail to
  # push. Queue instead of cancelling.
  cancel-in-progress: false

permissions:
  contents: write

jobs:
  cut:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
        with:
          ref: main
          # Full history + tags: the counter is derived from the tag list.
          fetch-depth: 0

      - name: Decide whether to cut a beta
        id: decide
        env:
          GH_TOKEN: ${{ github.token }}
          FORCE: ${{ inputs.force }}
        run: |
          set -euo pipefail

          base=$(grep -E '^version = "[0-9]+\.[0-9]+\.[0-9]+' Cargo.toml \
            | head -n1 | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+[^"]*)".*/\1/')
          echo "base version: $base"

          # -- Condition 2: the stall-guard. ------------------------------
          # After stable v0.71.0 ships, a v0.71.0-beta.N sorts BELOW it under
          # semver and would walk `rupu update` backwards on the beta channel.
          # Deliberately NOT bypassable by `force`.
          latest_stable=$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' \
            | grep -vE -- '-beta' | sort -V | tail -n1 || true)
          if [ -n "$latest_stable" ] && [ "v$base" = "$latest_stable" ]; then
            echo "::notice::base $base equals the shipped stable $latest_stable — waiting for the version bump; skipping"
            echo "go=false" >> "$GITHUB_OUTPUT"
            exit 0
          fi

          # -- Condition 3: main must be green. ---------------------------
          # Without this a red main ships a broken beta every morning.
          # Deliberately NOT bypassable by `force`.
          conclusion=$(gh run list --branch main --workflow ci.yml \
            --limit 1 --json conclusion --jq '.[0].conclusion // "none"')
          if [ "$conclusion" != "success" ]; then
            echo "::notice::latest ci.yml run on main concluded '$conclusion' — skipping"
            echo "go=false" >> "$GITHUB_OUTPUT"
            exit 0
          fi

          # -- Condition 1: main must have moved. -------------------------
          # This one IS bypassable: re-releasing an unchanged main is a
          # legitimate manual act (a botched upload, a signing fix).
          prev=$(git tag -l "v$base-beta" "v$base-beta.*" | sort -V | tail -n1 || true)
          if [ -n "$prev" ] && [ "$FORCE" != "true" ]; then
            if [ "$(git rev-parse HEAD)" = "$(git rev-parse "$prev^{commit}")" ]; then
              echo "::notice::main is unchanged since $prev — skipping"
              echo "go=false" >> "$GITHUB_OUTPUT"
              exit 0
            fi
          fi

          tag=$(git tag -l | scripts/next-beta-version.sh "$base")
          echo "next tag: $tag"
          echo "tag=$tag" >> "$GITHUB_OUTPUT"
          echo "go=true"  >> "$GITHUB_OUTPUT"

      - name: Push the tag
        if: steps.decide.outputs.go == 'true'
        run: |
          set -euo pipefail
          git tag "${{ steps.decide.outputs.tag }}"
          git push origin "${{ steps.decide.outputs.tag }}"
          echo "::notice::pushed ${{ steps.decide.outputs.tag }} — release.yml takes it from here"
```

- [ ] **Step 2: Validate the YAML parses**

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release-beta.yml')); print('valid')"
```

Expected: `valid`.

- [ ] **Step 3: Verify the skip logic reads correctly against the live repo**

This runs the same commands the workflow will, read-only, so a logic error surfaces before the first unattended 07:37 run:

```bash
base=$(grep -E '^version = "[0-9]+\.[0-9]+\.[0-9]+' Cargo.toml | head -n1 | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+[^"]*)".*/\1/')
echo "base: $base"
git fetch origin --tags --quiet
echo "latest stable: $(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' | grep -vE -- '-beta' | sort -V | tail -n1)"
echo "latest beta for this base: $(git tag -l "v$base-beta" "v$base-beta.*" | sort -V | tail -n1)"
echo "next would be: $(git tag -l | scripts/next-beta-version.sh "$base")"
```

Confirm the "next would be" value is greater than every existing beta for that base, and that the base is not equal to the latest stable.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release-beta.yml
git commit -m "feat(release): daily beta cron, skipped when main is unchanged or red"
```

---

### Task 5: `release-stable.yml`

**Files:**
- Create: `.github/workflows/release-stable.yml`

**Interfaces:**
- Consumes: `scripts/pick-promotable-beta.sh` (Task 2); `make bump VERSION=<X.Y.Z>` (existing, creates a `release: bump workspace to vX.Y.Z` commit and does **not** push).
- Produces: a pushed `v<X.Y.Z>` tag on a soaked beta's commit, plus a bump commit on `main`.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release-stable.yml`:

```yaml
name: release-stable

# Promotes a beta that has already soaked. Stable is therefore always a build
# that shipped as a beta first — never code nobody has installed.
on:
  schedule:
    # Sunday 09:17 UTC, after the morning beta slot.
    - cron: "17 9 * * 0"
  workflow_dispatch: {}

concurrency:
  group: release-stable
  cancel-in-progress: false

permissions:
  contents: write

env:
  # Days a beta must have existed before it can become stable. Two means a
  # Thursday beta can promote on Sunday but Saturday's cannot.
  SOAK_DAYS: "2"

jobs:
  promote:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v5
        with:
          ref: main
          fetch-depth: 0
          # A PAT is not needed — contents:write plus the default token can
          # push to an unprotected branch. If main ever gains protection, this
          # step is the one that breaks, loudly.
          token: ${{ github.token }}

      - name: Pick a promotable beta
        id: pick
        run: |
          set -euo pipefail

          # `creatordate` is the tag's own date for annotated tags; these are
          # lightweight, so it resolves to the commit date — which is the age
          # the soak window is actually about.
          candidates=$(git for-each-ref --format='%(refname:short)%09%(creatordate:unix)' \
            'refs/tags/v*-beta' 'refs/tags/v*-beta.*')

          tag=$(printf '%s\n' "$candidates" \
            | scripts/pick-promotable-beta.sh "$SOAK_DAYS" "$(date -u +%s)")

          if [ -z "$tag" ]; then
            echo "::notice::no beta older than ${SOAK_DAYS}d to promote — skipping this week"
            echo "go=false" >> "$GITHUB_OUTPUT"
            exit 0
          fi

          # Refuse a beta whose commit is no longer reachable from main (a
          # force-push or rebase dropped it). Promoting it would ship code
          # that is no longer in the tree.
          sha=$(git rev-parse "$tag^{commit}")
          # HEAD, not origin/main: the checkout above already put us on main,
          # and a remote-tracking ref is not guaranteed to exist in the
          # actions/checkout working copy.
          if ! git merge-base --is-ancestor "$sha" HEAD; then
            echo "::error::$tag ($sha) is not an ancestor of main — refusing to promote unreachable code"
            exit 1
          fi

          version="${tag#v}"; version="${version%%-beta*}"
          echo "tag=$tag"          >> "$GITHUB_OUTPUT"
          echo "sha=$sha"          >> "$GITHUB_OUTPUT"
          echo "version=$version"  >> "$GITHUB_OUTPUT"
          echo "go=true"           >> "$GITHUB_OUTPUT"
          echo "promoting $tag ($sha) -> v$version"

      - name: Tag the promoted commit as stable
        if: steps.pick.outputs.go == 'true'
        run: |
          set -euo pipefail
          v="v${{ steps.pick.outputs.version }}"
          if git rev-parse "$v" >/dev/null 2>&1; then
            echo "::notice::$v already exists — nothing to promote"
            exit 0
          fi
          # Tag the BETA's commit, not main's head: that is what makes stable
          # byte-identical in content to the beta that soaked.
          git tag "$v" "${{ steps.pick.outputs.sha }}"
          git push origin "$v"
          echo "::notice::pushed $v — release.yml publishes it"

      - name: Bump the base version for the next beta series
        if: steps.pick.outputs.go == 'true'
        run: |
          set -euo pipefail
          # Without this the next beta would be v<same>-beta.N, which sorts
          # BELOW the stable just shipped and walks `rupu update` backwards.
          #
          # This push to main is the one narrowly-scoped exception to the
          # never-commit-to-main rule (see the design doc). Nothing else in CI
          # may push to main.
          version="${{ steps.pick.outputs.version }}"
          major="${version%%.*}"
          minor="${version#*.}"; minor="${minor%%.*}"
          next="$major.$((minor + 1)).0"

          git config user.name  "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

          make bump VERSION="$next"
          git push origin HEAD:main
          echo "::notice::bumped base version to $next for the next beta series"
```

- [ ] **Step 2: Validate the YAML parses**

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release-stable.yml')); print('valid')"
```

Expected: `valid`.

- [ ] **Step 3: Verify the promotion picker against the live tag set**

```bash
git fetch origin --tags --quiet
git for-each-ref --format='%(refname:short)%09%(creatordate:unix)' \
  'refs/tags/v*-beta' 'refs/tags/v*-beta.*' \
  | scripts/pick-promotable-beta.sh 2 "$(date -u +%s)"
```

Expected: either a single beta tag, or empty output with exit 0. Both are correct answers depending on the repo's tag state — confirm the result matches what `git tag -l 'v*-beta*'` shows.

- [ ] **Step 4: Verify the minor-bump arithmetic**

```bash
for version in 0.71.0 0.9.3 1.0.0; do
  major="${version%%.*}"; minor="${version#*.}"; minor="${minor%%.*}"
  echo "$version -> $major.$((minor + 1)).0"
done
```

Expected exactly:

```
0.71.0 -> 0.72.0
0.9.3 -> 0.10.0
1.0.0 -> 1.1.0
```

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release-stable.yml
git commit -m "feat(release): weekly stable promotion of a soaked beta"
```

---

### Task 6: Retire the local publish path

**Files:**
- Modify: `Makefile` (delete the `gh-beta`, `gh-stable`, `gh-build` targets and their comment block, ~lines 60–90)
- Delete: `scripts/gh-build.sh`

**Interfaces:**
- Consumes: `release-beta.yml`'s `workflow_dispatch` (Task 4) — the replacement escape hatch.

- [ ] **Step 1: Confirm nothing else references them**

```bash
grep -rn "gh-build\|gh-beta\|gh-stable" --include='*.md' --include='Makefile' --include='*.yml' --include='*.sh' --include='*.rs' . \
  | grep -v '^./docs/superpowers/' | grep -v '^./target/'
```

Every hit must be either the Makefile targets themselves or `scripts/gh-build.sh`. If a doc (e.g. `README.md`, a runbook) references them, note the file — Step 3 updates it.

- [ ] **Step 2: Delete the targets and the script**

Remove from `Makefile`: the `gh-beta:`, `gh-stable:` and `gh-build:` targets **and** the explanatory comment block above `gh-beta` that describes the rolling-tag scheme (it documents only the deleted path). Leave `release:`, `cp-web:`, `cp:`, `bump:` and everything else untouched.

```bash
git rm scripts/gh-build.sh
```

- [ ] **Step 3: Point the docs at the new path**

For every doc hit found in Step 1, replace the `make gh-beta` / `make gh-stable` instruction with:

```
gh workflow run release-beta.yml            # cut a beta now
gh workflow run release-beta.yml -f force=true   # even if main has not moved
```

and note that stable is promoted from a soaked beta weekly, not published directly.

- [ ] **Step 4: Verify the Makefile still parses and the remaining targets are intact**

```bash
make -n release >/dev/null && echo "release ok"
make -n cp-web  >/dev/null && echo "cp-web ok"
make bump 2>&1 | head -1     # expect the usage line, not a parse error
make -n gh-beta 2>&1 | head -1   # expect "No rule to make target"
```

Expected: `release ok`, `cp-web ok`, `usage: make bump VERSION=<X.Y.Z>`, and a *no rule* error for `gh-beta`.

- [ ] **Step 5: Commit**

```bash
git add -A Makefile scripts docs
git commit -m "refactor(release): retire the macOS-only local publish path"
```

---

## What is NOT covered by automated tests

Both scripts are unit-tested (14 cases). Three conditions live in workflow YAML
and are exercised only by the manual steps in Tasks 4 and 5:

- the **stall-guard** (base version equal to the shipped stable),
- the **CI-green** check on `main`,
- the **unreachable-commit** refusal in the promoter.

Spec §5 lists the first and third as test cases. They are not unit-testable
where they currently live; extracting them into a third script would be the way
to close that, and is deliberately deferred rather than faked with a test that
does not actually exercise the workflow.

## Verification

After all six tasks:

- [ ] `scripts/tests/release-cadence-tests.sh` → `14 passed, 0 failed`.
- [ ] Both new workflow files parse as YAML.
- [ ] `gh workflow list` shows `release-beta` and `release-stable` once the branch is on `main`.
- [ ] **Dry-run the beta cut before trusting the cron:** merge to `main`, then `gh workflow run release-beta.yml` and confirm it either pushes a sensible `-beta.N` tag or logs a specific skip reason. This is the one end-to-end check that cannot be done from a branch, because the workflow checks out `main`.
- [ ] Confirm `release.yml` accepted the pushed tag and resolved it to `channel=beta`, `prerelease=true`.
- [ ] Leave `release-stable.yml` to fire on its own the first Sunday, or dispatch it manually once a beta has soaked 2 days.
