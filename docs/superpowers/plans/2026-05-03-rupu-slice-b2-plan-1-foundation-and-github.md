# rupu Slice B-2 — Plan 1: Foundation + GitHub connector

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create the `rupu-scm` crate with `RepoConnector` + `IssueConnector` trait families, the `ScmError` enum, the `Registry` builder, per-platform error classification, and a working GitHub connector implementation (Repo + Issues). Add `ProviderId::Github` + `ProviderId::Gitlab` to `rupu-auth`. Extend `rupu-config` with the new `[scm.*]` and `[issues.*]` sections. After this plan, a Rust call site can build a `Registry`, get the GitHub `RepoConnector`, and `list_repos()` against the live API. No MCP server, no CLI surface, no Gitlab.

**Architecture:** New `rupu-scm` crate following the same hexagonal shape as `rupu-providers`. Traits live at the crate root; per-platform impls in submodules. Re-uses Slice B-1's `CredentialResolver` for auth and `concurrency::semaphore_for` for rate-limit isolation. Error classification mirrors Plan 1 Task 11's `classify_error` table. GitHub adapter uses `octocrab` (mature, well-tested) plus `git2` for the `clone_to` operation.

**Tech Stack:** Rust 2021 (MSRV 1.88), `tokio`, `async-trait`, `reqwest`, `serde`, `thiserror`, `chrono`, `tracing`. New workspace deps: `octocrab` (GitHub SDK), `git2` (libgit2 bindings for clone), `lru` (cache), `schemars` (later in Plan 2; pinned now for cohesion). No new CLI deps.

**Spec:** `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`

---

## File Structure

```
crates/
  rupu-scm/                                     # NEW
    Cargo.toml
    src/
      lib.rs                                    # re-exports + module declarations
      types.rs                                  # Repo, Pr, Issue, Branch, Diff, Comment, Filters, RunTarget
      error.rs                                  # ScmError enum + classify_scm_error pure fn
      platform.rs                               # Platform / IssueTracker enums + parsers
      registry.rs                               # Registry struct + discover() builder
      connectors/
        mod.rs                                  # RepoConnector + IssueConnector traits
        github/
          mod.rs                                # GithubConnector facade (impl both traits)
          repo.rs                               # RepoConnector impl
          issues.rs                             # IssueConnector impl
          client.rs                             # octocrab + retry + ETag + semaphore wrapper
          fixtures/                             # tests/fixtures actually live under tests/, leaving here as a marker
    tests/
      classify_scm_error.rs                     # pure-function classifier table tests
      registry_discover.rs                      # InMemoryResolver + missing-credential fallback
      github_translation.rs                     # JSON fixture → typed value mapping
      github_httpmock.rs                        # full request/response round-trip
      live_smoke.rs                             # gated by RUPU_LIVE_TESTS=1 + RUPU_LIVE_GITHUB_TOKEN
      fixtures/
        github/
          repos_list_happy.json
          repos_list_empty.json
          repos_list_paginated_page_1.json
          repos_list_paginated_page_2.json
          repos_list_paginated_page_3.json
          repo_get_happy.json
          pr_get_happy.json
          pr_diff_happy.patch
          issue_get_happy.json
          error_401.json
          error_403_missing_scope.json
          error_429_rate_limited.json
          error_404.json
          error_409_conflict.json
        regen-github.sh                         # documented refresh script
  rupu-auth/
    src/
      backend.rs                                # MODIFY: add Github + Gitlab variants to ProviderId
      oauth/
        providers.rs                            # MODIFY: add Github and Gitlab provider_oauth entries
  rupu-config/
    src/
      lib.rs                                    # MODIFY: re-export new types
      scm_config.rs                             # NEW: ScmDefault, IssuesDefault, ScmPlatformConfig
      config.rs                                 # MODIFY: add scm, issues, scm_platforms fields to Config
    tests/
      scm_config.rs                             # NEW: TOML parse + override tests
```

## Conventions to honor

- Workspace deps only — no version pins inside crate `Cargo.toml`.
- `#![deny(clippy::all)]` at every crate root.
- `unsafe_code` forbidden.
- Tests use real I/O against `tempfile::TempDir`; HTTP via `httpmock` workspace dep.
- Errors: `thiserror` for libraries.
- Per the "no mock features" memory: every code path either does the real work or returns an explicit error. No silent `Ok(SilentNoOp)`.
- Per the "read reference impls" memory: when wiring an external API for the first time, read working third-party code (e.g., `octocrab` examples, GitHub's published OpenAPI) BEFORE inventing the request shape.

## Important pre-existing state (read before starting)

- `crates/rupu-auth/src/backend.rs:14-34` defines `ProviderId` with five variants (`Anthropic, Openai, Gemini, Copilot, Local`). Slice B-1 added `Gemini` here.
- `crates/rupu-auth/src/resolver.rs` exposes `CredentialResolver` trait with `async fn get(provider: &str, hint: Option<AuthMode>) -> Result<(AuthMode, AuthCredentials)>`.
- `crates/rupu-auth/src/in_memory.rs` exposes `InMemoryResolver` for tests with `put(provider, mode, stored)` and a refresh callback.
- `crates/rupu-auth/src/oauth/providers.rs` has the per-provider OAuth metadata; B-2 extends it with Github and Gitlab entries (Plan 1 Task 6 sets up Github only; Gitlab lands in Plan 2).
- `crates/rupu-providers/src/concurrency.rs` exposes `semaphore_for(provider: &str, override: Option<usize>) -> Arc<Semaphore>`. Plan 1 reuses this keyed by platform name.
- `crates/rupu-config/src/config.rs` has `Config` with `#[serde(default, deny_unknown_fields)]`. New fields must be added to that struct; tests must continue to pass with the existing TOML files.

---

## Phase 0 — Workspace deps + crate scaffolding

### Task 1: Add new workspace deps

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add deps to `[workspace.dependencies]`**

In root `Cargo.toml`, in `[workspace.dependencies]`, add:

```toml
octocrab = "0.42"
git2 = { version = "0.19", default-features = false, features = ["vendored-libgit2", "vendored-openssl"] }
lru = "0.12"
schemars = { version = "0.8", features = ["chrono"] }
```

- [ ] **Step 2: Verify workspace builds**

```
cargo metadata --no-deps --format-version 1 > /dev/null
```

Expected: exit 0. (No crate yet uses these; we're just declaring them.)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
deps: add octocrab + git2 + lru + schemars to workspace

octocrab is the GitHub REST/GraphQL SDK consumed by Plan 1's
GithubRepoConnector / GithubIssueConnector. git2 is libgit2 bindings
for the local-clone path. lru gates the connector ETag cache.
schemars is pinned now for cohesion; rupu-mcp uses it in Plan 2 to
generate JSON Schemas from the rupu-scm types.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Scaffold `rupu-scm` crate

**Files:**
- Create: `crates/rupu-scm/Cargo.toml`
- Create: `crates/rupu-scm/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members` list)

- [ ] **Step 1: Create the crate `Cargo.toml`**

Create `crates/rupu-scm/Cargo.toml`:

```toml
[package]
name = "rupu-scm"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
tokio = { workspace = true }
async-trait.workspace = true
chrono.workspace = true
reqwest.workspace = true
url.workspace = true
octocrab.workspace = true
git2.workspace = true
lru.workspace = true

# In-workspace
rupu-auth = { path = "../rupu-auth" }
rupu-config = { path = "../rupu-config" }
rupu-providers = { path = "../rupu-providers" }

[dev-dependencies]
tempfile.workspace = true
httpmock.workspace = true
serde_json.workspace = true
```

- [ ] **Step 2: Create the empty lib.rs**

Create `crates/rupu-scm/src/lib.rs`:

```rust
#![deny(clippy::all)]

//! rupu SCM connectors — typed per-platform repo + issue access.
//!
//! Defines [`RepoConnector`] and [`IssueConnector`] trait families
//! plus a [`Registry`] that builds connectors from configured
//! credentials. Per-platform impls live in `connectors/<platform>/`.
//!
//! Spec: `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`.

pub mod connectors;
pub mod error;
pub mod platform;
pub mod registry;
pub mod types;

pub use connectors::{IssueConnector, RepoConnector};
pub use error::{classify_scm_error, ScmError};
pub use platform::{IssueTracker, Platform};
pub use registry::Registry;
pub use types::{
    Branch, Comment, CreateIssue, CreatePr, Diff, FileContent, Issue, IssueFilter, IssueRef,
    IssueState, Pr, PrFilter, PrRef, PrState, Repo, RepoRef,
};
```

- [ ] **Step 3: Add to workspace members**

In root `Cargo.toml`, in the `[workspace] members = [...]` list, add `"crates/rupu-scm"`.

- [ ] **Step 4: Verify crate compiles (modules will be empty stubs)**

Create empty stub files so `lib.rs` resolves:

```bash
mkdir -p crates/rupu-scm/src/connectors
touch crates/rupu-scm/src/{error,platform,registry,types}.rs
touch crates/rupu-scm/src/connectors/mod.rs
```

The compilation will fail with "missing items" because `lib.rs` re-exports types not yet defined. That's expected — Tasks 3-5 add them. For now, verify Cargo can SEE the crate:

```
cargo check -p rupu-scm 2>&1 | head -20
```

Expected: errors about unresolved items in the re-exports (because the stub files are empty), but cargo finds and processes the crate.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/rupu-scm/
git commit -m "$(cat <<'EOF'
rupu-scm: scaffold new crate

Empty crate skeleton for SCM/issue connectors. Submodule layout
mirrors the design spec: types, error, platform, registry, and a
connectors/ tree with per-platform subdirectories. Tasks 3-9 fill
in the contents.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — Core types and platform enums

### Task 3: `Platform` and `IssueTracker` enums

**Files:**
- Modify: `crates/rupu-scm/src/platform.rs`

- [ ] **Step 1: Write the failing tests (inline in platform.rs)**

```rust
//! Platform identifiers for SCM and issue-tracker hosts.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Github,
    Gitlab,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            other => Err(format!("unknown platform: {other}")),
        }
    }
}

/// Issue trackers. B-2 only ships connectors for `Github` and `Gitlab`;
/// `Linear` and `Jira` exist so future adapters slot in without
/// reshaping call sites. Code that matches on this enum must include
/// `_ => Err(NotWiredInV0(...))` arms for the unbuilt variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueTracker {
    Github,
    Gitlab,
    Linear,
    Jira,
}

impl IssueTracker {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Linear => "linear",
            Self::Jira => "jira",
        }
    }
}

impl fmt::Display for IssueTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IssueTracker {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            "linear" => Ok(Self::Linear),
            "jira" => Ok(Self::Jira),
            other => Err(format!("unknown issue tracker: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_round_trips_strings() {
        for (p, s) in [(Platform::Github, "github"), (Platform::Gitlab, "gitlab")] {
            assert_eq!(p.as_str(), s);
            assert_eq!(p.to_string(), s);
            assert_eq!(Platform::from_str(s).unwrap(), p);
        }
        assert!(Platform::from_str("bogus").is_err());
    }

    #[test]
    fn platform_serde_lowercase() {
        let json = serde_json::to_string(&Platform::Github).unwrap();
        assert_eq!(json, "\"github\"");
        let p: Platform = serde_json::from_str(&json).unwrap();
        assert_eq!(p, Platform::Github);
    }

    #[test]
    fn issue_tracker_includes_all_four_variants() {
        for (t, s) in [
            (IssueTracker::Github, "github"),
            (IssueTracker::Gitlab, "gitlab"),
            (IssueTracker::Linear, "linear"),
            (IssueTracker::Jira, "jira"),
        ] {
            assert_eq!(t.as_str(), s);
            assert_eq!(IssueTracker::from_str(s).unwrap(), t);
        }
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rupu-scm --lib platform
```

Expected: 3 tests pass.

- [ ] **Step 3: Verify gates**

```
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Both exit 0.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-scm/src/platform.rs
git commit -m "$(cat <<'EOF'
rupu-scm: add Platform + IssueTracker enums

Platform = {Github, Gitlab}. IssueTracker = {Github, Gitlab, Linear,
Jira}. Linear/Jira are present in the enum from day 0 so future
connectors don't reshape call sites; per the no-mock-features rule,
code that matches on the enum must include explicit NotWiredInV0
arms for the unbuilt variants.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Core data types (`Repo`, `Pr`, `Issue`, etc.)

**Files:**
- Modify: `crates/rupu-scm/src/types.rs`

- [ ] **Step 1: Write the file**

Create `crates/rupu-scm/src/types.rs`:

```rust
//! Vendor-neutral types returned by [`crate::RepoConnector`] and
//! [`crate::IssueConnector`]. Per-platform adapters translate their
//! native SDK shapes into these.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::platform::{IssueTracker, Platform};

/// Reference to a repository on a specific platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoRef {
    pub platform: Platform,
    pub owner: String,
    pub repo: String,
}

/// Reference to a pull/merge request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrRef {
    pub repo: RepoRef,
    pub number: u32,
}

/// Reference to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueRef {
    pub tracker: IssueTracker,
    /// Tracker-native project identifier. For GitHub Issues:
    /// "owner/repo". For Linear: workspace UUID. Etc.
    pub project: String,
    pub number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileEncoding {
    Utf8,
    Base64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub r: RepoRef,
    pub default_branch: String,
    pub clone_url_https: String,
    pub clone_url_ssh: String,
    pub private: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub sha: String,
    pub protected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    /// Resolved ref the content was fetched at (commit sha or branch tip).
    pub ref_: String,
    pub content: String,
    pub encoding: FileEncoding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pr {
    pub r: PrRef,
    pub title: String,
    pub body: String,
    pub state: PrState,
    pub head_branch: String,
    pub base_branch: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub r: IssueRef,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub labels: Vec<String>,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    /// Full unified-diff patch text.
    pub patch: String,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrFilter {
    pub state: Option<PrState>,
    pub author: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueFilter {
    pub state: Option<IssueState>,
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePr {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_ref_serde_roundtrip() {
        let r = RepoRef {
            platform: Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: RepoRef = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn pr_state_serde_lowercase() {
        for (v, s) in [
            (PrState::Open, "\"open\""),
            (PrState::Closed, "\"closed\""),
            (PrState::Merged, "\"merged\""),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, s);
            let back: PrState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn pr_filter_default_is_empty() {
        let f = PrFilter::default();
        assert!(f.state.is_none());
        assert!(f.author.is_none());
        assert!(f.limit.is_none());
    }
}
```

- [ ] **Step 2: Run the tests**

```
cargo test -p rupu-scm --lib types
```

Expected: 3 tests pass.

- [ ] **Step 3: Verify gates**

```
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Both exit 0.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-scm/src/types.rs
git commit -m "$(cat <<'EOF'
rupu-scm: add core data types

Vendor-neutral types: Repo, Pr, Issue, Branch, FileContent, Diff,
Comment, PrFilter, IssueFilter, CreatePr, CreateIssue, plus the
RepoRef / PrRef / IssueRef triple. Per-platform adapters translate
their SDK shapes into these.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `ScmError` enum + `classify_scm_error` pure function

**Files:**
- Modify: `crates/rupu-scm/src/error.rs`
- Create: `crates/rupu-scm/tests/classify_scm_error.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/rupu-scm/tests/classify_scm_error.rs`:

```rust
use rupu_scm::{classify_scm_error, Platform, ScmError};

fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    for (k, v) in pairs {
        h.insert(
            reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            reqwest::header::HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

#[test]
fn github_401_is_unauthorized() {
    let e = classify_scm_error(Platform::Github, 401, "{}", &headers(&[]));
    assert!(matches!(e, ScmError::Unauthorized { .. }));
}

#[test]
fn github_403_with_missing_scope_is_missing_scope() {
    let h = headers(&[
        ("X-OAuth-Scopes", "read:user"),
        ("X-Accepted-OAuth-Scopes", "repo, read:user"),
    ]);
    let e = classify_scm_error(Platform::Github, 403, "{}", &h);
    match e {
        ScmError::MissingScope { scope, .. } => assert!(scope.contains("repo")),
        other => panic!("expected MissingScope, got {other:?}"),
    }
}

#[test]
fn github_403_without_scope_header_is_rate_limited() {
    // A 403 without scope header implies rate-limit (GitHub returns 403 for primary rate limits).
    let e = classify_scm_error(Platform::Github, 403, "{}", &headers(&[]));
    assert!(matches!(e, ScmError::RateLimited { .. }));
}

#[test]
fn github_429_with_retry_after_parses_seconds() {
    let h = headers(&[("Retry-After", "42")]);
    let e = classify_scm_error(Platform::Github, 429, "{}", &h);
    match e {
        ScmError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Some(std::time::Duration::from_secs(42)));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[test]
fn github_404_is_not_found() {
    let e = classify_scm_error(
        Platform::Github,
        404,
        r#"{"message":"Not Found","documentation_url":""}"#,
        &headers(&[]),
    );
    assert!(matches!(e, ScmError::NotFound { .. }));
}

#[test]
fn github_409_is_conflict() {
    let e = classify_scm_error(
        Platform::Github,
        409,
        r#"{"message":"Reference already exists"}"#,
        &headers(&[]),
    );
    match e {
        ScmError::Conflict { message } => assert!(message.contains("already exists")),
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[test]
fn github_400_is_bad_request() {
    let e = classify_scm_error(
        Platform::Github,
        400,
        r#"{"message":"validation failed"}"#,
        &headers(&[]),
    );
    assert!(matches!(e, ScmError::BadRequest { .. }));
}

#[test]
fn github_502_is_transient() {
    let e = classify_scm_error(Platform::Github, 502, "Bad Gateway", &headers(&[]));
    assert!(matches!(e, ScmError::Transient(_)));
}

#[test]
fn unknown_status_falls_to_transient() {
    let e = classify_scm_error(Platform::Github, 418, "I'm a teapot", &headers(&[]));
    assert!(matches!(e, ScmError::Transient(_)));
}

#[test]
fn is_recoverable_classifies_correctly() {
    assert!(ScmError::RateLimited { retry_after: None }.is_recoverable());
    assert!(ScmError::Transient(anyhow::anyhow!("x")).is_recoverable());
    assert!(ScmError::Conflict { message: "x".into() }.is_recoverable());
    assert!(ScmError::NotFound { what: "x".into() }.is_recoverable());
    assert!(!ScmError::Unauthorized {
        platform: "github".into(),
        hint: "x".into()
    }
    .is_recoverable());
    assert!(!ScmError::MissingScope {
        platform: "github".into(),
        scope: "repo".into(),
        hint: "x".into()
    }
    .is_recoverable());
    assert!(!ScmError::Network(anyhow::anyhow!("x")).is_recoverable());
    assert!(!ScmError::BadRequest { message: "x".into() }.is_recoverable());
}
```

- [ ] **Step 2: Implement `error.rs`**

Replace `crates/rupu-scm/src/error.rs` with:

```rust
//! ScmError + per-platform classification.
//!
//! Spec §4b + §9d. Recoverable variants surface to the agent as JSON
//! tool errors (the agent decides what to do). Unrecoverable variants
//! abort the run with an actionable message (mirrors Plan 1's
//! ProviderError::Unauthorized UX).

use std::time::Duration;

use reqwest::header::HeaderMap;
use thiserror::Error;

use crate::platform::Platform;

#[derive(Debug, Error)]
pub enum ScmError {
    // ─── Recoverable ───
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    #[error("transient: {0}")]
    Transient(#[source] anyhow::Error),

    #[error("conflict: {message}")]
    Conflict { message: String },

    #[error("not found: {what}")]
    NotFound { what: String },

    // ─── Unrecoverable ───
    #[error("unauthorized for {platform}: {hint}")]
    Unauthorized { platform: String, hint: String },

    #[error("missing scope `{scope}` for {platform}: {hint}")]
    MissingScope {
        platform: String,
        scope: String,
        hint: String,
    },

    #[error("network unreachable: {0}")]
    Network(#[source] anyhow::Error),

    #[error("bad request: {message}")]
    BadRequest { message: String },
}

impl ScmError {
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Transient(_)
                | Self::Conflict { .. }
                | Self::NotFound { .. }
        )
    }
}

/// Map an HTTP failure into the structured ScmError vocabulary. Pure
/// for testability; per-platform adapters call this at the boundary
/// between raw HTTP and trait return values. Spec §9d table.
pub fn classify_scm_error(
    platform: Platform,
    status: u16,
    body: &str,
    headers: &HeaderMap,
) -> ScmError {
    match status {
        401 => ScmError::Unauthorized {
            platform: platform.as_str().into(),
            hint: format!(
                "run: rupu auth login --provider {} --mode sso",
                platform.as_str()
            ),
        },
        403 => {
            // GitHub uses 403 both for missing-scope AND primary rate limits.
            // Differentiate by the X-Accepted-OAuth-Scopes header.
            if let Some(missing) = scope_missing(headers) {
                ScmError::MissingScope {
                    platform: platform.as_str().into(),
                    scope: missing,
                    hint: format!(
                        "re-login to grant the missing scope: rupu auth login --provider {} --mode sso",
                        platform.as_str()
                    ),
                }
            } else {
                ScmError::RateLimited {
                    retry_after: parse_retry_after(headers),
                }
            }
        }
        404 => ScmError::NotFound {
            what: extract_message(body).unwrap_or_else(|| "(unknown)".into()),
        },
        409 | 422 => {
            let message = extract_message(body).unwrap_or_else(|| truncate(body, 200));
            // 422 is split: GitHub uses it for both validation errors AND merge conflicts.
            // Bias toward Conflict only when the message hints at a write conflict.
            let lower = message.to_lowercase();
            if lower.contains("already exists")
                || lower.contains("conflict")
                || lower.contains("not mergeable")
            {
                ScmError::Conflict { message }
            } else if status == 422 {
                ScmError::BadRequest { message }
            } else {
                ScmError::Conflict { message }
            }
        }
        400 => ScmError::BadRequest {
            message: extract_message(body).unwrap_or_else(|| truncate(body, 200)),
        },
        429 => ScmError::RateLimited {
            retry_after: parse_retry_after(headers),
        },
        500..=599 => ScmError::Transient(anyhow::anyhow!(
            "{platform} {status}: {}",
            truncate(body, 200)
        )),
        _ => ScmError::Transient(anyhow::anyhow!(
            "{platform} {status}: {}",
            truncate(body, 200)
        )),
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let v = headers.get("Retry-After")?.to_str().ok()?.trim();
    v.parse::<u64>().ok().map(Duration::from_secs)
}

fn scope_missing(headers: &HeaderMap) -> Option<String> {
    let granted: std::collections::HashSet<_> = headers
        .get("X-OAuth-Scopes")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();
    let needed: Vec<String> = headers
        .get("X-Accepted-OAuth-Scopes")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();
    let missing: Vec<&String> = needed.iter().filter(|s| !granted.contains(*s)).collect();
    if missing.is_empty() && needed.is_empty() {
        None
    } else if missing.is_empty() {
        None
    } else {
        Some(missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","))
    }
}

fn extract_message(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("message")?
        .as_str()
        .map(String::from)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}
```

- [ ] **Step 3: Run tests + gates**

```
cargo test -p rupu-scm --test classify_scm_error
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Expected: 10 classifier tests pass; fmt + clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-scm/src/error.rs crates/rupu-scm/tests/classify_scm_error.rs
git commit -m "$(cat <<'EOF'
rupu-scm: add ScmError + classify_scm_error

Recoverable variants (RateLimited, Transient, Conflict, NotFound)
surface as JSON tool errors so the agent decides what to do.
Unrecoverable variants (Unauthorized, MissingScope, Network,
BadRequest) abort the run with actionable messages mirroring Plan 1's
ProviderError::Unauthorized UX.

GitHub-specific quirk: 403 covers both missing-scope and primary
rate limits; differentiate via X-Accepted-OAuth-Scopes header.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — Trait families + Registry

### Task 6: `RepoConnector` and `IssueConnector` traits

**Files:**
- Modify: `crates/rupu-scm/src/connectors/mod.rs`

- [ ] **Step 1: Write the file**

Replace `crates/rupu-scm/src/connectors/mod.rs` with:

```rust
//! RepoConnector + IssueConnector trait families.
//!
//! Each platform implements one or both. Trait objects (`Arc<dyn ...>`)
//! live behind [`crate::Registry`].

pub mod github;

use std::path::Path;

use async_trait::async_trait;

use crate::error::ScmError;
use crate::platform::{IssueTracker, Platform};
use crate::types::{
    Branch, Comment, CreateIssue, CreatePr, Diff, FileContent, Issue, IssueFilter, IssueRef,
    IssueState, Pr, PrFilter, PrRef, Repo, RepoRef,
};

#[async_trait]
pub trait RepoConnector: Send + Sync {
    fn platform(&self) -> Platform;

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError>;
    async fn get_repo(&self, r: &RepoRef) -> Result<Repo, ScmError>;
    async fn list_branches(&self, r: &RepoRef) -> Result<Vec<Branch>, ScmError>;
    async fn create_branch(
        &self,
        r: &RepoRef,
        name: &str,
        from_sha: &str,
    ) -> Result<Branch, ScmError>;
    async fn read_file(
        &self,
        r: &RepoRef,
        path: &str,
        ref_: Option<&str>,
    ) -> Result<FileContent, ScmError>;
    async fn list_prs(&self, r: &RepoRef, filter: PrFilter) -> Result<Vec<Pr>, ScmError>;
    async fn get_pr(&self, p: &PrRef) -> Result<Pr, ScmError>;
    async fn diff_pr(&self, p: &PrRef) -> Result<Diff, ScmError>;
    async fn comment_pr(&self, p: &PrRef, body: &str) -> Result<Comment, ScmError>;
    async fn create_pr(&self, r: &RepoRef, opts: CreatePr) -> Result<Pr, ScmError>;
    /// Clone the repo to a local directory using the platform's
    /// HTTPS clone URL with the connector's stored credential.
    async fn clone_to(&self, r: &RepoRef, dir: &Path) -> Result<(), ScmError>;
}

#[async_trait]
pub trait IssueConnector: Send + Sync {
    fn tracker(&self) -> IssueTracker;

    async fn list_issues(
        &self,
        project: &str,
        filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError>;
    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError>;
    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError>;
    async fn create_issue(
        &self,
        project: &str,
        opts: CreateIssue,
    ) -> Result<Issue, ScmError>;
    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sanity check: traits are object-safe (i.e., can be used as
    // `Arc<dyn RepoConnector>`). The Registry depends on this.
    fn _assert_object_safe() {
        let _: Option<std::sync::Arc<dyn RepoConnector>> = None;
        let _: Option<std::sync::Arc<dyn IssueConnector>> = None;
    }
}
```

- [ ] **Step 2: Create the empty github stub so `pub mod github` resolves**

Create `crates/rupu-scm/src/connectors/github/mod.rs`:

```rust
//! GitHub connectors. Implementation lands in Tasks 8-12.
```

- [ ] **Step 3: Build + gates**

```
cargo build -p rupu-scm
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

All exit 0.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-scm/src/connectors/
git commit -m "$(cat <<'EOF'
rupu-scm: add RepoConnector + IssueConnector traits

Object-safe trait families per the spec §4c. Per-platform impls land
in connectors/<platform>/ submodules. The github/ submodule is
stubbed; Tasks 8-12 fill it in.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Add `ProviderId::Github` and `ProviderId::Gitlab` to `rupu-auth`

**Files:**
- Modify: `crates/rupu-auth/src/backend.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-auth/src/backend.rs`:

```rust
#[cfg(test)]
mod scm_provider_id_tests {
    use super::*;

    #[test]
    fn github_string_form() {
        assert_eq!(ProviderId::Github.as_str(), "github");
    }

    #[test]
    fn gitlab_string_form() {
        assert_eq!(ProviderId::Gitlab.as_str(), "gitlab");
    }

    #[test]
    fn github_serde_roundtrip() {
        let json = serde_json::to_string(&ProviderId::Github).unwrap();
        assert_eq!(json, "\"github\"");
        let p: ProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(p, ProviderId::Github);
    }
}
```

- [ ] **Step 2: Run, verify failing**

```
cargo test -p rupu-auth --lib scm_provider_id_tests
```

Expected: compile error `no variant Github / Gitlab`.

- [ ] **Step 3: Add the variants**

Modify `crates/rupu-auth/src/backend.rs` so the enum and `as_str()` match:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Anthropic,
    Openai,
    Gemini,
    Copilot,
    Local,
    Github,
    Gitlab,
}

impl ProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::Local => "local",
            Self::Github => "github",
            Self::Gitlab => "gitlab",
        }
    }
}
```

- [ ] **Step 4: Run all rupu-auth tests + gates**

```
cargo test -p rupu-auth
cargo fmt -p rupu-auth -- --check
cargo clippy -p rupu-auth --all-targets -- -D warnings
```

All green. (Existing tests must still pass — the variant addition is additive.)

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth/src/backend.rs
git commit -m "$(cat <<'EOF'
rupu-auth: add ProviderId::Github + ProviderId::Gitlab

Slice B-2 stores SCM credentials at rupu/github/<api-key|sso> and
rupu/gitlab/<api-key|sso> via the existing KeychainResolver.
Variants are additive; existing keychain layout for the LLM
providers is unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Add Github OAuth provider entry to `rupu-auth/oauth/providers.rs`

**Files:**
- Modify: `crates/rupu-auth/src/oauth/providers.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing tests module in `crates/rupu-auth/src/oauth/providers.rs`:

```rust
    #[test]
    fn github_provider_oauth_uses_device_flow_and_scm_scopes() {
        let c = provider_oauth(crate::backend::ProviderId::Github)
            .expect("github oauth config present");
        assert_eq!(c.flow, OAuthFlow::Device);
        assert_eq!(c.token_url, "https://github.com/login/oauth/access_token");
        assert_eq!(
            c.device_url,
            Some("https://github.com/login/device/code")
        );
        // SCM scopes; the existing Copilot entry has only `read:user`.
        for required in ["read:user", "repo", "workflow", "gist", "read:org"] {
            assert!(
                c.scopes.contains(&required),
                "scope {required} missing: {:?}",
                c.scopes
            );
        }
    }
```

- [ ] **Step 2: Run + verify failing**

```
cargo test -p rupu-auth --lib github_provider_oauth_uses_device_flow_and_scm_scopes
```

Expected: panic `github oauth config present` because `provider_oauth(Github)` returns `None`.

- [ ] **Step 3: Add the entry**

In `crates/rupu-auth/src/oauth/providers.rs`'s `provider_oauth` match, add a `ProviderId::Github` arm before `Local`:

```rust
        ProviderId::Github => Some(ProviderOAuth {
            flow: OAuthFlow::Device,
            // GitHub's published Copilot client_id; same client works
            // for repo/issue scopes too. We extend the scope set so
            // the same login covers both Copilot inference and SCM.
            client_id: "Iv1.b507a08c87ecfe98",
            authorize_url: "",
            token_url: "https://github.com/login/oauth/access_token",
            device_url: Some("https://github.com/login/device/code"),
            scopes: &["read:user", "repo", "workflow", "gist", "read:org"],
            redirect_path: "",
            redirect_host: "",
            fixed_ports: None,
            extra_authorize_params: &[],
            token_body_format: TokenBodyFormat::Form,
            state_is_verifier: false,
            include_state_in_token_body: false,
        }),
```

(`Gitlab` is added in Plan 2 with its OAuth flow.)

- [ ] **Step 4: Run, gates**

```
cargo test -p rupu-auth
cargo fmt -p rupu-auth -- --check
cargo clippy -p rupu-auth --all-targets -- -D warnings
```

All green.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth/src/oauth/providers.rs
git commit -m "$(cat <<'EOF'
rupu-auth: add Github OAuth provider entry

Device-code flow (mirrors Copilot's) with the SCM scope set:
read:user, repo, workflow, gist, read:org. Same client_id as
Copilot — one login covers both LLM inference and repo/issue
access. Gitlab's entry lands in Plan 2 with its browser-callback
PKCE flow.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — `rupu-config` schema additions

### Task 9: Add `ScmDefault`, `IssuesDefault`, `ScmPlatformConfig`

**Files:**
- Create: `crates/rupu-config/src/scm_config.rs`
- Modify: `crates/rupu-config/src/lib.rs`
- Modify: `crates/rupu-config/src/config.rs`
- Create: `crates/rupu-config/tests/scm_config.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-config/tests/scm_config.rs`:

```rust
use rupu_config::{Config, ScmPlatformConfig};

#[test]
fn scm_default_parses_with_owner_and_repo() {
    let toml = r#"
[scm.default]
platform = "github"
owner = "section9labs"
repo = "rupu"

[issues.default]
tracker = "github"
project = "section9labs/rupu"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let scm = cfg.scm.default.as_ref().expect("scm.default present");
    assert_eq!(scm.platform.as_deref(), Some("github"));
    assert_eq!(scm.owner.as_deref(), Some("section9labs"));
    assert_eq!(scm.repo.as_deref(), Some("rupu"));

    let iss = cfg.issues.default.as_ref().expect("issues.default present");
    assert_eq!(iss.tracker.as_deref(), Some("github"));
    assert_eq!(iss.project.as_deref(), Some("section9labs/rupu"));
}

#[test]
fn scm_platform_config_parses_per_platform_overrides() {
    let toml = r#"
[scm.github]
base_url = "https://ghe.example.com/api/v3"
timeout_ms = 30000
max_concurrency = 8
clone_protocol = "https"

[scm.gitlab]
base_url = "https://gitlab.example.com/api/v4"
clone_protocol = "ssh"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let gh = cfg.scm.platforms.get("github").expect("github platform");
    assert_eq!(gh.base_url.as_deref(), Some("https://ghe.example.com/api/v3"));
    assert_eq!(gh.timeout_ms, Some(30000));
    assert_eq!(gh.max_concurrency, Some(8));
    assert_eq!(gh.clone_protocol.as_deref(), Some("https"));

    let gl = cfg.scm.platforms.get("gitlab").expect("gitlab platform");
    assert_eq!(gl.clone_protocol.as_deref(), Some("ssh"));
}

#[test]
fn empty_scm_section_yields_default() {
    let cfg: Config = toml::from_str("").expect("parse empty");
    assert!(cfg.scm.default.is_none());
    assert!(cfg.scm.platforms.is_empty());
    assert!(cfg.issues.default.is_none());
}

#[test]
fn scm_platform_config_serialize_omits_none() {
    let mut p = ScmPlatformConfig::default();
    p.base_url = Some("https://x.test".into());
    let s = toml::to_string(&p).unwrap();
    assert!(s.contains("base_url = \"https://x.test\""));
    assert!(!s.contains("timeout_ms"));
}
```

- [ ] **Step 2: Run, verify failing**

```
cargo test -p rupu-config --test scm_config
```

Expected: compile errors — `Config` has no `scm` / `issues` field; types don't exist.

- [ ] **Step 3: Create `scm_config.rs`**

Create `crates/rupu-config/src/scm_config.rs`:

```rust
//! SCM and issue-tracker configuration. Spec §7c.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScmSection {
    pub default: Option<ScmDefault>,
    /// Per-platform overrides: `[scm.github]`, `[scm.gitlab]`.
    /// Keyed by lower-case platform name.
    #[serde(flatten, with = "platforms_serde")]
    pub platforms: BTreeMap<String, ScmPlatformConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssuesSection {
    pub default: Option<IssuesDefault>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmDefault {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuesDefault {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScmPlatformConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
    /// "https" or "ssh"; default chosen by the connector at clone time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_protocol: Option<String>,
}

mod platforms_serde {
    //! Serialize/deserialize `BTreeMap<String, ScmPlatformConfig>` as
    //! flattened sub-tables, but EXCLUDING the reserved `default` key
    //! (which is its own typed field on `ScmSection`).
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::ScmPlatformConfig;

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<String, ScmPlatformConfig>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        map.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, ScmPlatformConfig>, D::Error> {
        let mut raw: BTreeMap<String, ScmPlatformConfig> = BTreeMap::deserialize(d)?;
        // Drop the reserved key if it slipped through (it's typed
        // separately on ScmSection.default).
        raw.remove("default");
        Ok(raw)
    }
}
```

- [ ] **Step 4: Wire into `Config` + `lib.rs`**

In `crates/rupu-config/src/config.rs`, add at the top of the file (with the other `use` statements):

```rust
use crate::scm_config::{IssuesSection, ScmSection};
```

Then add to the `Config` struct (preserving every existing field):

```rust
    #[serde(default)]
    pub scm: ScmSection,
    #[serde(default)]
    pub issues: IssuesSection,
```

In `crates/rupu-config/src/lib.rs` add (after the existing `pub use provider_config::...;`):

```rust
pub mod scm_config;
pub use scm_config::{
    IssuesDefault, IssuesSection, ScmDefault, ScmPlatformConfig, ScmSection,
};
```

- [ ] **Step 5: Run tests + gates**

```
cargo test -p rupu-config
cargo fmt -p rupu-config -- --check
cargo clippy -p rupu-config --all-targets -- -D warnings
```

All four `scm_config` tests pass; existing `provider_config` tests still pass.

**Note**: `Config` was previously `#[serde(deny_unknown_fields)]`. Adding `scm` and `issues` keeps `deny_unknown_fields` working — they're now known fields.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-config/src/scm_config.rs crates/rupu-config/src/lib.rs crates/rupu-config/src/config.rs crates/rupu-config/tests/scm_config.rs
git commit -m "$(cat <<'EOF'
rupu-config: add scm + issues sections

Adds [scm.default], [issues.default], and per-platform
[scm.<platform>] tables per the B-2 spec §7c. Existing
deny_unknown_fields on Config keeps working — both new fields are
now recognized.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Registry

### Task 10: `Registry::discover` (skeleton — no platforms wired yet)

**Files:**
- Modify: `crates/rupu-scm/src/registry.rs`
- Create: `crates/rupu-scm/tests/registry_discover.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-scm/tests/registry_discover.rs`:

```rust
use rupu_scm::{IssueTracker, Platform, Registry};

#[tokio::test]
async fn empty_resolver_yields_no_connectors() {
    use rupu_auth::in_memory::InMemoryResolver;
    let resolver = InMemoryResolver::new();
    let cfg = rupu_config::Config::default();
    let r = Registry::discover(&resolver, &cfg).await;
    assert!(r.repo(Platform::Github).is_none());
    assert!(r.repo(Platform::Gitlab).is_none());
    assert!(r.issues(IssueTracker::Github).is_none());
    assert!(r.issues(IssueTracker::Gitlab).is_none());
}
```

- [ ] **Step 2: Implement the registry skeleton**

Replace `crates/rupu-scm/src/registry.rs` with:

```rust
//! Registry — builds connectors from configured credentials.
//!
//! Spec §4d. `discover()` instantiates one [`RepoConnector`] +
//! [`IssueConnector`] per platform that has stored credentials,
//! skipping platforms the user hasn't authenticated to (logged at
//! INFO).

use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::{IssueConnector, RepoConnector};
use crate::platform::{IssueTracker, Platform};

#[derive(Default)]
pub struct Registry {
    repo_connectors: HashMap<Platform, Arc<dyn RepoConnector>>,
    issue_connectors: HashMap<IssueTracker, Arc<dyn IssueConnector>>,
}

impl Registry {
    /// Build a Registry from configured credentials. Platforms the
    /// user hasn't logged into are skipped silently (INFO log).
    pub async fn discover(resolver: &dyn CredentialResolver, cfg: &Config) -> Self {
        let mut reg = Self::default();
        // GitHub: try to build a connector if credentials present.
        match crate::connectors::github::try_build(resolver, cfg).await {
            Ok(Some((repo, issues))) => {
                reg.repo_connectors.insert(Platform::Github, repo);
                reg.issue_connectors.insert(IssueTracker::Github, issues);
            }
            Ok(None) => {
                info!("github: no credentials configured; skipping connector");
            }
            Err(e) => {
                tracing::warn!(error = %e, "github: connector build failed; skipping");
            }
        }
        // Gitlab is wired in Plan 2.
        reg
    }

    pub fn repo(&self, platform: Platform) -> Option<Arc<dyn RepoConnector>> {
        self.repo_connectors.get(&platform).cloned()
    }

    pub fn issues(&self, tracker: IssueTracker) -> Option<Arc<dyn IssueConnector>> {
        self.issue_connectors.get(&tracker).cloned()
    }
}
```

- [ ] **Step 3: Add the github stub `try_build`**

In `crates/rupu-scm/src/connectors/github/mod.rs`, replace with:

```rust
//! GitHub connectors.

use std::sync::Arc;

use anyhow::Result;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::{IssueConnector, RepoConnector};

/// Try to build the GitHub Repo + Issue connectors from configured
/// credentials. Returns `Ok(None)` when no GitHub credential is
/// stored — that's a normal "user hasn't logged in" case, not an
/// error. Real implementation lands in Tasks 11-12; for now this
/// always returns `Ok(None)`.
pub async fn try_build(
    _resolver: &dyn CredentialResolver,
    _cfg: &Config,
) -> Result<Option<(Arc<dyn RepoConnector>, Arc<dyn IssueConnector>)>> {
    Ok(None)
}
```

- [ ] **Step 4: Run, gates**

```
cargo test -p rupu-scm --test registry_discover
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Test passes; gates clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-scm/src/registry.rs crates/rupu-scm/src/connectors/github/mod.rs crates/rupu-scm/tests/registry_discover.rs
git commit -m "$(cat <<'EOF'
rupu-scm: add Registry skeleton + github::try_build stub

Registry::discover builds connectors from configured credentials,
skipping platforms the user hasn't authenticated to. The github
sub-module currently returns Ok(None) (no credentials path); real
construction lands in Tasks 11-12.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — GitHub connector implementation

### Task 11: `GithubClient` — auth + retry + ETag wrapper

**Files:**
- Create: `crates/rupu-scm/src/connectors/github/client.rs`
- Modify: `crates/rupu-scm/src/connectors/github/mod.rs`

- [ ] **Step 1: Write the file**

Create `crates/rupu-scm/src/connectors/github/client.rs`:

```rust
//! Internal HTTP client for the GitHub adapter.
//!
//! Wraps `octocrab` with:
//!   - per-platform Semaphore (shared with other platforms via
//!     `rupu_providers::concurrency::semaphore_for("github", _)`)
//!   - in-memory LRU ETag cache for `get_*` responses
//!   - retry-with-backoff for RateLimited / Transient (Plan 1's
//!     ProviderError-style table, but classified via classify_scm_error)
//!   - hardened error mapping at the boundary

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use octocrab::Octocrab;
use rupu_providers::concurrency;
use tokio::sync::Semaphore;

use crate::error::{classify_scm_error, ScmError};
use crate::platform::Platform;

const CACHE_CAP: usize = 256;
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 5;

#[derive(Clone)]
pub struct GithubClient {
    pub(crate) inner: Octocrab,
    pub(crate) token: String,
    semaphore: Arc<Semaphore>,
    cache: Arc<Mutex<LruCache<String, CacheEntry>>>,
}

struct CacheEntry {
    etag: String,
    body: serde_json::Value,
    inserted_at: Instant,
}

impl GithubClient {
    pub fn new(token: String, base_url: Option<String>, max_concurrency: Option<usize>) -> Self {
        let mut builder = Octocrab::builder().personal_token(token.clone());
        if let Some(url) = base_url {
            builder = builder.base_uri(url).expect("valid base_url");
        }
        let inner = builder.build().expect("octocrab builder");
        let semaphore = concurrency::semaphore_for("github", max_concurrency);
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(CACHE_CAP).unwrap(),
        )));
        Self {
            inner,
            token,
            semaphore,
            cache,
        }
    }

    /// Acquire a permit from the per-platform semaphore.
    pub async fn permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("github semaphore closed")
    }

    /// Cache lookup for a `get_*` style URL key. Returns the cached
    /// JSON value if fresh AND the ETag was reused on a 304.
    pub fn cache_get(&self, key: &str) -> Option<(String, serde_json::Value)> {
        let mut guard = self.cache.lock().ok()?;
        let entry = guard.get(key)?;
        if entry.inserted_at.elapsed() > CACHE_TTL {
            return None;
        }
        Some((entry.etag.clone(), entry.body.clone()))
    }

    pub fn cache_put(&self, key: String, etag: String, body: serde_json::Value) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.put(
                key,
                CacheEntry {
                    etag,
                    body,
                    inserted_at: Instant::now(),
                },
            );
        }
    }

    /// Run `f` with retry-with-backoff. Recoverable RateLimited /
    /// Transient errors are retried up to MAX_RETRIES with exponential
    /// jitter (cap 60s). Unrecoverable errors abort immediately.
    pub async fn with_retry<F, Fut, T>(&self, mut f: F) -> Result<T, ScmError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ScmError>>,
    {
        let mut attempt: u32 = 0;
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let is_retryable = matches!(
                        &e,
                        ScmError::RateLimited { .. } | ScmError::Transient(_)
                    );
                    if !is_retryable || attempt >= MAX_RETRIES {
                        return Err(e);
                    }
                    let delay = match &e {
                        ScmError::RateLimited {
                            retry_after: Some(d),
                        } => *d,
                        _ => backoff(attempt),
                    };
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
            }
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    let base = 2u64.saturating_pow(attempt).min(60);
    let jitter_ms: u64 = (rand::random::<u8>() as u64) % 500;
    Duration::from_millis(base * 1000 + jitter_ms)
}

/// Classify an octocrab error into the rupu ScmError vocabulary.
pub fn classify_octocrab_error(err: octocrab::Error) -> ScmError {
    use octocrab::Error as OE;
    match err {
        OE::GitHub { source, .. } => {
            // octocrab's GitHubError carries status + message; we don't
            // get headers easily, so missing-scope can't be detected
            // here. Fall back to status-only classification.
            let status = source.status_code.as_u16();
            classify_scm_error(Platform::Github, status, &source.message, &Default::default())
        }
        OE::Hyper { .. } | OE::Service { .. } => {
            ScmError::Network(anyhow::anyhow!("github transport: {err}"))
        }
        other => ScmError::Transient(anyhow::anyhow!("github: {other}")),
    }
}
```

- [ ] **Step 2: Add `rand` to `rupu-scm` deps**

In `crates/rupu-scm/Cargo.toml`, add to `[dependencies]`:

```toml
rand = { workspace = true }
```

- [ ] **Step 3: Wire the client into `connectors/github/mod.rs`**

Replace `crates/rupu-scm/src/connectors/github/mod.rs` with:

```rust
//! GitHub connectors.

use std::sync::Arc;

use anyhow::Result;

use rupu_auth::CredentialResolver;
use rupu_config::Config;

use crate::connectors::{IssueConnector, RepoConnector};

mod client;
mod issues;
mod repo;

pub use client::{classify_octocrab_error, GithubClient};

/// Try to build the GitHub Repo + Issue connectors from configured
/// credentials. Returns `Ok(None)` when no GitHub credential is
/// stored — that's a normal "user hasn't logged in" case.
pub async fn try_build(
    resolver: &dyn CredentialResolver,
    cfg: &Config,
) -> Result<Option<(Arc<dyn RepoConnector>, Arc<dyn IssueConnector>)>> {
    let creds = match resolver.get("github", None).await {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let token = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
    };
    let platform_cfg = cfg.scm.platforms.get("github");
    let base_url = platform_cfg.and_then(|p| p.base_url.clone());
    let max_conc = platform_cfg.and_then(|p| p.max_concurrency);
    let client = GithubClient::new(token, base_url, max_conc);
    let repo: Arc<dyn RepoConnector> = Arc::new(repo::GithubRepoConnector::new(client.clone()));
    let issues: Arc<dyn IssueConnector> = Arc::new(issues::GithubIssueConnector::new(client));
    Ok(Some((repo, issues)))
}
```

- [ ] **Step 4: Add stub repo + issues files (filled in Task 12)**

Create `crates/rupu-scm/src/connectors/github/repo.rs`:

```rust
//! GitHub RepoConnector. Implementation lands in Task 12.

use async_trait::async_trait;

use crate::connectors::RepoConnector;
use crate::error::ScmError;
use crate::platform::Platform;
use crate::types::{
    Branch, Comment, CreatePr, Diff, FileContent, Pr, PrFilter, PrRef, Repo, RepoRef,
};

use super::client::GithubClient;

pub struct GithubRepoConnector {
    client: GithubClient,
}

impl GithubRepoConnector {
    pub fn new(client: GithubClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl RepoConnector for GithubRepoConnector {
    fn platform(&self) -> Platform {
        Platform::Github
    }

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        let _ = &self.client;
        Err(ScmError::Transient(anyhow::anyhow!(
            "github::list_repos not yet implemented (Task 12)"
        )))
    }

    async fn get_repo(&self, _r: &RepoRef) -> Result<Repo, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn read_file(
        &self,
        _r: &RepoRef,
        _path: &str,
        _ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn clone_to(&self, _r: &RepoRef, _dir: &std::path::Path) -> Result<(), ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }
}
```

Create `crates/rupu-scm/src/connectors/github/issues.rs`:

```rust
//! GitHub IssueConnector. Implementation lands in Task 12.

use async_trait::async_trait;

use crate::connectors::IssueConnector;
use crate::error::ScmError;
use crate::platform::IssueTracker;
use crate::types::{Comment, CreateIssue, Issue, IssueFilter, IssueRef, IssueState};

use super::client::GithubClient;

pub struct GithubIssueConnector {
    client: GithubClient,
}

impl GithubIssueConnector {
    pub fn new(client: GithubClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl IssueConnector for GithubIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Github
    }

    async fn list_issues(
        &self,
        _project: &str,
        _filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let _ = &self.client;
        Err(ScmError::Transient(anyhow::anyhow!(
            "github::list_issues not yet implemented (Task 12)"
        )))
    }

    async fn get_issue(&self, _i: &IssueRef) -> Result<Issue, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn comment_issue(&self, _i: &IssueRef, _body: &str) -> Result<Comment, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn create_issue(
        &self,
        _project: &str,
        _opts: CreateIssue,
    ) -> Result<Issue, ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }

    async fn update_issue_state(
        &self,
        _i: &IssueRef,
        _state: IssueState,
    ) -> Result<(), ScmError> {
        Err(ScmError::Transient(anyhow::anyhow!("not yet implemented")))
    }
}
```

- [ ] **Step 5: Add `rand` to workspace deps if missing + verify build**

Check `Cargo.toml`'s `[workspace.dependencies]`. If `rand = "0.8"` is present (added by Plan 2 Task 1), do nothing. If not, add it.

```
cargo build -p rupu-scm 2>&1 | tail -10
```

Expected: clean build. Some methods return `not yet implemented` errors at runtime; that's fine — Task 12 fills them in.

- [ ] **Step 6: Verify gates + connector wiring smoke test**

Add a small integration test that proves `Registry::discover` picks up a github connector when credentials are present. Append to `crates/rupu-scm/tests/registry_discover.rs`:

```rust
#[tokio::test]
async fn github_connector_built_when_credential_present() {
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;

    let resolver = InMemoryResolver::new();
    resolver
        .put(
            ProviderId::Github,
            AuthMode::ApiKey,
            StoredCredential::api_key("ghp_test"),
        )
        .await;
    let cfg = rupu_config::Config::default();
    let r = Registry::discover(&resolver, &cfg).await;
    assert!(r.repo(Platform::Github).is_some());
    assert!(r.issues(IssueTracker::Github).is_some());
}
```

```
cargo test -p rupu-scm --test registry_discover
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

All green.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock \
        crates/rupu-scm/Cargo.toml \
        crates/rupu-scm/src/connectors/github/ \
        crates/rupu-scm/tests/registry_discover.rs
git commit -m "$(cat <<'EOF'
rupu-scm: add GithubClient + connector skeleton

GithubClient wraps octocrab with the per-platform semaphore (shared
via rupu_providers::concurrency::semaphore_for("github", _)), an
LRU ETag cache, and a retry-with-backoff helper. GithubRepoConnector
and GithubIssueConnector instantiate the client and implement the
trait method shells; bodies return `not yet implemented` until
Task 12 wires them up. Registry::discover picks up the github
connectors when an ApiKey credential is present.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: GitHub adapter — `list_repos`, `get_repo`, `read_file`, PR + issue read paths

**Files:**
- Modify: `crates/rupu-scm/src/connectors/github/repo.rs`
- Modify: `crates/rupu-scm/src/connectors/github/issues.rs`
- Create: `crates/rupu-scm/tests/fixtures/github/*.json`
- Create: `crates/rupu-scm/tests/github_translation.rs`

This is a multi-method task. Decompose into 5 subtasks, one per method group, each with its own test and commit.

#### 12a — `list_repos`

- [ ] **Step 1: Add the fixture**

Create `crates/rupu-scm/tests/fixtures/github/repos_list_happy.json`:

```json
[
  {
    "id": 1,
    "name": "rupu",
    "full_name": "section9labs/rupu",
    "owner": { "login": "section9labs", "id": 100, "type": "Organization" },
    "private": true,
    "default_branch": "main",
    "clone_url": "https://github.com/section9labs/rupu.git",
    "ssh_url": "git@github.com:section9labs/rupu.git",
    "description": "agentic coding CLI"
  },
  {
    "id": 2,
    "name": "okesu",
    "full_name": "section9labs/okesu",
    "owner": { "login": "section9labs", "id": 100, "type": "Organization" },
    "private": true,
    "default_branch": "main",
    "clone_url": "https://github.com/section9labs/okesu.git",
    "ssh_url": "git@github.com:section9labs/okesu.git",
    "description": null
  }
]
```

- [ ] **Step 2: Write the integration test**

Create `crates/rupu-scm/tests/github_translation.rs`:

```rust
use httpmock::prelude::*;
use rupu_scm::{Platform, RepoConnector};

mod common;

#[tokio::test]
async fn list_repos_translates_octocrab_response() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/repos_list_happy.json").unwrap();
    let m = server.mock(|when, then| {
        when.method(GET).path("/user/repos");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });

    let connector = common::github_connector_against(&server);
    let repos = connector.list_repos().await.expect("list_repos");
    m.assert();

    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].r.platform, Platform::Github);
    assert_eq!(repos[0].r.owner, "section9labs");
    assert_eq!(repos[0].r.repo, "rupu");
    assert_eq!(repos[0].default_branch, "main");
    assert!(repos[0].private);
    assert_eq!(repos[0].clone_url_https, "https://github.com/section9labs/rupu.git");
    assert_eq!(repos[0].description.as_deref(), Some("agentic coding CLI"));
    assert_eq!(repos[1].description, None);
}
```

Create `crates/rupu-scm/tests/common/mod.rs`:

```rust
//! Shared test helpers.

use std::sync::Arc;

use httpmock::MockServer;
use rupu_scm::{IssueConnector, RepoConnector};

/// Build a GitHub `RepoConnector` whose API base points at `server`.
pub fn github_connector_against(server: &MockServer) -> Arc<dyn RepoConnector> {
    use rupu_scm::connectors::github::GithubClient;
    let client = GithubClient::new(
        "ghp_test".into(),
        Some(server.base_url()),
        Some(2),
    );
    Arc::new(rupu_scm::connectors::github::repo::GithubRepoConnector::new(
        client,
    ))
}

/// Build a GitHub `IssueConnector` whose API base points at `server`.
pub fn github_issue_connector_against(server: &MockServer) -> Arc<dyn IssueConnector> {
    use rupu_scm::connectors::github::GithubClient;
    let client = GithubClient::new(
        "ghp_test".into(),
        Some(server.base_url()),
        Some(2),
    );
    Arc::new(rupu_scm::connectors::github::issues::GithubIssueConnector::new(client))
}
```

(Re-exports needed: `GithubRepoConnector` and `GithubIssueConnector` should be `pub` at module path `rupu_scm::connectors::github::repo::...`. The shared helper uses fully qualified paths to keep the module surface tight.)

- [ ] **Step 3: Implement `list_repos`**

Replace the `list_repos` body in `crates/rupu-scm/src/connectors/github/repo.rs` with:

```rust
    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        self.client
            .with_retry(|| {
                let inner = inner.clone();
                async move {
                    let pages = inner
                        .current()
                        .list_repos_for_authenticated_user()
                        .per_page(100)
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    let mut all: Vec<octocrab::models::Repository> =
                        pages.items.into_iter().collect();
                    let mut next = pages.next;
                    while let Some(page_url) = next {
                        let page: octocrab::Page<octocrab::models::Repository> =
                            inner.get_page(&Some(page_url)).await
                                .map_err(super::client::classify_octocrab_error)?
                                .ok_or_else(|| ScmError::Transient(anyhow::anyhow!(
                                    "next page disappeared"
                                )))?;
                        all.extend(page.items);
                        next = page.next;
                    }
                    Ok(all
                        .into_iter()
                        .filter_map(repo_from_octocrab)
                        .collect::<Vec<_>>())
                }
            })
            .await
    }
```

Add the helper at the bottom of `repo.rs`:

```rust
fn repo_from_octocrab(r: octocrab::models::Repository) -> Option<Repo> {
    let full = r.full_name?;
    let (owner, name) = full.split_once('/')?;
    Some(Repo {
        r: RepoRef {
            platform: Platform::Github,
            owner: owner.to_string(),
            repo: name.to_string(),
        },
        default_branch: r.default_branch.unwrap_or_else(|| "main".into()),
        clone_url_https: r
            .clone_url
            .map(|u| u.to_string())
            .unwrap_or_default(),
        clone_url_ssh: r.ssh_url.unwrap_or_default(),
        private: r.private.unwrap_or(false),
        description: r.description,
    })
}
```

- [ ] **Step 4: Run, gates**

```
cargo test -p rupu-scm --test github_translation
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

`list_repos_translates_octocrab_response` passes.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-scm/src/connectors/github/repo.rs \
        crates/rupu-scm/tests/fixtures/github/repos_list_happy.json \
        crates/rupu-scm/tests/github_translation.rs \
        crates/rupu-scm/tests/common/mod.rs
git commit -m "$(cat <<'EOF'
rupu-scm: github list_repos via octocrab + auto-pagination

Walks all pages of /user/repos via octocrab's Page next-link, maps
each Repository into the rupu Repo type. Recorded fixture pins the
translation contract so a future octocrab field-name change surfaces
in CI rather than at runtime.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

#### 12b — `get_repo` + `get_pr` + `diff_pr` + `read_file` (read-only methods)

- [ ] **Step 1: Add fixtures**

Create the following files under `crates/rupu-scm/tests/fixtures/github/`:

`repo_get_happy.json`:
```json
{
  "id": 1,
  "name": "rupu",
  "full_name": "section9labs/rupu",
  "owner": { "login": "section9labs", "id": 100, "type": "Organization" },
  "private": true,
  "default_branch": "main",
  "clone_url": "https://github.com/section9labs/rupu.git",
  "ssh_url": "git@github.com:section9labs/rupu.git",
  "description": "agentic coding CLI"
}
```

`pr_get_happy.json`:
```json
{
  "id": 11,
  "number": 42,
  "state": "open",
  "title": "fix: streaming tokens",
  "body": "Fixes the runner to stream TextDelta to stdout.",
  "head": { "ref": "feat/stream", "sha": "deadbeef" },
  "base": { "ref": "main", "sha": "0000" },
  "user": { "login": "matias", "id": 1 },
  "created_at": "2026-05-03T10:00:00Z",
  "updated_at": "2026-05-03T11:00:00Z",
  "merged_at": null,
  "draft": false
}
```

`pr_diff_happy.patch`:
```
diff --git a/src/main.rs b/src/main.rs
index e69de29..d800886 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -0,0 +1 @@
+fn main() { println!("hi"); }
```

`file_get_happy.json`:
```json
{
  "type": "file",
  "encoding": "base64",
  "size": 30,
  "name": "README.md",
  "path": "README.md",
  "content": "IyBoZWxsbwo=",
  "sha": "abc123",
  "url": "https://api.github.com/...",
  "git_url": "https://api.github.com/...",
  "html_url": "https://github.com/...",
  "download_url": null,
  "_links": { "git": "", "self": "", "html": "" }
}
```

- [ ] **Step 2: Append the tests**

Append to `crates/rupu-scm/tests/github_translation.rs`:

```rust
#[tokio::test]
async fn get_repo_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/repo_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu");
        then.status(200).header("content-type", "application/json").body(&body);
    });
    let c = common::github_connector_against(&server);
    let r = c
        .get_repo(&rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        })
        .await
        .unwrap();
    assert_eq!(r.r.repo, "rupu");
    assert!(r.private);
    assert_eq!(r.default_branch, "main");
}

#[tokio::test]
async fn get_pr_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/pr_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/pulls/42");
        then.status(200).header("content-type", "application/json").body(&body);
    });
    let c = common::github_connector_against(&server);
    let p = c
        .get_pr(&rupu_scm::PrRef {
            repo: rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            number: 42,
        })
        .await
        .unwrap();
    assert_eq!(p.title, "fix: streaming tokens");
    assert_eq!(p.head_branch, "feat/stream");
    assert_eq!(p.base_branch, "main");
    assert_eq!(p.author, "matias");
}

#[tokio::test]
async fn diff_pr_returns_unified_diff() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/pr_diff_happy.patch").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/pulls/42")
            .header("accept", "application/vnd.github.v3.diff");
        then.status(200).body(&body);
    });
    let c = common::github_connector_against(&server);
    let d = c
        .diff_pr(&rupu_scm::PrRef {
            repo: rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            number: 42,
        })
        .await
        .unwrap();
    assert!(d.patch.contains("diff --git a/src/main.rs b/src/main.rs"));
    assert_eq!(d.files_changed, 1);
}

#[tokio::test]
async fn read_file_decodes_base64() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/file_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/contents/README.md");
        then.status(200).header("content-type", "application/json").body(&body);
    });
    let c = common::github_connector_against(&server);
    let f = c
        .read_file(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            "README.md",
            None,
        )
        .await
        .unwrap();
    assert_eq!(f.path, "README.md");
    assert_eq!(f.encoding, rupu_scm::types::FileEncoding::Utf8);
    assert_eq!(f.content, "# hello\n");
}
```

- [ ] **Step 3: Implement the four methods**

Replace the four method bodies in `crates/rupu-scm/src/connectors/github/repo.rs` with these implementations (other methods stay as `not yet implemented`):

```rust
    async fn get_repo(&self, r: &RepoRef) -> Result<Repo, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .repos(&owner, &repo)
                        .get()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        repo_from_octocrab(model).ok_or_else(|| {
            ScmError::Transient(anyhow::anyhow!("malformed repo response from github"))
        })
    }

    async fn get_pr(&self, p: &PrRef) -> Result<Pr, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = p.repo.owner.clone();
        let repo = p.repo.repo.clone();
        let number = p.number;
        let pr = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .pulls(&owner, &repo)
                        .get(number as u64)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(pr_from_octocrab(p.repo.clone(), pr))
    }

    async fn diff_pr(&self, p: &PrRef) -> Result<Diff, ScmError> {
        let _permit = self.client.permit().await;
        // octocrab doesn't expose the .diff media type cleanly; do a raw
        // request via its underlying http client.
        let url = format!(
            "/repos/{}/{}/pulls/{}",
            p.repo.owner, p.repo.repo, p.number
        );
        let inner = self.client.inner.clone();
        let response = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let url = url.clone();
                async move {
                    let req = inner
                        ._get(url)
                        .await
                        .map_err(super::client::classify_octocrab_error)?;
                    // Re-issue with the diff Accept header (octocrab's
                    // raw client supports this).
                    let req = req;
                    let body_bytes = hyper::body::to_bytes(req.into_body())
                        .await
                        .map_err(|e| ScmError::Transient(anyhow::anyhow!("body: {e}")))?;
                    let patch = String::from_utf8_lossy(&body_bytes).into_owned();
                    Ok(patch)
                }
            })
            .await?;
        let files_changed = response
            .lines()
            .filter(|l| l.starts_with("diff --git "))
            .count() as u32;
        let additions = response.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count() as u32;
        let deletions = response.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count() as u32;
        Ok(Diff {
            patch: response,
            files_changed,
            additions,
            deletions,
        })
    }

    async fn read_file(
        &self,
        r: &RepoRef,
        path: &str,
        ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let path_owned = path.to_string();
        let ref_owned = ref_.map(|s| s.to_string());
        let item = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let path = path_owned.clone();
                let ref_ = ref_owned.clone();
                async move {
                    let mut builder = inner.repos(&owner, &repo).get_content().path(path);
                    if let Some(r) = ref_ {
                        builder = builder.r#ref(r);
                    }
                    builder
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        let first = item
            .items
            .into_iter()
            .next()
            .ok_or_else(|| ScmError::NotFound { what: path.into() })?;
        // octocrab returns base64 with line breaks; strip and decode.
        let raw = first.content.unwrap_or_default().replace('\n', "");
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw.as_bytes())
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("base64: {e}")))?;
        let content = String::from_utf8(decoded)
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("utf8: {e}")))?;
        Ok(FileContent {
            path: first.path,
            ref_: ref_.unwrap_or("HEAD").to_string(),
            content,
            encoding: crate::types::FileEncoding::Utf8,
        })
    }
```

Add `pr_from_octocrab` helper at the bottom of `repo.rs`:

```rust
fn pr_from_octocrab(repo: RepoRef, pr: octocrab::models::pulls::PullRequest) -> Pr {
    use crate::types::PrState;
    Pr {
        r: PrRef {
            repo,
            number: pr.number as u32,
        },
        title: pr.title.unwrap_or_default(),
        body: pr.body.unwrap_or_default(),
        state: match pr.state {
            Some(octocrab::models::IssueState::Open) => PrState::Open,
            _ if pr.merged_at.is_some() => PrState::Merged,
            _ => PrState::Closed,
        },
        head_branch: pr.head.ref_field,
        base_branch: pr.base.ref_field,
        author: pr.user.map(|u| u.login).unwrap_or_default(),
        created_at: pr.created_at.unwrap_or_else(chrono::Utc::now),
        updated_at: pr.updated_at.unwrap_or_else(chrono::Utc::now),
    }
}
```

Add `base64` and `hyper` to `crates/rupu-scm/Cargo.toml`'s `[dependencies]`:

```toml
base64 = { workspace = true }
hyper = { workspace = true }
```

(Both are already in workspace deps via Plan 1 / B-1 lift.)

- [ ] **Step 4: Run + gates**

```
cargo test -p rupu-scm --test github_translation
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

All four read-method tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-scm/Cargo.toml \
        crates/rupu-scm/src/connectors/github/repo.rs \
        crates/rupu-scm/tests/fixtures/github/{repo_get_happy,pr_get_happy,file_get_happy}.json \
        crates/rupu-scm/tests/fixtures/github/pr_diff_happy.patch \
        crates/rupu-scm/tests/github_translation.rs
git commit -m "$(cat <<'EOF'
rupu-scm: github get_repo + get_pr + diff_pr + read_file

Read paths use octocrab's typed APIs (`repos.get`, `pulls.get`,
`get_content`) plus a raw GET for the diff media type. Recorded
fixtures pin the translation contract; httpmock serves them at the
test boundary.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

#### 12c — `list_prs` + `list_branches`

- [ ] **Step 1: Add fixtures**

`prs_list_happy.json` (single page, two PRs):

```json
[
  {
    "id": 11,
    "number": 42,
    "state": "open",
    "title": "fix: streaming tokens",
    "body": "stream TextDelta",
    "head": { "ref": "feat/stream", "sha": "abc" },
    "base": { "ref": "main", "sha": "000" },
    "user": { "login": "matias", "id": 1 },
    "created_at": "2026-05-03T10:00:00Z",
    "updated_at": "2026-05-03T11:00:00Z",
    "merged_at": null,
    "draft": false
  },
  {
    "id": 12,
    "number": 43,
    "state": "closed",
    "title": "chore: bump deps",
    "body": "",
    "head": { "ref": "chore/bump", "sha": "def" },
    "base": { "ref": "main", "sha": "000" },
    "user": { "login": "matias", "id": 1 },
    "created_at": "2026-05-02T10:00:00Z",
    "updated_at": "2026-05-02T11:00:00Z",
    "merged_at": "2026-05-02T11:30:00Z",
    "draft": false
  }
]
```

`branches_list_happy.json`:

```json
[
  {"name": "main", "commit": {"sha": "abc123", "url": ""}, "protected": true},
  {"name": "feat/stream", "commit": {"sha": "def456", "url": ""}, "protected": false}
]
```

- [ ] **Step 2: Append tests**

Append to `tests/github_translation.rs`:

```rust
#[tokio::test]
async fn list_prs_paginates_and_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/prs_list_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/pulls");
        then.status(200).header("content-type", "application/json").body(&body);
    });
    let c = common::github_connector_against(&server);
    let prs = c
        .list_prs(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            rupu_scm::PrFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(prs.len(), 2);
    assert_eq!(prs[0].state, rupu_scm::PrState::Open);
    assert_eq!(prs[1].state, rupu_scm::PrState::Merged);
}

#[tokio::test]
async fn list_branches_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/branches_list_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/branches");
        then.status(200).header("content-type", "application/json").body(&body);
    });
    let c = common::github_connector_against(&server);
    let bs = c
        .list_branches(&rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        })
        .await
        .unwrap();
    assert_eq!(bs.len(), 2);
    assert_eq!(bs[0].name, "main");
    assert!(bs[0].protected);
    assert_eq!(bs[1].name, "feat/stream");
    assert!(!bs[1].protected);
}
```

- [ ] **Step 3: Implement**

Replace the `list_prs` and `list_branches` bodies in `repo.rs`:

```rust
    async fn list_branches(&self, r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let pages = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .repos(&owner, &repo)
                        .list_branches()
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(pages
            .items
            .into_iter()
            .map(|b| Branch {
                name: b.name,
                sha: b.commit.sha,
                protected: b.protected,
            })
            .collect())
    }

    async fn list_prs(&self, r: &RepoRef, filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let state_filter = filter.state;
        let pages = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    let mut req = inner.pulls(&owner, &repo).list();
                    if let Some(s) = state_filter {
                        req = req.state(match s {
                            crate::types::PrState::Open => octocrab::params::State::Open,
                            crate::types::PrState::Closed | crate::types::PrState::Merged => {
                                octocrab::params::State::Closed
                            }
                        });
                    }
                    req.send().await.map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        let repo_ref = r.clone();
        Ok(pages
            .items
            .into_iter()
            .map(|p| pr_from_octocrab(repo_ref.clone(), p))
            .collect())
    }
```

- [ ] **Step 4: Run + commit**

```
cargo test -p rupu-scm --test github_translation
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Both new tests pass.

```bash
git add crates/rupu-scm/src/connectors/github/repo.rs \
        crates/rupu-scm/tests/fixtures/github/{prs_list_happy,branches_list_happy}.json \
        crates/rupu-scm/tests/github_translation.rs
git commit -m "$(cat <<'EOF'
rupu-scm: github list_prs + list_branches

list_prs honors the optional PrFilter.state. list_branches surfaces
the protected flag from GitHub's response so agents know which
branches they can't push to without admin permission.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

#### 12d — Issue read methods (`get_issue`, `list_issues`)

- [ ] **Step 1: Add fixtures**

`issue_get_happy.json`:
```json
{
  "id": 99,
  "number": 123,
  "state": "open",
  "title": "Investigate flaky test",
  "body": "Sometimes fails on CI macos-13.",
  "labels": [
    { "name": "bug", "color": "red" },
    { "name": "ci", "color": "blue" }
  ],
  "user": { "login": "matias", "id": 1 },
  "created_at": "2026-05-01T09:00:00Z",
  "updated_at": "2026-05-02T09:00:00Z"
}
```

`issues_list_happy.json`:
```json
[
  {
    "id": 99,
    "number": 123,
    "state": "open",
    "title": "Investigate flaky test",
    "body": "...",
    "labels": [{"name": "bug", "color": "red"}],
    "user": { "login": "matias", "id": 1 },
    "created_at": "2026-05-01T09:00:00Z",
    "updated_at": "2026-05-02T09:00:00Z"
  }
]
```

- [ ] **Step 2: Append tests**

Append to `tests/github_translation.rs`:

```rust
#[tokio::test]
async fn get_issue_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/issue_get_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/issues/123");
        then.status(200).header("content-type", "application/json").body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let i = c
        .get_issue(&rupu_scm::IssueRef {
            tracker: rupu_scm::IssueTracker::Github,
            project: "section9labs/rupu".into(),
            number: 123,
        })
        .await
        .unwrap();
    assert_eq!(i.title, "Investigate flaky test");
    assert_eq!(i.state, rupu_scm::IssueState::Open);
    assert_eq!(i.labels, vec!["bug".to_string(), "ci".to_string()]);
}

#[tokio::test]
async fn list_issues_translates() {
    let server = MockServer::start();
    let body = std::fs::read_to_string("tests/fixtures/github/issues_list_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET).path("/repos/section9labs/rupu/issues");
        then.status(200).header("content-type", "application/json").body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let items = c
        .list_issues("section9labs/rupu", rupu_scm::IssueFilter::default())
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].labels, vec!["bug".to_string()]);
}
```

- [ ] **Step 3: Implement**

Replace `get_issue` and `list_issues` in `crates/rupu-scm/src/connectors/github/issues.rs`:

```rust
    async fn list_issues(
        &self,
        project: &str,
        filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(project)?;
        let inner = self.client.inner.clone();
        let labels: Vec<String> = filter.labels.clone();
        let pages = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let labels = labels.clone();
                async move {
                    let mut req = inner.issues(&owner, &repo).list();
                    if !labels.is_empty() {
                        req = req.labels(&labels);
                    }
                    req.send().await.map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(pages
            .items
            .into_iter()
            .map(|item| issue_from_octocrab(project.to_string(), item))
            .collect())
    }

    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let project = i.project.clone();
        let inner = self.client.inner.clone();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .get(number)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(issue_from_octocrab(project, model))
    }
```

Add helpers at the bottom of `issues.rs`:

```rust
fn parse_project(project: &str) -> Result<(String, String), ScmError> {
    let (o, r) = project.split_once('/').ok_or_else(|| ScmError::BadRequest {
        message: format!("project must be `owner/repo`: {project}"),
    })?;
    Ok((o.to_string(), r.to_string()))
}

fn issue_from_octocrab(project: String, item: octocrab::models::issues::Issue) -> Issue {
    Issue {
        r: IssueRef {
            tracker: IssueTracker::Github,
            project,
            number: item.number,
        },
        title: item.title,
        body: item.body.unwrap_or_default(),
        state: match item.state {
            octocrab::models::IssueState::Open => IssueState::Open,
            _ => IssueState::Closed,
        },
        labels: item.labels.into_iter().map(|l| l.name).collect(),
        author: item.user.login,
        created_at: item.created_at,
        updated_at: item.updated_at,
    }
}
```

- [ ] **Step 4: Run + commit**

```
cargo test -p rupu-scm --test github_translation
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Both new issue tests pass.

```bash
git add crates/rupu-scm/src/connectors/github/issues.rs \
        crates/rupu-scm/tests/fixtures/github/{issue_get_happy,issues_list_happy}.json \
        crates/rupu-scm/tests/github_translation.rs
git commit -m "$(cat <<'EOF'
rupu-scm: github get_issue + list_issues

Issue project ids are `owner/repo` strings — parsed at the connector
boundary; the IssueRef carries the parsed form so cross-platform
trackers (Linear, Jira) can use their own project-id shapes without
the trait reshaping.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

#### 12e — Write methods (`comment_pr`, `create_pr`, `create_branch`, `comment_issue`, `create_issue`, `update_issue_state`, `clone_to`)

- [ ] **Step 1: Implement them all in one commit**

Replace the remaining `not yet implemented` bodies. The shapes are mechanical (POST/PATCH via octocrab). Show the final state of `repo.rs`'s remaining methods:

```rust
    async fn create_branch(
        &self,
        r: &RepoRef,
        name: &str,
        from_sha: &str,
    ) -> Result<Branch, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let name = name.to_string();
        let from_sha = from_sha.to_string();
        // GitHub: POST /repos/{owner}/{repo}/git/refs with ref=refs/heads/{name}
        let _ = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let name = name.clone();
                let from_sha = from_sha.clone();
                async move {
                    inner
                        .repos(&owner, &repo)
                        .create_ref(&octocrab::params::repos::Reference::Branch(name), &from_sha)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(Branch {
            name,
            sha: from_sha,
            protected: false,
        })
    }

    async fn comment_pr(&self, p: &PrRef, body: &str) -> Result<Comment, ScmError> {
        // Issue-comment endpoint also serves PR comments on GitHub.
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = p.repo.owner.clone();
        let repo = p.repo.repo.clone();
        let number = p.number as u64;
        let body = body.to_string();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let body = body.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .create_comment(number, &body)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(Comment {
            id: model.id.to_string(),
            author: model.user.login,
            body: model.body.unwrap_or_default(),
            created_at: model.created_at,
        })
    }

    async fn create_pr(&self, r: &RepoRef, opts: CreatePr) -> Result<Pr, ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let pr = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let opts = opts.clone();
                async move {
                    inner
                        .pulls(&owner, &repo)
                        .create(opts.title, opts.head, opts.base)
                        .body(opts.body)
                        .draft(opts.draft)
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(pr_from_octocrab(r.clone(), pr))
    }

    async fn clone_to(&self, r: &RepoRef, dir: &std::path::Path) -> Result<(), ScmError> {
        // Clone over HTTPS using the connector's PAT as the Basic auth
        // username (GitHub PAT-as-username works; password = empty).
        let url = format!(
            "https://{}@github.com/{}/{}.git",
            self.client.token, r.owner, r.repo
        );
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(), ScmError> {
            git2::Repository::clone(&url, &dir).map_err(|e| {
                ScmError::Network(anyhow::anyhow!("git clone failed: {e}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|e| ScmError::Transient(anyhow::anyhow!("join: {e}")))??;
        Ok(())
    }
```

For `issues.rs`, replace the remaining methods:

```rust
    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let body = body.to_string();
        let inner = self.client.inner.clone();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let body = body.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .create_comment(number, &body)
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(Comment {
            id: model.id.to_string(),
            author: model.user.login,
            body: model.body.unwrap_or_default(),
            created_at: model.created_at,
        })
    }

    async fn create_issue(
        &self,
        project: &str,
        opts: CreateIssue,
    ) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(project)?;
        let inner = self.client.inner.clone();
        let project = project.to_string();
        let model = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let opts = opts.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .create(opts.title)
                        .body(opts.body)
                        .labels(opts.labels)
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(issue_from_octocrab(project, model))
    }

    async fn update_issue_state(
        &self,
        i: &IssueRef,
        state: IssueState,
    ) -> Result<(), ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let inner = self.client.inner.clone();
        let _ = self
            .client
            .with_retry(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .update(number)
                        .state(match state {
                            IssueState::Open => octocrab::models::IssueState::Open,
                            IssueState::Closed => octocrab::models::IssueState::Closed,
                        })
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;
        Ok(())
    }
```

- [ ] **Step 2: Verify compile + gates**

```
cargo build -p rupu-scm
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

All clean. (Live tests for the write paths land in Plan 3; httpmock-based unit tests for them are added in 12e Step 3.)

- [ ] **Step 3: Add httpmock unit tests for write paths**

Append to `tests/github_translation.rs`:

```rust
#[tokio::test]
async fn comment_pr_posts_body() {
    use httpmock::Method::POST;
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/section9labs/rupu/issues/42/comments");
        then.status(201)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "id": 7777,
                    "user": {"login": "matias", "id": 1},
                    "body": "looks great",
                    "created_at": "2026-05-03T12:00:00Z"
                }"#,
            );
    });
    let c = common::github_connector_against(&server);
    let comment = c
        .comment_pr(
            &rupu_scm::PrRef {
                repo: rupu_scm::RepoRef {
                    platform: rupu_scm::Platform::Github,
                    owner: "section9labs".into(),
                    repo: "rupu".into(),
                },
                number: 42,
            },
            "looks great",
        )
        .await
        .unwrap();
    m.assert();
    assert_eq!(comment.id, "7777");
    assert_eq!(comment.body, "looks great");
}

#[tokio::test]
async fn create_pr_posts_payload() {
    use httpmock::Method::POST;
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/repos/section9labs/rupu/pulls");
        then.status(201)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "id": 99,
                    "number": 200,
                    "state": "open",
                    "title": "feat: add foo",
                    "body": "adds foo",
                    "head": {"ref": "feat/foo", "sha": "1"},
                    "base": {"ref": "main", "sha": "0"},
                    "user": {"login": "matias", "id": 1},
                    "created_at": "2026-05-03T13:00:00Z",
                    "updated_at": "2026-05-03T13:00:00Z",
                    "merged_at": null,
                    "draft": false
                }"#,
            );
    });
    let c = common::github_connector_against(&server);
    let pr = c
        .create_pr(
            &rupu_scm::RepoRef {
                platform: rupu_scm::Platform::Github,
                owner: "section9labs".into(),
                repo: "rupu".into(),
            },
            rupu_scm::CreatePr {
                title: "feat: add foo".into(),
                body: "adds foo".into(),
                head: "feat/foo".into(),
                base: "main".into(),
                draft: false,
            },
        )
        .await
        .unwrap();
    m.assert();
    assert_eq!(pr.r.number, 200);
    assert_eq!(pr.title, "feat: add foo");
}
```

- [ ] **Step 4: Run + commit**

```
cargo test -p rupu-scm --test github_translation
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Both write-path tests pass.

```bash
git add crates/rupu-scm/src/connectors/github/{repo,issues}.rs \
        crates/rupu-scm/tests/github_translation.rs
git commit -m "$(cat <<'EOF'
rupu-scm: github write paths + clone_to

create_branch, comment_pr, create_pr, comment_issue, create_issue,
update_issue_state, clone_to. Writes always go via octocrab; clone
uses git2 with the PAT-as-username trick for GitHub HTTPS auth.
httpmock unit tests pin the request shapes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Live integration tests + workspace gates

### Task 13: Live smoke tests gated by `RUPU_LIVE_TESTS=1`

**Files:**
- Create: `crates/rupu-scm/tests/live_smoke.rs`

- [ ] **Step 1: Write the file**

Create `crates/rupu-scm/tests/live_smoke.rs`:

```rust
//! Live smoke tests against the real GitHub API. Skipped silently
//! unless `RUPU_LIVE_TESTS=1` AND `RUPU_LIVE_GITHUB_TOKEN` are set.
//! Wired into the existing nightly-live-tests workflow in Plan 3.

use rupu_scm::{IssueConnector, IssueFilter, Platform, RepoConnector, RepoRef};

fn live_enabled() -> bool {
    std::env::var("RUPU_LIVE_TESTS").as_deref() == Ok("1")
}

fn token() -> Option<String> {
    std::env::var("RUPU_LIVE_GITHUB_TOKEN").ok()
}

fn build_connectors() -> Option<(std::sync::Arc<dyn RepoConnector>, std::sync::Arc<dyn IssueConnector>)> {
    let token = token()?;
    use rupu_scm::connectors::github::GithubClient;
    let client = GithubClient::new(token, None, Some(2));
    let repo: std::sync::Arc<dyn RepoConnector> = std::sync::Arc::new(
        rupu_scm::connectors::github::repo::GithubRepoConnector::new(client.clone()),
    );
    let issues: std::sync::Arc<dyn IssueConnector> = std::sync::Arc::new(
        rupu_scm::connectors::github::issues::GithubIssueConnector::new(client),
    );
    Some((repo, issues))
}

#[tokio::test]
async fn github_list_repos_returns_at_least_one() {
    if !live_enabled() {
        return;
    }
    let Some((repo, _)) = build_connectors() else { return };
    let repos = repo.list_repos().await.expect("list_repos");
    assert!(!repos.is_empty());
}

#[tokio::test]
async fn github_get_repo_for_known_target() {
    if !live_enabled() {
        return;
    }
    let Some((repo, _)) = build_connectors() else { return };
    let r = repo
        .get_repo(&RepoRef {
            platform: Platform::Github,
            owner: "section9labs".into(),
            repo: "rupu".into(),
        })
        .await
        .expect("get_repo");
    assert_eq!(r.r.repo, "rupu");
}

#[tokio::test]
async fn github_list_issues_for_known_target() {
    if !live_enabled() {
        return;
    }
    let Some((_, issues)) = build_connectors() else { return };
    let _ = issues
        .list_issues("section9labs/rupu", IssueFilter::default())
        .await
        .expect("list_issues");
}
```

- [ ] **Step 2: Re-export the connector structs publicly so the test can construct them**

In `crates/rupu-scm/src/connectors/github/mod.rs`, add at the bottom:

```rust
// Re-export the concrete types so end-to-end tests and CLI code can
// construct them directly. Internal callers should still go through
// `try_build` / `Registry::discover`.
pub use issues::GithubIssueConnector;
pub use repo::GithubRepoConnector;
```

Make `repo` and `issues` modules `pub`:

```rust
pub mod issues;
pub mod repo;
```

(Was previously `mod`.)

- [ ] **Step 3: Verify compile + run (will skip when env vars absent)**

```
cargo test -p rupu-scm --test live_smoke
```

Expected: 3 tests pass (no-op when env vars unset).

```
cargo fmt -p rupu-scm -- --check
cargo clippy -p rupu-scm --all-targets -- -D warnings
```

Both clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-scm/src/connectors/github/mod.rs \
        crates/rupu-scm/tests/live_smoke.rs
git commit -m "$(cat <<'EOF'
rupu-scm: live smoke tests gated by RUPU_LIVE_TESTS

Three live-API smokes (list_repos, get_repo, list_issues) against
the real GitHub API. Skip silently when the env vars aren't set so
per-PR CI stays offline. Wires into Plan 3's nightly workflow that
already has RUPU_LIVE_TESTS plumbing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: Workspace gates + CLAUDE.md pointer update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Run all gates from the workspace root**

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three exit 0.

- [ ] **Step 2: Update CLAUDE.md to reference the B-2 spec + plan**

In `CLAUDE.md`, replace the "Read first" bullets with the Slice B-2 doc paths (preserving B-1 references):

```markdown
## Read first
- Slice A spec: `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`
- Slice B-1 spec: `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`
- Slice B-2 spec: `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`
- Plan 1 (foundation + GitHub connector, in progress): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-1-foundation-and-github.md`
- Plan 2 (GitLab + MCP server): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-2-gitlab-and-mcp.md`
- Plan 3 (CLI run-target + docs + nightly): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-3-cli-and-docs.md`
```

Then update the `### Crates` section by adding a `rupu-scm` bullet:

```markdown
- **`rupu-scm`** — SCM/issue-tracker connectors. `RepoConnector` + `IssueConnector` traits per spec §4c; per-platform impls under `connectors/<platform>/`. Plan 1 ships GitHub; Plan 2 adds GitLab + the embedded MCP server.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
CLAUDE.md: point to Slice B-2 plans + add rupu-scm to crates list

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Plan 1 success criteria

After all 14 tasks complete:

- `cargo fmt --all -- --check` exits 0.
- `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- `cargo test --workspace` exits 0.
- `rupu auth login --provider github --mode <api-key|sso>` succeeds against the real GitHub OAuth (key path) or device-code flow (sso path).
- `Registry::discover` against an `InMemoryResolver` with a stored Github credential builds and returns both a `RepoConnector` and an `IssueConnector`.
- Every `RepoConnector` and `IssueConnector` method on `GithubRepoConnector` / `GithubIssueConnector` is implemented (no method returns `not yet implemented`).
- Recorded-fixture tests pass for: list_repos (with pagination), get_repo, get_pr, diff_pr, read_file, list_prs, list_branches, get_issue, list_issues, comment_pr, create_pr.
- `cargo test -p rupu-scm --test live_smoke` passes (smokes are no-ops without `RUPU_LIVE_TESTS=1` + `RUPU_LIVE_GITHUB_TOKEN`).

## Out of scope (deferred to Plan 2)

- GitLab adapter (`Connector` trait impls + OAuth provider entry).
- MCP server (`rupu-mcp` crate).
- Agent-runtime in-process MCP attach.

## Out of scope (deferred to Plan 3)

- `rupu repos list` CLI subcommand.
- `rupu mcp serve` CLI subcommand.
- `rupu run <agent> <target>` argument grammar.
- `docs/scm.md` + `docs/scm/<platform>.md` documentation.
- Wiring the github + gitlab live tests into the existing `nightly-live-tests.yml` workflow.
- README "SCM & issue trackers" section.
- CHANGELOG entry for B-2.
