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
| Linux credentials | File backend | Drops `libdbus-sys`, which makes a fully static musl build possible. |
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

### 3. Credentials on Linux: file backend

**Correction to an earlier assumption:** the file backend is already the default on every
platform. `KeychainResolver::with_service` (`crates/rupu-auth/src/resolver.rs:97-135`)
resolves `RUPU_AUTH_BACKEND` → probe cache → **file**, and the comment there explains why
(cdhash-bound keychain requirements break bare CLI binaries on every rebuild). Only the
module-level doc comment at `resolver.rs:44-51` still describes keyring-as-default; it is
stale and should be corrected. So no default-flipping work is required.

What is actually required is removing the dependency, which is more invasive than flipping
a default. `keyring` moves from a flat `[workspace.dependencies]` entry to per-target
dependency tables in its two consumers, `rupu-auth` and `rupu-workspace`, so that on Linux
the crate is absent entirely. Because `keyring::Error` is woven into public API surface,
that requires `cfg`-gating across six files:

| File | Keyring surface to gate |
|---|---|
| `crates/rupu-auth/src/backend.rs:60` | `AuthError::Keyring(#[from] keyring::Error)` variant |
| `crates/rupu-auth/src/keyring.rs` | whole module (`KeyringBackend`) |
| `crates/rupu-auth/src/lib.rs:41` | `pub use keyring::KeyringBackend` |
| `crates/rupu-auth/src/probe.rs:11` | keychain-availability probe |
| `crates/rupu-auth/src/resolver.rs` | `Storage::Keyring` variant + its match arms |
| `crates/rupu-workspace/src/host_store.rs:39-40` | `HostStoreError::Keyring` variant |

**No mock store.** `keyring` v3 compiles on Linux with no platform feature enabled by
falling back to an in-memory mock credential store. That must not ship: it would accept
writes and lose them silently, which is precisely the silent-noop failure mode this project
rejects. Removing the dependency outright is what makes the failure honest.

Consequently `rupu auth backend --use keychain` must return an explicit
"not supported on this platform" error on Linux (`crates/rupu-cli/src/cmd/auth.rs:443`
already has the unknown-backend error path to extend), never a silent fallback.

The file backend itself is unchanged: `~/.rupu/auth.json`, permissions reset to 0600 on
every write, a `tracing::warn!` if found wider than 0600
(`crates/rupu-auth/src/json_file.rs`).

**Stated plainly:** with the dependency removed there is no keyring option on Linux. Desktop
Linux users get plaintext-with-0600, not gnome-keyring or KWallet. This is accepted
deliberately — it is already the correct behavior for the headless server and container
case, which is the dominant Linux deployment for rupu. Per-OS credential integration is
deferred until the platforms themselves are working, at which point Linux can get
secret-service support the same way macOS has keychain support today.

macOS and Windows credential behavior are unchanged.

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

Seven PRs. Only #5 has meaningful external setup friction; #0 is the one that can change
the shape of everything after it.

0. **Linux spike.** Get the CLI building and its tests passing for
   `x86_64-unknown-linux-musl` in a container. Output is a list of required `cfg` gates and
   image dependencies — sizing input for the rest.
1. **Platform naming contract.** `--print-platform`, the mapping unit test, removal of the
   `uname` derivation.
2. **Credential target-gating.** `keyring` moves to per-target tables; Linux defaults to the
   file backend; tests for the Linux default.
3. **Build image + Linux CI gate.** The container definition and `ci.yml`.
4. **Release workflow, Linux jobs.** Tag trigger, channel derivation, `web` artifact job,
   two Linux build jobs, publish job.
5. **Release workflow, macOS job.** Certificate import, signing, notarization.
6. **Docs + `install.sh`.**

## Out of scope

- Native Windows binaries (WSL2 is the answer; see "Why not native Windows").
- Intel macOS (`darwin-x64`) binaries.
- `rupu-app` on any non-macOS platform.
- Replacing the `/bin/sh` and `/bin/kill` shellouts with portable equivalents.
- Secret-service or KWallet integration on Linux — deferred until the platforms work.
- Package-manager distribution (Homebrew tap, AUR, apt repo, Nix).
- Formatting cleanup of the 532 rustfmt diff sites on `main`.
