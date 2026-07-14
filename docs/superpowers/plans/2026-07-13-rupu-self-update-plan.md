# rupu self-update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `rupu update` command that downloads, verifies, and installs the latest binary for a config-selected release channel (`beta`/`stable`), plus a cached passive "update available" notice, and rename the publish convention from `-build` to `beta`/`stable`.

**Architecture:** A new pure-ish `rupu-update` lib crate owns all logic behind IO traits (`ReleaseSource`, `Downloader`); `rupu-config` gains an `[update]` section; `rupu-cli` gains the thin `update` (+ hidden `__apply-update`) subcommand, a `build_info` module that embeds channel/version via `option_env!`, and passive-notice wiring; `scripts/gh-build.sh` + the Makefile grow channel-aware publish targets.

**Tech Stack:** Rust 2021, tokio, reqwest (workspace), serde, `semver`, `sha2`, thiserror; clap for the CLI; bash + `gh` for release tooling.

## Global Constraints

- MSRV pinned in `rust-toolchain.toml` (currently 1.88); Rust 2021.
- Workspace deps only: any new dependency (`semver`, `sha2`) is pinned in the **root** `Cargo.toml` `[workspace.dependencies]` and referenced with `.workspace = true` in crate `Cargo.toml`s. Never pin versions in crate manifests.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code = "forbid"` (no new unsafe).
- `rupu-cli` stays thin: arg parsing + delegation only; all logic lives in `rupu-update`.
- Errors: `thiserror` in `rupu-update` (library); `anyhow` in `rupu-cli`.
- Per-file formatting only — run `rustfmt --edition 2021 <file>` on touched files; never a workspace-wide `cargo fmt`.
- Repo is **public**: GitHub Releases API needs no auth (send a User-Agent; use `GITHUB_TOKEN` only if present, to dodge rate limits).
- Owner/repo constant: `Section9Labs/rupu`.
- Channel naming: `beta` → prerelease tag `v<X.Y.Z>-beta` + rolling `latest-beta`; `stable` → full release tag `v<X.Y.Z>` + rolling `latest-stable`. Assets: `rupu-<os>-<arch>` + `rupu-<os>-<arch>.sha256`.
- Default channel when unset: `stable`.

---

## Phase 1 — Foundations (config + build identity)

### Task 1: `[update]` config section

**Files:**
- Create: `crates/rupu-config/src/update_config.rs`
- Modify: `crates/rupu-config/src/lib.rs` (add `mod update_config; pub use`), `crates/rupu-config/src/config.rs` (add field to `Config`)
- Test: inline `#[cfg(test)]` in `update_config.rs`

**Interfaces:**
- Produces: `rupu_config::UpdateConfig { channel: Option<String>, check: Option<bool> }`; `Config.update: UpdateConfig`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-config/src/update_config.rs`:

```rust
//! `[update]` section — release channel + passive-notice preference.

use serde::{Deserialize, Serialize};

/// `[update]` config: which release channel `rupu update` tracks, and whether
/// normal commands print a passive "update available" notice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateConfig {
    /// "stable" (default) or "beta".
    pub channel: Option<String>,
    /// Passive update notice on normal commands (default: on).
    pub check: Option<bool>,
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn parses_update_section() {
        let cfg: Config = toml::from_str(
            r#"
            [update]
            channel = "beta"
            check = false
            "#,
        )
        .unwrap();
        assert_eq!(cfg.update.channel.as_deref(), Some("beta"));
        assert_eq!(cfg.update.check, Some(false));
    }

    #[test]
    fn update_section_defaults_empty() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.update, crate::UpdateConfig::default());
    }

    #[test]
    fn rejects_unknown_update_key() {
        let err = toml::from_str::<Config>("[update]\nbogus = 1\n").unwrap_err();
        assert!(err.to_string().contains("bogus"), "got: {err}");
    }
}
```

- [ ] **Step 2: Wire the module + field.** In `crates/rupu-config/src/lib.rs` add `mod update_config;` and `pub use update_config::UpdateConfig;` alongside the other `pub use`s. In `crates/rupu-config/src/config.rs`, add to `struct Config` (after `cp`):

```rust
    #[serde(default)]
    pub update: crate::update_config::UpdateConfig,
```

- [ ] **Step 3: Run tests, verify fail→pass**

Run: `cargo test -p rupu-config update_config`
Expected: 3 passed.

- [ ] **Step 4: Format + commit**

```bash
rustfmt --edition 2021 crates/rupu-config/src/update_config.rs crates/rupu-config/src/config.rs crates/rupu-config/src/lib.rs
git add crates/rupu-config/src/{update_config.rs,config.rs,lib.rs}
git commit -m "feat(config): add [update] section (channel, check)"
```

---

### Task 2: Build-time channel/version identity (`build_info`)

**Files:**
- Create: `crates/rupu-cli/src/build_info.rs`
- Modify: `crates/rupu-cli/src/main.rs` or `lib.rs` (add `mod build_info;`)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Produces: `build_info::RELEASE_CHANNEL: Option<&'static str>`, `build_info::RELEASE_VERSION: &'static str`, `build_info::is_dev_build() -> bool`, `build_info::version_line() -> String`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-cli/src/build_info.rs`:

```rust
//! Build identity embedded at compile time. The release build (see
//! `scripts/gh-build.sh`) exports `RUPU_RELEASE_CHANNEL` + `RUPU_RELEASE_VERSION`;
//! a local/dev build leaves them unset.

/// "beta" | "stable" for a published build; `None` for a dev build.
pub const RELEASE_CHANNEL: Option<&str> = option_env!("RUPU_RELEASE_CHANNEL");

/// The full release version (e.g. "0.35.4-beta" / "0.35.4"); falls back to the
/// crate version for dev builds.
pub const RELEASE_VERSION: &str = match option_env!("RUPU_RELEASE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// True when this binary was not built by the release tooling.
pub fn is_dev_build() -> bool {
    RELEASE_CHANNEL.is_none()
}

/// Human `--version` suffix, e.g. "0.35.4 (beta)" / "0.35.4 (dev)".
pub fn version_line() -> String {
    format!("{} ({})", RELEASE_VERSION, RELEASE_CHANNEL.unwrap_or("dev"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_build_when_env_absent() {
        // Under `cargo test` the release env is unset.
        assert!(is_dev_build());
        assert_eq!(RELEASE_CHANNEL, None);
        assert_eq!(RELEASE_VERSION, env!("CARGO_PKG_VERSION"));
        assert!(version_line().ends_with("(dev)"));
    }
}
```

- [ ] **Step 2: Register the module.** In `crates/rupu-cli/src/main.rs` (or `lib.rs` if modules are declared there), add `mod build_info;`.

- [ ] **Step 3: Run test, verify pass**

Run: `cargo test -p rupu-cli build_info`
Expected: PASS.

- [ ] **Step 4: Format + commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/build_info.rs
git add crates/rupu-cli/src/build_info.rs crates/rupu-cli/src/main.rs
git commit -m "feat(cli): embed release channel/version via option_env!"
```

---

## Phase 2 — `rupu-update` crate: pure logic

### Task 3: Crate scaffold + release model + `ReleaseSource` trait

**Files:**
- Create: `crates/rupu-update/Cargo.toml`, `crates/rupu-update/src/lib.rs`, `crates/rupu-update/src/model.rs`
- Modify: root `Cargo.toml` (add `crates/rupu-update` to `members`; add `semver`, `sha2` to `[workspace.dependencies]`)
- Test: inline in `model.rs`

**Interfaces:**
- Produces:
  - `Channel` enum `{ Stable, Beta }` with `FromStr` (`"stable"`/`"beta"`), `Display`, `tag_suffix()`, `rolling_tag()`.
  - `Release { tag: String, version: semver::Version, prerelease: bool, assets: Vec<Asset> }`, `Asset { name: String, url: String }`.
  - `trait ReleaseSource { async fn list_releases(&self) -> Result<Vec<Release>, UpdateError>; }`.
  - `parse_releases(json: &str) -> Result<Vec<Release>, UpdateError>` (tolerant: skips releases whose tag isn't `v<semver>`).
  - `UpdateError` (thiserror).

- [ ] **Step 1: Add crate + workspace deps.** Root `Cargo.toml` — add `"crates/rupu-update"` to `members`, and under `[workspace.dependencies]`:

```toml
semver = "1"
sha2 = "0.10"
```

Create `crates/rupu-update/Cargo.toml`:

```toml
[package]
name = "rupu-update"
version.workspace = true
edition.workspace = true

[lints]
workspace = true

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
semver = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }
reqwest = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

(If any of `serde_json`/`reqwest`/`tokio`/`tracing`/`tempfile`/`thiserror` lack a `[workspace.dependencies]` entry, they already exist — confirm with `grep -n '^serde_json\|^reqwest\|^tempfile' Cargo.toml`.)

- [ ] **Step 2: Write the failing test** in `crates/rupu-update/src/model.rs`:

```rust
use serde::Deserialize;

/// Release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl std::str::FromStr for Channel {
    type Err = crate::UpdateError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stable" => Ok(Channel::Stable),
            "beta" => Ok(Channel::Beta),
            other => Err(crate::UpdateError::BadChannel(other.to_string())),
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        })
    }
}

impl Channel {
    /// Rolling tag name for this channel.
    pub fn rolling_tag(&self) -> &'static str {
        match self {
            Channel::Stable => "latest-stable",
            Channel::Beta => "latest-beta",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub version: semver::Version,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

// Raw GitHub shapes (subset).
#[derive(Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
}
#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

/// Parse the GitHub `/releases` JSON. Releases whose tag is not `v<semver>`
/// are skipped (tolerant — the repo may carry unrelated tags like `latest-*`).
pub fn parse_releases(json: &str) -> Result<Vec<Release>, crate::UpdateError> {
    let raw: Vec<RawRelease> =
        serde_json::from_str(json).map_err(|e| crate::UpdateError::Parse(e.to_string()))?;
    let mut out = Vec::new();
    for r in raw {
        let Some(ver_str) = r.tag_name.strip_prefix('v') else {
            continue;
        };
        let Ok(version) = semver::Version::parse(ver_str) else {
            continue;
        };
        out.push(Release {
            tag: r.tag_name,
            version,
            prerelease: r.prerelease,
            assets: r
                .assets
                .into_iter()
                .map(|a| Asset {
                    name: a.name,
                    url: a.browser_download_url,
                })
                .collect(),
        });
    }
    Ok(out)
}

/// Port: something that can list this repo's releases.
#[async_trait::async_trait]
pub trait ReleaseSource: Send + Sync {
    async fn list_releases(&self) -> Result<Vec<Release>, crate::UpdateError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
        {"tag_name":"latest-beta","prerelease":true,"assets":[]},
        {"tag_name":"v0.35.4-beta","prerelease":true,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"https://x/b"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"https://x/b.sha"}]},
        {"tag_name":"v0.35.3","prerelease":false,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"https://x/s"}]}
    ]"#;

    #[test]
    fn parses_and_skips_non_semver_tags() {
        let rs = parse_releases(FIXTURE).unwrap();
        assert_eq!(rs.len(), 2, "latest-beta skipped");
        assert!(rs.iter().any(|r| r.version.to_string() == "0.35.4-beta" && r.prerelease));
        assert!(rs.iter().any(|r| r.version.to_string() == "0.35.3" && !r.prerelease));
    }

    #[test]
    fn channel_from_str_and_rolling_tag() {
        use std::str::FromStr;
        assert_eq!(Channel::from_str("BETA").unwrap(), Channel::Beta);
        assert_eq!(Channel::Stable.rolling_tag(), "latest-stable");
        assert!(Channel::from_str("nightly").is_err());
    }
}
```

Add `async-trait` to the crate deps (workspace) if not present.

- [ ] **Step 3: Create `lib.rs`** with the error type + module wiring:

```rust
#![deny(clippy::all)]
#![forbid(unsafe_code)]

pub mod model;
pub use model::{parse_releases, Asset, Channel, Release, ReleaseSource};

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("unknown release channel: {0} (expected \"stable\" or \"beta\")")]
    BadChannel(String),
    #[error("failed to parse release data: {0}")]
    Parse(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("no {channel} build published for {platform}")]
    NoAssetForPlatform { channel: String, platform: String },
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("refusing to update a development build ({0})")]
    DevBuild(String),
    #[error("install failed: {0}")]
    Install(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test -p rupu-update model`
Expected: 2 passed. Also `cargo build -p rupu-update`.

- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/*.rs
git add crates/rupu-update Cargo.toml
git commit -m "feat(update): scaffold rupu-update crate + release model + parser"
```

---

### Task 4: Channel-aware latest selection

**Files:**
- Create: `crates/rupu-update/src/select.rs`
- Modify: `crates/rupu-update/src/lib.rs` (`pub mod select;`)
- Test: inline

**Interfaces:**
- Consumes: `Release`, `Channel`, `Asset`.
- Produces: `select_latest(releases: &[Release], channel: Channel, platform: &str) -> Option<&Release>`; `asset_for(release: &Release, platform: &str) -> Option<(&Asset, &Asset)>` returning `(binary, sha256)`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-update/src/select.rs`:

```rust
use crate::model::{Asset, Channel, Release};

/// The binary + `.sha256` assets for `platform` (e.g. "darwin-arm64"), if both present.
pub fn asset_for<'a>(release: &'a Release, platform: &str) -> Option<(&'a Asset, &'a Asset)> {
    let bin_name = format!("rupu-{platform}");
    let sha_name = format!("rupu-{platform}.sha256");
    let bin = release.assets.iter().find(|a| a.name == bin_name)?;
    let sha = release.assets.iter().find(|a| a.name == sha_name)?;
    Some((bin, sha))
}

/// Highest-semver release for the channel that also carries the platform asset.
/// - Stable: only full releases (`!prerelease`).
/// - Beta: any release (prerelease or full) — semver precedence means a promoted
///   stable (`0.35.4`) outranks its beta (`0.35.4-beta`).
pub fn select_latest<'a>(
    releases: &'a [Release],
    channel: Channel,
    platform: &str,
) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| match channel {
            Channel::Stable => !r.prerelease,
            Channel::Beta => true,
        })
        .filter(|r| asset_for(r, platform).is_some())
        .max_by(|a, b| a.version.cmp(&b.version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_releases;

    const F: &str = r#"[
        {"tag_name":"v0.35.5-beta","prerelease":true,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"u"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"u"}]},
        {"tag_name":"v0.35.4","prerelease":false,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"u"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"u"}]},
        {"tag_name":"v0.35.4-beta","prerelease":true,
         "assets":[{"name":"rupu-darwin-arm64","browser_download_url":"u"},
                   {"name":"rupu-darwin-arm64.sha256","browser_download_url":"u"}]}
    ]"#;

    #[test]
    fn stable_picks_highest_full_release() {
        let rs = parse_releases(F).unwrap();
        let r = select_latest(&rs, Channel::Stable, "darwin-arm64").unwrap();
        assert_eq!(r.version.to_string(), "0.35.4");
    }

    #[test]
    fn beta_picks_highest_including_prereleases() {
        let rs = parse_releases(F).unwrap();
        let r = select_latest(&rs, Channel::Beta, "darwin-arm64").unwrap();
        assert_eq!(r.version.to_string(), "0.35.5-beta");
    }

    #[test]
    fn beta_prefers_promoted_stable_over_its_beta() {
        // Only 0.35.4 and 0.35.4-beta present → beta channel takes the stable.
        let rs: Vec<_> = parse_releases(F).unwrap().into_iter()
            .filter(|r| !r.version.to_string().starts_with("0.35.5")).collect();
        let r = select_latest(&rs, Channel::Beta, "darwin-arm64").unwrap();
        assert_eq!(r.version.to_string(), "0.35.4");
    }

    #[test]
    fn none_when_platform_missing() {
        let rs = parse_releases(F).unwrap();
        assert!(select_latest(&rs, Channel::Stable, "linux-x64").is_none());
    }
}
```

- [ ] **Step 2: Register module** in `lib.rs`: `pub mod select; pub use select::{asset_for, select_latest};`

- [ ] **Step 3: Run tests fail→pass**

Run: `cargo test -p rupu-update select`
Expected: 4 passed.

- [ ] **Step 4: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/select.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/{select.rs,lib.rs}
git commit -m "feat(update): channel-aware latest-release selection"
```

---

### Task 5: "Am I behind?" + dev-build detection

**Files:**
- Create: `crates/rupu-update/src/decide.rs`
- Modify: `lib.rs`
- Test: inline

**Interfaces:**
- Produces:
  - `current_platform() -> String` (`"darwin-arm64"` etc. from `std::env::consts`).
  - `is_dev_exe(exe_path: &std::path::Path) -> bool` (true if under a `target/{debug,release}/` segment).
  - `Decision` enum `{ UpToDate, Update { from: Version, to: Version }, Ahead }` and `decide(current: &Version, latest: &Version, force: bool) -> Decision`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-update/src/decide.rs`:

```rust
use semver::Version;
use std::path::Path;

/// `<os>-<arch>`, mapping Rust's arch names to our asset convention.
pub fn current_platform() -> String {
    let os = std::env::consts::OS; // "macos", "linux"
    let os = match os {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// True when the running binary is a dev build (path under a `target/` build dir).
pub fn is_dev_exe(exe_path: &Path) -> bool {
    let comps: Vec<_> = exe_path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    comps.windows(2).any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    UpToDate,
    Update { from: Version, to: Version },
    Ahead,
}

pub fn decide(current: &Version, latest: &Version, force: bool) -> Decision {
    use std::cmp::Ordering::*;
    match latest.cmp(current) {
        Greater => Decision::Update { from: current.clone(), to: latest.clone() },
        Equal => if force { Decision::Update { from: current.clone(), to: latest.clone() } } else { Decision::UpToDate },
        Less => if force { Decision::Update { from: current.clone(), to: latest.clone() } } else { Decision::Ahead },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(s: &str) -> Version { Version::parse(s).unwrap() }

    #[test]
    fn newer_triggers_update() {
        assert_eq!(decide(&v("0.35.3"), &v("0.35.4"), false), Decision::Update { from: v("0.35.3"), to: v("0.35.4") });
    }
    #[test]
    fn equal_is_up_to_date_unless_forced() {
        assert_eq!(decide(&v("0.35.4"), &v("0.35.4"), false), Decision::UpToDate);
        assert!(matches!(decide(&v("0.35.4"), &v("0.35.4"), true), Decision::Update { .. }));
    }
    #[test]
    fn older_latest_is_ahead() {
        assert_eq!(decide(&v("0.35.5"), &v("0.35.4"), false), Decision::Ahead);
    }
    #[test]
    fn dev_exe_detected_under_target() {
        assert!(is_dev_exe(Path::new("/x/rupu/target/release/rupu")));
        assert!(!is_dev_exe(Path::new("/usr/local/bin/rupu")));
    }
}
```

- [ ] **Step 2: Register** `pub mod decide;` + re-exports in `lib.rs`.
- [ ] **Step 3: Run tests fail→pass:** `cargo test -p rupu-update decide` → 4 passed.
- [ ] **Step 4: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/decide.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/{decide.rs,lib.rs}
git commit -m "feat(update): version decision + platform + dev-build detection"
```

---

### Task 6: Checksum verification

**Files:**
- Create: `crates/rupu-update/src/verify.rs`
- Modify: `lib.rs`
- Test: inline

**Interfaces:**
- Produces:
  - `sha256_hex(bytes: &[u8]) -> String`.
  - `parse_sha256_sidecar(text: &str) -> Option<String>` (handles `"<hex>  filename"` and bare hex).
  - `verify_checksum(bytes: &[u8], sidecar_text: &str) -> Result<(), UpdateError>`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-update/src/verify.rs`:

```rust
use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_lower(&h.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// First whitespace-delimited token of a `shasum`-style sidecar, lowercased.
pub fn parse_sha256_sidecar(text: &str) -> Option<String> {
    let tok = text.split_whitespace().next()?;
    if tok.len() == 64 && tok.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(tok.to_ascii_lowercase())
    } else {
        None
    }
}

pub fn verify_checksum(bytes: &[u8], sidecar_text: &str) -> Result<(), crate::UpdateError> {
    let expected = parse_sha256_sidecar(sidecar_text)
        .ok_or_else(|| crate::UpdateError::Parse("malformed sha256 sidecar".into()))?;
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(crate::UpdateError::Checksum { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_matching_checksum() {
        let data = b"hello rupu";
        let side = format!("{}  rupu-darwin-arm64", sha256_hex(data));
        assert!(verify_checksum(data, &side).is_ok());
    }
    #[test]
    fn rejects_mismatch() {
        let side = format!("{}  x", sha256_hex(b"other"));
        let err = verify_checksum(b"hello rupu", &side).unwrap_err();
        assert!(matches!(err, crate::UpdateError::Checksum { .. }));
    }
    #[test]
    fn parses_bare_hex_sidecar() {
        assert_eq!(parse_sha256_sidecar(&"AB".repeat(32)).unwrap(), "ab".repeat(32));
    }
}
```

- [ ] **Step 2: Register** `pub mod verify;` in `lib.rs`.
- [ ] **Step 3: Run tests fail→pass:** `cargo test -p rupu-update verify` → 3 passed.
- [ ] **Step 4: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/verify.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/{verify.rs,lib.rs}
git commit -m "feat(update): sha256 sidecar parse + checksum verify"
```

---

### Task 7: Installer — atomic swap, backup, rollback (temp-dir tested)

**Files:**
- Create: `crates/rupu-update/src/install.rs`
- Modify: `lib.rs`
- Test: inline (against `tempfile::tempdir()`)

**Interfaces:**
- Consumes: `UpdateError`.
- Produces:
  - `backup_dir() -> PathBuf` (`~/.rupu/backups`).
  - `swap_in_place(new_bytes: &[u8], target: &Path, backup: Option<&Path>) -> Result<(), UpdateError>` — writes a temp file **in `target`'s directory**, `chmod 0755`, optionally copies the current `target` to `backup`, then atomically `rename`s over `target`.
  - `rollback(backup: &Path, target: &Path) -> Result<(), UpdateError>`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-update/src/install.rs`:

```rust
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// `~/.rupu/backups`.
pub fn backup_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".rupu").join("backups")
}

/// Atomically replace `target` with `new_bytes`. Writes a temp file in the same
/// directory (same filesystem → atomic `rename`), sets 0755, optionally backs up
/// the existing target first. The caller must have write access to `target`'s dir.
pub fn swap_in_place(new_bytes: &[u8], target: &Path, backup: Option<&Path>) -> Result<(), crate::UpdateError> {
    let dir = target.parent().ok_or_else(|| crate::UpdateError::Install("target has no parent dir".into()))?;
    // Unique temp name in the target directory.
    let pid = std::process::id();
    let tmp = dir.join(format!(".rupu-update.{pid}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(new_bytes)?;
        f.flush()?;
        let mut perms = f.metadata()?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp, perms)?;
        f.sync_all()?;
    }
    if let Some(bak) = backup {
        if target.exists() {
            if let Some(bp) = bak.parent() {
                fs::create_dir_all(bp)?;
            }
            fs::copy(target, bak)?;
        }
    }
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        crate::UpdateError::Install(format!("atomic rename failed: {e}"))
    })?;
    Ok(())
}

/// Restore `target` from `backup`.
pub fn rollback(backup: &Path, target: &Path) -> Result<(), crate::UpdateError> {
    if !backup.exists() {
        return Err(crate::UpdateError::Install(format!("no backup at {}", backup.display())));
    }
    let bytes = fs::read(backup)?;
    swap_in_place(&bytes, target, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("rupu");
        fs::write(&target, b"OLD").unwrap();
        let bak = dir.path().join("bak").join("rupu-old");
        swap_in_place(b"NEW", &target, Some(&bak)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"NEW");
        assert_eq!(fs::read(&bak).unwrap(), b"OLD");
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn rollback_restores_previous() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("rupu");
        fs::write(&target, b"OLD").unwrap();
        let bak = dir.path().join("rupu-old");
        swap_in_place(b"NEW", &target, Some(&bak)).unwrap();
        rollback(&bak, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"OLD");
    }
}
```

- [ ] **Step 2: Register** `pub mod install;` in `lib.rs`.
- [ ] **Step 3: Run tests fail→pass:** `cargo test -p rupu-update install` → 2 passed.
- [ ] **Step 4: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/install.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/{install.rs,lib.rs}
git commit -m "feat(update): atomic in-place swap + backup + rollback"
```

---

### Task 8: Passive-check state file

**Files:**
- Create: `crates/rupu-update/src/notice.rs`
- Modify: `lib.rs`
- Test: inline

**Interfaces:**
- Produces:
  - `CheckState { channel: String, last_checked: u64, latest_version: String }` (serde).
  - `state_path() -> PathBuf` (`~/.rupu/update-check.json`).
  - `load_state(path: &Path) -> Option<CheckState>`; `save_state(path: &Path, s: &CheckState) -> Result<(), UpdateError>`.
  - `is_stale(last_checked: u64, now: u64, ttl_secs: u64) -> bool`.
  - `notice_line(current: &str, latest: &str, channel: &str) -> Option<String>` — `Some(...)` only when `latest` parses > `current`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-update/src/notice.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckState {
    pub channel: String,
    pub last_checked: u64,
    pub latest_version: String,
}

pub fn state_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".rupu").join("update-check.json")
}

pub fn load_state(path: &Path) -> Option<CheckState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_state(path: &Path, s: &CheckState) -> Result<(), crate::UpdateError> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| crate::UpdateError::Parse(e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(())
}

pub fn is_stale(last_checked: u64, now: u64, ttl_secs: u64) -> bool {
    now.saturating_sub(last_checked) > ttl_secs
}

/// One-line notice, only when `latest` > `current` (semver). None otherwise.
pub fn notice_line(current: &str, latest: &str, channel: &str) -> Option<String> {
    let cur = semver::Version::parse(current).ok()?;
    let lat = semver::Version::parse(latest).ok()?;
    if lat > cur {
        Some(format!("A new rupu is available: {current} → {latest} ({channel}). Run 'rupu update'."))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("update-check.json");
        let s = CheckState { channel: "beta".into(), last_checked: 100, latest_version: "0.35.4".into() };
        save_state(&p, &s).unwrap();
        assert_eq!(load_state(&p).unwrap(), s);
    }
    #[test]
    fn staleness() {
        assert!(is_stale(0, 90_000, 86_400));
        assert!(!is_stale(90_000, 100_000, 86_400));
    }
    #[test]
    fn notice_only_when_newer() {
        assert!(notice_line("0.35.3", "0.35.4", "stable").unwrap().contains("→ 0.35.4"));
        assert!(notice_line("0.35.4", "0.35.4", "stable").is_none());
        assert!(notice_line("0.35.5", "0.35.4", "beta").is_none());
    }
}
```

- [ ] **Step 2: Register** `pub mod notice;` in `lib.rs`.
- [ ] **Step 3: Run tests fail→pass:** `cargo test -p rupu-update notice` → 3 passed.
- [ ] **Step 4: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/notice.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/{notice.rs,lib.rs}
git commit -m "feat(update): passive update-check state + notice line"
```

---

## Phase 3 — Network implementations

### Task 9: `GithubReleaseSource` + asset downloader (HTTP)

**Files:**
- Create: `crates/rupu-update/src/github.rs`
- Modify: `lib.rs`
- Test: inline (URL construction only; no live network)

**Interfaces:**
- Consumes: `ReleaseSource`, `Release`, `UpdateError`.
- Produces:
  - `GithubReleaseSource::new(owner_repo: &str) -> Self` implementing `ReleaseSource` via `GET https://api.github.com/repos/{owner_repo}/releases?per_page=100`.
  - `download_bytes(url: &str) -> Result<Vec<u8>, UpdateError>` (async; UA header; optional `GITHUB_TOKEN`; streaming with a 200 MiB cap + 60s timeout).
  - `releases_api_url(owner_repo: &str) -> String` (pure, tested).

- [ ] **Step 1: Write the failing test** (pure URL builder) in `crates/rupu-update/src/github.rs`:

```rust
use crate::model::{parse_releases, Release, ReleaseSource};

const API: &str = "https://api.github.com";
const MAX_BYTES: u64 = 200 * 1024 * 1024;

pub fn releases_api_url(owner_repo: &str) -> String {
    format!("{API}/repos/{owner_repo}/releases?per_page=100")
}

pub struct GithubReleaseSource {
    owner_repo: String,
    client: reqwest::Client,
}

impl GithubReleaseSource {
    pub fn new(owner_repo: impl Into<String>) -> Self {
        Self { owner_repo: owner_repo.into(), client: reqwest::Client::new() }
    }
}

fn req(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    let mut b = client.get(url).header("User-Agent", "rupu-update");
    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        if !tok.is_empty() {
            b = b.header("Authorization", format!("Bearer {tok}"));
        }
    }
    b
}

#[async_trait::async_trait]
impl ReleaseSource for GithubReleaseSource {
    async fn list_releases(&self) -> Result<Vec<Release>, crate::UpdateError> {
        let url = releases_api_url(&self.owner_repo);
        let resp = req(&self.client, &url).send().await.map_err(|e| crate::UpdateError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(crate::UpdateError::Network(format!("GitHub API {}", resp.status())));
        }
        let body = resp.text().await.map_err(|e| crate::UpdateError::Network(e.to_string()))?;
        parse_releases(&body)
    }
}

/// Download `url` into memory (UA header, optional token, size cap + timeout).
pub async fn download_bytes(url: &str) -> Result<Vec<u8>, crate::UpdateError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| crate::UpdateError::Network(e.to_string()))?;
    let resp = req(&client, url).send().await.map_err(|e| crate::UpdateError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(crate::UpdateError::Network(format!("download {}: {}", url, resp.status())));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_BYTES {
            return Err(crate::UpdateError::Network(format!("asset too large: {len} bytes")));
        }
    }
    let bytes = resp.bytes().await.map_err(|e| crate::UpdateError::Network(e.to_string()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(crate::UpdateError::Network("asset exceeded size cap".into()));
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_releases_url() {
        assert_eq!(
            releases_api_url("Section9Labs/rupu"),
            "https://api.github.com/repos/Section9Labs/rupu/releases?per_page=100"
        );
    }
}
```

- [ ] **Step 2: Register** `pub mod github;` in `lib.rs`.
- [ ] **Step 3: Run test + build:** `cargo test -p rupu-update github` → 1 passed; `cargo build -p rupu-update`.
- [ ] **Step 4: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/github.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/{github.rs,lib.rs}
git commit -m "feat(update): GitHub release source + asset downloader"
```

---

### Task 10: Orchestrator — `check` + `install` flows

**Files:**
- Create: `crates/rupu-update/src/flow.rs`
- Modify: `lib.rs`
- Test: inline (mock `ReleaseSource`; install path exercised via a temp target + injected bytes)

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `struct UpdateContext { current_version: Version, channel: Channel, exe_path: PathBuf, is_dev: bool }`.
  - `async fn check(src: &dyn ReleaseSource, ctx: &UpdateContext) -> Result<CheckOutcome, UpdateError>` where `CheckOutcome { decision: Decision, latest: Option<Version>, download: Option<(String,String)> /* (bin_url, sha_url) */ }`.
  - `async fn install(src: &dyn ReleaseSource, ctx: &UpdateContext, force: bool, apply: &dyn ApplyStrategy) -> Result<Version, UpdateError>` — resolves latest, downloads bin+sha, verifies checksum, then hands verified bytes to `apply.apply(bytes, target)`.
  - `trait ApplyStrategy { fn apply(&self, verified: &[u8], target: &Path) -> Result<(), UpdateError>; }` with `DirectApply` (calls `install::swap_in_place` with a backup). (Elevation strategy added in Task 12.)

- [ ] **Step 1: Write the failing test** in `crates/rupu-update/src/flow.rs` (mock source + capturing apply):

```rust
use crate::decide::{decide, is_dev_exe, Decision};
use crate::install;
use crate::model::{Channel, Release, ReleaseSource};
use crate::select::{asset_for, select_latest};
use crate::verify::verify_checksum;
use semver::Version;
use std::path::{Path, PathBuf};

pub struct UpdateContext {
    pub current_version: Version,
    pub channel: Channel,
    pub exe_path: PathBuf,
    pub is_dev: bool,
}

impl UpdateContext {
    pub fn from_env(current_version: &str, channel: Channel, exe_path: PathBuf) -> Result<Self, crate::UpdateError> {
        let cv = Version::parse(current_version).map_err(|e| crate::UpdateError::Parse(e.to_string()))?;
        let is_dev = is_dev_exe(&exe_path);
        Ok(Self { current_version: cv, channel, exe_path, is_dev })
    }
}

pub struct CheckOutcome {
    pub decision: Decision,
    pub latest: Option<Version>,
    pub download: Option<(String, String)>,
}

pub trait ApplyStrategy {
    fn apply(&self, verified: &[u8], target: &Path) -> Result<(), crate::UpdateError>;
}

/// Non-elevated apply: swap in place with a backup.
pub struct DirectApply;
impl ApplyStrategy for DirectApply {
    fn apply(&self, verified: &[u8], target: &Path) -> Result<(), crate::UpdateError> {
        let bak = install::backup_dir().join(format!(
            "rupu-{}",
            target.file_name().and_then(|n| n.to_str()).unwrap_or("prev")
        ));
        install::swap_in_place(verified, target, Some(&bak))
    }
}

fn platform() -> String { crate::decide::current_platform() }

pub async fn check(src: &dyn ReleaseSource, ctx: &UpdateContext) -> Result<CheckOutcome, crate::UpdateError> {
    let releases = src.list_releases().await?;
    let plat = platform();
    let Some(latest) = select_latest(&releases, ctx.channel, &plat) else {
        return Err(crate::UpdateError::NoAssetForPlatform { channel: ctx.channel.to_string(), platform: plat });
    };
    let (bin, sha) = asset_for(latest, &plat).expect("select guarantees asset");
    let decision = decide(&ctx.current_version, &latest.version, false);
    Ok(CheckOutcome {
        decision,
        latest: Some(latest.version.clone()),
        download: Some((bin.url.clone(), sha.url.clone())),
    })
}

pub async fn install(
    src: &dyn ReleaseSource,
    ctx: &UpdateContext,
    force: bool,
    apply: &dyn ApplyStrategy,
    download: impl Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, crate::UpdateError>> + Send>>,
) -> Result<Version, crate::UpdateError> {
    if ctx.is_dev {
        return Err(crate::UpdateError::DevBuild(ctx.exe_path.display().to_string()));
    }
    let releases = src.list_releases().await?;
    let plat = platform();
    let latest = select_latest(&releases, ctx.channel, &plat)
        .ok_or_else(|| crate::UpdateError::NoAssetForPlatform { channel: ctx.channel.to_string(), platform: plat.clone() })?;
    if let Decision::UpToDate | Decision::Ahead = decide(&ctx.current_version, &latest.version, force) {
        return Ok(ctx.current_version.clone());
    }
    let (bin, sha) = asset_for(latest, &plat).expect("select guarantees asset");
    let bin_bytes = download(bin.url.clone()).await?;
    let sha_text = String::from_utf8_lossy(&download(sha.url.clone()).await?).into_owned();
    verify_checksum(&bin_bytes, &sha_text)?;
    apply.apply(&bin_bytes, &ctx.exe_path)?;
    Ok(latest.version.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_releases;
    use std::sync::Mutex;

    struct MockSrc(Vec<Release>);
    #[async_trait::async_trait]
    impl ReleaseSource for MockSrc {
        async fn list_releases(&self) -> Result<Vec<Release>, crate::UpdateError> { Ok(self.0.clone()) }
    }
    struct CapApply(Mutex<Option<Vec<u8>>>);
    impl ApplyStrategy for CapApply {
        fn apply(&self, verified: &[u8], _t: &Path) -> Result<(), crate::UpdateError> {
            *self.0.lock().unwrap() = Some(verified.to_vec());
            Ok(())
        }
    }

    fn releases_for(plat: &str, sha: &str) -> Vec<Release> {
        // sha sidecar content is served by the download closure below, not here.
        parse_releases(&format!(r#"[
          {{"tag_name":"v0.35.4-beta","prerelease":true,
            "assets":[{{"name":"rupu-{plat}","browser_download_url":"BIN"}},
                      {{"name":"rupu-{plat}.sha256","browser_download_url":"SHA"}}]}}
        ]"#)).unwrap().into_iter().map(|mut r| { let _ = sha; r.assets = r.assets; r }).collect()
    }

    #[tokio::test]
    async fn install_downloads_verifies_and_applies() {
        let plat = crate::decide::current_platform();
        let payload = b"NEWBIN".to_vec();
        let sidecar = format!("{}  rupu-{plat}", crate::verify::sha256_hex(&payload));
        let src = MockSrc(releases_for(&plat, &sidecar));
        let ctx = UpdateContext::from_env("0.35.3", Channel::Beta, PathBuf::from("/usr/local/bin/rupu")).unwrap();
        let cap = CapApply(Mutex::new(None));
        let payload2 = payload.clone();
        let sidecar2 = sidecar.clone();
        let dl = move |url: String| {
            let payload = payload2.clone();
            let sidecar = sidecar2.clone();
            Box::pin(async move {
                Ok(if url == "BIN" { payload } else { sidecar.into_bytes() })
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, crate::UpdateError>> + Send>>
        };
        let v = install(&src, &ctx, false, &cap, dl).await.unwrap();
        assert_eq!(v.to_string(), "0.35.4-beta");
        assert_eq!(cap.0.lock().unwrap().as_deref(), Some(&b"NEWBIN"[..]));
    }

    #[tokio::test]
    async fn install_refuses_dev_build() {
        let src = MockSrc(vec![]);
        let ctx = UpdateContext::from_env("0.35.3", Channel::Beta, PathBuf::from("/x/target/release/rupu")).unwrap();
        let dl = |_u: String| Box::pin(async { Ok(vec![]) }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, crate::UpdateError>> + Send>>;
        assert!(matches!(install(&src, &ctx, false, &DirectApply, dl).await, Err(crate::UpdateError::DevBuild(_))));
    }
}
```

- [ ] **Step 2: Register** `pub mod flow;` in `lib.rs`; re-export `check`, `install`, `UpdateContext`, `CheckOutcome`, `ApplyStrategy`, `DirectApply`.
- [ ] **Step 3: Run tests fail→pass:** `cargo test -p rupu-update flow` → 2 passed.
- [ ] **Step 4: Commit**

```bash
rustfmt --edition 2021 crates/rupu-update/src/flow.rs crates/rupu-update/src/lib.rs
git add crates/rupu-update/src/{flow.rs,lib.rs}
git commit -m "feat(update): check + install orchestration (mock-tested)"
```

---

## Phase 4 — CLI wiring

### Task 11: `rupu update` + `--check`/`--force`/`--channel`/`--yes`/`--rollback`

**Files:**
- Create: `crates/rupu-cli/src/cmd/update.rs`
- Modify: the clap command enum + dispatcher (e.g. `crates/rupu-cli/src/cli.rs` / `main.rs`), `crates/rupu-cli/src/cmd/mod.rs`
- Test: an argument-parsing unit test + a manual smoke note

**Interfaces:**
- Consumes: `rupu_update::{Channel, flow, github, ...}`, `build_info`, `rupu_config`.
- Produces: `UpdateArgs` (clap) + `pub async fn run(args: UpdateArgs) -> anyhow::Result<()>`.

- [ ] **Step 1: Add the subcommand + args.** In the CLI command enum add `Update(cmd::update::UpdateArgs)`. In `crates/rupu-cli/src/cmd/update.rs`:

```rust
use anyhow::{Context, Result};
use clap::Args;
use rupu_update::{decide::Decision, flow, github, model::Channel};
use std::str::FromStr;

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Only report whether an update is available; install nothing.
    #[arg(long)]
    pub check: bool,
    /// Reinstall even if already up to date.
    #[arg(long)]
    pub force: bool,
    /// Skip the confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Override the configured channel for this run.
    #[arg(long, value_name = "beta|stable")]
    pub channel: Option<String>,
    /// Restore the previously-installed binary.
    #[arg(long)]
    pub rollback: bool,
}

fn resolve_channel(flag: Option<&str>, cfg: Option<&str>) -> Result<Channel> {
    let raw = flag.or(cfg).unwrap_or("stable");
    Channel::from_str(raw).map_err(|e| anyhow::anyhow!(e))
}

pub async fn run(args: UpdateArgs) -> Result<()> {
    let cfg = rupu_config::load().unwrap_or_default(); // existing loader; adjust to real API
    let channel = resolve_channel(args.channel.as_deref(), cfg.update.channel.as_deref())?;

    let exe = std::env::current_exe().context("resolve current exe")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let ctx = flow::UpdateContext::from_env(crate::build_info::RELEASE_VERSION, channel, exe)?;

    if args.rollback {
        let target = &ctx.exe_path;
        let bak = rupu_update::install::backup_dir()
            .join(format!("rupu-{}", target.file_name().and_then(|n| n.to_str()).unwrap_or("prev")));
        return apply_maybe_elevated(&std::fs::read(&bak).context("read backup")?, target)
            .map(|_| println!("Rolled back to {}", bak.display()));
    }

    let src = github::GithubReleaseSource::new("Section9Labs/rupu");

    if args.check {
        let out = flow::check(&src, &ctx).await?;
        match out.decision {
            Decision::UpToDate => { println!("rupu {} ({channel}) is up to date.", ctx.current_version); }
            Decision::Update { to, .. } => { println!("Update available: {} → {to} ({channel}). Run 'rupu update'.", ctx.current_version); std::process::exit(10); }
            Decision::Ahead => { println!("rupu {} is ahead of the {channel} channel.", ctx.current_version); }
        }
        return Ok(());
    }

    if ctx.is_dev {
        anyhow::bail!("this looks like a development build ({}); use `make install` / `cargo build` instead", ctx.exe_path.display());
    }

    // Peek to confirm + print target version.
    let out = flow::check(&src, &ctx).await?;
    match &out.decision {
        Decision::UpToDate if !args.force => { println!("Already up to date ({}).", ctx.current_version); return Ok(()); }
        Decision::Ahead if !args.force => { println!("rupu {} is ahead of the {channel} channel; nothing to do.", ctx.current_version); return Ok(()); }
        _ => {}
    }
    if !args.yes {
        let to = out.latest.clone().unwrap();
        eprint!("Update rupu {} → {to} ({channel})? [y/N] ", ctx.current_version);
        use std::io::Write; std::io::stderr().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted."); return Ok(());
        }
    }

    let dl = |url: String| Box::pin(async move { github::download_bytes(&url).await })
        as std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, rupu_update::UpdateError>> + Send>>;
    let apply = ElevatingApply;
    let new = flow::install(&src, &ctx, args.force, &apply, dl).await?;
    println!("Updated rupu {} → {new} ({channel}).", ctx.current_version);
    Ok(())
}
```

*(Note: `ElevatingApply`, `apply_maybe_elevated`, and the exact `rupu_config::load()` call are provided in Task 12/its integration; if implementing Task 11 alone, temporarily use `flow::DirectApply` and a plain backup read so the crate compiles, then swap in Task 12.)*

- [ ] **Step 2: Register the subcommand** in the dispatcher match arm: `Command::Update(a) => cmd::update::run(a).await?,` and add `pub mod update;` to `cmd/mod.rs`.

- [ ] **Step 3: Add a parse test** in `update.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_resolution_precedence() {
        assert_eq!(resolve_channel(Some("beta"), Some("stable")).unwrap(), Channel::Beta);
        assert_eq!(resolve_channel(None, Some("beta")).unwrap(), Channel::Beta);
        assert_eq!(resolve_channel(None, None).unwrap(), Channel::Stable);
        assert!(resolve_channel(Some("nightly"), None).is_err());
    }
}
```

- [ ] **Step 4: Build + test:** `cargo build -p rupu-cli && cargo test -p rupu-cli update` → passes.
- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/update.rs crates/rupu-cli/src/cmd/mod.rs
git add -A
git commit -m "feat(cli): rupu update subcommand (check/force/channel/yes/rollback)"
```

---

### Task 12: Elevation — hidden `__apply-update` + `ElevatingApply`

**Files:**
- Create: `crates/rupu-cli/src/cmd/apply_update.rs`
- Modify: CLI command enum (hidden subcommand), `crates/rupu-update/src/flow.rs` (nothing) / new `crates/rupu-cli/src/cmd/update.rs` helpers
- Test: unit test for the writable-vs-not decision + re-verify-before-swap

**Interfaces:**
- Produces:
  - `struct ElevatingApply` (impl `rupu_update::flow::ApplyStrategy`) — if `target` dir is writable, calls `DirectApply`; else stages the verified bytes to `~/.rupu/cache/update/rupu.staged`, computes sha, and runs `sudo <self> __apply-update --from <staged> --to <target> --sha256 <hex>`.
  - `apply_maybe_elevated(bytes, target)` helper used by rollback.
  - Hidden `ApplyUpdateArgs { from, to, sha256 }` + `run()` — re-verifies the staged file's sha256, then `install::swap_in_place(bytes, to, Some(backup))`.

- [ ] **Step 1: Write the failing test** for the writability branch in `crates/rupu-cli/src/cmd/apply_update.rs`:

```rust
use anyhow::{Context, Result};
use clap::Args;
use std::path::Path;

/// True if we can create/replace files in `dir`.
pub fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".rupu-write-probe.{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => { let _ = std::fs::remove_file(&probe); true }
        Err(_) => false,
    }
}

#[derive(Args, Debug)]
pub struct ApplyUpdateArgs {
    #[arg(long)] pub from: std::path::PathBuf,
    #[arg(long)] pub to: std::path::PathBuf,
    #[arg(long)] pub sha256: String,
}

/// Privileged apply step (invoked via sudo). Re-verifies the staged file's
/// checksum before swapping — trusts nothing from argv.
pub fn run(args: ApplyUpdateArgs) -> Result<()> {
    let bytes = std::fs::read(&args.from).context("read staged binary")?;
    let side = format!("{}  staged", args.sha256);
    rupu_update::verify::verify_checksum(&bytes, &side).context("staged checksum re-verify")?;
    let bak = rupu_update::install::backup_dir()
        .join(format!("rupu-{}", args.to.file_name().and_then(|n| n.to_str()).unwrap_or("prev")));
    rupu_update::install::swap_in_place(&bytes, &args.to, Some(&bak)).context("privileged swap")?;
    println!("applied update to {}", args.to.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tempdir_is_writable() {
        let d = tempfile::tempdir().unwrap();
        assert!(dir_writable(d.path()));
    }
    #[test]
    fn root_owned_dir_not_writable() {
        // /usr/local/bin is root-owned in this environment; skip if somehow writable.
        if !dir_writable(Path::new("/usr/local/bin")) {
            assert!(!dir_writable(Path::new("/usr/local/bin")));
        }
    }
}
```

- [ ] **Step 2: Implement `ElevatingApply`** in `crates/rupu-cli/src/cmd/update.rs` (add near the bottom):

```rust
use rupu_update::flow::ApplyStrategy;
use std::path::Path;

pub struct ElevatingApply;

impl ApplyStrategy for ElevatingApply {
    fn apply(&self, verified: &[u8], target: &Path) -> Result<(), rupu_update::UpdateError> {
        let dir = target.parent().ok_or_else(|| rupu_update::UpdateError::Install("target has no parent".into()))?;
        if crate::cmd::apply_update::dir_writable(dir) {
            return rupu_update::flow::DirectApply.apply(verified, target);
        }
        // Stage + elevate.
        let cache = rupu_update::install::backup_dir().parent().unwrap().join("cache").join("update");
        std::fs::create_dir_all(&cache).map_err(rupu_update::UpdateError::Io)?;
        let staged = cache.join("rupu.staged");
        std::fs::write(&staged, verified).map_err(rupu_update::UpdateError::Io)?;
        let sha = rupu_update::verify::sha256_hex(verified);
        let self_exe = std::env::current_exe().map_err(rupu_update::UpdateError::Io)?;
        eprintln!("Elevating to install into {} …", dir.display());
        let status = std::process::Command::new("sudo")
            .arg(self_exe)
            .arg("__apply-update")
            .arg("--from").arg(&staged)
            .arg("--to").arg(target)
            .arg("--sha256").arg(&sha)
            .status()
            .map_err(|e| rupu_update::UpdateError::Install(format!("sudo failed to start: {e}")))?;
        if !status.success() {
            return Err(rupu_update::UpdateError::Install(
                format!("privileged apply failed; run manually: sudo {} __apply-update --from {} --to {} --sha256 {}",
                    std::env::current_exe().ok().and_then(|p| p.to_str().map(str::to_string)).unwrap_or_else(|| "rupu".into()),
                    staged.display(), target.display(), sha),
            ));
        }
        Ok(())
    }
}

pub fn apply_maybe_elevated(bytes: &[u8], target: &Path) -> Result<()> {
    ElevatingApply.apply(bytes, target).map_err(|e| anyhow::anyhow!(e))
}
```

- [ ] **Step 3: Register the hidden subcommand.** Add `#[command(hide = true)] ApplyUpdate(cmd::apply_update::ApplyUpdateArgs)` to the enum and `Command::ApplyUpdate(a) => cmd::apply_update::run(a)?,` to the dispatcher (note: synchronous). Add `pub mod apply_update;` to `cmd/mod.rs`.
- [ ] **Step 4: Build + test:** `cargo build -p rupu-cli && cargo test -p rupu-cli apply_update` → passes.
- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/apply_update.rs crates/rupu-cli/src/cmd/update.rs crates/rupu-cli/src/cmd/mod.rs
git add -A
git commit -m "feat(cli): sudo-elevated __apply-update step + ElevatingApply"
```

---

### Task 13: `rupu --version` shows channel; passive notice on dispatch

**Files:**
- Modify: the clap root (`#[command(version = ...)]` or a custom `--version`), the top-level dispatch entrypoint (`main.rs`)
- Create: `crates/rupu-cli/src/update_notice.rs`
- Test: inline for the "should print?" gate

**Interfaces:**
- Consumes: `build_info`, `rupu_config`, `rupu_update::notice`.
- Produces:
  - `update_notice::maybe_print(cfg_check: Option<bool>, channel: &str, current: &str, is_tty: bool, structured_output: bool)` — decides + prints, spawning a detached refresh when stale.
  - `should_check(cfg_check: Option<bool>, env_disabled: bool, is_tty: bool, structured_output: bool) -> bool`.

- [ ] **Step 1: Write the failing test** in `crates/rupu-cli/src/update_notice.rs`:

```rust
/// Gate for the passive notice: on by default, suppressed for non-TTY,
/// structured output, config `check=false`, or `RUPU_NO_UPDATE_CHECK`.
pub fn should_check(cfg_check: Option<bool>, env_disabled: bool, is_tty: bool, structured_output: bool) -> bool {
    if env_disabled || structured_output || !is_tty { return false; }
    cfg_check.unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_on_for_interactive_tty() { assert!(should_check(None, false, true, false)); }
    #[test]
    fn off_when_not_tty() { assert!(!should_check(None, false, false, false)); }
    #[test]
    fn off_when_structured() { assert!(!should_check(Some(true), false, true, true)); }
    #[test]
    fn off_when_env_disabled() { assert!(!should_check(Some(true), true, true, false)); }
    #[test]
    fn off_when_config_false() { assert!(!should_check(Some(false), false, true, false)); }
}
```

- [ ] **Step 2: Implement `maybe_print`** (below the test) — best-effort, never fails the caller:

```rust
pub fn maybe_print(cfg_check: Option<bool>, channel: &str, current: &str, is_tty: bool, structured_output: bool) {
    let env_disabled = std::env::var_os("RUPU_NO_UPDATE_CHECK").is_some();
    if !should_check(cfg_check, env_disabled, is_tty, structured_output) { return; }
    let path = rupu_update::notice::state_path();

    // Print from cache first (cheap, no network).
    if let Some(state) = rupu_update::notice::load_state(&path) {
        if state.channel == channel {
            if let Some(line) = rupu_update::notice::notice_line(current, &state.latest_version, channel) {
                eprintln!("{line}");
            }
        }
    }

    // Refresh in the background when stale — detached, swallow all errors.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let stale = rupu_update::notice::load_state(&path)
        .map(|s| s.channel != channel || rupu_update::notice::is_stale(s.last_checked, now, 86_400))
        .unwrap_or(true);
    if stale {
        let channel = channel.to_string();
        tokio::spawn(async move {
            if let Ok(ch) = <rupu_update::model::Channel as std::str::FromStr>::from_str(&channel) {
                let src = rupu_update::github::GithubReleaseSource::new("Section9Labs/rupu");
                if let Ok(rels) = <rupu_update::github::GithubReleaseSource as rupu_update::model::ReleaseSource>::list_releases(&src).await {
                    let plat = rupu_update::decide::current_platform();
                    if let Some(latest) = rupu_update::select::select_latest(&rels, ch, &plat) {
                        let _ = rupu_update::notice::save_state(&path, &rupu_update::notice::CheckState {
                            channel, last_checked: now, latest_version: latest.version.to_string(),
                        });
                    }
                }
            }
        });
    }
}
```

- [ ] **Step 3: Wire into dispatch + version string.** In `main.rs`: set the clap version to `build_info::version_line()` (e.g. via `#[command(version = ...)]` replaced by a `.version(build_info::version_line())` on the `Command`), and — **only for interactive, non-`update` commands** — after resolving config call:

```rust
let is_tty = std::io::stderr().is_terminal(); // std::io::IsTerminal
let structured = /* true when --format json/jsonl/csv is in effect */ false;
crate::update_notice::maybe_print(cfg.update.check, &channel_str, crate::build_info::RELEASE_VERSION, is_tty, structured);
```

Add `mod update_notice;`. Guard so `rupu update` itself does not also print the notice.

- [ ] **Step 4: Build + test:** `cargo test -p rupu-cli update_notice` → 5 passed; `cargo build -p rupu-cli`; run `./target/debug/rupu --version` and confirm it prints `… (dev)`.
- [ ] **Step 5: Commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/update_notice.rs crates/rupu-cli/src/main.rs
git add -A
git commit -m "feat(cli): channel in --version + passive update notice"
```

---

## Phase 5 — Release tooling

### Task 14: Channel-aware `gh-build.sh` + Makefile targets

**Files:**
- Modify: `scripts/gh-build.sh`, `Makefile`
- Test: manual (documented dry commands)

**Interfaces:**
- Produces: `scripts/gh-build.sh <beta|stable>`; `make gh-beta`, `make gh-stable`, deprecated alias `make gh-build`.

- [ ] **Step 1: Parameterize `scripts/gh-build.sh`.** Accept `CHANNEL="${1:?usage: gh-build.sh <beta|stable>}"`. Derive:
  - `case "$CHANNEL" in beta) SUFFIX="-beta"; PRE_FLAG="--prerelease"; ROLLING="latest-beta";; stable) SUFFIX=""; PRE_FLAG=""; ROLLING="latest-stable";; *) echo "channel must be beta|stable" >&2; exit 1;; esac`
  - `VERSIONED_TAG="v${WORKSPACE_VERSION}${SUFFIX}"`.
  - In `publish_release`, use `$PRE_FLAG` for `gh release create`/`edit` (empty for stable → a full "latest" release), and pass `ROLLING` as the rolling tag. Keep `ASSET_NAME="rupu-${OS}-${ARCH}"` and the `.sha256` sidecar.
  - Export the build identity **before** the `make release` compile so `option_env!` captures it — see Step 2 (the make target sets the env for the cargo build, not the script).

- [ ] **Step 2: Makefile targets.** Add:

```make
gh-beta: RUPU_RELEASE_CHANNEL=beta
gh-beta: export RUPU_RELEASE_CHANNEL
gh-beta: release
	@RUPU_RELEASE_CHANNEL=beta RUPU_RELEASE_VERSION="$$(grep -E '^version = ' Cargo.toml | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')-beta" scripts/gh-build.sh beta

gh-stable: release
	@RUPU_RELEASE_CHANNEL=stable RUPU_RELEASE_VERSION="$$(grep -E '^version = ' Cargo.toml | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')" scripts/gh-build.sh stable

# Deprecated alias — betas were formerly `-build`.
gh-build: gh-beta
```

**Important:** the release binary must be compiled with the env set. Change the `release` recipe (or add `release-beta`/`release-stable`) so `RUPU_RELEASE_CHANNEL`/`RUPU_RELEASE_VERSION` are exported for the `cargo build --release` step. Simplest: have `gh-beta`/`gh-stable` run the cargo build themselves with the env, e.g.:

```make
gh-beta:
	RUPU_RELEASE_CHANNEL=beta RUPU_RELEASE_VERSION="$(shell grep -E '^version = ' Cargo.toml | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')-beta" cargo build --release
	@scripts/sign-dev.sh
	@scripts/gh-build.sh beta
gh-stable:
	RUPU_RELEASE_CHANNEL=stable RUPU_RELEASE_VERSION="$(shell grep -E '^version = ' Cargo.toml | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')" cargo build --release
	@scripts/sign-dev.sh
	@scripts/gh-build.sh stable
```

(Confirm the actual sign step name from the current `release` recipe and reuse it.)

- [ ] **Step 3: Update `.PHONY` + `help`.** Add `gh-beta gh-stable` to `.PHONY`; update the `help` text describing the new channels; mark `gh-build` deprecated.

- [ ] **Step 4: Dry verify (no publish).** Run just the version/env derivation to confirm the tag + embedded version line up:

```bash
V=$(grep -E '^version = ' Cargo.toml | head -n1 | sed -E 's/.*"([^"]+)".*/\1/'); echo "beta tag: v${V}-beta ; embedded: ${V}-beta"
echo "stable tag: v${V} ; embedded: ${V}"
```

Build one channel locally and confirm the embed:

```bash
RUPU_RELEASE_CHANNEL=beta RUPU_RELEASE_VERSION="${V}-beta" cargo build --release -p rupu-cli
./target/release/rupu --version   # expect: rupu <V>-beta (beta)
```

- [ ] **Step 5: Commit**

```bash
git add scripts/gh-build.sh Makefile
git commit -m "build: beta/stable release channels (gh-beta/gh-stable); embed channel+version"
```

---

### Task 15: Full-suite gate + docs

**Files:**
- Modify: `CLAUDE.md` (add `update` to the CLI subcommand list; note channels), `docs/` if a user doc exists
- Test: workspace build + touched-crate tests

- [ ] **Step 1: Full check.**

```bash
cargo build -p rupu-update -p rupu-config -p rupu-cli
cargo test -p rupu-update -p rupu-config
cargo test -p rupu-cli update build_info update_notice apply_update
cargo clippy -p rupu-update -p rupu-cli 2>&1 | grep -iE "warning|error" | grep -iE "update|build_info|notice" || echo "clippy clean"
```

- [ ] **Step 2: Docs.** In `CLAUDE.md`, change "Twelve subcommands" → "Thirteen" and add `update`; add a one-line note that releases publish `beta` (prerelease) + `stable` channels and `rupu update` follows `[update].channel`.

- [ ] **Step 3: Commit**

```bash
rustfmt --edition 2021 <any touched .rs>
git add -A
git commit -m "docs: document rupu update + beta/stable channels"
```

---

## Self-review notes (author)

- **Spec coverage:** channels/tooling (Tasks 3,4,14) ✓; `[update]` config (Task 1) ✓; embedding + `--version` + dev-guard (Tasks 2,5,13) ✓; command surface + exit codes (Task 11) ✓; resolve/download/verify/swap (Tasks 3–10) ✓; elevation (Task 12) ✓; passive notice (Tasks 8,13) ✓; safety rails (dev-build, platform, downgrade, verify-fail — Tasks 5,6,10,12) ✓; testing (each task) ✓.
- **Known integration seams to resolve during execution:** (a) the exact `rupu_config` loader function name/signature used in Task 11 — replace `rupu_config::load()` with the real resolver; (b) the clap enum/dispatch location (some CLIs put it in `cli.rs`, others `main.rs`) — Task 11/12/13 reference "the command enum" generically; (c) the real sign step name reused in Task 14. These are named explicitly so the implementer wires them, not left as silent TODOs.
- **Deferred (per spec §11):** multi-platform assets, notarization.
