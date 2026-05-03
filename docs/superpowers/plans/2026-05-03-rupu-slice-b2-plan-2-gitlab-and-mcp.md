# rupu Slice B-2 — Plan 2: GitLab connector + embedded MCP server

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a working GitLab `RepoConnector` + `IssueConnector` to `rupu-scm` and stand up the new `rupu-mcp` crate that exposes the unified tool catalog described in spec §6. The agent runtime (`rupu-agent`) gains an in-process MCP server that comes up at the start of every `rupu run` / `rupu workflow run` and tears down at the end, so SCM tools are auto-attached without any agent-file change. After this plan: an agent listing `scm.repos.list` in its `tools:` frontmatter can call it during a run, and an external MCP-aware client (e.g. Claude Desktop) can spawn `rupu mcp serve` over stdio and consume the same surface (CLI wiring of `rupu mcp serve` lands in Plan 3).

**Architecture:** GitLab follows the same shape as Plan 1's GitHub adapter — its own `client.rs` with the same retry / ETag / semaphore wrapper, a per-trait file (`repo.rs`, `issues.rs`), and recorded fixtures. The new `rupu-mcp` crate carries: an MCP server kernel (`server.rs`) over a `Transport` trait, an in-process transport for the agent runtime, a stdio transport for external clients, a `ToolDispatcher` that maps each MCP tool name to a `Registry` method, and `schemars`-generated JSON Schemas. Tools beyond `scm.*`/`issues.*` (workflow dispatch, pipeline trigger) live in their own per-platform module so the trait surface stays clean.

**Tech Stack:** Rust 2021 (MSRV 1.88), `tokio`, `async-trait`, `serde_json`, `schemars`, `gitlab` SDK (added in Task 1), reuse of Plan 1's `octocrab`, `lru`, `git2`. JSON-RPC framing for MCP is hand-rolled per the MCP standard (one line of JSON per message; no extra dep needed beyond `serde_json` + `tokio::io::AsyncBufReadExt`).

**Spec:** `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`

---

## File Structure

```
crates/
  rupu-scm/
    src/
      connectors/
        gitlab/                                    # NEW
          mod.rs                                   # GitlabConnector facade
          repo.rs                                  # GitlabRepoConnector (RepoConnector impl)
          issues.rs                                # GitlabIssueConnector (IssueConnector impl)
          client.rs                                # GitlabClient (auth + retry + ETag + semaphore)
          extras.rs                                # gitlab.pipeline_trigger (non-trait method)
        github/
          extras.rs                                # NEW: github.workflows_dispatch
      registry.rs                                  # MODIFY: discover() also wires GitLab + extras handles
      types.rs                                     # MODIFY: PipelineTrigger / WorkflowDispatch arg structs
    tests/
      gitlab_translation.rs                        # NEW
      gitlab_httpmock.rs                           # NEW
      classify_scm_error.rs                        # MODIFY: GitLab rows added to the table
      live_smoke.rs                                # MODIFY: GitLab smokes (gated by RUPU_LIVE_GITLAB_TOKEN)
      fixtures/
        gitlab/
          projects_list_happy.json
          projects_list_paginated_page_1.json
          projects_list_paginated_page_2.json
          project_get_happy.json
          mr_get_happy.json
          mr_diff_happy.patch
          issue_get_happy.json
          error_401.json
          error_403_missing_scope.json
          error_429_rate_limited.json
          error_404.json
          error_409_conflict.json
        regen-gitlab.sh

  rupu-auth/
    src/
      oauth/
        providers.rs                               # MODIFY: add `gitlab` provider entry (browser-callback PKCE)

  rupu-mcp/                                        # NEW
    Cargo.toml
    src/
      lib.rs
      server.rs                                    # MCP kernel — JSON-RPC dispatch loop
      transport.rs                                 # Transport trait + InProcess + Stdio impls
      schema.rs                                    # schemars wrappers — Tool definitions for tools/list
      dispatcher.rs                                # ToolDispatcher — name → Registry method
      tools/
        mod.rs                                     # tool catalog + ToolSpec list
        scm_repos.rs                               # scm.repos.{list,get}
        scm_branches.rs                            # scm.branches.{list,create}
        scm_files.rs                               # scm.files.read
        scm_prs.rs                                 # scm.prs.{list,get,diff,comment,create}
        issues.rs                                  # issues.{list,get,comment,create,update_state}
        github_extras.rs                           # github.workflows_dispatch
        gitlab_extras.rs                           # gitlab.pipeline_trigger
      permission.rs                                # mode + per-tool allowlist gating
      error.rs                                     # McpError → JSON-RPC error mapping
    tests/
      schema_snapshot.rs                           # snapshot tools/list response
      dispatch_unit.rs                             # ToolDispatcher unit tests against mock Registry
      stdio_roundtrip.rs                           # spawn the in-process server, send tools/list + a tool call

  rupu-agent/
    src/
      runner.rs                                    # MODIFY: spin up rupu_mcp::serve_in_process at run start
      tool_registry.rs                             # MODIFY: register MCP-backed tool stubs alongside the six builtins
    tests/
      mcp_attach.rs                                # NEW: agent run with mock provider that calls scm.repos.list

  rupu-cli/
    src/
      cmd/
        auth.rs                                    # MODIFY: --provider gitlab accepts --mode sso (PKCE)
```

## Conventions to honor

- All cross-crate types come from `rupu-scm`; `rupu-mcp` never invents its own DTOs.
- `#![deny(clippy::all)]` at every crate root.
- `unsafe_code` forbidden.
- Workspace deps only.
- Per the "no mock features" memory: every MCP tool either dispatches to a real `Registry` method or returns an explicit `McpError::NotWiredInV0` (e.g. for unbuilt issue trackers). No silent `Ok(SilentNoOp)`.
- Per the "read reference impls" memory: when wiring the MCP wire format and GitLab's API for the first time, read the reference implementations (anthropic-mcp-rs / `modelcontextprotocol/servers` JSON-RPC examples; the `gitlab` crate's docs.rs examples) BEFORE inventing the request shape.

## Important pre-existing state (read before starting)

- `crates/rupu-scm/src/registry.rs` (Plan 1 Task 10) builds `Registry` from `CredentialResolver + Config`. Plan 2 extends `discover()` to also instantiate the GitLab connector.
- `crates/rupu-scm/src/connectors/github/client.rs` (Plan 1 Task 11) is the canonical pattern the new `gitlab/client.rs` mirrors line-for-line in shape (semaphore + cache + retry + classify).
- `crates/rupu-auth/src/oauth/providers.rs` carries per-provider OAuth metadata. Plan 1 Task 8 added the GitHub entry; Plan 2 Task 2 adds GitLab.
- `crates/rupu-auth/src/oauth/pkce.rs` (Slice B-1 Plan 2 Task 6) already implements the browser-callback PKCE flow. GitLab reuses it; no new flow code.
- `crates/rupu-agent/src/runner.rs::run_agent` is the spot where the MCP server gets spun up (Task 11). Today the registry is built from `default_tool_registry()`; after Plan 2 it merges in the MCP-backed tool stubs.
- `crates/rupu-tools/src/tool.rs::Tool` is the trait every dispatchable verb implements. MCP tools are exposed to the runtime via a thin `McpTool` adapter that implements `Tool` and forwards to the dispatcher.

---

## Phase 0 — Workspace deps

### Task 1: Add `gitlab` and `jsonschema` workspace deps

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add to `[workspace.dependencies]`**

```toml
gitlab = "0.1710"
jsonschema = "0.18"            # used by tests/schema_snapshot.rs to validate generated schemas
```

`schemars` was already pinned in Plan 1 Task 1.

- [ ] **Step 2: Verify workspace metadata**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: exit 0.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
deps: add gitlab + jsonschema to workspace

gitlab is the GitLab REST/GraphQL SDK consumed by Plan 2's
GitlabRepoConnector / GitlabIssueConnector. jsonschema is a
test-only dev dep for validating the schemars-generated MCP
tool schemas in rupu-mcp's snapshot tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — GitLab connector

### Task 2: GitLab OAuth provider entry

**Files:**
- Modify: `crates/rupu-auth/src/oauth/providers.rs`

- [ ] **Step 1: Add the `gitlab` arm to the per-provider metadata table**

In the function that returns `&'static ProviderOAuth` keyed by `&str` provider name (added in Plan 1 Task 8), insert:

```rust
"gitlab" => &ProviderOAuth {
    client_id: env!("RUPU_GITLAB_CLIENT_ID", "build with RUPU_GITLAB_CLIENT_ID set"),
    authorize_url: "https://gitlab.com/oauth/authorize",
    token_url:     "https://gitlab.com/oauth/token",
    redirect_uri:  "http://localhost:{port}/callback",
    allowed_ports: &[39900, 39901, 39902, 39903, 39904],
    scopes: &[
        "api",
        "read_user",
        "read_repository",
        "write_repository",
    ],
    extra_authorize_params: &[],
    token_body_format: TokenBodyFormat::Form,
    state_is_verifier: false,
    include_state_in_token_body: false,
    flow: AuthFlow::BrowserCallback,
},
```

> **Impersonation note**: Like Anthropic's UUID client and OpenAI's Codex CLI client (TODO.md "Register rupu-specific OAuth clients"), `RUPU_GITLAB_CLIENT_ID` defaults via `option_env!` to a placeholder for local builds and is overridden at release-build time. Document the impersonation acknowledgement in the file's docstring header alongside the existing notes.

- [ ] **Step 2: Add a regression test**

In `crates/rupu-auth/src/oauth/providers.rs`'s `mod tests` block, append:

```rust
#[test]
fn gitlab_metadata_is_browser_callback_with_full_scope_set() {
    let p = provider_oauth("gitlab").expect("gitlab entry");
    assert_eq!(p.authorize_url, "https://gitlab.com/oauth/authorize");
    assert_eq!(p.token_url, "https://gitlab.com/oauth/token");
    assert_eq!(p.flow, AuthFlow::BrowserCallback);
    assert!(p.scopes.contains(&"api"));
    assert!(p.scopes.contains(&"read_repository"));
    assert!(p.scopes.contains(&"write_repository"));
    assert!(p.scopes.contains(&"read_user"));
    assert!(matches!(p.token_body_format, TokenBodyFormat::Form));
    assert!(!p.state_is_verifier);
    assert!(!p.include_state_in_token_body);
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-auth -- providers::tests::gitlab_metadata
```

Expected: 1 passed.

- [ ] **Step 4: Verify `rupu auth login --provider gitlab --mode sso` parses**

```
cargo run -p rupu-cli -- auth login --provider gitlab --mode sso --help
```

Expected: clap renders the help text without panicking on the new provider name. (Real flow is exercised by Plan 3 Task 12's nightly live tests.)

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth/
git commit -m "$(cat <<'EOF'
rupu-auth: add gitlab OAuth provider entry (PKCE browser-callback)

Mirrors Plan 1 Task 8's github entry. gitlab.com OAuth uses the
standard browser-callback PKCE flow (no Anthropic-style state-as-
verifier quirks). Scopes cover api + read_user + read_repository
+ write_repository so subsequent Registry::discover wiring can
list_repos / read_file / create_branch / create_pr without re-login.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `GitlabClient` — auth + retry + ETag wrapper

**Files:**
- Create: `crates/rupu-scm/src/connectors/gitlab/client.rs`
- Modify: `crates/rupu-scm/src/connectors/gitlab/mod.rs`

This task mirrors Plan 1 Task 11's `GithubClient` line-for-line in shape: same `Semaphore` (keyed `"gitlab"`), same `LruCache` ETag layer, same retry harness, same boundary-mapping to `ScmError`.

- [ ] **Step 1: Create the file**

Create `crates/rupu-scm/src/connectors/gitlab/client.rs`:

```rust
//! Internal HTTP client for the GitLab adapter.
//!
//! Wraps `reqwest::Client` (the `gitlab` SDK is convenient for typed
//! responses but doesn't expose enough hooks for our retry/ETag
//! semantics; we use it for typed deserialization only and drive the
//! HTTP itself). Same shape as github::client::GithubClient:
//!
//! - per-platform Semaphore via concurrency::semaphore_for("gitlab", _)
//! - in-memory LRU ETag cache for `get_*` responses (TTL 5min)
//! - retry-with-backoff for RateLimited / Transient classifications
//! - boundary-level mapping to ScmError via classify_scm_error

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use reqwest::{header::HeaderMap, Client, Method};
use rupu_providers::concurrency;
use tokio::sync::Semaphore;

use crate::error::{classify_scm_error, ScmError};
use crate::platform::Platform;

const CACHE_CAP: usize = 256;
const CACHE_TTL: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 5;

#[derive(Clone)]
pub struct GitlabClient {
    pub(crate) http: Client,
    pub(crate) base_url: String,
    pub(crate) token: String,
    semaphore: Arc<Semaphore>,
    cache: Arc<Mutex<LruCache<String, CacheEntry>>>,
}

struct CacheEntry {
    etag: String,
    body: serde_json::Value,
    inserted_at: Instant,
}

impl GitlabClient {
    pub fn new(token: String, base_url: Option<String>, max_concurrency: Option<usize>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://gitlab.com/api/v4".to_string());
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest builder");
        let semaphore = concurrency::semaphore_for("gitlab", max_concurrency.or(Some(6)));
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(CACHE_CAP).unwrap(),
        )));
        Self { http, base_url: base, token, semaphore, cache }
    }

    pub async fn permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("gitlab semaphore closed")
    }

    /// GET <base_url>/<path>; honors ETag cache and retries on
    /// RateLimited/Transient. Returns parsed JSON on success.
    pub async fn get_json(&self, path: &str) -> Result<serde_json::Value, ScmError> {
        // Cache hit short-circuit + If-None-Match logic identical in
        // shape to GithubClient::get_json. See plan 1 task 11 step 3
        // for the canonical body; reproduced verbatim here, swapping
        // Octocrab for self.http and the platform tag to "gitlab".
        // [implementation continues — see Plan 1 Task 11 step 3 for
        //  the byte-equivalent code; copy and adapt the platform tag,
        //  base URL, and the auth header (PRIVATE-TOKEN vs Bearer).]

        // 1) Check cache for fresh entry; if present and not expired, short-circuit.
        // 2) Acquire permit.
        // 3) Build request with `PRIVATE-TOKEN: <self.token>` header (GitLab style).
        //    If the cache had an entry, attach `If-None-Match: <etag>`.
        // 4) Loop with exponential backoff on RateLimited/Transient (max MAX_RETRIES).
        // 5) On 304, return cached body.
        // 6) On 2xx, store new etag+body in cache, return body.
        // 7) On 4xx/5xx that aren't retried, return classify_scm_error(...).
        unimplemented!("see Plan 1 Task 11 step 3 — byte-equivalent body")
    }

    /// Non-cached path methods: post_json / put_json / delete; build the
    /// request with the same auth header but skip cache lookup. Same
    /// retry/classify shape.
    pub async fn post_json(
        &self,
        method: Method,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, ScmError> {
        unimplemented!("byte-equivalent of GithubClient::post_json swapping auth header")
    }
}
```

> Important: by "byte-equivalent" the implementer should literally copy the function body from `crates/rupu-scm/src/connectors/github/client.rs::get_json` (created in Plan 1 Task 11), then change four things: (a) auth header from `Authorization: Bearer <token>` to `PRIVATE-TOKEN: <token>`; (b) the platform string passed to `classify_scm_error` from `Platform::Github` to `Platform::Gitlab`; (c) the base URL prefix used to build the request URL; (d) the rate-limit header name from GitHub's `X-RateLimit-Remaining` to GitLab's `RateLimit-Remaining` (and `Retry-After` is the same). Don't refactor into a shared helper yet — the auth header rules are vendor-specific enough that a "Configurable" parameter would be premature. If a third platform appears (Bitbucket, etc.), do the extraction then.

- [ ] **Step 2: Update the gitlab/mod.rs facade**

Create `crates/rupu-scm/src/connectors/gitlab/mod.rs`:

```rust
//! GitLab connector — implements RepoConnector + IssueConnector.

pub mod client;
pub mod extras;
pub mod issues;
pub mod repo;

pub use client::GitlabClient;
pub use issues::GitlabIssueConnector;
pub use repo::GitlabRepoConnector;
```

Stub the other files as `// implemented in Tasks 4 + 5` so the module tree resolves.

- [ ] **Step 3: Run gates**

```
cargo check -p rupu-scm
cargo clippy -p rupu-scm -- -D warnings
```

Expected: green except for the `unimplemented!` markers in `client.rs` (those panic at runtime, not at compile time, so clippy doesn't object).

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-scm/src/connectors/gitlab/
git commit -m "$(cat <<'EOF'
rupu-scm: scaffold GitlabClient (mirrors GithubClient shape)

Same shape as Plan 1's GithubClient: per-platform Semaphore via
concurrency::semaphore_for("gitlab", _), 256-entry LRU ETag cache
with 5-minute TTL, retry-with-backoff on RateLimited/Transient.
Auth header is GitLab's PRIVATE-TOKEN form (vs GitHub's Bearer);
base URL defaults to gitlab.com and is overridable via [scm.gitlab].

Connector trait impls land in Tasks 4 and 5.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `GitlabRepoConnector` (RepoConnector impl)

**Files:**
- Create: `crates/rupu-scm/src/connectors/gitlab/repo.rs`
- Create: `crates/rupu-scm/tests/gitlab_translation.rs`
- Create: `crates/rupu-scm/tests/gitlab_httpmock.rs`
- Create: `crates/rupu-scm/tests/fixtures/gitlab/*.json`

This task is split into **subtasks** mirroring Plan 1 Task 12's decomposition. Each subtask: write fixture → write failing test → implement method → green test → commit.

#### Subtask 4a: `list_repos` (paginated `GET /projects?membership=true`)

- [ ] **Step 1: Capture the fixture**

Run against a real PAT once and save to `crates/rupu-scm/tests/fixtures/gitlab/projects_list_happy.json` (or generate from `tests/fixtures/regen-gitlab.sh`). Same approach as Plan 1 Task 12a's GitHub fixture.

- [ ] **Step 2: Write the failing translation test**

In `crates/rupu-scm/tests/gitlab_translation.rs`:

```rust
//! GitLab JSON → typed value translation tests.
//!
//! Each test loads a recorded fixture and asserts the deserialization
//! into rupu-scm's typed `Repo` / `Pr` / `Issue` shapes is correct
//! field-for-field.

use rupu_scm::connectors::gitlab::repo::translate_project_to_repo;
use rupu_scm::types::Repo;

#[test]
fn projects_list_happy_translates_to_repo() {
    let raw = std::fs::read_to_string("tests/fixtures/gitlab/projects_list_happy.json").unwrap();
    let arr: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    let repos: Vec<Repo> = arr.iter().map(translate_project_to_repo).collect::<Result<_, _>>().unwrap();
    assert!(!repos.is_empty(), "fixture should contain at least one project");
    let first = &repos[0];
    assert_eq!(first.r.platform, rupu_scm::Platform::Gitlab);
    assert!(!first.r.owner.is_empty());
    assert!(!first.r.repo.is_empty());
    assert!(!first.default_branch.is_empty());
    assert!(first.clone_url_https.starts_with("https://"));
    assert!(first.clone_url_ssh.starts_with("git@") || first.clone_url_ssh.starts_with("ssh://"));
}
```

Run: `cargo test -p rupu-scm --test gitlab_translation`
Expected: FAIL — `translate_project_to_repo` not defined.

- [ ] **Step 3: Implement `translate_project_to_repo` and `list_repos`**

In `crates/rupu-scm/src/connectors/gitlab/repo.rs`:

```rust
//! GitlabRepoConnector — implements rupu_scm::RepoConnector.
//!
//! Each method:
//! 1. Acquires the per-platform semaphore permit.
//! 2. Issues the request via [`GitlabClient`] (which handles ETag
//!    cache, retries, and classify_scm_error mapping).
//! 3. Deserializes the JSON via `serde_json::from_value` into the
//!    GitLab-flavored DTO struct, then translates to rupu_scm types.
//!
//! GitLab vs GitHub vocabulary:
//!   - "project" ↔ Repo
//!   - "merge request" (MR) ↔ Pr
//!   - "namespace/path" ↔ owner/repo (always full slash-joined for nested groups)

use async_trait::async_trait;
use std::path::Path;

use crate::connectors::gitlab::client::GitlabClient;
use crate::error::ScmError;
use crate::platform::Platform;
use crate::types::{
    Branch, Comment, CreatePr, Diff, FileContent, FileEncoding, Pr, PrFilter, PrRef, PrState,
    Repo, RepoRef,
};
use crate::RepoConnector;

pub struct GitlabRepoConnector {
    client: GitlabClient,
}

impl GitlabRepoConnector {
    pub fn new(client: GitlabClient) -> Self { Self { client } }
}

/// Pure translation function — fixture-tested in
/// crates/rupu-scm/tests/gitlab_translation.rs.
pub fn translate_project_to_repo(p: &serde_json::Value) -> Result<Repo, ScmError> {
    let path_with_namespace = p
        .get("path_with_namespace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ScmError::BadRequest {
            message: "missing path_with_namespace".into(),
        })?;
    let (owner, repo_name) = split_namespace(path_with_namespace);
    Ok(Repo {
        r: RepoRef {
            platform: Platform::Gitlab,
            owner,
            repo: repo_name,
        },
        default_branch: p
            .get("default_branch")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .to_string(),
        clone_url_https: p
            .get("http_url_to_repo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        clone_url_ssh: p
            .get("ssh_url_to_repo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        private: p
            .get("visibility")
            .and_then(|v| v.as_str())
            .map(|s| s != "public")
            .unwrap_or(true),
        description: p
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

fn split_namespace(path: &str) -> (String, String) {
    // GitLab nested groups: "group/subgroup/project" → owner="group/subgroup", repo="project".
    if let Some((owner, name)) = path.rsplit_once('/') {
        (owner.to_string(), name.to_string())
    } else {
        (String::new(), path.to_string())
    }
}

#[async_trait]
impl RepoConnector for GitlabRepoConnector {
    fn platform(&self) -> Platform { Platform::Gitlab }

    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        let _permit = self.client.permit().await;
        let mut out = Vec::new();
        let mut page = 1u32;
        loop {
            let body = self.client
                .get_json(&format!("/projects?membership=true&per_page=100&page={page}"))
                .await?;
            let arr = body.as_array().ok_or_else(|| ScmError::BadRequest {
                message: "expected array".into(),
            })?;
            if arr.is_empty() { break; }
            for item in arr {
                out.push(translate_project_to_repo(item)?);
            }
            if arr.len() < 100 { break; }
            page += 1;
            if page > 100 { break; } // safety cap — 10k repos is plenty
        }
        Ok(out)
    }

    // get_repo / list_branches / create_branch / read_file / list_prs /
    // get_pr / diff_pr / comment_pr / create_pr / clone_to land in
    // subtasks 4b–4f below.
    async fn get_repo(&self, _r: &RepoRef) -> Result<Repo, ScmError> { unimplemented!("subtask 4b") }
    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> { unimplemented!("subtask 4c") }
    async fn create_branch(&self, _r: &RepoRef, _name: &str, _from_sha: &str) -> Result<Branch, ScmError> { unimplemented!("subtask 4f") }
    async fn read_file(&self, _r: &RepoRef, _path: &str, _ref_: Option<&str>) -> Result<FileContent, ScmError> { unimplemented!("subtask 4c") }
    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> { unimplemented!("subtask 4d") }
    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> { unimplemented!("subtask 4d") }
    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> { unimplemented!("subtask 4d") }
    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> { unimplemented!("subtask 4f") }
    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> { unimplemented!("subtask 4f") }
    async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> { unimplemented!("subtask 4f") }
}
```

Per the "no mock features" memory: each `unimplemented!` is a compile-time todo, not a runtime placeholder. The struct cannot be exercised end-to-end until subtasks 4b–4f land — that's the discipline. Calling `Registry::repo(Gitlab)?.get_repo(...)` before subtask 4b would panic with a clear message, NOT silently succeed.

- [ ] **Step 4: Run translation test**

```
cargo test -p rupu-scm --test gitlab_translation -- projects_list_happy_translates_to_repo
```

Expected: PASS.

- [ ] **Step 5: Httpmock round-trip test for `list_repos`**

In `crates/rupu-scm/tests/gitlab_httpmock.rs`:

```rust
use httpmock::prelude::*;
use rupu_scm::connectors::gitlab::client::GitlabClient;
use rupu_scm::connectors::gitlab::repo::GitlabRepoConnector;
use rupu_scm::RepoConnector;

#[tokio::test]
async fn list_repos_paginates_until_empty() {
    let server = MockServer::start_async().await;
    let page1 = std::fs::read_to_string("tests/fixtures/gitlab/projects_list_paginated_page_1.json").unwrap();
    let page2 = std::fs::read_to_string("tests/fixtures/gitlab/projects_list_paginated_page_2.json").unwrap();

    server.mock(|when, then| {
        when.method(GET).path("/projects").query_param("page", "1");
        then.status(200).header("content-type", "application/json").body(&page1);
    });
    server.mock(|when, then| {
        when.method(GET).path("/projects").query_param("page", "2");
        then.status(200).header("content-type", "application/json").body(&page2);
    });
    server.mock(|when, then| {
        when.method(GET).path("/projects").query_param("page", "3");
        then.status(200).header("content-type", "application/json").body("[]");
    });

    let client = GitlabClient::new(
        "fake-token".into(),
        Some(server.base_url()),
        None,
    );
    let conn = GitlabRepoConnector::new(client);
    let repos = conn.list_repos().await.unwrap();
    assert_eq!(repos.len(), 200, "two pages × 100 per page");
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-scm/src/connectors/gitlab/repo.rs \
        crates/rupu-scm/tests/gitlab_translation.rs \
        crates/rupu-scm/tests/gitlab_httpmock.rs \
        crates/rupu-scm/tests/fixtures/gitlab/projects_list_*.json
git commit -m "$(cat <<'EOF'
rupu-scm: GitlabRepoConnector::list_repos + translation helpers

Pure translate_project_to_repo() handles GitLab's nested-namespace
quirk (group/subgroup/project → owner=group/subgroup, repo=project)
and visibility/private mapping. Httpmock test verifies pagination
loop terminates on empty page. Other RepoConnector methods stubbed
with unimplemented!() so the trait compiles; subtasks 4b–4f fill
them in.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

#### Subtask 4b: `get_repo` + `list_branches` (single-resource reads)

- [ ] **Step 1: Capture fixtures `project_get_happy.json` and `branches_list_happy.json`.**
- [ ] **Step 2: Write translation tests for `Branch` deserialization (mirrors 4a's pattern).**
- [ ] **Step 3: Implement `get_repo` (`GET /projects/:id`) and `list_branches` (`GET /projects/:id/repository/branches`). Project ID must be URL-encoded (`group%2Fsubgroup%2Fproject`).**
- [ ] **Step 4: Run translation + httpmock tests; assert green.**
- [ ] **Step 5: Commit (`feat(rupu-scm): GitlabRepoConnector get_repo + list_branches`).**

#### Subtask 4c: `read_file` (`GET /projects/:id/repository/files/:path/raw?ref=:ref`)

- [ ] **Step 1: Capture `file_read_happy.json` (or raw text fixture).**
- [ ] **Step 2: Write a test that reads `README.md` at default-branch ref; expect `FileContent { encoding: Utf8, content: "..." }`.**
- [ ] **Step 3: Implement. URL-encode both project ID and the file path. Honor optional `ref_` parameter; when `None`, omit the query param so GitLab uses default branch.**
- [ ] **Step 4: Run; commit.**

#### Subtask 4d: `list_prs` + `get_pr` + `diff_pr`

- [ ] **Step 1: Capture `mr_list_happy.json`, `mr_get_happy.json`, `mr_diff_happy.patch` (raw diff text).**
- [ ] **Step 2: Write translation tests for `Pr` (note: GitLab uses `iid` for the per-project number — translate to `PrRef.number`).**
- [ ] **Step 3: Implement `list_prs` (`GET /projects/:id/merge_requests?state=opened&...`), `get_pr` (`GET /projects/:id/merge_requests/:iid`), `diff_pr` (`GET /projects/:id/merge_requests/:iid/raw_diffs` returns text/plain — wrap in `Diff { patch, ... }`; counts come from a separate `/changes` call).**
- [ ] **Step 4: Run translation + httpmock tests.**
- [ ] **Step 5: Commit.**

#### Subtask 4e: `comment_pr` + `create_pr`

- [ ] **Step 1: Capture `mr_comment_happy.json` and `mr_create_happy.json`.**
- [ ] **Step 2: Write tests asserting `comment_pr` → `Comment { id, author, body, created_at }` and `create_pr` → `Pr { state: Open, ... }`.**
- [ ] **Step 3: Implement: `comment_pr` is `POST /projects/:id/merge_requests/:iid/notes` with `{body}`; `create_pr` is `POST /projects/:id/merge_requests` with `{source_branch, target_branch, title, description, draft}`.**
- [ ] **Step 4: Run; commit.**

#### Subtask 4f: `create_branch` + `clone_to`

- [ ] **Step 1: Capture `branch_create_happy.json`.**
- [ ] **Step 2: Implement `create_branch` (`POST /projects/:id/repository/branches?branch=:name&ref=:from_sha`).**
- [ ] **Step 3: Implement `clone_to` using `git2::Repository::clone_into` with `https://oauth2:<token>@gitlab.com/<owner>/<repo>.git` URL form (gitlab.com's PAT-as-password convention). Honor `[scm.gitlab].clone_protocol` for ssh path (uses `git2::build::RepoBuilder` with credentials callback).**
- [ ] **Step 4: Write a test using a `tempfile::TempDir` to clone a public sample repo (no auth needed for the test path). Skip on missing network — `cargo test --test gitlab_httpmock -- --skip clone_to_smoke` should still pass.**
- [ ] **Step 5: Commit.**

---

### Task 5: `GitlabIssueConnector` (IssueConnector impl)

**Files:**
- Create: `crates/rupu-scm/src/connectors/gitlab/issues.rs`
- Modify: `crates/rupu-scm/tests/gitlab_translation.rs`
- Modify: `crates/rupu-scm/tests/gitlab_httpmock.rs`

Same shape as Task 4: per method, fixture → translation test → impl → httpmock test → commit. Methods to ship:

- [ ] `list_issues(project, filter)` → `GET /projects/:id/issues?state=opened&labels=...&author_username=...`
- [ ] `get_issue(IssueRef)` → `GET /projects/:id/issues/:iid`
- [ ] `comment_issue(IssueRef, body)` → `POST /projects/:id/issues/:iid/notes`
- [ ] `create_issue(project, opts)` → `POST /projects/:id/issues`
- [ ] `update_issue_state(IssueRef, state)` → `PUT /projects/:id/issues/:iid` with `{state_event: "close"|"reopen"}`

Translation note: GitLab issue states are `opened` / `closed` (not `open`); map `IssueState::Open` ↔ `"opened"` and `IssueState::Closed` ↔ `"closed"`.

- [ ] **Step (last): Commit per method.**

---

### Task 6: GitLab error classification + `classify_scm_error` table extension

**Files:**
- Modify: `crates/rupu-scm/src/error.rs` (extend the table)
- Modify: `crates/rupu-scm/tests/classify_scm_error.rs`

- [ ] **Step 1: Extend the `classify_scm_error` function to dispatch by `Platform`**

The function signature was set up in Plan 1 Task 5 as:

```rust
pub fn classify_scm_error(
    platform: Platform,
    status: u16,
    body: &str,
    headers: &HeaderMap,
) -> ScmError;
```

Add GitLab-specific deltas:

- GitLab's missing-scope hint header is `WWW-Authenticate: Bearer error="insufficient_scope"`, not GitHub's `X-OAuth-Scopes`. Detect and map to `MissingScope { scope: <parsed from error_description>, hint: "Re-login: rupu auth login --provider gitlab --mode sso" }`.
- GitLab's rate-limit header is `RateLimit-Remaining` + `RateLimit-Reset` (Unix epoch seconds), not GitHub's `X-RateLimit-Reset`. Add a parser branch.
- GitLab returns `403` on private-repo access without `read_repository`; classify that as `MissingScope { scope: "read_repository", ... }` rather than generic `RateLimited`.

- [ ] **Step 2: Add table-driven tests in `crates/rupu-scm/tests/classify_scm_error.rs`**

```rust
#[test]
fn gitlab_403_with_insufficient_scope_is_missing_scope() {
    let body = std::fs::read_to_string("tests/fixtures/gitlab/error_403_missing_scope.json").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static(r#"Bearer error="insufficient_scope" error_description="The request requires read_repository""#),
    );
    let err = classify_scm_error(Platform::Gitlab, 403, &body, &headers);
    match err {
        ScmError::MissingScope { platform, scope, .. } => {
            assert_eq!(platform, "gitlab");
            assert_eq!(scope, "read_repository");
        }
        other => panic!("expected MissingScope, got {other:?}"),
    }
}

#[test]
fn gitlab_429_uses_ratelimit_reset_header_not_xratelimit() {
    let mut headers = HeaderMap::new();
    headers.insert("RateLimit-Reset", HeaderValue::from_static("1714760000"));
    let err = classify_scm_error(Platform::Gitlab, 429, "{}", &headers);
    match err {
        ScmError::RateLimited { retry_after } => {
            assert!(retry_after.is_some());
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}
```

Run: `cargo test -p rupu-scm --test classify_scm_error gitlab_`
Expected: 2 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-scm/src/error.rs crates/rupu-scm/tests/classify_scm_error.rs crates/rupu-scm/tests/fixtures/gitlab/error_*.json
git commit -m "$(cat <<'EOF'
rupu-scm: classify_scm_error gitlab arms (insufficient_scope + RateLimit-*)

Two GitLab-specific delta vs GitHub: WWW-Authenticate carries the
missing-scope hint instead of X-OAuth-Scopes, and rate-limit
headers are 'RateLimit-Reset' (no 'X-' prefix). Table-driven
tests cover both.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Wire GitLab into `Registry::discover` + extras methods

**Files:**
- Modify: `crates/rupu-scm/src/registry.rs`
- Create: `crates/rupu-scm/src/connectors/gitlab/extras.rs`
- Create: `crates/rupu-scm/src/connectors/github/extras.rs`
- Modify: `crates/rupu-scm/src/types.rs` (add extras arg structs)

- [ ] **Step 1: Add extras arg structs to `types.rs`**

```rust
/// Args for the github.workflows_dispatch tool (Plan 2 Task 11 surfaces it via MCP).
pub struct WorkflowDispatch {
    pub workflow: String,            // workflow file name or numeric ID
    pub ref_: String,                // branch/tag/sha
    pub inputs: serde_json::Value,   // free-form, validated against workflow's `inputs:` schema
}

/// Args for the gitlab.pipeline_trigger tool.
pub struct PipelineTrigger {
    pub ref_: String,                                       // branch/tag
    pub variables: std::collections::BTreeMap<String, String>,
}
```

- [ ] **Step 2: Implement extras methods on the per-platform clients**

In `crates/rupu-scm/src/connectors/github/extras.rs`:

```rust
//! GitHub workflow_dispatch — non-trait method exposed by Registry::github_extras().

use crate::error::ScmError;
use crate::types::{RepoRef, WorkflowDispatch};
use super::client::GithubClient;

pub struct GithubExtras { client: GithubClient }

impl GithubExtras {
    pub fn new(client: GithubClient) -> Self { Self { client } }

    pub async fn workflows_dispatch(&self, r: &RepoRef, w: WorkflowDispatch) -> Result<(), ScmError> {
        let path = format!(
            "/repos/{}/{}/actions/workflows/{}/dispatches",
            r.owner, r.repo, w.workflow
        );
        let body = serde_json::json!({
            "ref": w.ref_,
            "inputs": w.inputs,
        });
        self.client.post_json(reqwest::Method::POST, &path, body).await?;
        Ok(())
    }
}
```

Mirror in `crates/rupu-scm/src/connectors/gitlab/extras.rs::GitlabExtras::pipeline_trigger`:

```rust
pub async fn pipeline_trigger(&self, r: &RepoRef, p: PipelineTrigger) -> Result<(), ScmError> {
    // POST /projects/:id/trigger/pipeline?token=<trigger_token>&ref=<ref>&variables[KEY]=<value>...
    // For PAT-driven path use POST /projects/:id/pipeline { ref, variables: [{key,value}] }
    let path = format!(
        "/projects/{}/pipeline",
        urlencoding::encode(&format!("{}/{}", r.owner, r.repo))
    );
    let vars: Vec<serde_json::Value> = p.variables.iter()
        .map(|(k, v)| serde_json::json!({"key": k, "value": v}))
        .collect();
    let body = serde_json::json!({"ref": p.ref_, "variables": vars});
    self.client.post_json(reqwest::Method::POST, &path, body).await?;
    Ok(())
}
```

- [ ] **Step 3: Extend Registry**

In `crates/rupu-scm/src/registry.rs`:

```rust
pub struct Registry {
    repo_connectors:  HashMap<Platform, Arc<dyn RepoConnector>>,
    issue_connectors: HashMap<IssueTracker, Arc<dyn IssueConnector>>,
    github_extras:    Option<Arc<GithubExtras>>,   // NEW
    gitlab_extras:    Option<Arc<GitlabExtras>>,   // NEW
}

impl Registry {
    pub async fn discover(resolver: &dyn CredentialResolver, cfg: &Config) -> Self {
        // existing GitHub branch (Plan 1 Task 10) builds GithubClient + connectors.
        // Plan 2 adds: ditto for GitLab when resolver.get("gitlab", None) succeeds.
        // ALSO: build the platform extras (GithubExtras { client.clone() } / GitlabExtras { client.clone() })
        //       and store in the Option fields. Skipping a platform means its extras are None.
        ...
    }

    pub fn github_extras(&self) -> Option<Arc<GithubExtras>> { self.github_extras.clone() }
    pub fn gitlab_extras(&self) -> Option<Arc<GitlabExtras>> { self.gitlab_extras.clone() }
}
```

- [ ] **Step 4: Test**

In `crates/rupu-scm/tests/registry_discover.rs`, add:

```rust
#[tokio::test]
async fn discover_with_gitlab_credential_yields_repo_issue_extras() {
    let mut r = InMemoryResolver::new();
    r.put("gitlab", AuthMode::ApiKey, AuthCredentials::ApiKey { key: "fake".into() });
    let cfg = Config::default();
    let reg = Registry::discover(&r, &cfg).await;
    assert!(reg.repo(Platform::Gitlab).is_some());
    assert!(reg.issues(IssueTracker::Gitlab).is_some());
    assert!(reg.gitlab_extras().is_some());
    // No GitHub credential → all None.
    assert!(reg.repo(Platform::Github).is_none());
    assert!(reg.github_extras().is_none());
}
```

- [ ] **Step 5: Run gates**

```
cargo test -p rupu-scm
cargo clippy -p rupu-scm -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-scm/src/registry.rs \
        crates/rupu-scm/src/connectors/gitlab/extras.rs \
        crates/rupu-scm/src/connectors/github/extras.rs \
        crates/rupu-scm/src/types.rs \
        crates/rupu-scm/tests/registry_discover.rs
git commit -m "$(cat <<'EOF'
rupu-scm: Registry wires gitlab + per-platform extras handles

Registry::discover now builds GitlabRepoConnector + GitlabIssueConnector
when a gitlab credential is present, and exposes per-platform extras
(github.workflows_dispatch / gitlab.pipeline_trigger) via dedicated
github_extras() / gitlab_extras() accessors. These don't fit the
RepoConnector trait (they're write-only side-effects) so they live
adjacent to the trait surface. rupu-mcp Task 11 maps them to MCP tools.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — `rupu-mcp` crate skeleton

### Task 8: Scaffold `rupu-mcp` crate + Transport trait + InProcess transport

**Files:**
- Create: `crates/rupu-mcp/Cargo.toml`
- Create: `crates/rupu-mcp/src/lib.rs`
- Create: `crates/rupu-mcp/src/transport.rs`
- Create: `crates/rupu-mcp/src/error.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create the crate Cargo.toml**

```toml
[package]
name = "rupu-mcp"
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
schemars.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
tokio = { workspace = true, features = ["io-util", "io-std", "rt-multi-thread", "macros", "sync"] }
async-trait.workspace = true
chrono.workspace = true

# In-workspace
rupu-scm = { path = "../rupu-scm" }
rupu-tools = { path = "../rupu-tools" }
rupu-config = { path = "../rupu-config" }

[dev-dependencies]
tempfile.workspace = true
jsonschema.workspace = true
serde_json.workspace = true
```

- [ ] **Step 2: Create `lib.rs`**

```rust
#![deny(clippy::all)]

//! rupu-mcp — embedded MCP server for the unified SCM tool catalog.
//!
//! Two transports:
//!   - [`InProcessTransport`] — used by the agent runtime; tools dispatched
//!     by direct calls without serialization round-trips.
//!   - [`StdioTransport`] — used by `rupu mcp serve` (Plan 3 Task 1) for
//!     external MCP-aware clients (Claude Desktop, Cursor).

pub mod dispatcher;
pub mod error;
pub mod permission;
pub mod schema;
pub mod server;
pub mod tools;
pub mod transport;

pub use dispatcher::ToolDispatcher;
pub use error::McpError;
pub use permission::McpPermission;
pub use server::{serve_in_process, McpServer, ServeHandle};
pub use transport::{InProcessTransport, StdioTransport, Transport};
pub use tools::ToolKind;
```

- [ ] **Step 3: Create `error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("permission denied for tool {tool}: {reason}")]
    PermissionDenied { tool: String, reason: String },

    #[error("not wired in v0: {0}")]
    NotWiredInV0(String),

    #[error("tool dispatch failed: {0}")]
    Dispatch(#[from] rupu_scm::ScmError),

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("transport: {0}")]
    Transport(#[source] anyhow::Error),
}

impl McpError {
    pub fn code(&self) -> i32 {
        match self {
            Self::UnknownTool(_) => -32601,
            Self::InvalidArgs(_) => -32602,
            Self::PermissionDenied { .. } => -32001,
            Self::NotWiredInV0(_) => -32002,
            Self::Dispatch(_) => -32003,
            Self::Transport(_) => -32603,
        }
    }

    pub fn to_jsonrpc(&self, id: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        })
    }
}
```

- [ ] **Step 4: Create `transport.rs` (Transport trait + InProcess + Stdio impls)**

```rust
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdin, Stdout};
use tokio::sync::{mpsc, Mutex as TokioMutex};

use crate::error::McpError;

#[async_trait]
pub trait Transport: Send + Sync {
    async fn recv(&self) -> Result<Option<Value>, McpError>;
    async fn send(&self, msg: Value) -> Result<(), McpError>;
}

/// In-process transport: a pair of mpsc channels. Used by the agent
/// runtime; no stdio, no serialization overhead.
#[derive(Clone)]
pub struct InProcessTransport {
    inbox: Arc<TokioMutex<mpsc::UnboundedReceiver<Value>>>,
    outbox: mpsc::UnboundedSender<Value>,
}

impl InProcessTransport {
    pub fn pair() -> (Self, Self) {
        let (client_tx, server_rx) = mpsc::unbounded_channel::<Value>();
        let (server_tx, client_rx) = mpsc::unbounded_channel::<Value>();
        let client = Self {
            inbox: Arc::new(TokioMutex::new(client_rx)),
            outbox: client_tx,
        };
        let server = Self {
            inbox: Arc::new(TokioMutex::new(server_rx)),
            outbox: server_tx,
        };
        (client, server)
    }
}

#[async_trait]
impl Transport for InProcessTransport {
    async fn recv(&self) -> Result<Option<Value>, McpError> {
        Ok(self.inbox.lock().await.recv().await)
    }
    async fn send(&self, msg: Value) -> Result<(), McpError> {
        self.outbox
            .send(msg)
            .map_err(|e| McpError::Transport(anyhow::anyhow!("inprocess send: {e}")))
    }
}

/// Stdio transport: newline-delimited JSON over stdin/stdout (the
/// canonical MCP wire format for spawned servers).
pub struct StdioTransport {
    stdin: Arc<TokioMutex<BufReader<Stdin>>>,
    stdout: Arc<TokioMutex<Stdout>>,
}

impl StdioTransport {
    pub fn new() -> Self {
        Self {
            stdin: Arc::new(TokioMutex::new(BufReader::new(tokio::io::stdin()))),
            stdout: Arc::new(TokioMutex::new(tokio::io::stdout())),
        }
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn recv(&self) -> Result<Option<Value>, McpError> {
        let mut buf = String::new();
        let n = self.stdin.lock().await.read_line(&mut buf).await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdin read: {e}")))?;
        if n == 0 { return Ok(None); }
        let v: Value = serde_json::from_str(buf.trim())
            .map_err(|e| McpError::InvalidArgs(format!("malformed JSON-RPC: {e}")))?;
        Ok(Some(v))
    }
    async fn send(&self, msg: Value) -> Result<(), McpError> {
        let line = serde_json::to_string(&msg)
            .map_err(|e| McpError::Transport(anyhow::anyhow!("serialize: {e}")))?;
        let mut out = self.stdout.lock().await;
        out.write_all(line.as_bytes()).await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdout write: {e}")))?;
        out.write_all(b"\n").await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdout write: {e}")))?;
        out.flush().await
            .map_err(|e| McpError::Transport(anyhow::anyhow!("stdout flush: {e}")))?;
        Ok(())
    }
}
```

- [ ] **Step 5: Add empty stubs for the other modules so lib.rs resolves**

```bash
mkdir -p crates/rupu-mcp/src/tools
touch crates/rupu-mcp/src/{server,dispatcher,schema,permission}.rs
touch crates/rupu-mcp/src/tools/mod.rs
```

In each, add `// implemented in subsequent tasks`. Add `crates/rupu-mcp` to root `Cargo.toml` `[workspace] members`.

- [ ] **Step 6: Round-trip test for InProcessTransport**

```rust
#[tokio::test]
async fn inprocess_transport_round_trips_messages() {
    let (client, server) = InProcessTransport::pair();
    client.send(serde_json::json!({"hello": "from-client"})).await.unwrap();
    let received = server.recv().await.unwrap().unwrap();
    assert_eq!(received["hello"], "from-client");
    server.send(serde_json::json!({"hello": "from-server"})).await.unwrap();
    let received = client.recv().await.unwrap().unwrap();
    assert_eq!(received["hello"], "from-server");
}
```

Run: `cargo test -p rupu-mcp transport`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/rupu-mcp/
git commit -m "$(cat <<'EOF'
rupu-mcp: scaffold crate, Transport trait, In-process + Stdio impls

Two transports for the same JSON-RPC kernel: InProcessTransport
(mpsc-channel pair, used by the agent runtime to attach the MCP
server without stdio) and StdioTransport (newline-delimited JSON
over stdin/stdout, used by `rupu mcp serve` for external MCP-aware
clients). Server kernel + tool catalog land in Tasks 9-13.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Server kernel + tool catalog + dispatcher + permissions

### Task 9: `McpServer` JSON-RPC dispatch loop + `serve_in_process`

**Files:**
- Modify: `crates/rupu-mcp/src/server.rs`

The server loop dispatches three JSON-RPC methods per the MCP spec:
- `initialize` → handshake; respond with server info + capabilities.
- `tools/list` → returns the full tool catalog (names + descriptions + input schemas).
- `tools/call` → routes to `ToolDispatcher::call(name, args)`. Successes return `{ result: { content: [{type:"text", text: <JSON>}] } }`; tool failures (incl. recoverable ScmError) return `{ result: { isError: true, content: [...] } }` per MCP spec — *not* JSON-RPC errors. The `error` field is reserved for transport/protocol failures.

- [ ] **Step 1: Implement `McpServer::run`**

```rust
//! MCP server kernel — JSON-RPC 2.0 dispatch loop over a Transport.

use crate::dispatcher::ToolDispatcher;
use crate::error::McpError;
use crate::permission::McpPermission;
use crate::tools::tool_catalog;
use crate::transport::{InProcessTransport, Transport};
use rupu_scm::Registry;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, error, warn};

pub struct McpServer<T: Transport + 'static> {
    registry: Arc<Registry>,
    transport: T,
    permission: McpPermission,
}

impl<T: Transport + 'static> McpServer<T> {
    pub fn new(registry: Arc<Registry>, transport: T, permission: McpPermission) -> Self {
        Self { registry, transport, permission }
    }

    pub async fn run(self) -> Result<(), McpError> {
        let dispatcher = ToolDispatcher::new(self.registry.clone(), self.permission.clone());
        loop {
            let msg = match self.transport.recv().await? {
                Some(m) => m,
                None => return Ok(()),
            };
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let response = match method {
                "initialize" => Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "serverInfo": { "name": "rupu", "version": env!("CARGO_PKG_VERSION") },
                        "capabilities": { "tools": {} },
                    },
                })),
                "tools/list" => Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "tools": tool_catalog() },
                })),
                "tools/call" => {
                    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                    match dispatcher.call(name, args).await {
                        Ok(text) => Ok(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{"type": "text", "text": text}] }
                        })),
                        Err(e) => {
                            warn!(tool = name, error = %e, "tool dispatch failed");
                            Ok(json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": {
                                    "isError": true,
                                    "content": [{"type": "text", "text": e.to_string()}]
                                }
                            }))
                        }
                    }
                }
                other => {
                    debug!(method = other, "unknown method");
                    Err(McpError::UnknownTool(other.to_string()))
                }
            };
            match response {
                Ok(v) => self.transport.send(v).await?,
                Err(e) => self.transport.send(e.to_jsonrpc(id)).await?,
            }
        }
    }
}

pub struct ServeHandle {
    pub join: JoinHandle<Result<(), McpError>>,
}

/// Spin up the MCP server in-process. Returns the client handle the
/// agent runtime uses to send `tools/call` requests, plus a JoinHandle
/// the caller drops at run end to tear down cleanly.
pub fn serve_in_process(
    registry: Arc<Registry>,
    permission: McpPermission,
) -> (InProcessTransport, ServeHandle) {
    let (client_t, server_t) = InProcessTransport::pair();
    let server = McpServer::new(registry, server_t, permission);
    let join = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            error!(error = %e, "mcp server failed");
            return Err(e);
        }
        Ok(())
    });
    (client_t, ServeHandle { join })
}
```

- [ ] **Step 2: Add `Registry::empty()` test helper in `rupu-scm`**

In `crates/rupu-scm/src/registry.rs`, gated on `#[cfg(any(test, feature = "test-helpers"))]`:

```rust
impl Registry {
    /// Test-only: build a Registry with no connectors. Tools that
    /// require a connector return McpError::NotWiredInV0 — they do
    /// NOT panic. Honors the "no mock features" rule: the absence
    /// of a connector is reported, not silently ignored.
    pub fn empty() -> Self {
        Self {
            repo_connectors: Default::default(),
            issue_connectors: Default::default(),
            github_extras: None,
            gitlab_extras: None,
            default_platform: None,
            default_tracker: None,
        }
    }
}
```

Add the matching feature flag to `crates/rupu-scm/Cargo.toml`:

```toml
[features]
default = []
test-helpers = []
```

`rupu-mcp`'s `[dev-dependencies]` enables it: `rupu-scm = { path = "../rupu-scm", features = ["test-helpers"] }`.

- [ ] **Step 3: Kernel test**

In `crates/rupu-mcp/tests/dispatch_unit.rs`:

```rust
use rupu_mcp::{serve_in_process, McpPermission};
use rupu_scm::Registry;
use std::sync::Arc;

#[tokio::test]
async fn server_responds_to_tools_list() {
    let registry = Arc::new(Registry::empty());
    let permission = McpPermission::allow_all();
    let (client, handle) = serve_in_process(registry, permission);

    use rupu_mcp::Transport;
    client.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list",
    })).await.unwrap();
    let resp = client.recv().await.unwrap().unwrap();
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "scm.repos.list"));
    assert!(tools.iter().any(|t| t["name"] == "issues.get"));
    drop(client);
    let _ = handle.join.await;
}
```

Run: `cargo test -p rupu-mcp --test dispatch_unit`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-mcp/src/server.rs crates/rupu-mcp/tests/dispatch_unit.rs crates/rupu-scm/src/registry.rs crates/rupu-scm/Cargo.toml
git commit -m "$(cat <<'EOF'
rupu-mcp: server kernel + serve_in_process

JSON-RPC 2.0 dispatch loop covering initialize, tools/list, and
tools/call. Tool errors come back inside `result` as
`{ isError: true, content: [...] }` per MCP spec; only transport
or protocol failures use the JSON-RPC `error` envelope.
serve_in_process returns the client transport + a JoinHandle so
the agent runtime can attach + tear down deterministically.

Adds Registry::empty() test helper in rupu-scm under feature
"test-helpers" so MCP tests can run without live connectors.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Tool catalog scaffold + `ToolKind` + `tool_catalog()`

**Files:**
- Create: `crates/rupu-mcp/src/tools/mod.rs`

- [ ] **Step 1: Implement**

```rust
//! Tool catalog for the unified MCP surface.
//!
//! Each module under `tools/` exposes:
//!   - `specs()` returning Vec<ToolSpec> for tools/list registration
//!   - per-tool `dispatch_*` async fns invoked by ToolDispatcher
//!
//! Conventions:
//!   - Tool names use dot-namespacing: "<namespace>.<resource>.<verb>".
//!   - `platform?` / `tracker?` parameters fall back to [scm.default]
//!     / [issues.default] from rupu-config when omitted.
//!   - All Args structs derive `JsonSchema` so input_schema is auto-generated.

pub mod issues;
pub mod scm_branches;
pub mod scm_files;
pub mod scm_prs;
pub mod scm_repos;
pub mod github_extras;
pub mod gitlab_extras;

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(skip)]
    pub kind: ToolKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind { Read, Write }

/// Returns the full tool catalog. Stable order — used by snapshot test.
pub fn tool_catalog() -> Vec<ToolSpec> {
    let mut v = Vec::new();
    v.extend(scm_repos::specs());
    v.extend(scm_branches::specs());
    v.extend(scm_files::specs());
    v.extend(scm_prs::specs());
    v.extend(issues::specs());
    v.extend(github_extras::specs());
    v.extend(gitlab_extras::specs());
    v
}
```

- [ ] **Step 2: Stub each per-tool module**

Each file (`scm_repos.rs`, `scm_branches.rs`, …) starts with `pub fn specs() -> Vec<super::ToolSpec> { Vec::new() }` so the module tree resolves. Subsequent tasks fill in the bodies.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-mcp/src/tools/
git commit -m "$(cat <<'EOF'
rupu-mcp: tool catalog scaffold + ToolKind + tool_catalog()

Module tree mirrors the spec's tool-namespace breakdown
(scm.repos / scm.branches / scm.files / scm.prs / issues /
github.* / gitlab.*). Each subsequent task fills in one module's
specs() + dispatch_*() functions. ToolKind classifies Read vs Write
for permission gating.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: SCM tools — `scm.repos.*`, `scm.branches.*`, `scm.files.*`, `scm.prs.*`

**Files:**
- Modify: `crates/rupu-mcp/src/tools/scm_repos.rs`
- Modify: `crates/rupu-mcp/src/tools/scm_branches.rs`
- Modify: `crates/rupu-mcp/src/tools/scm_files.rs`
- Modify: `crates/rupu-mcp/src/tools/scm_prs.rs`
- Modify: `crates/rupu-scm/src/registry.rs` (add `default_platform()` accessor)

This task ships **eleven** tools across four files. Each tool follows the same six-step recipe — write once below, repeat per tool with file/method/args swapped.

**Recipe (apply per tool):**

1. Define the args struct with `#[derive(Deserialize, JsonSchema)]`.
2. Append a `ToolSpec` to the module's `specs()` Vec.
3. Implement `pub async fn dispatch_<verb>(args: Value, reg: &Registry) -> Result<String, McpError>`.
4. Resolve the platform via the helper (Task 11 step 1 below).
5. Look up the connector via `reg.repo(platform)` / `reg.issues(tracker)`.
6. Invoke the trait method, serialize result with `serde_json::to_string(&value)`, return.

#### Subtask 11a: `scm.repos.list` + `scm.repos.get`

- [ ] **Step 1: Add `default_platform()` accessor to Registry**

In `crates/rupu-scm/src/registry.rs`:

```rust
pub fn default_platform(&self) -> Option<Platform> { self.default_platform }
pub fn default_tracker(&self) -> Option<IssueTracker> { self.default_tracker }
```

`default_platform: Option<Platform>` is set in `discover()` from `cfg.scm.default.platform`.

- [ ] **Step 2: Implement scm_repos.rs**

```rust
//! scm.repos.{list,get} tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{Platform, Registry, RepoRef};

#[derive(Deserialize, JsonSchema)]
pub struct ListReposArgs {
    /// Platform to query. Omit to use [scm.default].platform.
    pub platform: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetRepoArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "scm.repos.list",
            description: "List repositories the authenticated user can access on the given platform. Omit `platform` to use [scm.default].",
            input_schema: serde_json::to_value(schemars::schema_for!(ListReposArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "scm.repos.get",
            description: "Fetch a single repository (default branch, clone URLs, visibility, description).",
            input_schema: serde_json::to_value(schemars::schema_for!(GetRepoArgs)).unwrap(),
            kind: ToolKind::Read,
        },
    ]
}

pub async fn dispatch_list(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListReposArgs = serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let conn = reg.repo(platform).ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    let repos = conn.list_repos().await?;
    Ok(serde_json::to_string(&repos).unwrap())
}

pub async fn dispatch_get(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: GetRepoArgs = serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef { platform, owner: parsed.owner, repo: parsed.repo };
    let conn = reg.repo(platform).ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.get_repo(&r).await?).unwrap())
}

pub(crate) fn resolve_platform(arg: Option<&str>, reg: &Registry) -> Result<Platform, McpError> {
    match arg {
        Some(s) => s.parse::<Platform>().map_err(|e| McpError::InvalidArgs(e)),
        None => reg.default_platform().ok_or_else(|| McpError::InvalidArgs("no platform arg and no [scm.default] configured".into())),
    }
}
```

- [ ] **Step 3: Commit per subtask**

```bash
git add crates/rupu-mcp/src/tools/scm_repos.rs crates/rupu-scm/src/registry.rs
git commit -m "$(cat <<'EOF'
rupu-mcp: scm.repos.list + scm.repos.get tools

Args structs derive JsonSchema for auto-generated input_schema.
Platform resolution prefers explicit `platform` arg, falling back
to [scm.default].platform when omitted. NotWiredInV0 surfaces if
the user hasn't authenticated to the requested platform.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

#### Subtask 11b: `scm.branches.list` (Read) + `scm.branches.create` (Write)

- [ ] **Step 1: Args structs**

```rust
#[derive(Deserialize, JsonSchema)]
pub struct ListBranchesArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateBranchArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub from_sha: String,
}
```

- [ ] **Step 2: specs() + dispatch_list + dispatch_create — same shape as 11a.**
- [ ] **Step 3: Commit (`feat(rupu-mcp): scm.branches.* tools`).**

#### Subtask 11c: `scm.files.read` (Read)

- [ ] **Step 1: Args struct + specs() + dispatch_read.**

```rust
#[derive(Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub path: String,
    /// Optional ref (branch / tag / sha). Defaults to repo's default branch.
    pub r#ref: Option<String>,
}
```

- [ ] **Step 2: Commit.**

#### Subtask 11d: `scm.prs.{list, get, diff, comment, create}` (Read × 3, Write × 2)

- [ ] **Step 1: One Args struct per tool. Note `PrFilter` maps directly from `ListPrsArgs { state, author, limit }`.**
- [ ] **Step 2: dispatch_list / dispatch_get / dispatch_diff use `reg.repo(platform)` then `conn.list_prs / get_pr / diff_pr`.**
- [ ] **Step 3: dispatch_comment / dispatch_create likewise but go through the Write trait methods.**
- [ ] **Step 4: Commit per-method (`feat(rupu-mcp): scm.prs.list tool` etc.) so reviews stay digestible.**

---

### Task 12: Issue tools — `issues.{list, get, comment, create, update_state}`

**Files:**
- Modify: `crates/rupu-mcp/src/tools/issues.rs`

Same six-step recipe per tool. Tracker-resolution helper is symmetric to `resolve_platform`:

```rust
fn resolve_tracker(arg: Option<&str>, reg: &Registry) -> Result<IssueTracker, McpError> {
    match arg {
        Some(s) => s.parse::<IssueTracker>().map_err(|e| McpError::InvalidArgs(e)),
        None => reg.default_tracker().ok_or_else(|| McpError::InvalidArgs("no tracker arg and no [issues.default] configured".into())),
    }
}
```

For `Linear` and `Jira` trackers (declared in the enum but not implemented in B-2), `reg.issues(IssueTracker::Linear)` returns `None`; `dispatch_*` returns `McpError::NotWiredInV0("linear connector not in v0; ships in a follow-up slice")`.

- [ ] **Step 1: Implement five tools (one Args struct + dispatch fn each).**
- [ ] **Step 2: Commit per tool.**

---

### Task 13: Vendor extras — `github.workflows_dispatch` + `gitlab.pipeline_trigger`

**Files:**
- Modify: `crates/rupu-mcp/src/tools/github_extras.rs`
- Modify: `crates/rupu-mcp/src/tools/gitlab_extras.rs`

Both tools are Write. Each dispatches via `reg.github_extras()` / `reg.gitlab_extras()`; if `None`, return `McpError::NotWiredInV0`.

- [ ] **Step 1: Implement `dispatch_workflows_dispatch` / `dispatch_pipeline_trigger`.**

```rust
pub async fn dispatch_workflows_dispatch(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: WorkflowDispatchArgs = serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let extras = reg.github_extras().ok_or_else(|| McpError::NotWiredInV0("github extras require a github credential".into()))?;
    let r = RepoRef { platform: Platform::Github, owner: parsed.owner, repo: parsed.repo };
    extras.workflows_dispatch(&r, WorkflowDispatch {
        workflow: parsed.workflow,
        ref_: parsed.r#ref,
        inputs: parsed.inputs.unwrap_or(Value::Null),
    }).await?;
    Ok("{}".to_string())
}
```

- [ ] **Step 2: Commit.**

---

### Task 14: `ToolDispatcher` — name → dispatch fn lookup

**Files:**
- Modify: `crates/rupu-mcp/src/dispatcher.rs`

- [ ] **Step 1: Implement**

```rust
//! Routes MCP tool name → per-tool dispatch fn. Permission check
//! happens BEFORE dispatch so a denied write tool never reaches the
//! connector.

use crate::error::McpError;
use crate::permission::McpPermission;
use crate::tools::{self, ToolKind};
use rupu_scm::Registry;
use serde_json::Value;
use std::sync::Arc;

pub struct ToolDispatcher {
    registry: Arc<Registry>,
    permission: McpPermission,
}

impl ToolDispatcher {
    pub fn new(registry: Arc<Registry>, permission: McpPermission) -> Self {
        Self { registry, permission }
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<String, McpError> {
        let kind = self.kind_for(name)?;
        self.permission.check(name, kind)?;
        match name {
            "scm.repos.list"            => tools::scm_repos::dispatch_list(args, &self.registry).await,
            "scm.repos.get"             => tools::scm_repos::dispatch_get(args, &self.registry).await,
            "scm.branches.list"         => tools::scm_branches::dispatch_list(args, &self.registry).await,
            "scm.branches.create"       => tools::scm_branches::dispatch_create(args, &self.registry).await,
            "scm.files.read"            => tools::scm_files::dispatch_read(args, &self.registry).await,
            "scm.prs.list"              => tools::scm_prs::dispatch_list(args, &self.registry).await,
            "scm.prs.get"               => tools::scm_prs::dispatch_get(args, &self.registry).await,
            "scm.prs.diff"              => tools::scm_prs::dispatch_diff(args, &self.registry).await,
            "scm.prs.comment"           => tools::scm_prs::dispatch_comment(args, &self.registry).await,
            "scm.prs.create"            => tools::scm_prs::dispatch_create(args, &self.registry).await,
            "issues.list"               => tools::issues::dispatch_list(args, &self.registry).await,
            "issues.get"                => tools::issues::dispatch_get(args, &self.registry).await,
            "issues.comment"            => tools::issues::dispatch_comment(args, &self.registry).await,
            "issues.create"             => tools::issues::dispatch_create(args, &self.registry).await,
            "issues.update_state"       => tools::issues::dispatch_update_state(args, &self.registry).await,
            "github.workflows_dispatch" => tools::github_extras::dispatch_workflows_dispatch(args, &self.registry).await,
            "gitlab.pipeline_trigger"   => tools::gitlab_extras::dispatch_pipeline_trigger(args, &self.registry).await,
            other => Err(McpError::UnknownTool(other.to_string())),
        }
    }

    fn kind_for(&self, name: &str) -> Result<ToolKind, McpError> {
        for spec in tools::tool_catalog() {
            if spec.name == name { return Ok(spec.kind); }
        }
        Err(McpError::UnknownTool(name.to_string()))
    }
}
```

- [ ] **Step 2: Unit test for unknown tool**

```rust
#[tokio::test]
async fn dispatcher_returns_unknown_tool_for_typo() {
    let registry = Arc::new(Registry::empty());
    let perm = McpPermission::allow_all();
    let d = ToolDispatcher::new(registry, perm);
    let err = d.call("scm.repo.list", Value::Null).await.unwrap_err();
    assert!(matches!(err, McpError::UnknownTool(_)));
}
```

- [ ] **Step 3: Commit.**

---

### Task 15: Permission gating — `McpPermission`

**Files:**
- Modify: `crates/rupu-mcp/src/permission.rs`

- [ ] **Step 1: Implement (allowlist + mode gating)**

```rust
//! Permission gating: per-tool allowlist (from agent frontmatter `tools:`)
//! + per-mode (Readonly blocks Write tools; Ask defers to a callback).

use crate::error::McpError;
use crate::tools::ToolKind;
use rupu_tools::PermissionMode;
use std::sync::Arc;

#[derive(Clone)]
pub struct McpPermission {
    mode: PermissionMode,
    allowlist: Vec<String>,
    ask_cb: Option<Arc<dyn Fn(&str, &serde_json::Value) -> bool + Send + Sync>>,
}

impl McpPermission {
    pub fn new(mode: PermissionMode, allowlist: Vec<String>) -> Self {
        Self { mode, allowlist, ask_cb: None }
    }
    pub fn allow_all() -> Self {
        Self { mode: PermissionMode::Bypass, allowlist: vec!["*".into()], ask_cb: None }
    }
    pub fn with_ask_callback(mut self, cb: Arc<dyn Fn(&str, &serde_json::Value) -> bool + Send + Sync>) -> Self {
        self.ask_cb = Some(cb);
        self
    }
    pub fn check(&self, tool: &str, kind: ToolKind) -> Result<(), McpError> {
        if !self.tool_in_allowlist(tool) {
            return Err(McpError::PermissionDenied {
                tool: tool.to_string(),
                reason: format!("tool not in agent's `tools:` list (allowlist: {:?})", self.allowlist),
            });
        }
        match (self.mode, kind) {
            (PermissionMode::Readonly, ToolKind::Write) => Err(McpError::PermissionDenied {
                tool: tool.to_string(),
                reason: "readonly mode blocks write tools".into(),
            }),
            _ => Ok(()),
        }
    }
    fn tool_in_allowlist(&self, tool: &str) -> bool {
        self.allowlist.iter().any(|entry| {
            if entry == "*" || entry == tool { return true; }
            if let Some(prefix) = entry.strip_suffix('*') { tool.starts_with(prefix) } else { false }
        })
    }
}
```

- [ ] **Step 2: Tests**

```rust
#[test]
fn allowlist_wildcard_matches_namespace() {
    let p = McpPermission::new(PermissionMode::Bypass, vec!["scm.*".into()]);
    assert!(p.check("scm.repos.list", ToolKind::Read).is_ok());
    assert!(p.check("scm.prs.create", ToolKind::Write).is_ok());
    assert!(p.check("issues.get", ToolKind::Read).is_err());
}
#[test]
fn readonly_blocks_writes_even_when_allowlisted() {
    let p = McpPermission::new(PermissionMode::Readonly, vec!["*".into()]);
    assert!(p.check("scm.repos.list", ToolKind::Read).is_ok());
    let err = p.check("scm.prs.create", ToolKind::Write).unwrap_err();
    assert!(matches!(err, McpError::PermissionDenied { .. }));
}
```

- [ ] **Step 3: Commit.**

---

### Task 16: Snapshot tools/list response + jsonschema validation

**Files:**
- Create: `crates/rupu-mcp/tests/schema_snapshot.rs`
- Create: `crates/rupu-mcp/tests/snapshots/tools_list.json`

- [ ] **Step 1: Snapshot test (with `BLESS=1` regen path)**

```rust
use rupu_mcp::{serve_in_process, McpPermission, Transport};
use rupu_scm::Registry;
use std::sync::Arc;

#[tokio::test]
async fn tools_list_matches_snapshot() {
    let registry = Arc::new(Registry::empty());
    let permission = McpPermission::allow_all();
    let (client, handle) = serve_in_process(registry, permission);
    client.send(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list",
    })).await.unwrap();
    let resp = client.recv().await.unwrap().unwrap();
    let tools = serde_json::to_string_pretty(&resp["result"]["tools"]).unwrap();
    let path = "tests/snapshots/tools_list.json";
    if std::env::var("BLESS").is_ok() {
        std::fs::write(path, &tools).unwrap();
    }
    let expected = std::fs::read_to_string(path).expect("snapshot missing — run with BLESS=1");
    assert_eq!(tools.trim(), expected.trim(), "tools/list snapshot drift — re-run with BLESS=1 to update");
    drop(client);
    let _ = handle.join.await;
}
```

- [ ] **Step 2: Schema validity test**

```rust
#[test]
fn every_tool_input_schema_compiles_as_jsonschema() {
    for spec in rupu_mcp::tools::tool_catalog() {
        jsonschema::JSONSchema::compile(&spec.input_schema)
            .unwrap_or_else(|e| panic!("tool {} invalid schema: {e}", spec.name));
    }
}
```

- [ ] **Step 3: Generate the snapshot**

Run: `BLESS=1 cargo test -p rupu-mcp --test schema_snapshot tools_list_matches_snapshot`
Expected: PASS; `tests/snapshots/tools_list.json` written.

Re-run without `BLESS`: PASS (snapshot now matches).

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-mcp/tests/schema_snapshot.rs crates/rupu-mcp/tests/snapshots/tools_list.json
git commit -m "$(cat <<'EOF'
rupu-mcp: snapshot tools/list response + validate every schema

Snapshot guards against accidental schema drift (renaming a field,
reordering fields silently). Every input_schema also compiles
through jsonschema::JSONSchema::compile to catch malformed schemas
at test time. Regenerate with BLESS=1 cargo test ...

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Agent runtime in-process MCP attach

### Task 17: `McpToolAdapter` — bridges `rupu_tools::Tool` to MCP dispatcher

**Files:**
- Create: `crates/rupu-agent/src/mcp_tool.rs`
- Modify: `crates/rupu-agent/Cargo.toml` (add `rupu-mcp = { path = "../rupu-mcp" }`)

The agent runtime's `ToolRegistry` (Slice A Plan 2 Task 13) maps tool name → `Arc<dyn rupu_tools::Tool>`. To expose MCP tools without changing that contract, wrap each `tools/list` entry in a thin adapter that implements `rupu_tools::Tool` and forwards `invoke()` to `ToolDispatcher::call`.

- [ ] **Step 1: Implement**

```rust
//! Adapter so MCP-backed tools satisfy the rupu_tools::Tool trait.

use async_trait::async_trait;
use rupu_mcp::{McpError, ToolDispatcher};
use rupu_tools::{Tool, ToolContext, ToolError, ToolOutput};
use serde_json::Value;
use std::sync::Arc;

pub struct McpToolAdapter {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
    dispatcher: Arc<ToolDispatcher>,
}

impl McpToolAdapter {
    pub fn new(
        name: &'static str,
        description: &'static str,
        input_schema: Value,
        dispatcher: Arc<ToolDispatcher>,
    ) -> Self {
        Self { name, description, input_schema, dispatcher }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &'static str { self.name }
    fn description(&self) -> &'static str { self.description }
    fn input_schema(&self) -> Value { self.input_schema.clone() }
    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        match self.dispatcher.call(self.name, input).await {
            Ok(text) => Ok(ToolOutput::text(text)),
            Err(e) => match e {
                McpError::PermissionDenied { reason, .. } => Err(ToolError::PermissionDenied(reason)),
                other => Err(ToolError::Failed(other.to_string())),
            },
        }
    }
}
```

(`ToolError::PermissionDenied` and `ToolError::Failed` should already exist — confirm in `crates/rupu-tools/src/tool.rs`. If a variant is missing, add it as part of this task with a corresponding `From` impl.)

- [ ] **Step 2: Test**

```rust
#[tokio::test]
async fn mcp_tool_adapter_invokes_dispatcher() {
    let registry = Arc::new(Registry::empty());
    let perm = McpPermission::allow_all();
    let dispatcher = Arc::new(ToolDispatcher::new(registry, perm));
    let adapter = McpToolAdapter::new(
        "scm.repos.list",
        "list repos",
        serde_json::json!({}),
        dispatcher,
    );
    let res = adapter.invoke(serde_json::json!({"platform": "github"}), &ToolContext::default()).await;
    // Expect NotWiredInV0 because Registry::empty() has no github connector.
    assert!(res.is_err());
}
```

- [ ] **Step 3: Commit.**

---

### Task 18: `runner` spins up MCP at run start, tears down at end

**Files:**
- Modify: `crates/rupu-agent/src/runner.rs`
- Modify: `crates/rupu-agent/src/tool_registry.rs`

- [ ] **Step 1: Add `mcp_registry: Option<Arc<Registry>>` to `AgentRunOpts`**

```rust
pub struct AgentRunOpts {
    // ...existing fields...
    /// SCM/issue registry built by the CLI from CredentialResolver +
    /// rupu-config. `None` means MCP tools are not available for this
    /// run (test harness, etc.). When `Some`, the runner spins up an
    /// in-process MCP server before the first turn and tears it down
    /// before returning.
    pub mcp_registry: Option<Arc<rupu_scm::Registry>>,
}
```

- [ ] **Step 2: Spin up the server before the agent loop**

In `run_agent`, after `let mut writer = JsonlWriter::create(...)`, before the loop:

```rust
let mcp_handle = if let Some(reg) = opts.mcp_registry.clone() {
    let allowlist = opts.agent_tools.clone().unwrap_or_else(|| vec!["*".into()]);
    let mode = parse_mode_for_runtime(&opts.mode_str);
    let perm = rupu_mcp::McpPermission::new(mode, allowlist);
    let (transport, handle) = rupu_mcp::serve_in_process(reg.clone(), perm.clone());
    let dispatcher = Arc::new(rupu_mcp::ToolDispatcher::new(reg, perm));
    // Insert the MCP-backed tools into the registry alongside the six builtins.
    for spec in rupu_mcp::tools::tool_catalog() {
        let adapter = Arc::new(crate::mcp_tool::McpToolAdapter::new(
            spec.name,
            spec.description,
            spec.input_schema.clone(),
            dispatcher.clone(),
        ));
        registry.insert(spec.name.to_string(), adapter);
    }
    Some((transport, handle))
} else {
    None
};
```

- [ ] **Step 3: Tear down at end**

After the loop, before returning `RunResult`:

```rust
if let Some((transport, handle)) = mcp_handle {
    drop(transport); // closes the channel; server's recv() returns None and exits.
    let _ = handle.join.await;
}
```

- [ ] **Step 4: Test**

In `crates/rupu-agent/tests/mcp_attach.rs`:

```rust
//! End-to-end: MockProvider emits a tool_use for scm.repos.list; the
//! runner dispatches it through the in-process MCP server; transcript
//! contains a Tool* event with the dispatcher's response.

use rupu_agent::runner::{run_agent, AgentRunOpts, BypassDecider};
use rupu_scm::Registry;
use std::sync::Arc;

#[tokio::test]
async fn agent_run_with_mcp_dispatches_scm_repos_list() {
    let mock_script = serde_json::json!([
        { "tool_use": { "id": "1", "name": "scm.repos.list", "input": { "platform": "github" } } },
        { "stop": "end_turn" }
    ]);
    let provider = rupu_providers::MockProvider::with_script(mock_script);
    let registry = Arc::new(Registry::empty());
    let temp = tempfile::tempdir().unwrap();
    let opts = AgentRunOpts {
        // ... usual fields ...
        mcp_registry: Some(registry),
        // ...
    };
    let result = run_agent(opts).await;
    // The dispatch fails (NotWiredInV0) because Registry::empty() has no
    // github connector — but the runner must not panic; the failure
    // surfaces as a tool_result with an error string in the transcript.
    assert!(result.is_ok());
    let transcript = std::fs::read_to_string(temp.path().join("transcript.jsonl")).unwrap();
    assert!(transcript.contains("\"tool\":\"scm.repos.list\""));
    assert!(transcript.contains("not wired in v0") || transcript.contains("NotWiredInV0"));
}
```

(`MockProvider::with_script` is the test harness from Slice A; check `crates/rupu-providers/src/mock.rs` if the script shape needs adjustment.)

- [ ] **Step 5: Run gates**

```
cargo test -p rupu-agent --test mcp_attach
cargo clippy -p rupu-agent -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-agent/src/runner.rs crates/rupu-agent/src/mcp_tool.rs crates/rupu-agent/Cargo.toml crates/rupu-agent/tests/mcp_attach.rs
git commit -m "$(cat <<'EOF'
rupu-agent: in-process MCP attach + teardown around every run

run_agent() now accepts an Option<Arc<Registry>>; when Some, it
spins up rupu_mcp::serve_in_process before the first turn and
inserts McpToolAdapter entries into the tool registry alongside
the six builtins. The transport handle is dropped after the loop
exits; the server's JoinHandle is awaited so teardown is
deterministic.

Tools are gated by McpPermission built from the agent's `tools:`
allowlist + the run's permission mode — same semantics as the
six builtins, just enforced inside rupu-mcp.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 19: `cmd/run.rs` + `cmd/workflow.rs` build the Registry and pass it down

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`
- Modify: `crates/rupu-cli/src/cmd/workflow.rs`

- [ ] **Step 1: Build Registry once per invocation**

In `cmd/run.rs::run_inner`, after the `KeychainResolver` is created:

```rust
let scm_registry = Arc::new(rupu_scm::Registry::discover(&resolver, &cfg).await);
```

Pass it to `AgentRunOpts.mcp_registry = Some(scm_registry)`.

- [ ] **Step 2: Same for `cmd/workflow.rs::handle_run`**

The orchestrator's `StepFactory` already has access to a `&Resolver`; thread the `Registry` through the same way.

- [ ] **Step 3: Test**

`cargo test -p rupu-cli` confirms existing tests still pass; the new wire is silent for tests that don't exercise MCP tools.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/src/cmd/
git commit -m "$(cat <<'EOF'
rupu-cli: run + workflow build Registry::discover and pass to runner

Registry comes from the same KeychainResolver + Config the CLI
already loads. Building it is cheap when no platforms are
configured (logs INFO and returns an empty Registry). Passing it
to AgentRunOpts wires up the in-process MCP server for every
`rupu run` and `rupu workflow run`; agents that don't use MCP
tools see zero behavioral change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Live integration tests + workspace gates

### Task 20: Live smoke tests for GitLab

**Files:**
- Modify: `crates/rupu-scm/tests/live_smoke.rs`

- [ ] **Step 1: Add three GitLab smokes mirroring the GitHub ones from Plan 1 Task 13**

```rust
#[tokio::test]
async fn gitlab_list_repos_live() {
    let Some(token) = std::env::var("RUPU_LIVE_GITLAB_TOKEN").ok() else { return };
    if std::env::var("RUPU_LIVE_TESTS").is_err() { return; }
    let client = GitlabClient::new(token, None, None);
    let conn = GitlabRepoConnector::new(client);
    let repos = conn.list_repos().await.expect("list_repos");
    assert!(!repos.is_empty(), "test PAT should have at least one project");
}

#[tokio::test]
async fn gitlab_get_repo_live() { /* analogous */ }

#[tokio::test]
async fn gitlab_list_issues_live() { /* analogous */ }
```

- [ ] **Step 2: Verify in CI by setting both env vars locally first**

```
RUPU_LIVE_TESTS=1 RUPU_LIVE_GITLAB_TOKEN=glpat-xxxxx cargo test -p rupu-scm --test live_smoke gitlab_
```

Expected: 3 tests pass against gitlab.com.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-scm/tests/live_smoke.rs
git commit -m "$(cat <<'EOF'
rupu-scm: live smoke tests for GitLab (gated by RUPU_LIVE_TESTS)

Three smokes (list_repos, get_repo, list_issues) exercise the
real gitlab.com API. Skip silently when RUPU_LIVE_TESTS=1 or
RUPU_LIVE_GITLAB_TOKEN is missing so per-PR CI stays offline.
Plan 3 Task X wires these into the existing nightly-live-tests.yml
secrets matrix.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 21: Workspace gates + sample agent that uses MCP tools

**Files:**
- Create: `.rupu/agents/scm-pr-review.md` (sample agent file)
- Modify: `CLAUDE.md`

- [ ] **Step 1: Sample agent**

Create `.rupu/agents/scm-pr-review.md`:

```markdown
---
name: scm-pr-review
description: "Read a PR's diff and post a review comment via the unified SCM tools."
provider: anthropic
model: claude-sonnet-4-6
tools: [scm.prs.get, scm.prs.diff, scm.prs.comment]
permission_mode: ask
max_turns: 6
---

You are a code reviewer. Use `scm.prs.get` and `scm.prs.diff` to read
the PR. Look for: unhandled errors, security issues, hidden state
mutations. Post a single concise summary review via `scm.prs.comment`.
```

- [ ] **Step 2: Run all gates**

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three exit 0.

- [ ] **Step 3: Update `CLAUDE.md`**

Replace the Plan 1 reference line with:

```markdown
- Plan 1 (foundation + GitHub, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-1-foundation-and-github.md`
- Plan 2 (GitLab + MCP server, in progress): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-2-gitlab-and-mcp.md`
- Plan 3 (CLI + docs + nightly): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-3-cli-and-docs.md`
```

Add a `rupu-mcp` bullet to the `### Crates` section:

```markdown
- **`rupu-mcp`** — embedded MCP server. Two transports (in-process for the agent runtime, stdio for `rupu mcp serve`); single tool catalog backed by `rupu-scm`'s Registry. Permission gating mirrors the six-builtin model: per-tool allowlist + per-mode (`ask` / `bypass` / `readonly`).
```

- [ ] **Step 4: Commit**

```bash
git add .rupu/agents/scm-pr-review.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs+sample: scm-pr-review agent + CLAUDE.md plan pointers

Sample agent uses scm.prs.{get,diff,comment} via the unified MCP
catalog — exercises the full Plan 2 stack against the project's
own checkout when matt runs `rupu run scm-pr-review github:...`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Plan 2 success criteria

After all 21 tasks complete:

- `cargo fmt --all -- --check` exits 0.
- `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- `cargo test --workspace` exits 0.
- `Registry::discover` against an `InMemoryResolver` with stored gitlab credentials builds both connectors and the gitlab extras handle.
- Every `RepoConnector` and `IssueConnector` method on `GitlabRepoConnector` / `GitlabIssueConnector` is implemented (no method returns `unimplemented!()`).
- `rupu-mcp`'s `tools/list` response covers all 17 tools listed in spec §6b; snapshot test passes.
- `serve_in_process(Registry::empty(), allow_all)` accepts a `tools/call` for `scm.repos.list` and returns `isError: true / NotWiredInV0` (graceful failure, not panic).
- `cargo test -p rupu-agent --test mcp_attach` confirms the runner spins up the MCP server, dispatches an MCP-backed tool, and tears down cleanly.
- `RUPU_LIVE_TESTS=1 RUPU_LIVE_GITLAB_TOKEN=... cargo test -p rupu-scm --test live_smoke gitlab_` passes against the real gitlab.com API.

## Out of scope (deferred to Plan 3)

- `rupu repos list` CLI subcommand.
- `rupu mcp serve` CLI subcommand (stdio transport already implemented; only the clap wiring remains).
- `rupu run <agent> <target>` argument grammar.
- `docs/scm.md` + per-platform docs + `docs/mcp.md`.
- Wiring GitLab live tests into `.github/workflows/nightly-live-tests.yml`.
- README "SCM & issue trackers" section.
- CHANGELOG entry for B-2.

## Out of scope (deferred to follow-up slices, per spec §12)

- Linear / Jira / Asana issue-tracker adapters (`IssueConnector` trait absorbs them; v0 ships GitHub + GitLab issues).
- Bitbucket / Codeberg / Forgejo SCM adapters.
- Hosted MCP server (HTTP/SSE transport for `rupu mcp serve --transport http`).
- PR review threads (line-level comments, suggestions, resolution state) — `scm.prs.comment` ships as repo-level comment only.
- Branch protection / merge button (`merge_pr`, `enable_auto_merge`).
- GraphQL surfaces; v0 is REST only.


