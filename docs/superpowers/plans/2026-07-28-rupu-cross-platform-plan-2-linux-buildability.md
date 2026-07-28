# Cross-Platform Plan 2: Linux Buildability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the rupu CLI build as a fully static Linux musl binary and keep it that way with a per-PR CI gate.

**Architecture:** Two threads land in one arc. First, the release-asset platform name gets a single owner in `rupu-update` and is read back out of the built binary, killing the publisher/updater drift. Second, a pinned Alpine container reproduces the Linux build identically on a laptop and in CI, producing a static musl binary, and a new `ci.yml` runs build + clippy + test on every PR. The credential work that unblocks static linking is Plan 1 and must land first.

**Tech Stack:** Rust 1.95 (pinned), `cargo`, musl via `rust:alpine`, Docker/nerdctl containers, GitHub Actions, `clap` 4, `assert_cmd`.

**Spec:** `docs/superpowers/specs/2026-07-27-rupu-cross-platform-release-design.md`

**Depends on:** Plan 1 (`2026-07-28-rupu-plan-1-retire-keyring-backend.md`) must land first. It removes `keyring`, and with it `libdbus-sys`, from the dependency graph — without that, the static musl build in Task 3 cannot link.

**Scope note:** This plan covers spec §1 (naming), §2 (Linux build shape), and §5 (CI gate). Spec §3 (credentials) is Plan 1; spec §4 (release workflow) and §6 (docs, `install.sh`) are Plan 3.

## Global Constraints

- Rust edition 2021; toolchain pinned to **1.95** in `rust-toolchain.toml`. CI honors that pin via rustup; local development currently runs Homebrew 1.97.1 with no rustup, so CI may surface clippy lints never seen locally.
- **Workspace dependencies only.** Versions are pinned in the root `Cargo.toml`; never write a version in a crate `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`. `unsafe_code = "forbid"` with no exemption anywhere (Plan 1 deleted the last one).
- **No mock or silent-noop code paths.** If something cannot work on a platform, it must fail loudly with an explicit message.
- Canonical release-asset platform name: `rupu-<os>-<arch>`, os ∈ `{darwin, linux}`, arch ∈ `{arm64, x64}`.
- `rupu-app` is macOS-only (GPUI + objc2). Every Linux build and test command must be scoped `--workspace --exclude rupu-app` or `-p <crate>`. The workspace declares no `default-members`, so an unscoped `cargo build` will try to compile GPUI.
- **Never run `cargo fmt` package-wide.** `main` has 532 rustfmt diff sites. Format only the specific files you touched, e.g. `rustfmt crates/rupu-update/src/decide.rs`.
- Every change goes through a feature branch and a PR. Never commit directly to `main`.
- Errors: `thiserror` in libraries, `anyhow` in `rupu-cli`.

---

### Task 1: Extract the platform-name mapping into a pure, testable function

Today `current_platform()` reads `std::env::consts` inline, so the mapping table can only ever be tested for the host you happen to be on. Extracting the pure function lets one test cover every published target from any machine.

**Files:**
- Modify: `crates/rupu-update/src/decide.rs:4-17`
- Modify: `crates/rupu-update/src/lib.rs:11` (export the new function)
- Test: `crates/rupu-update/src/decide.rs` (inline `mod tests`, existing block starts at line 68)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `rupu_update::decide::platform_name(os: &str, arch: &str) -> String`, re-exported as `rupu_update::platform_name`. `current_platform() -> String` keeps its existing signature and behavior.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` block in `crates/rupu-update/src/decide.rs` (after the `use super::*;` line at 69):

```rust
    #[test]
    fn platform_name_maps_every_published_target() {
        assert_eq!(platform_name("macos", "aarch64"), "darwin-arm64");
        assert_eq!(platform_name("linux", "x86_64"), "linux-x64");
        assert_eq!(platform_name("linux", "aarch64"), "linux-arm64");
    }

    #[test]
    fn platform_name_maps_targets_we_can_build_but_do_not_publish() {
        assert_eq!(platform_name("macos", "x86_64"), "darwin-x64");
    }

    #[test]
    fn platform_name_passes_unknown_pairs_through_unchanged() {
        // Not published, but the mapping must not silently mangle a
        // target we never anticipated — a wrong-but-plausible name is
        // worse than an obviously-unsupported one.
        assert_eq!(platform_name("freebsd", "riscv64"), "freebsd-riscv64");
    }

    #[test]
    fn current_platform_delegates_to_the_pure_mapping() {
        assert_eq!(
            current_platform(),
            platform_name(std::env::consts::OS, std::env::consts::ARCH)
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-update decide`
Expected: FAIL — `cannot find function 'platform_name' in this scope`

- [ ] **Step 3: Write the implementation**

Replace `crates/rupu-update/src/decide.rs:4-17` (the `current_platform` function and its doc comment) with:

```rust
/// Map a Rust `(os, arch)` pair to rupu's canonical release-asset platform
/// name.
///
/// Kept pure — rather than reading `std::env::consts` inline — so the whole
/// mapping table is testable from any host. This function is the single
/// owner of the asset-naming convention: `scripts/gh-build.sh` and the
/// release workflow both read it back out of the built binary via
/// `rupu update --print-platform` rather than deriving names independently.
/// A second, independent derivation is exactly the defect this replaces
/// (`uname -m` yields `x86_64` where this yields `x64`).
pub fn platform_name(os: &str, arch: &str) -> String {
    let os = match os {
        "macos" => "darwin",
        other => other,
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// `<os>-<arch>` for the host this binary is running on.
pub fn current_platform() -> String {
    platform_name(std::env::consts::OS, std::env::consts::ARCH)
}
```

- [ ] **Step 4: Export it**

In `crates/rupu-update/src/lib.rs:11`, change:

```rust
pub use decide::{current_platform, decide, is_dev_exe, Decision};
```

to:

```rust
pub use decide::{current_platform, decide, is_dev_exe, platform_name, Decision};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-update`
Expected: PASS — all four new tests plus the existing `decide` and `select` tests.

- [ ] **Step 6: Format only the files you touched and commit**

```bash
rustfmt crates/rupu-update/src/decide.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/decide.rs crates/rupu-update/src/lib.rs
git commit -m "refactor(update): extract pure platform_name() mapping

Makes the full asset-naming table testable from any host, and gives the
release pipeline a single owner for the name to read back."
```

---

### Task 2: `rupu update --print-platform`, and rewire `gh-build.sh` to use it

This is the fix for the naming defect. After this task, the publisher asks the binary what it is instead of guessing from `uname`.

**Files:**
- Modify: `crates/rupu-cli/src/cmd/update.rs:23-42` (add the flag), and the top of `async fn run` at line 77
- Modify: `scripts/gh-build.sh` (the `OS`/`ARCH`/`ASSET_NAME` block)
- Test: `crates/rupu-cli/tests/cli_update_platform.rs` (create)

**Interfaces:**
- Consumes: `rupu_update::current_platform()` from Task 1.
- Produces: `rupu update --print-platform` printing `<os>-<arch>` and a trailing newline to stdout, exit 0. The release workflow in Plan 3 depends on this exact contract.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/cli_update_platform.rs`:

```rust
//! `rupu update --print-platform` is the release pipeline's only source
//! for an asset's platform name. If this contract breaks, published
//! assets stop matching what `rupu update` looks for — the exact defect
//! this flag exists to prevent — so it is tested end-to-end through the
//! real binary rather than as a unit test.

use assert_cmd::Command;

#[test]
fn print_platform_matches_the_update_crate_mapping() {
    let expected = format!("{}\n", rupu_update::current_platform());

    Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["update", "--print-platform"])
        .assert()
        .success()
        .stdout(expected);
}

#[test]
fn print_platform_needs_no_network_or_config() {
    // The flag must short-circuit before config loading and before any
    // GitHub API call, so the release pipeline can invoke it inside a
    // sandboxed build container with no credentials and no network.
    Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["update", "--print-platform"])
        .env("RUPU_HOME", "/nonexistent-on-purpose")
        .assert()
        .success();
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cli --test cli_update_platform`
Expected: FAIL — clap rejects `--print-platform` as an unknown argument, so the process exits non-zero.

- [ ] **Step 3: Add the flag**

In `crates/rupu-cli/src/cmd/update.rs`, add this field to `struct UpdateArgs` after the `rollback` field (line 41):

```rust
    /// Print this build's release-asset platform name (e.g. `linux-x64`)
    /// and exit. The release pipeline uses this to name assets from the
    /// binary itself, so the publisher and `rupu update` cannot drift.
    #[arg(long)]
    pub print_platform: bool,
```

- [ ] **Step 4: Short-circuit in `run`**

In `crates/rupu-cli/src/cmd/update.rs`, make this the first statement of `async fn run` (before `let cfg = load_cli_config();` at line 78):

```rust
    // Before config load and before any network call: this must work in a
    // bare build container with no credentials.
    if args.print_platform {
        println!("{}", rupu_update::current_platform());
        return Ok(ExitCode::SUCCESS);
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p rupu-cli --test cli_update_platform`
Expected: PASS — both tests.

- [ ] **Step 6: Rewire `gh-build.sh`**

In `scripts/gh-build.sh`, replace these three lines:

```bash
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
ASSET_NAME="rupu-${OS}-${ARCH}"
```

with:

```bash
# Ask the binary what it is. Deriving this independently (e.g. from
# `uname -m`) is how publisher and updater drift: `uname -m` says
# `x86_64` where `rupu_update::platform_name` says `x64`, so an x86_64
# release would publish `rupu-linux-x86_64` while `rupu update` looked
# for `rupu-linux-x64`.
PLATFORM="$("$BIN" update --print-platform)"
if [[ -z "$PLATFORM" ]]; then
  echo "\`$BIN update --print-platform\` produced no output — is this an older binary?" >&2
  exit 1
fi
ASSET_NAME="rupu-${PLATFORM}"
```

- [ ] **Step 7: Verify the script still names the asset correctly**

Run:
```bash
cargo build --release -p rupu-cli
./target/release/rupu update --print-platform
```
Expected: `darwin-arm64` on an Apple Silicon Mac — identical to what the old `uname` derivation produced there, confirming this is a no-op on the one platform published today.

- [ ] **Step 8: Format and commit**

```bash
rustfmt crates/rupu-cli/src/cmd/update.rs crates/rupu-cli/tests/cli_update_platform.rs
git add crates/rupu-cli/src/cmd/update.rs crates/rupu-cli/tests/cli_update_platform.rs scripts/gh-build.sh
git commit -m "feat(update): add --print-platform; name release assets from the binary

Fixes a latent defect: gh-build.sh derived asset names from \`uname -m\`
(\`x86_64\`) while rupu-update expects \`x64\`. They agree only on Apple
Silicon, so publishing any x86_64 build would have shipped a broken
update path."
```

---

### Task 3: Pinned Linux build container

Establishes the reproducible Linux toolchain before any Linux-specific code changes, so subsequent tasks can actually verify themselves. The deliverable is deliberately a small crate: proving the image works is separable from making the whole CLI compile.

**Files:**
- Create: `docker/linux-build.Dockerfile`
- Modify: `Makefile` (add `linux-build` target and `CONTAINER` variable; add to `.PHONY` on line 1)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: a container image tag `rupu-linux-build`, and `make linux-build` which builds `rupu-cli` inside it into `target/linux-musl/`. Task 6's CI workflow invokes the same Dockerfile.

- [ ] **Step 1: Write the Dockerfile**

Create `docker/linux-build.Dockerfile`:

```dockerfile
# Linux build image for the rupu CLI.
#
# Produces a fully static musl binary: the `rust:*-alpine` images use
# `*-unknown-linux-musl` as the host target, so a plain `cargo build`
# inside this image is already static — no `--target` flag and no
# `-C target-feature=+crt-static` needed.
#
# Static linking is the point. A glibc build made on ubuntu-latest
# (glibc 2.39) silently refuses to run on Debian 12, Ubuntu 22.04,
# RHEL 9, or Amazon Linux 2. This binary runs on all of them, plus
# Alpine, distroless, and any WSL2 distribution.
#
# Built natively on each architecture — linux/amd64 on ubuntu-latest,
# linux/arm64 on ubuntu-24.04-arm — so there is no emulation anywhere.
#
# The Rust version MUST track rust-toolchain.toml (currently 1.95).
FROM rust:1.95-alpine3.20

# musl-dev, gcc, g++: the musl C toolchain that vendored libgit2 and
#   aws-lc-rs compile against.
# perl, make: required by git2's `vendored-openssl` feature.
# cmake, clang, clang-dev: required by aws-lc-rs, which rupu-cli installs
#   as the process-level rustls provider (crates/rupu-cli/src/main.rs:15).
# pkgconf: consulted by several -sys build scripts.
# git: build scripts and several tests shell out to it.
# bash: some test fixtures write `#!/bin/sh` scripts but the test harness
#   itself is easier to debug with a real shell present.
# nodejs, npm: `make cp-web` builds the embedded CP UI (used by Plan 3's
#   release job; harmless here).
RUN apk add --no-cache \
      musl-dev gcc g++ make perl cmake clang clang-dev \
      pkgconf git bash nodejs npm

ENV CARGO_TERM_COLOR=always
WORKDIR /work
```

- [ ] **Step 2: Confirm the base image tag exists, and pin it**

Run: `${CONTAINER:-docker} pull rust:1.95-alpine3.20`

If that tag does not exist, list the available Alpine variants for the pinned Rust version and choose the newest Alpine minor, then update the `FROM` line. Do not fall back to a different Rust version — the toolchain pin is a global constraint.

Then record the digest so the image is reproducible:
```bash
${CONTAINER:-docker} inspect --format='{{index .RepoDigests 0}}' rust:1.95-alpine3.20
```
Append the resulting `sha256:...` to the `FROM` line as `FROM rust:1.95-alpine3.20@sha256:...`.

- [ ] **Step 3: Add the Makefile target**

Add near the top of `Makefile`, after the `.PHONY` line:

```make
# Container runtime. This machine aliases `docker` to `lima nerdctl` in
# the interactive shell, but make runs a non-interactive shell where
# aliases do not apply — so pass it explicitly:
#   make linux-build CONTAINER="lima nerdctl"
CONTAINER ?= docker
```

Add the target (place it next to the other build targets, after `release`):

```make
# Build the Linux CLI binary in the pinned musl container. Uses a
# separate target dir so it never collides with the host's macOS
# artifacts. Same Dockerfile CI uses, so a green CI run is reproducible
# locally with one command.
linux-build:
	$(CONTAINER) build -f docker/linux-build.Dockerfile -t rupu-linux-build .
	$(CONTAINER) run --rm -v "$(PWD)":/work -w /work rupu-linux-build \
		cargo build --release -p rupu-cli --target-dir target/linux-musl
```

Add `linux-build` to the `.PHONY` list on line 1.

- [ ] **Step 4: Verify the image builds and Rust works inside it**

Run:
```bash
make linux-build CONTAINER="lima nerdctl" 2>&1 | tail -40
```

Expected: the image builds and `cargo` compiles. With Plan 1 landed there is no `libdbus-sys` in the graph, so this should get a long way. If it fails on a missing D-Bus header, Plan 1 did not fully land — go verify `cargo tree --workspace --target x86_64-unknown-linux-musl | grep dbus` is empty rather than adding `dbus-dev` to the image.

- [ ] **Step 5: Prove the toolchain itself is sound on the riskiest dependency**

Run:
```bash
lima nerdctl run --rm -v "$PWD":/work -w /work rupu-linux-build \
  cargo build --release -p rupu-update --target-dir target/linux-musl
```

Expected: PASS. This is the meaningful gate for this task — `rupu-update` pulls `reqwest`/`rustls`/`aws-lc-rs`, so a green build here proves the riskiest C dependency in the tree compiles under musl.

If `aws-lc-rs` fails to build, that is the single most likely blocker in this whole plan. Record the exact error in the PR description before attempting a fix; the documented fallback is to switch the workspace's rustls provider from `aws-lc-rs` to `ring` (`crates/rupu-cli/src/main.rs:15`), which is a design change that needs matt's sign-off, not a unilateral fix.

- [ ] **Step 6: Verify the output is genuinely static**

Run:
```bash
lima nerdctl run --rm -v "$PWD":/work -w /work rupu-linux-build \
  sh -c 'file target/linux-musl/release/librupu_update.rlib >/dev/null && echo rlib-ok'
```
Expected: `rlib-ok`. (A staticness check on an executable comes in Task 4, once `rupu-cli` links.)

- [ ] **Step 7: Commit**

```bash
git add docker/linux-build.Dockerfile Makefile
git commit -m "build: pinned Alpine musl container for Linux builds

Same image CI and \`make linux-build\` use, so a CI failure reproduces
locally with one command. Static musl by construction: a glibc build
would not run on Debian 12, RHEL 9, or Amazon Linux 2."
```

---

### Task 4: Get the full `rupu-cli` binary linking under musl

Plan 1 removed the known blocker. This task closes out whatever remains — most likely vendored libgit2 and OpenSSL.

**Files:**
- Modify: `docker/linux-build.Dockerfile` (only if a genuinely missing build tool is found)
- Modify: whichever crates fail to compile

**Interfaces:**
- Consumes: Task 3, and Plan 1.
- Produces: `target/linux-musl/release/rupu`, a static executable.

- [ ] **Step 1: Run the full build**

```bash
make linux-build CONTAINER="lima nerdctl" 2>&1 | tail -60
```

- [ ] **Step 2: Fix failures by category**

Work through failures using these rules, in order:

1. **A missing build tool** (`perl not found`, `cmake not found`) → add the package to `docker/linux-build.Dockerfile` with a comment naming which dependency needs it.
2. **A macOS-only API referenced from unconditional code** → add a `#[cfg]` gate and a Linux implementation that returns an explicit error. Do not stub it to return success.
3. **A `-sys` crate that cannot build under musl** → stop and record the exact error in the PR. Do not swap the dependency or disable the feature that pulls it without matt's sign-off; those are design changes, not build fixes.

- [ ] **Step 3: Verify the binary is genuinely static and runs**

```bash
lima nerdctl run --rm -v "$PWD":/work -w /work rupu-linux-build sh -c '
  file target/linux-musl/release/rupu
  ldd target/linux-musl/release/rupu 2>&1 | head -3
  ./target/linux-musl/release/rupu --version
  ./target/linux-musl/release/rupu update --print-platform
'
```

Expected:
- `file` reports `ELF 64-bit LSB executable, ... statically linked`
- `ldd` reports `not a dynamic executable` (or equivalent)
- `--version` prints the workspace version
- `--print-platform` prints `linux-x64` on an amd64 host, `linux-arm64` on an arm64 host

The last line is the end-to-end proof that Tasks 1, 2, and 3 compose: the Linux binary names itself exactly as the release pipeline will expect.

- [ ] **Step 4: Verify it runs outside the build image**

```bash
lima nerdctl run --rm -v "$PWD":/work -w /work alpine:3.20 \
  /work/target/linux-musl/release/rupu --version
lima nerdctl run --rm -v "$PWD":/work -w /work debian:12 \
  /work/target/linux-musl/release/rupu --version
```
Expected: both print the version. Debian 12 ships glibc 2.36 and has no musl at all — a successful run there is the proof that static linking achieved its purpose.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "build: rupu-cli links as a static musl binary

Verified running unmodified on alpine:3.20 and debian:12."
```

---

### Task 5: Get the Linux test suite green

The file list here genuinely cannot be known in advance — nothing has ever run these tests on Linux. What *can* be specified is the decision rule for each failure, which is the part where judgment goes wrong.

**Files:**
- Modify: test files, as discovered
- Modify: product code, where a test reveals a real Linux bug

**Interfaces:**
- Consumes: Task 4's working build.
- Produces: `cargo test --workspace --exclude rupu-app` passing in the container. Task 6's CI gate enforces it from then on.

- [ ] **Step 1: Get the baseline**

```bash
lima nerdctl run --rm -v "$PWD":/work -w /work rupu-linux-build \
  cargo test --workspace --exclude rupu-app --target-dir target/linux-musl 2>&1 | tail -80
```

Record the full list of failing tests in the PR description before changing anything. That list is the evidence for every judgment call below.

- [ ] **Step 2: Classify each failure and apply the matching rule**

| Symptom | Rule |
|---|---|
| Test shells out to `security`, `codesign`, `xattr`, or `open` | macOS-only behavior. Gate the test `#[cfg(target_os = "macos")]`. |
| Test asserts on a keychain backend | Should not exist — Plan 1 deleted that backend. If one survives, delete it rather than gating it. |
| Test hardcodes `/tmp` or a macOS-specific path | Portability bug in the test. Fix it to use `tempfile`/`assert_fs`, do not gate it. |
| Test asserts file permissions via `PermissionsExt` | Valid on Linux. If it fails, the product behavior differs — investigate before touching the test. |
| Product code misbehaves on Linux | **Fix the product code.** Do not gate the test. |

The rule that matters: **gating a test that reveals a real Linux bug converts a visible failure into an invisible one.** If you cannot articulate why a behavior is legitimately macOS-only, it is not — it is a bug, and it belongs in the PR description either fixed or explicitly listed as known-broken.

- [ ] **Step 3: Re-run until green**

```bash
lima nerdctl run --rm -v "$PWD":/work -w /work rupu-linux-build \
  cargo test --workspace --exclude rupu-app --target-dir target/linux-musl
```
Expected: PASS.

- [ ] **Step 4: Confirm macOS did not regress**

```bash
cargo test --workspace --exclude rupu-app
```
Expected: PASS — the same set that passed before this plan started. A `cfg` gate added in Step 2 must not have removed macOS coverage.

- [ ] **Step 5: Format only touched files and commit**

```bash
git commit -am "test: green on Linux

Gates genuinely macOS-only tests and fixes portability bugs in the rest.
Failures that reflect real Linux product bugs are fixed in the product,
not gated; see the PR description for the full baseline list."
```

---

### Task 6: Per-PR Linux CI gate

The repo has no build or test CI today. This adds the first, on the platform most likely to break silently.

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `docker/linux-build.Dockerfile` from Task 3; the green test suite from Task 5.
- Produces: a required-status-check candidate named `ci / linux`.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/ci.yml`:

```yaml
name: ci

on:
  pull_request:
  push:
    branches: [main]

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  linux:
    runs-on: ubuntu-latest
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@v4

      # Same Dockerfile `make linux-build` uses, so a red CI run
      # reproduces locally with one command.
      - name: Build the pinned musl image
        run: docker build -f docker/linux-build.Dockerfile -t rupu-linux-build .

      # Guards the property that makes static linking possible. Without
      # this, a future crate re-adding `keyring` would silently
      # reintroduce libdbus-sys and break the release build at
      # the worst possible moment.
      - name: Assert keyring and dbus stay out of the Linux graph
        run: |
          leaked=$(docker run --rm -v "$PWD":/work -w /work rupu-linux-build \
            cargo tree --workspace -e normal --prefix none \
            | grep -E '^(keyring|libdbus-sys|dbus-secret-service) ' | sort -u || true)
          if [ -n "$leaked" ]; then
            echo "::error::keyring/dbus leaked into the Linux dependency graph:"
            echo "$leaked"
            exit 1
          fi

      - name: Build
        run: |
          docker run --rm -v "$PWD":/work -w /work rupu-linux-build \
            cargo build --workspace --exclude rupu-app --locked

      - name: Clippy
        run: |
          docker run --rm -v "$PWD":/work -w /work rupu-linux-build \
            cargo clippy --workspace --exclude rupu-app --all-targets --locked -- -D warnings

      - name: Test
        run: |
          docker run --rm -v "$PWD":/work -w /work rupu-linux-build \
            cargo test --workspace --exclude rupu-app --locked

      # Informational only: `main` currently has 532 rustfmt diff sites,
      # so this cannot block. Promote to a blocking step once a dedicated
      # formatting-cleanup PR has landed.
      - name: Format (informational)
        continue-on-error: true
        run: |
          docker run --rm -v "$PWD":/work -w /work rupu-linux-build \
            cargo fmt --all --check
```

- [ ] **Step 2: Verify the workflow is valid YAML and its jobs parse**

Run: `gh workflow view ci --repo Section9Labs/rupu 2>/dev/null || python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('yaml ok')"`
Expected: `yaml ok` (the `gh` form only works once the workflow is on the default branch).

- [ ] **Step 3: Commit and push, then confirm the gate actually runs**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: per-PR Linux build, clippy, and test gate

First build/test CI in the repo. Runs in the same pinned musl container
as \`make linux-build\`, and guards the keyring/dbus exclusion that makes
static linking possible."
git push origin HEAD
```

Then watch the run:
```bash
gh run watch --exit-status
```
Expected: the `linux` job passes. If clippy fails on lints never seen locally, that is the 1.95-vs-1.97.1 toolchain gap described in Global Constraints — fix the lints, do not relax the gate.

- [ ] **Step 4: Confirm caching and runtime are acceptable**

Run: `gh run list --workflow=ci --limit 3`

If the job exceeds ~30 minutes, add `Swatinem/rust-cache@v2` and mount a persistent cargo registry volume into the `docker run` invocations. Do not reduce the scope of what is built or tested to make it faster.

---

## Self-Review

**Spec coverage.**

| Spec section | Covered by |
|---|---|
| §1 Platform naming, one source of truth | Tasks 1, 2 |
| §1 `--print-platform` | Task 2 |
| §1 mapping unit test | Task 1 |
| §1 remove `uname` derivation from `gh-build.sh` | Task 2, Step 6 |
| §2 static musl, native per-arch, pinned container | Tasks 3, 7 |
| §2 `rupu-app` excluded | Global Constraints; Task 6 |
| §3 six-file `cfg` gating table | Tasks 4, 5 |
| §5 per-PR Linux gate | Task 6 |
| §5 `fmt` non-blocking | Task 6, Step 1 |
| §5 rustup honors the 1.95 pin | Global Constraints; Task 6, Step 3 |
| §6 Risks: aws-lc-rs under musl | Task 3, Step 5 |
| §6 Risks: macOS-only tests need gates | Task 5 |
| §6 Risks: vendored OpenSSL needs perl | Task 3, Step 1; Task 4, Step 2 |

Spec §3 (credentials) is Plan 1; spec §4 (release workflow) and §6 (docs, `install.sh`) are Plan 3. Neither is a gap in this plan.

**Deferred to Plan 3:** release workflow with tag trigger and channel derivation; the `web` artifact job; macOS signing and notarization in CI; `install.sh`; README and `docs/RELEASING.md` updates; publishing the build image to ghcr for faster CI.

**Placeholder scan.** No "TBD"/"TODO"/"implement later". Task 5 is discovery-shaped by necessity — nothing has run these tests on Linux — so it specifies a decision rule per failure category rather than a file list, and requires the baseline failure list be recorded before any change. Task 4 Step 2 does the same for build failures.

**Type consistency.** `platform_name(os: &str, arch: &str) -> String` is defined in Task 1 and consumed by name in Task 1 Step 4 (export), Task 2 (via `current_platform()`), and Task 4 Step 3 (via the CLI flag). `current_platform() -> String` keeps its pre-existing signature throughout. The container image tag `rupu-linux-build` is identical in Task 3's Makefile target and Task 6's workflow.
