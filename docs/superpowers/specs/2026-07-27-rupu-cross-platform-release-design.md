# rupu cross-platform release — design

**Date:** 2026-07-27
**Status:** approved, pending implementation plan
**Scope:** publish Linux binaries alongside macOS on every beta/stable release, move the
release itself into CI, and add the repo's first per-PR test gate.

## Problem

rupu releases are macOS-only by construction. `make gh-beta` runs
`cargo build --release -p rupu-cli` on a developer laptop, signs via `scripts/sign-dev.sh`,
and `scripts/gh-build.sh` uploads exactly one asset whose name comes from `uname`:
`rupu-darwin-arm64` plus a `.sha256` sidecar. There is no CI that builds or tests anything
— `.github/workflows/` contains only a nightly live-API smoke job.

Three consequences:

1. There is no Linux binary, so anyone who is not on an Apple Silicon Mac must build from
   source.
2. The release depends on one machine being awake, with the right toolchain and an
   un-dirty working tree.
3. Nothing catches a Linux-breaking change, because nothing ever compiles for Linux.

There is also a latent naming defect. `scripts/gh-build.sh` derives the asset name from
`uname -m`, while `rupu_update::decide::current_platform()`
(`crates/rupu-update/src/decide.rs:5`) maps `x86_64` → `x64`. The two agree by coincidence
on Apple Silicon (`arm64` both ways) and would disagree on every x86_64 host: the publisher
would write `rupu-linux-x86_64` while `rupu update` looks for `rupu-linux-x64`. Publishing
a Linux binary without fixing this ships a broken update path.

## Decisions taken

These were settled during brainstorming and constrain everything below.

| Decision | Choice | Consequence |
|---|---|---|
| Windows | WSL2, not native | The entire native-Windows port is out of scope. See "Why not native Windows". |
| Build host | GitHub Actions, containerized | Free on this public repo; native runners per arch, no emulation. |
| macOS build | Moves into CI | Releasing becomes "push a tag"; notarization enters the release path. |
| Credentials | Retire keyring on **all** platforms | The keyring path was too complex for its value. Also drops `libdbus-sys`, which makes a static musl build possible. |
| Arc scope | Release pipeline **and** a per-PR Linux CI gate | Linux correctness is enforced between releases, not discovered by users. |

### Why not native Windows

Native Windows is a materially larger project than Linux, and WSL2 removes the need for it:

- `BashTool` hardcodes `Command::new("/bin/sh")` (`crates/rupu-tools/src/bash.rs:73`). This
  is the core agent tool, not an edge case; Windows needs a shell port abstraction.
- Six `/bin/kill` shellouts implement run liveness and SIGTERM-based pause/cancel
  (`crates/rupu-orchestrator/src/runs.rs:2321`, `crates/rupu-cli/src/cmd/session.rs:7370`,
  `crates/rupu-cli/src/cmd/autoflow.rs:2104`). Windows has no signals; this needs job
  objects.
- Detached run spawning uses `process_group(0)` under `#[cfg(unix)]`
  (`crates/rupu-cli/src/cp_launcher.rs:64`, `cp_agent_launcher.rs:58`).
- `rupu-update` swaps the binary in place, which Windows forbids for a running `.exe`.
- Roughly 80 of 145 test files depend on `/bin/sh`, `chmod`, `PermissionsExt`, or `/tmp`.
- Authenticode signing requires a purchased certificate.

Windows users install the Linux binary under WSL2. This is documented, not silently
implied. If native Windows is ever wanted it gets its own spec.

## Design

### 1. Platform naming: one source of truth

The canonical asset name stays `rupu-<os>-<arch>` with os ∈ `{darwin, linux}` and
arch ∈ `{arm64, x64}`. `rupu_update::decide::current_platform()` becomes its only owner.

- Add a hidden `rupu update --print-platform` flag that prints `current_platform()` and
  exits.
- The release workflow names each asset by **running the binary it just built** and reading
  that output. Every runner is native to its target, so the binary always executes. Drift
  between what the publisher writes and what the updater looks for becomes structurally
  impossible rather than merely tested.
- Add a unit test asserting the expected mapping for each supported target triple, so the
  contract is also checked without a release.
- Replace the `uname`-derived `ASSET_NAME` computation in `scripts/gh-build.sh` with a call
  to `--print-platform`. The script itself is retained as a break-glass path (§4); only its
  independent guess at the platform name goes away.

Every release publishes six assets:

| Asset | Platform | Notes |
|---|---|---|
| `rupu-darwin-arm64` (+ `.sha256`) | macOS, Apple Silicon | signed + notarized |
| `rupu-linux-x64` (+ `.sha256`) | Linux, x86_64 | static musl |
| `rupu-linux-arm64` (+ `.sha256`) | Linux, aarch64 | static musl |

Intel macOS (`darwin-x64`) is **not** published. Nothing in the design prevents adding it
later — it is one more matrix row — but there is no known user for it today.

"Which binary is for which OS" is answered three ways: the asset name itself, a table in
the generated release notes, and an `install.sh` that detects the host and fetches the
right one.

### 2. Linux build shape: static musl

Targets `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`, built on native
runners (`ubuntu-latest` and `ubuntu-24.04-arm`) inside a pinned container image. No
emulation on either arch.

Static linking is chosen over a dynamically-linked glibc build because it removes an entire
class of support problem. A binary built against glibc 2.39 (the `ubuntu-latest` default)
silently refuses to run on Debian 12, Ubuntu 22.04, RHEL 9, or Amazon Linux 2. A static
musl binary runs on all of those, plus Alpine, distroless images, and any WSL2 distribution
the user happens to have installed.

This is only possible because of the credential decision in §3: with `sync-secret-service`
gone, `libdbus-sys` leaves the Linux dependency graph. The remaining C dependency is
`git2`'s vendored libgit2 + OpenSSL, which builds under musl but requires `perl` and
`cmake` in the build image.

Confirmed by `cargo tree` against both target triples: the Windows graph does **not** pull
`openssl-sys` (libgit2-sys gates it to `cfg(unix)`), and the Linux graph pulls
`dbus-secret-service` + `libdbus-sys` solely via `keyring`.

`rupu-app` is macOS-only (GPUI + objc2) and stays that way. The workspace declares no
`default-members`, so every Linux job must scope itself explicitly — either
`-p rupu-cli` or `--workspace --exclude rupu-app` — or it will attempt to compile GPUI.

### 3. Credentials: retire the keyring backend entirely

The keyring backend is being removed on **every** platform, not gated per-platform. It was
too complex to be worth its cost, and the file backend is already the default everywhere.
This is a decision about rupu's credential storage generally; making Linux buildable is a
beneficiary, not the motivation.

**The file backend is already the default.** `KeychainResolver::with_service`
(`crates/rupu-auth/src/resolver.rs:97-135`) resolves `RUPU_AUTH_BACKEND` → probe cache →
**file**, and the comment there explains why: keychain requirements for a bare CLI binary
are cdhash-bound, so every rebuild invalidates them and reads silently fail. Only the
module-level doc comment at `resolver.rs:36-51` still describes keyring-as-default; it is
stale. So no default needs flipping — what is removed is the alternative.

**What goes away:**

| Target | Detail |
|---|---|
| `crates/rupu-keychain-acl/` | Entire crate (585 lines). Its only consumer is `rupu-auth`. |
| `crates/rupu-auth/src/keyring.rs` | `KeyringBackend` (104 lines) |
| `crates/rupu-auth/src/keychain_layout.rs` | **Kept, not deleted** — renamed `account_key`. See the correction below. |
| `crates/rupu-auth/src/probe.rs` | Keychain-availability probe (148 lines) |
| `crates/rupu-auth/src/resolver.rs` | `Storage` enum collapses to a single file path |
| `crates/rupu-auth/src/backend.rs:60` | `AuthError::Keyring` variant |
| `crates/rupu-workspace/src/host_store.rs:331-360` | Three keyring helpers; host tokens move to the file store |
| `crates/rupu-auth/tests/keyring_ignored.rs` | Deleted with `KeyringBackend` |
| root `Cargo.toml:104` | The `keyring` workspace dependency |

**Correction (found during implementation, PR #554): `keychain_layout` must NOT be
deleted.** It was listed above as "keyring addressing only". It is not:
`key_for(..).account` produces `"anthropic/api-key"`, which is the **on-disk key format of
`auth.json`** — the file backend was built to reuse the keychain's account strings.
Deleting the module would have silently changed the key format and orphaned every existing
user's credentials. It is instead renamed `account_key`, returns plain `String`s rather
than a `KeychainKey` struct, and carries a test pinning the exact key strings as a
compatibility contract. The keychain framing goes; the format contract stays.

**A notable side effect, stated accurately.** `rupu-keychain-acl` wraps Security.framework
FFI and is exempt from `unsafe_code = "forbid"`. Deleting it does **not** leave the
workspace exemption-free, as an earlier draft of this spec claimed: `rupu-app` also sets
`unsafe_code = "deny"` with three `#[allow(unsafe_code)]` sites for objc2/AppKit. The
correct, narrower claim is that `rupu-app` is not in `rupu-cli`'s dependency graph (the only
`rupu-app*` entry there is `rupu-app-canvas`, which is pure Rust), so **every crate linked
into the shipped `rupu` binary is `forbid(unsafe_code)`**.

**What explicitly stays.** `rupu-providers` reads *Claude Code's* keychain entry to import
credentials from it (`crates/rupu-providers/src/auth/discovery.rs:69`,
`anthropic.rs::load_claude_code_keychain`). That is a one-way import from another tool, it
does not use the `keyring` crate, and it is unaffected. Removing rupu's own keychain
*storage* is not the same as removing the ability to *discover* credentials other tools
left in a keychain.

**Migration: clean break with a detection notice.** There is no existing keychain→file
migration path (`legacy_key_for` addresses a legacy keychain *account naming* scheme, not a
file migration), so credentials still in a keychain would otherwise strand silently. On
macOS, a `security find-generic-password` probe — a shellout, requiring no `keyring` crate,
the same technique `rupu-providers` already uses — detects a stranded entry and prints a
one-time notice telling the user to run `rupu auth login`. No automatic import: keeping the
full keychain addressing scheme alive purely for migration would preserve exactly the
complexity this removal exists to shed, and there is no clean shellout equivalent on
Windows, so an import would silently break Windows users regardless.

The probe must cover all three historical account shapes — `<name>/api-key`, `<name>/sso`,
and the Slice A bare `<name>` — for **both built-in and user-defined (openai-compatible)
providers**. `store_named` wrote named providers under the same accounts, so a
built-ins-only probe misses them silently, which is the exact failure the notice exists to
prevent. (Found during implementation by reading a real `rupu auth status` that listed a
user-defined `oracle` provider.)

**The file backend itself is unchanged:** `~/.rupu/auth.json`, permissions reset to 0600 on
every write, a `tracing::warn!` if found wider than 0600
(`crates/rupu-auth/src/json_file.rs`), honoring `RUPU_HOME` and `RUPU_AUTH_FILE`.

**Stated plainly:** after this change rupu stores credentials in a chmod-600 file on macOS,
Windows, and Linux alike. No OS keystore is used anywhere. This matches what `gh`, `aws`,
`gcloud`, `kubectl`, and `terraform` do — none of them use the OS keychain by default —
and it is the reasoning already written into `resolver.rs:110-135`.

### 4. Release workflow

**Trigger:** push of a `v*` tag, plus `workflow_dispatch` for manual reruns.

**Channel derivation** preserves the existing two-tag scheme exactly, so `rupu update`
needs no changes:

- `v0.68.16-beta` → `--prerelease`, rolling tag `latest-beta`, versioned tag `v0.68.16-beta`
- `v0.68.16` → full release, rolling tag `latest-stable`, versioned tag `v0.68.16`

**Job graph:**

1. `web` — runs `make cp-web` once and uploads `web/dist` as an artifact. All three binaries
   then embed a byte-identical UI. Today that identity is guaranteed only by the releaser
   remembering to run `make cp-web` before `make release`.
2. `build-linux-x64`, `build-linux-arm64`, `build-macos` — each downloads the `web/dist`
   artifact, builds with `RUPU_RELEASE_CHANNEL`/`RUPU_RELEASE_VERSION` set (so
   `crates/rupu-cli/src/build_info.rs` embeds them), derives its asset name via
   `--print-platform`, computes the sha256 sidecar, and uploads both as job artifacts.
3. `publish` — creates or updates both release tags and uploads all six assets in one step.

The two-phase upload (build jobs produce artifacts, one publish job writes the release) is
deliberate: a partial release where some platforms uploaded and others failed is worse than
no release.

**The macOS job** imports the Developer ID Application certificate from a base64 `.p12`
secret into a temporary keychain, builds, signs via the existing `scripts/sign-dev.sh`, and
notarizes via `scripts/notarize-release.sh` using an App Store Connect API key. This puts
notarization into the release path for the first time — it exists as a script today but is
not wired into `gh-beta`/`gh-stable`, so published binaries are currently signed but not
notarized.

Signing is not optional for macOS: `rupu-update`'s `verify.rs` runs
`codesign --verify --strict` and refuses to swap in an unsigned binary.

Both `scripts/sign-dev.sh` and `scripts/notarize-release.sh` already no-op cleanly on
non-macOS, so the Linux jobs can invoke shared Makefile targets without special-casing.

**Local flow after this change:** `make gh-beta` / `make gh-stable` become thin wrappers
that bump the version and push the tag. `scripts/gh-build.sh` is kept intact and documented
as a break-glass path for publishing a darwin-only asset from a laptop when CI is
unavailable.

**Secrets required:** `APPLE_CERT_P12_BASE64`, `APPLE_CERT_PASSWORD`,
`APPLE_TEAM_ID`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`, `APPLE_API_KEY_BASE64`. On a
public repository these are not exposed to workflows triggered by fork pull requests.

### 5. Per-PR CI gate

A new `ci.yml` on pull requests and pushes to main: build, clippy, and test on Linux,
scoped `--workspace --exclude rupu-app`.

Two things about this that matter:

- CI uses rustup and therefore honors `rust-toolchain.toml`'s pinned **1.95**. The repo is
  currently developed against Homebrew's **1.97.1** with no rustup installed, so the pin is
  not in effect locally. CI may surface clippy lints that have never been seen on a
  developer machine. This is a feature, but it means the first green run may take a few
  iterations.
- **`cargo fmt --check` cannot be in the gate initially.** `main` has 532 formatting diff
  sites under rustfmt 1.9.0. It goes in as a non-blocking informational step, or a
  dedicated formatting-cleanup PR lands first. Either way it does not block the rest of
  this arc.

A macOS leg of the gate is possible (macOS runners are also free on public repos) but is
not included: it duplicates what the developer already runs locally, and doubles gate
latency for the platform that is least likely to regress.

### 6. Documentation

- `README.md` install section gains a platform table and a WSL2 note for Windows, replacing
  the current cargo-install-only instructions.
- `docs/RELEASING.md` is rewritten for the tag-triggered flow, retaining the break-glass
  local path.
- A new `install.sh` at the repo root detects OS/arch, resolves the latest release for a
  channel, downloads the matching asset, verifies its sha256, and installs it.

## Risks and unknowns

**Dominant unknown: this workspace has never been compiled for Linux.** Not once. Every
sizing below is conditional on a spike that gets
`cargo build -p rupu-cli --target x86_64-unknown-linux-musl` green in a container and runs
the test suite. Known candidates for what that finds:

- macOS-specific tests (keychain via `security`, `codesign`) needing `cfg` gates before the
  Linux job can be green.
- Vendored OpenSSL under musl — buildable, but needs `perl` in the image and is a common
  source of friction.
- musl's allocator is slower than glibc's under heavy multithreaded allocation. If it shows
  up in practice, the mitigation is swapping in mimalloc. Not pre-emptively addressed.
- `/bin/kill` and `/bin/sh` shellouts work on Linux but are working-by-accident rather than
  by design. They are left alone in this arc; replacing them with portable equivalents is
  cleanup, not a Linux requirement.

**Secondary risks:**

- Apple certificate rotation in CI is an ongoing maintenance cost.
- A `.p12` in GitHub secrets is a real secret to manage, though this is the standard
  approach.
- The `git tag -f` + `git push --force` in the current `gh-build.sh` re-triggers on tag
  push. The new workflow must not recursively trigger itself; the publish job uses the
  GitHub API rather than pushing tags.

## Work breakdown

Three plans, each producing working software on its own, executed in order.

**Plan 1 — Retire the keyring credential backend** (spec §3).
`docs/superpowers/plans/2026-07-28-rupu-plan-1-retire-keyring-backend.md`
Standalone value independent of cross-platform work: deletes `rupu-keychain-acl` and the
keyring code paths, moves host tokens to the file store, adds the stranded-keychain
detection notice, and leaves every crate in the shipped binary `forbid(unsafe_code)`. Also a
prerequisite — it is what removes `libdbus-sys` from the Linux graph.

**Plan 2 — Linux buildability** (spec §1, §2, §5).
`docs/superpowers/plans/2026-07-28-rupu-cross-platform-plan-2-linux-buildability.md`
The platform-naming contract, the pinned musl build container, a statically-linked
`rupu` proven to run on `alpine:3.20` and `debian:12`, a green Linux test suite, and the
repo's first per-PR CI gate.

**Plan 3 — CI release pipeline** (spec §4, §6).
Tag-triggered release workflow with channel derivation, the shared `web/dist` artifact job,
Linux and macOS build jobs, macOS certificate import + signing + notarization, the atomic
publish job, `install.sh`, and the README / `docs/RELEASING.md` rewrites. Written once
Plan 2 lands.

Only Plan 3's macOS job has meaningful external setup friction (Apple credentials as
repository secrets). Plan 2's container task is the one that can change the shape of what
follows it, since nothing has ever compiled this workspace for Linux.

## Out of scope

- Native Windows binaries (WSL2 is the answer; see "Why not native Windows").
- Intel macOS (`darwin-x64`) binaries.
- `rupu-app` on any non-macOS platform.
- Replacing the `/bin/sh` and `/bin/kill` shellouts with portable equivalents.
- Any OS-keystore credential integration, on any platform. The keyring backend is being
  retired outright (§3), not reimplemented per-OS.
- Automatic migration of credentials out of an existing OS keychain (§3 ships a detection
  notice instead).
- Package-manager distribution (Homebrew tap, AUR, apt repo, Nix).
- Formatting cleanup of the 532 rustfmt diff sites on `main`.
