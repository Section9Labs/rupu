# rupu Slice B-2: SCM + issue tracker connectors — Design

**Status:** Draft for review
**Date:** 2026-05-03
**Slice:** B-2 (second of three Slice B sub-projects)
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice B-1 design](./2026-05-02-rupu-slice-b1-multi-provider-design.md)

---

## 1. Goal

Add SCM/issue-tracker integration to rupu via a single in-process MCP server backed by typed per-platform connectors. Agents see one consistent tool surface (`scm.repos.list`, `scm.prs.comment`, `issues.get`, etc.) regardless of which platform the call resolves to. Vendor-specific code lives behind two trait families (`RepoConnector`, `IssueConnector`). Auth reuses Slice B-1's keychain + `CredentialResolver`.

This is the second of three Slice B sub-projects. B-1 (multi-provider auth) shipped. B-3 (`rupu init --with-samples`) follows.

## 2. Why

Coding agents need to read repo state and write back to it. Slice A made agents work on local checkouts; Slice B-2 lets them operate on remote repos, list PRs/issues, fetch PR diffs, post comments, open PRs. The platform-plugin shape matters from day 0 because the original Slice B brainstorm explicitly called out "support more than a single SCM and more than a single issue tracker; smart how we tackle this from day 0."

The MCP-server choice (vs shelling out to `gh`/`glab`, vs vendor-specific tools) gives us:
- One tool surface for agents regardless of platform.
- A standard way to expose the same surface to other MCP-aware clients (Claude Desktop, Cursor) via `rupu mcp serve`.
- Typed JSON results that don't burn agent tokens parsing CLI output.
- Single auth/keychain story shared with Slice B-1's LLM provider auth.

## 3. Architecture

Two new crates plus modifications to existing ones. Hexagonal: `rupu-scm` and `rupu-mcp` define ports; the agent runtime only knows traits / MCP.

| Crate | Status | Responsibility |
|---|---|---|
| `rupu-scm` | NEW | `RepoConnector` + `IssueConnector` traits, `ScmError`, per-platform impls (`github/`, `gitlab/`), credential resolution, cache+pagination+retry middleware. Internal — agents never call this directly. |
| `rupu-mcp` | NEW | Embedded MCP server runtime. Speaks MCP over stdio; reusable over SSE/HTTP later. Wraps `rupu-scm` as the unified tool surface for agents. |
| `rupu-agent` | MODIFY | Runner spins up `rupu-mcp` in-process at the start of every `rupu run`/`rupu workflow run`; tears it down at end. No agent-file changes for MCP itself. |
| `rupu-cli` | MODIFY | Adds `rupu mcp serve` (stdio MCP for external clients) and `rupu repos list`. `rupu run` accepts an optional `target` arg (`github:owner/repo#42`). `rupu auth login --provider github\|gitlab` extends Plan 1's flow with SCM scopes. |
| `rupu-auth` | MODIFY | `ProviderId::Github` and `ProviderId::Gitlab` join existing variants. OAuth scopes per provider extended. |
| `rupu-config` | MODIFY | New `[scm.default]`, `[issues.default]`, `[scm.<platform>]` sections. |
| `rupu-providers` | UNCHANGED | SCM is orthogonal to LLM providers. |

**Architectural rules preserved (from CLAUDE.md):**
- Hexagonal separation; agent runtime only knows traits/MCP.
- `rupu-cli` stays thin.
- Workspace deps only.
- `#![deny(clippy::all)]`, `unsafe_code` forbidden.

## 4. Core types

### 4a. `rupu-scm` types

```rust
pub enum Platform { Github, Gitlab }

pub enum IssueTracker { Github, Gitlab, Linear, Jira }
// B-2 ships connectors for Github + Gitlab only. Linear and Jira
// variants exist in the enum so adding adapters in a follow-up slice
// doesn't reshape the call sites — the spec for "smart from day 0"
// explicitly anticipates both. Code that matches on `IssueTracker`
// must include `_ => Err(NotWiredInV0(...))` arms for the unbuilt
// variants until they ship; honors the "no mock features" rule.

pub struct RepoRef { pub platform: Platform, pub owner: String, pub repo: String }
pub struct PrRef { pub repo: RepoRef, pub number: u32 }
pub struct IssueRef { pub tracker: IssueTracker, pub project: String, pub number: u64 }

pub struct Repo {
    pub r: RepoRef,
    pub default_branch: String,
    pub clone_url_https: String,
    pub clone_url_ssh: String,
    pub private: bool,
    pub description: Option<String>,
}

pub struct Pr {
    pub r: PrRef,
    pub title: String,
    pub body: String,
    pub state: PrState,        // Open | Closed | Merged
    pub head_branch: String,
    pub base_branch: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct Issue {
    pub r: IssueRef,
    pub title: String,
    pub body: String,
    pub state: IssueState,     // Open | Closed
    pub labels: Vec<String>,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct Branch { pub name: String, pub sha: String, pub protected: bool }
pub struct FileContent { pub path: String, pub ref_: String, pub content: String, pub encoding: FileEncoding }
pub struct Diff { pub patch: String, pub files_changed: u32, pub additions: u32, pub deletions: u32 }
pub struct Comment { pub id: String, pub author: String, pub body: String, pub created_at: DateTime<Utc> }
pub struct PrFilter { pub state: Option<PrState>, pub author: Option<String>, pub limit: Option<u32> }
pub struct IssueFilter { pub state: Option<IssueState>, pub labels: Vec<String>, pub author: Option<String>, pub limit: Option<u32> }
pub struct CreatePr { pub title: String, pub body: String, pub head: String, pub base: String, pub draft: bool }
pub struct CreateIssue { pub title: String, pub body: String, pub labels: Vec<String> }
```

### 4b. Error type

```rust
#[derive(thiserror::Error, Debug)]
pub enum ScmError {
    // Recoverable — surfaced to agent as JSON tool error
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
    #[error("transient: {0}")]
    Transient(#[source] anyhow::Error),
    #[error("conflict: {message}")]
    Conflict { message: String },
    #[error("not found: {what}")]
    NotFound { what: String },

    // Unrecoverable — aborts the run with an actionable message
    #[error("unauthorized for {platform}: {hint}")]
    Unauthorized { platform: String, hint: String },
    #[error("missing scope `{scope}` for {platform}: {hint}")]
    MissingScope { platform: String, scope: String, hint: String },
    #[error("network unreachable: {0}")]
    Network(#[source] anyhow::Error),
    #[error("bad request: {message}")]
    BadRequest { message: String },
}

impl ScmError {
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            ScmError::RateLimited { .. } | ScmError::Transient(_)
                | ScmError::Conflict { .. } | ScmError::NotFound { .. }
        )
    }
}
```

### 4c. Trait families

```rust
#[async_trait]
pub trait RepoConnector: Send + Sync {
    fn platform(&self) -> Platform;
    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError>;
    async fn get_repo(&self, r: &RepoRef) -> Result<Repo, ScmError>;
    async fn list_branches(&self, r: &RepoRef) -> Result<Vec<Branch>, ScmError>;
    async fn create_branch(&self, r: &RepoRef, name: &str, from_sha: &str) -> Result<Branch, ScmError>;
    async fn read_file(&self, r: &RepoRef, path: &str, ref_: Option<&str>) -> Result<FileContent, ScmError>;
    async fn list_prs(&self, r: &RepoRef, filter: PrFilter) -> Result<Vec<Pr>, ScmError>;
    async fn get_pr(&self, p: &PrRef) -> Result<Pr, ScmError>;
    async fn diff_pr(&self, p: &PrRef) -> Result<Diff, ScmError>;
    async fn comment_pr(&self, p: &PrRef, body: &str) -> Result<Comment, ScmError>;
    async fn create_pr(&self, r: &RepoRef, opts: CreatePr) -> Result<Pr, ScmError>;
    async fn clone_to(&self, r: &RepoRef, dir: &Path) -> Result<(), ScmError>;
}

#[async_trait]
pub trait IssueConnector: Send + Sync {
    fn tracker(&self) -> IssueTracker;
    async fn list_issues(&self, project: &str, filter: IssueFilter) -> Result<Vec<Issue>, ScmError>;
    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError>;
    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError>;
    async fn create_issue(&self, project: &str, opts: CreateIssue) -> Result<Issue, ScmError>;
    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError>;
}
```

### 4d. Registry

```rust
pub struct Registry {
    repo_connectors:  HashMap<Platform, Arc<dyn RepoConnector>>,
    issue_connectors: HashMap<IssueTracker, Arc<dyn IssueConnector>>,
}

impl Registry {
    /// Build from `~/.rupu/config.toml` + keychain credentials. Skips
    /// platforms the user hasn't authenticated to (logs INFO).
    pub async fn discover(resolver: &dyn CredentialResolver, cfg: &Config) -> Self;

    pub fn repo(&self, platform: Platform) -> Option<Arc<dyn RepoConnector>>;
    pub fn issues(&self, tracker: IssueTracker) -> Option<Arc<dyn IssueConnector>>;
}
```

## 5. Auth & credential resolution

Reuses Slice B-1's `CredentialResolver` end-to-end. New variants and scope additions only.

### 5a. New `ProviderId` variants

```rust
pub enum ProviderId {
    Anthropic, Openai, Gemini, Copilot, Local,
    Github,    // new (B-2)
    Gitlab,    // new (B-2)
}
```

`as_str()` returns `"github"` / `"gitlab"`. Keychain entries land at `rupu/github/<api-key|sso>` and `rupu/gitlab/<api-key|sso>`.

### 5b. OAuth scopes per platform

GitHub (device-code flow, mirrors Plan 2 Task 7's Copilot pattern):
```
read:user repo workflow gist read:org
```

GitLab (browser-callback PKCE flow for gitlab.com; self-hosted GitLab is deferred):
```
api read_user read_repository write_repository
```

### 5c. `rupu auth login` extension

- `--mode api-key`: prompts for / accepts `--key <PAT>`. Validates by probing `GET https://api.github.com/user` (or GitLab equivalent) with the token; reject the login if it 401s.
- `--mode sso`: device-code (GitHub) or browser-callback (GitLab) flow. Stores the token at `rupu/<provider>/sso` with refresh metadata.

### 5d. Credential routing inside `rupu-scm`

```rust
async fn github_repo_connector(resolver: &dyn CredentialResolver) -> Result<GithubRepoConnector, ScmError> {
    let (mode, creds) = resolver.get("github", None).await?;
    let token = match creds {
        AuthCredentials::ApiKey { key } => key,
        AuthCredentials::OAuth { access, .. } => access,
    };
    Ok(GithubRepoConnector::new(token, mode))
}
```

Same shape for GitLab. `Registry::discover` calls this once per platform. Missing credentials = platform skipped silently (logged at INFO), not a hard error.

### 5e. Scope drift detection

When a connector's first API call returns headers indicating insufficient scope (GitHub returns `X-Accepted-OAuth-Scopes` and `X-OAuth-Scopes`), the connector returns `ScmError::MissingScope { platform: "github", scope: "repo", hint: "Re-login to grant 'repo': rupu auth login --provider github --mode sso" }`. Unrecoverable — agent run aborts with the actionable message.

## 6. MCP server (agent-facing surface)

### 6a. Transport

- **Embedded (default)**: `rupu_mcp::serve_in_process(registry, transport)` returns a `JoinHandle`. Used by `rupu-agent`'s runner — no stdio, no subprocess. Tools invoked via direct in-process calls; the MCP layer enforces schema/permission contract.
- **Stdio (escape hatch)**: `rupu mcp serve` spawns the same server speaking JSON-RPC over stdin/stdout per the MCP standard. For Claude Desktop, Cursor, etc.
- **HTTP/SSE (deferred)**: `rupu mcp serve --transport http` returns `NotWiredInV0` in B-2; trait shape leaves room.

### 6b. Tool catalog

Each `RepoConnector` and `IssueConnector` method maps to one MCP tool. Tool names are namespaced:

- `scm.repos.list { platform?: string } -> Repo[]`
- `scm.repos.get { platform?, owner, repo } -> Repo`
- `scm.branches.list { platform?, owner, repo } -> Branch[]`
- `scm.branches.create { platform?, owner, repo, name, from_sha } -> Branch`
- `scm.files.read { platform?, owner, repo, path, ref?: string } -> FileContent`
- `scm.prs.list { platform?, owner, repo, state?, author? } -> Pr[]`
- `scm.prs.get { platform?, owner, repo, number } -> Pr`
- `scm.prs.diff { platform?, owner, repo, number } -> Diff`
- `scm.prs.comment { platform?, owner, repo, number, body } -> Comment`
- `scm.prs.create { platform?, owner, repo, title, body, head, base, draft? } -> Pr`
- `issues.list { tracker?, project?, state?, labels? } -> Issue[]`
- `issues.get { tracker?, project, number } -> Issue`
- `issues.comment { tracker?, project, number, body } -> Comment`
- `issues.create { tracker?, project, title, body, labels? } -> Issue`
- `issues.update_state { tracker?, project, number, state } -> ()`
- `github.workflows_dispatch { owner, repo, workflow, ref, inputs? } -> ()`
- `gitlab.pipeline_trigger { project, ref, variables? } -> ()`

`platform?` / `tracker?` / `repo?` are optional — if omitted, the server falls back to `[scm.default]` / `[issues.default]` from config.

### 6c. Schemas

Generated via `schemars` from the `rupu-scm` Rust types. Schemas are part of the MCP `tools/list` response, so agents (and any MCP-aware client) self-document.

### 6d. Permission gating

- **Per-tool**: agent's `tools:` frontmatter list must contain the tool name (or its prefix — `scm.*` allowlists everything in the namespace).
- **Per-mode**: `--mode readonly` blocks every write tool (`scm.prs.comment`, `scm.prs.create`, `scm.branches.create`, `issues.comment`, `issues.create`, `issues.update_state`, `github.workflows_dispatch`, `gitlab.pipeline_trigger`).
- **Per-mode**: `--mode ask` prompts for confirmation on writes (same UX as `bash` / `write_file`).

### 6e. Transcript integration

Every MCP tool call gets `Event::ToolCall` and `Event::ToolResult` events in the JSONL transcript (existing Slice A schema; no change). `ScmError`-classified errors appear in `tool_result.error` with `code` set to the variant name.

## 7. CLI surface

### 7a. New subcommands

```
rupu repos list [--platform <name>]
rupu mcp serve [--transport stdio|http]
```

Existing `rupu auth login --provider github --mode sso` already exists from Plan 2; B-2 adds the broader scope set.
Existing `rupu auth status` automatically picks up the new `github` / `gitlab` rows since it iterates `ProviderId` variants.

### 7b. Modified subcommands

`rupu run <agent> [target]` — `target` becomes optional. Three shapes:

```
rupu run review-pr                                       # uses [scm.default]
rupu run review-pr github:section9labs/rupu#42           # PR target — clones into tmpdir
rupu run fix-issue github:section9labs/rupu/issues/123   # issue target — same + issues.get pre-populated
```

The `target` parses into:

```rust
pub enum RunTarget {
    None,
    Repo { platform: Platform, owner: String, repo: String, ref_: Option<String> },
    Pr { platform: Platform, owner: String, repo: String, number: u32 },
    Issue { tracker: IssueTracker, project: String, number: u64 },
}
```

It's preloaded into the system prompt as a `## Run target` section so agents know what they're operating on.

`rupu workflow run <wf> [target]` — same target-arg extension; passed down to step agents.

### 7c. New config sections

```toml
# ~/.rupu/config.toml or <repo>/.rupu/config.toml
[scm.default]
platform = "github"
owner = "section9labs"
repo = "rupu"

[issues.default]
tracker = "github"
project = "section9labs/rupu"

[scm.github]
base_url = "https://api.github.com"            # overridable for GHES
timeout_ms = 30000
max_concurrency = 8
clone_protocol = "https"                       # https | ssh

[scm.gitlab]
base_url = "https://gitlab.com/api/v4"         # overridable for self-hosted
clone_protocol = "https"
```

Schema in `rupu-config` as new `ScmDefault`, `IssuesDefault`, `ScmPlatformConfig` structs. Same `#[serde(default, skip_serializing_if = ...)]` discipline as Plan 1's `ProviderConfig`.

### 7d. Help text grammar

```
TARGET formats:
  github:owner/repo                          # repo
  github:owner/repo#42                       # PR
  github:owner/repo/issues/123               # issue
  gitlab:group/project                       # repo
  gitlab:group/project!7                     # MR (gitlab uses !)
  gitlab:group/project/issues/9              # issue
```

## 8. Agent-side surface

### 8a. Frontmatter

```yaml
# .rupu/agents/review-pr.md
provider: anthropic
model: claude-sonnet-4-6
tools: [read_file, bash, scm.repos.get, scm.prs.get, scm.prs.diff, scm.prs.comment]
```

`tools:` list gates which MCP tools the agent can invoke (per-tool opt-in, principle of least privilege). Wildcards: `scm.*` allowlists all SCM tools.

### 8b. File access strategy (hybrid)

- **In-clone**: agent uses existing `read_file` / `bash` / `grep` / `glob` tools on the working tree. Default for all `rupu run` invocations from inside a checkout.
- **Cross-repo / specific-ref**: `scm.files.read` is the escape hatch. Agent supplies `(platform, owner, repo, path, ref?)`.

Two `rupu run` shapes (Section 7b):
- Inside an existing clone → no clone needed, agent uses working tree.
- Remote `target` (`github:owner/repo#42`) → rupu clones into a tmpdir; agent operates there.

### 8c. Writes

All writes (branches, commits to remote, comments, PRs) go through the SCM API tools, NOT via local `git push` from the working tree. Keeps auth/audit consistent and avoids "what if `git push` uses a different credential than what rupu authorized." Local commits to the working tree are fine — they just need to round-trip through `scm.branches.create` + `scm.prs.create` to land remotely.

## 9. Concurrency, caching, retry, error classification

### 9a. Per-platform semaphore

Reuses Plan 1 Task 10's `concurrency::semaphore_for` registry, keyed by `Platform` instead of LLM provider:

| Platform | Permits | Reason |
|---|---|---|
| github  | 8 | 5000 req/hr authenticated, ~1.4 req/s sustained |
| gitlab  | 6 | 2000 req/hr default (gitlab.com), more conservative |

Override per platform via `[scm.<name>].max_concurrency`.

### 9b. ETag cache

In-memory `LruCache<(Platform, String), CacheEntry>` per connector. Keys are URL paths; values store `etag`, `body_json`, `inserted_at`. Default capacity 256; TTL 5 min. Governs `get_*` calls only; `list_*` not cached. 304 short-circuits without re-deserializing. `cache: false` arg bypasses for force-refresh.

### 9c. Retry / backoff

Reuses Plan 1's retry harness:
- `RateLimited`: wait `retry_after` if present; else exponential with jitter, capped at 60s.
- `Transient` (5xx, network blip): exponential with jitter, capped at 60s, max 5 attempts.
- `Conflict`, `NotFound`, `BadRequest`: no retry.
- `Unauthorized`, `MissingScope`, `Network` (full DNS fail): no retry, abort run.

### 9d. Error classification

```rust
fn classify_scm_error(platform: Platform, status: u16, body: &str, headers: &HeaderMap) -> ScmError;
```

| Signal | Variant |
|---|---|
| HTTP 401 | `Unauthorized { hint }` |
| HTTP 403 + `X-OAuth-Scopes` missing required scope | `MissingScope { scope, hint }` |
| HTTP 403 (other) / 429 | `RateLimited { retry_after: parse_retry_after(headers) }` |
| HTTP 404 | `NotFound { what }` |
| HTTP 409 / 422 (write conflict) | `Conflict { message }` |
| HTTP 400 / 422 (validation) | `BadRequest { message }` |
| HTTP 500-503 | `Transient` |
| Connection refused / timeout | `Network` |
| Anything else | `Transient` (recoverable on safe side) |

Pure function, table-driven tests per platform — same shape as Plan 1 Task 11.

### 9e. Connector lifecycle

One `OnceLock<reqwest::Client>` per platform, lazily built from `[scm.<name>]` config (timeout, retries, base_url). `git2` clone uses its own connection per call.

## 10. Testing strategy

### 10a. Per-connector unit tests (`rupu-scm`)

- **Translation tests**: feed recorded JSON fixtures; assert deserialization → `Repo` / `Pr` / `Issue` / etc. correctly maps every field. One fixture per scenario per platform:
  - happy-path response (open PR with reviewers, comments, labels)
  - empty list, paginated list (3+ pages)
  - 401 unauthorized
  - 403 with scope header indicating missing scope
  - 429 rate-limit response with `Retry-After`
  - 404 not-found
  - 409/422 conflict on write
- **Pure-function `classify_scm_error` tests**: table-driven, one row per `(platform, status, body, headers) → ScmError` mapping.
- **httpmock-based integration tests** per connector method: full request → response round-trip.
- **`Registry::discover` tests** with `InMemoryResolver`: credentials present → connector built; absent → platform skipped silently.

### 10b. MCP-server tests (`rupu-mcp`)

- **Schema generation**: snapshot `tools/list` response per tool.
- **In-process invocation**: instantiate `serve_in_process(registry, ...)` against mock connectors; assert each tool name dispatches to the right method with the right parameters.
- **Default-fallback**: tool call omits `platform` arg → server fills from `[scm.default]`.
- **Permission gating**: tool not in agent's allowlist → error before reaching the connector. `--mode readonly` rejects every write tool.

### 10c. End-to-end tests (`rupu-cli`)

- `rupu repos list`: real flow with `InMemoryResolver` + mock connectors → table-rendered output.
- `rupu run` with target arg: parse `github:owner/repo#42` → `RunTarget::Pr(42)` → mock connector returns canned PR → agent's first message has the preloaded `## Run target` section. Use `RUPU_MOCK_PROVIDER_SCRIPT` for the LLM side.
- `rupu mcp serve --transport stdio`: spawn the binary, send a `tools/list` JSON-RPC request, assert response matches snapshot.

### 10d. Live integration tests (gated)

Extends Plan 3 Task 12's `RUPU_LIVE_TESTS=1` workflow:

- `RUPU_LIVE_GITHUB_TOKEN` — real PAT with `repo` + `read:user`.
- `RUPU_LIVE_GITLAB_TOKEN` — real PAT with `api` + `read_repository`.

Per-platform smoke tests: `list_repos`, `get_repo`, `read_file`, `list_prs`. Skipped silently when env vars absent. Run nightly via `nightly-live-tests.yml`.

### 10e. Fixture management

- Recorded JSON: `crates/rupu-scm/tests/fixtures/<platform>/<endpoint>/*.json`.
- Regen scripts: `crates/rupu-scm/tests/fixtures/regen-<platform>.sh` (uses real PAT + curl). Documented in the crate's README.

### 10f. Honors "no mock features" rule

Every connector method exercises real wire bytes (recorded fixtures + httpmock); no method returns a hardcoded `Ok(...)` placeholder. Live nightlies catch vendor drift.

## 11. Documentation

- **`README.md`** — adds "SCM & issue trackers" quick-start matrix (GitHub, GitLab × Repo/Issues, `rupu mcp serve`).
- **`docs/scm.md`** — canonical reference: platforms × capabilities matrix, connector model, MCP tool catalog with schemas, `target` arg grammar, caching/pagination/retry/error tables, config schema, troubleshooting.
- **`docs/scm/github.md`** — PAT acquisition, SSO walkthrough, example agent file, known quirks (GraphQL vs REST coverage, fine-grained PAT limitations, GHES `base_url`).
- **`docs/scm/gitlab.md`** — PAT acquisition, SSO walkthrough, example agent file, known quirks (gitlab.com vs self-hosted, MR-vs-PR vocabulary, group nesting in project paths).
- **`docs/mcp.md`** — short doc for users wiring `rupu mcp serve` into Claude Desktop / Cursor / external orchestrators. Sample `claude_desktop_config.json` snippet, full tool catalog reframed for MCP-client consumers.
- **`docs/providers/github.md`** — append a one-line cross-reference to `docs/scm/github.md` (shared keychain entry).
- **`CHANGELOG.md`** — Slice B-2 release notes.
- **In-code doc comments** — every public type in `rupu-scm` and `rupu-mcp` carries at minimum a one-line summary; non-trivial APIs include a usage example.

## 12. Out of scope

- Bitbucket, Codeberg, Forgejo, self-hosted GitLab — trait designed to absorb them; v0 ships gitlab.com only. Self-hosted GitLab works via `[scm.gitlab].base_url` override but isn't formally supported / tested.
- Linear, Jira, Asana issue trackers — `IssueConnector` trait is designed to absorb them; v0 ships GitHub Issues + GitLab Issues only.
- Webhook / poll trigger model — Slice D concern.
- Hosted MCP server (HTTP/SSE transport) — `rupu mcp serve --transport http` returns `NotWiredInV0`.
- Cross-platform repo + tracker pairing in the wild — config schema admits it but B-2 only ships same-platform pairings end-to-end.
- GraphQL surface for richer queries — REST only.
- Repo content writes via local clone + `git push` — writes always via SCM API.
- Code search across repos — defer until user demand surfaces.
- Repo-scoped fine-grained tokens — v0 uses classic PATs; documented gotcha in `docs/scm/github.md`.
- PR/MR review threads (line-level comments, suggestions, resolution state) — `prs.comment` ships as repo-level comment only.
- Branch protection / merge button (`merge_pr`, `enable_auto_merge`, etc.) — not in v0.
- Repo / issue search across all platforms — list filters per platform only.
- Hosted git-credential helper integration — `git clone` over HTTPS uses the token directly.
- MCP `resources/` and `prompts/` (vs `tools/`) — B-2 ships tools only.

## 13. Risks

- **MCP standard drift.** MCP is an emerging spec; tool definitions / transport details may change. Mitigation: pin to a specific MCP version; live-integration tests catch breakage.
- **Vendor API changes.** GitHub and GitLab evolve their APIs. Mitigation: recorded fixtures + nightly live tests + clear error messages when fields are missing.
- **Scope drift / re-login burden.** A token issued with `repo` scope today may not satisfy a future tool's needs. Mitigation: probe scopes on first use; surface `MissingScope` with the actionable re-login command.
- **Cache staleness.** ETag-based caching returns stale data if vendor doesn't advance the ETag for a meaningful change. Mitigation: short TTL (5 min) + explicit `cache: false` bypass.
- **GitLab self-hosted variation.** Self-hosted GitLab versions diverge in API behavior. Mitigation: not formally supported in v0; documented in `docs/scm/gitlab.md`.
- **Auth token loss / user fatigue.** Multi-vendor auth means more tokens to manage. Mitigation: shared single `rupu auth login --provider X --mode sso` UX; `rupu auth status` shows everything in one matrix.
- **Bare token in agent context.** If an agent writes a tool call result containing a token to its system prompt, the token leaks. Mitigation: connector responses never include the token; transcript scrubbing (already in place from Slice A).

## 14. Success criteria

- `rupu auth login --provider github --mode sso` and `rupu auth login --provider gitlab --mode sso` both complete end-to-end.
- `rupu repos list` shows authenticated user's repos across both platforms.
- `rupu run review-pr github:owner/repo#42` clones the repo, agent fetches PR diff via `scm.prs.diff`, posts review via `scm.prs.comment`, exits cleanly.
- `rupu run fix-issue github:owner/repo/issues/123` clones, agent reads issue via `issues.get`, makes code changes locally, opens a PR via `scm.prs.create`.
- `rupu mcp serve` over stdio responds to `tools/list` with the full tool catalog (schemas valid).
- An external MCP client (Claude Desktop) configured to spawn `rupu mcp serve` can call `scm.repos.list` and get the same response as `rupu repos list`.
- `Registry::discover` tolerates platforms the user hasn't authenticated to (skipped silently with INFO log).
- Recorded-fixture translation tests pass for every connector method × scenario.
- Nightly live tests pass for GitHub + GitLab when the appropriate PAT secrets are configured.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` all green.
