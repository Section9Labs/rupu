# rupu self-update (`rupu update`) — design

**Date:** 2026-07-13
**Status:** approved (brainstorm), pending implementation plan

## 1. Summary

Add a first-class self-update capability to the `rupu` CLI:

- An explicit **`rupu update`** command that downloads and installs the latest
  binary for the user's configured release channel, verifying integrity before
  swapping the running binary in place.
- A cheap, cached, non-blocking **passive notice** on normal commands that tells
  the user when a newer version is available.
- **Two release channels — `beta` and `stable`** — selected via
  `.rupu/config.toml`, replacing today's single `-build` prerelease convention.

## 2. Goals / non-goals

**Goals**
- One-command upgrade: `rupu update` fetches, verifies, and installs the newest
  build for the active channel.
- Clear, user-followable channels (`beta` / `stable`) instead of `-build`.
- Config-driven channel selection; per-run override via `--channel`.
- Integrity: SHA-256 verification + code-signature sanity + atomic swap.
- Works with the existing root-owned `/usr/local/bin/rupu` install by elevating
  only the final file swap.
- Fully testable without real network or touching real install locations.

**Non-goals (deferred / YAGNI)**
- Multi-platform builds beyond the currently-published `darwin-arm64` (the
  design is platform-parametric, but only darwin-arm64 assets exist today).
- Notarization (tracked separately in the backlog; the updater strips the
  quarantine xattr and verifies the existing Developer-ID signature).
- Auto-installing in the background without the user running `rupu update`
  (rejected during brainstorm — passive *notice* only).
- A package-manager distribution (brew tap, etc.).

## 3. Release channels & publish convention

### 3.1 Channels

| Channel  | GitHub release kind | Versioned tag   | Rolling tag     |
|----------|---------------------|-----------------|-----------------|
| `beta`   | prerelease          | `v<X.Y.Z>-beta` | `latest-beta`   |
| `stable` | full release        | `v<X.Y.Z>`      | `latest-stable` |

Both channels upload the same two assets per platform:
- `rupu-<os>-<arch>` (e.g. `rupu-darwin-arm64`) — the binary.
- `rupu-<os>-<arch>.sha256` — the SHA-256 sidecar.

**Promotion flow:** cut `v<X.Y.Z>-beta` for testing; when satisfied, publish
`v<X.Y.Z>` as a full (stable) release. A beta and its promoted stable share the
same `X.Y.Z`; semver precedence (`0.35.4-beta` < `0.35.4`) makes the beta
channel resolve to the stable once promoted.

### 3.2 Tooling changes

`scripts/gh-build.sh` is generalized to take a channel argument:

```
scripts/gh-build.sh beta      # → v<X.Y.Z>-beta (prerelease) + latest-beta
scripts/gh-build.sh stable    # → v<X.Y.Z>       (full)       + latest-stable
```

The channel controls: the versioned tag suffix, the `--prerelease` flag
(present for beta, absent for stable), and the rolling tag name. Asset naming
is standardized to `rupu-<os>-<arch>` (+ `.sha256`) for both.

Makefile targets:
- `make gh-beta`  → `make release` (build+sign) then `scripts/gh-build.sh beta`.
- `make gh-stable`→ `make release` then `scripts/gh-build.sh stable`.
- `make gh-build` retained as a deprecated alias for `make gh-beta` (keeps
  existing muscle memory working during transition).

**Migration:** existing `latest-build` / `v*-build` releases are left in place
as legacy. No back-fill; the new scheme applies going forward. The first stable
release must be published explicitly with `make gh-stable`.

### 3.3 Build-time channel/version embedding

The release build bakes identity into the binary so the running process knows
exactly what it is:

- `scripts/gh-build.sh` (and the make targets) export two env vars consumed by
  a `build.rs` in `rupu-cli` (or a small `rupu-build-info` crate):
  - `RUPU_RELEASE_CHANNEL` = `beta` | `stable`
  - `RUPU_RELEASE_VERSION` = the full tag version, e.g. `0.35.4-beta` / `0.35.4`
- `build.rs` emits `cargo:rustc-env` so the binary exposes:
  - `const RELEASE_CHANNEL: Option<&str>` (None for local/dev builds)
  - `const RELEASE_VERSION: &str` (falls back to `CARGO_PKG_VERSION` when the
    env is absent, i.e. a dev build)
- `rupu --version` prints e.g. `rupu 0.35.4 (beta)`; a dev build prints
  `rupu 0.35.4 (dev)`.
- A **dev build** (no `RUPU_RELEASE_CHANNEL`) is detected and `rupu update`
  refuses to run against it (see §7).

## 4. Config schema

New section on `rupu_config::Config`:

```toml
[update]
channel = "stable"   # "stable" | "beta"   (default: "stable")
check   = true       # passive update notice on normal commands (default: true)
```

```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateConfig {
    /// Release channel to track: "stable" (default) or "beta".
    pub channel: Option<String>,
    /// Whether normal commands print a passive "update available" notice.
    pub check: Option<bool>,
}
```

- Added as `pub update: UpdateConfig` on `Config`, `#[serde(default)]`.
- Resolution follows existing config merge: global `~/.rupu/config.toml` is the
  natural home; project `.rupu/config.toml` may override.
- Precedence for the effective channel: `--channel` flag > `[update].channel` >
  default `stable`.
- Unknown channel string → clear error listing valid values.
- `RUPU_NO_UPDATE_CHECK=1` (env) hard-disables the passive notice regardless of
  `[update].check`.

## 5. `rupu update` command surface

`rupu-cli` stays thin: arg parsing + delegation to the new `rupu-update` crate.

```
rupu update                 # check active channel; if newer, confirm → install
rupu update --check         # report current vs latest; install nothing
rupu update --channel beta  # override channel for this run
rupu update --yes           # non-interactive; skip the confirm prompt
rupu update --force         # reinstall even if already current
rupu update --rollback      # restore the previously-installed binary
```

Exit codes: `0` success / already-current; `10` update available (for
`--check`, to script on it); non-zero on error (network, verify, swap, refused).

## 6. Update flow

### 6.1 Resolve latest for the channel

1. Read effective channel (§4).
2. `GET https://api.github.com/repos/Section9Labs/rupu/releases` (public — no
   auth; send a UA header; honor `GITHUB_TOKEN` if present to avoid rate limits).
3. Parse releases; keep those whose tag matches the channel's version pattern
   and carry the platform asset:
   - **stable**: `prerelease == false`, tag `v<semver>` (no pre-release suffix).
   - **beta**: any release (prerelease or full), tag `v<semver>` or
     `v<semver>-beta`.
4. Pick the highest **semver** (pre-release-aware ordering).
5. Compare to the running binary's `RELEASE_VERSION` (§3.3). If latest ≤ current
   → "already up to date" (unless `--force`).

The GitHub client sits behind a trait (`ReleaseSource`) so tests inject a fixed
release list.

### 6.2 Download → verify → swap

1. Resolve the real target path: `std::env::current_exe()` → canonicalize
   (resolve symlinks).
2. **Dev-build guard:** if `RELEASE_CHANNEL` is None *or* the target path is
   under a repo `target/{debug,release}/` dir → refuse with guidance (§7).
3. Download the platform asset to a uniquely-named temp file **in the target's
   own directory** (same filesystem → atomic rename). If that dir is not
   writable, download to `~/.rupu/cache/update/` instead and take the elevated
   path (§6.3).
4. Download the `.sha256` sidecar; compute SHA-256 of the temp file; abort on
   mismatch.
5. Sanity checks on the temp file: it is a Mach-O executable;
   `codesign --verify --strict` passes; strip `com.apple.quarantine`; run
   `<temp> --version` and confirm it reports the expected version.
6. `chmod 0755`; fsync.
7. **Backup** the current binary to `~/.rupu/backups/rupu-<currentversion>`
   (for `--rollback`).
8. Atomic `rename(temp, target)` over the running binary (safe on macOS — the
   live process keeps its open inode; the new binary takes effect next launch).
9. Print success: `Updated rupu <old> → <new> (<channel>).`

Downloads use a streaming writer with a size sanity cap and a timeout.

### 6.3 Elevation (writing a root-owned target)

When the target directory is not writable by the current user:

1. Download + verify happen **as the normal user** (no root network access) into
   `~/.rupu/cache/update/`.
2. A minimal privileged apply step is re-exec'd:
   `sudo <self> __apply-update --from <verified-temp> --to <target> --sha256 <hex>`
   - `__apply-update` is an internal, hidden subcommand.
   - It **re-verifies** the SHA-256 of `--from` against `--sha256` (defense in
     depth — the root step trusts nothing from argv), then does only:
     backup → `chmod` → atomic `rename` to `--to`. No network, no GitHub, no
     config reads.
3. One password prompt (inherits the TTY). If `sudo` is unavailable or the user
   declines, print the exact manual command and exit non-zero.

This keeps the root surface to a single verified file swap.

## 7. Safety rails & edge cases

- **Dev build:** refuse when `RELEASE_CHANNEL` is None or the exe is under a
  repo `target/` dir. Message: "this looks like a development build; use
  `make install` / `cargo build` instead."
- **Wrong platform:** if no asset matches `<os>-<arch>`, error clearly ("no
  <channel> build published for <os>-<arch>").
- **Already current:** no-op with a friendly message (unless `--force`).
- **Downgrade protection:** only install when latest > current, unless `--force`
  (which still verifies signature/hash).
- **Network / API failure:** hard error for `rupu update`; silent for the
  passive check (§8).
- **Verify failure:** never swap; leave the temp file for inspection and report
  the expected vs actual hash.
- **Interrupted swap:** the swap itself is a single atomic `rename`; a crash
  before it leaves the old binary intact. The temp/backup files are the only
  cleanup surface.
- **Concurrent `rupu update`:** temp files are uniquely named; the final rename
  is atomic. A lightweight lockfile in `~/.rupu/cache/update/` avoids two
  concurrent downloads clobbering each other (best-effort).

## 8. Passive update notice

- State file `~/.rupu/update-check.json`:
  `{ "channel": "...", "last_checked": <unix>, "latest_version": "x.y.z[-beta]" }`.
- On CLI startup, after arg parse, if all of: `check` enabled, `RUPU_NO_UPDATE_CHECK`
  unset, stdout is an interactive TTY, and output is not a structured/JSON mode —
  then:
  - If the state is **stale** (`now - last_checked > 24h`), spawn a **detached,
    non-blocking** background refresh (its own short-lived task/process) that
    hits `ReleaseSource`, updates the state file, and swallows all errors. It
    never delays or fails the user's actual command.
  - Using the **cached** `latest_version`, if it is greater than the running
    version for the active channel, print exactly one line to **stderr**:
    `A new rupu is available: 0.35.3 → 0.35.4 (stable). Run 'rupu update'.`
- Never prints inside workflows, non-TTY, JSON/`--format` output, or when
  `[update].check = false` / `RUPU_NO_UPDATE_CHECK=1`.

## 9. Architecture / crate layout

Follows the hexagonal + thin-CLI rules:

- **`rupu-update`** (new lib crate) owns all logic:
  - `ReleaseSource` trait + `GithubReleaseSource` impl (HTTP behind it).
  - Channel resolution + semver comparison.
  - `Downloader` trait + real HTTP impl (asset + sha256).
  - Verifier (sha256, Mach-O check, codesign, quarantine strip).
  - Installer (backup, atomic swap, elevation orchestration, rollback).
  - Passive-check state (`update-check.json` read/write + staleness).
  - Pure functions where possible (parse releases, pick latest, compare) for
    dense unit testing.
- **`rupu-config`**: add `UpdateConfig` + `Config.update`.
- **`rupu-cli`**:
  - `update` subcommand (+ hidden `__apply-update`) — parse args, read config,
    call `rupu-update`.
  - Wire the passive notice into the top-level dispatch.
  - `build.rs` + version-string change for channel/version embedding (or a tiny
    `rupu-build-info` crate if cleaner to share).

## 10. Testing

- **`rupu-update` unit tests** (no network, no real install path):
  - Release-JSON parsing from fixtures (mixed prerelease/full, missing assets).
  - Channel filtering + highest-semver selection (stable vs beta, promotion
    case `0.35.4-beta` vs `0.35.4`).
  - "Am I behind" comparison incl. dev-version fallback.
  - SHA-256 verify: match / mismatch.
  - Dev-build detection (exe under `target/`, missing channel env).
  - Passive-check staleness + state-file round-trip.
  - Installer swap tested against a **temp dir**: write temp → verify → backup →
    rename → assert; rollback restores.
  - `ReleaseSource` / `Downloader` mocked with in-memory fixtures + a
    local-file asset (no real HTTP).
- **`rupu-config`**: `[update]` parse + defaults + unknown-channel rejection.
- **`rupu-cli`**: arg parsing for `update` flags; `--check` exit codes.
- No test writes to `/usr/local/bin` or invokes `sudo`.

## 11. Open items / deferred

- Multi-platform assets (linux, darwin-x64) — parametric now, published later.
- Notarization — separate backlog item; updater already verifies signature +
  strips quarantine.
- A `stable`-first default means existing users (all currently on `-build`)
  should set `channel = "beta"` if they want to keep tracking prereleases; the
  first `make gh-stable` establishes the stable line.
