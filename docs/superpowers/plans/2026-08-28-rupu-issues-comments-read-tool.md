# `issues.comments` Read Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give agents a way to *read* an issue's comment thread, closing the one gap that blocks a comment-driven human control channel.

**Architecture:** Add `list_comments` to the `IssueConnector` trait with a default "not supported" implementation (mirroring the existing `add_pr_labels` pattern at `crates/rupu-scm/src/connectors/mod.rs:75`, so the GitLab / Linear / Jira connectors and the registry's test mocks keep compiling untouched). Override it in the GitHub connector. Expose it as a new read-kind MCP tool `issues.comments`.

**Tech Stack:** Rust 2021, `async_trait`, `octocrab` 0.42, `httpmock` for connector tests, `schemars` for MCP tool input schemas, `insta`-style JSON snapshot for the tool catalog.

**Spec:** `~/Security/Chimera/docs/specs/2026-08-28-chimera-design.md` §12.1 (this change is a prerequisite for Chimera; it is a general-purpose rupu capability, not Chimera-specific).

## Global Constraints

- Rust 2021; MSRV pinned in `rust-toolchain.toml`.
- **Workspace deps only** — versions are pinned in the root `Cargo.toml`, never in a crate's `Cargo.toml`. This change adds no new dependency.
- `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`; `unsafe_code` forbidden.
- Libraries use `thiserror`; the CLI binary uses `anyhow`. This change is library-only.
- **Never run package-wide `cargo fmt`** — `main` is fmt-dirty under the pinned toolchain. Format only the specific files you touched: `cargo fmt -- <path>`.
- **All work goes through a feature branch and a PR.** Never commit to `main`.
- Hexagonal rule: `rupu-scm` defines the port; `rupu-mcp` adapts it. No business logic in `rupu-cli`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/rupu-scm/src/connectors/mod.rs` | `IssueConnector` port definition | Modify — add `list_comments` with a default impl |
| `crates/rupu-scm/src/connectors/github/issues.rs` | GitHub `IssueConnector` adapter | Modify — override `list_comments` |
| `crates/rupu-scm/tests/fixtures/github/issue_comments_happy.json` | Canned GitHub API response | Create |
| `crates/rupu-scm/tests/github_translation.rs` | GitHub adapter translation tests | Modify — add two tests |
| `crates/rupu-mcp/src/tools/issues.rs` | `issues.*` MCP tool specs + dispatchers | Modify — add args struct, `ToolSpec`, `dispatch_comments` |
| `crates/rupu-mcp/src/dispatcher.rs` | Tool-name → dispatcher routing | Modify — add one match arm |
| `crates/rupu-mcp/tests/snapshots/tools_list.json` | Tool catalog snapshot | Modify — regenerate |
| `CLAUDE.md` | Documented native tool catalog | Modify — one line |

---

### Task 1: `list_comments` on the `IssueConnector` port + GitHub adapter

**Files:**
- Modify: `crates/rupu-scm/src/connectors/mod.rs` (inside `pub trait IssueConnector`, after `comment_issue`)
- Modify: `crates/rupu-scm/src/connectors/github/issues.rs` (inside `impl IssueConnector for GithubIssueConnector`, after `comment_issue`)
- Create: `crates/rupu-scm/tests/fixtures/github/issue_comments_happy.json`
- Test: `crates/rupu-scm/tests/github_translation.rs`

**Interfaces:**
- Consumes: existing `Comment` (`crates/rupu-scm/src/types.rs:252` — fields `id: String`, `author: String`, `body: String`, `created_at: DateTime<Utc>`), `IssueRef`, `ScmError`, and the test helper `common::github_issue_connector_against(&server) -> Arc<dyn IssueConnector>` (`crates/rupu-scm/tests/common/mod.rs`).
- Produces: `IssueConnector::list_comments(&self, i: &IssueRef, limit: Option<u32>) -> Result<Vec<Comment>, ScmError>`. Returns comments oldest-first (GitHub's native order). `limit` caps the number returned; `None` means "the connector's default page" (100).

- [ ] **Step 1: Write the failing tests**

Create the fixture `crates/rupu-scm/tests/fixtures/github/issue_comments_happy.json`. This is the shape GitHub's `GET /repos/{owner}/{repo}/issues/{n}/comments` returns — a bare JSON array:

```json
[
  {
    "id": 1001,
    "node_id": "IC_kwDO1",
    "url": "https://api.github.com/repos/section9labs/rupu/issues/42/comments/1001",
    "html_url": "https://github.com/section9labs/rupu/issues/42#issuecomment-1001",
    "issue_url": "https://api.github.com/repos/section9labs/rupu/issues/42",
    "body": "first comment body",
    "created_at": "2026-08-01T10:00:00Z",
    "updated_at": "2026-08-01T10:00:00Z",
    "user": {
      "login": "mrbrutti",
      "id": 5001,
      "node_id": "U_kgDO1",
      "avatar_url": "https://avatars.githubusercontent.com/u/5001?v=4",
      "gravatar_id": "",
      "url": "https://api.github.com/users/mrbrutti",
      "html_url": "https://github.com/mrbrutti",
      "followers_url": "https://api.github.com/users/mrbrutti/followers",
      "following_url": "https://api.github.com/users/mrbrutti/following{/other_user}",
      "gists_url": "https://api.github.com/users/mrbrutti/gists{/gist_id}",
      "starred_url": "https://api.github.com/users/mrbrutti/starred{/owner}{/repo}",
      "subscriptions_url": "https://api.github.com/users/mrbrutti/subscriptions",
      "organizations_url": "https://api.github.com/users/mrbrutti/orgs",
      "repos_url": "https://api.github.com/users/mrbrutti/repos",
      "events_url": "https://api.github.com/users/mrbrutti/events{/privacy}",
      "received_events_url": "https://api.github.com/users/mrbrutti/received_events",
      "type": "User",
      "site_admin": false
    },
    "author_association": "OWNER"
  },
  {
    "id": 1002,
    "node_id": "IC_kwDO2",
    "url": "https://api.github.com/repos/section9labs/rupu/issues/42/comments/1002",
    "html_url": "https://github.com/section9labs/rupu/issues/42#issuecomment-1002",
    "issue_url": "https://api.github.com/repos/section9labs/rupu/issues/42",
    "body": "second comment body",
    "created_at": "2026-08-02T11:30:00Z",
    "updated_at": "2026-08-02T11:30:00Z",
    "user": {
      "login": "octocat",
      "id": 5002,
      "node_id": "U_kgDO2",
      "avatar_url": "https://avatars.githubusercontent.com/u/5002?v=4",
      "gravatar_id": "",
      "url": "https://api.github.com/users/octocat",
      "html_url": "https://github.com/octocat",
      "followers_url": "https://api.github.com/users/octocat/followers",
      "following_url": "https://api.github.com/users/octocat/following{/other_user}",
      "gists_url": "https://api.github.com/users/octocat/gists{/gist_id}",
      "starred_url": "https://api.github.com/users/octocat/starred{/owner}{/repo}",
      "subscriptions_url": "https://api.github.com/users/octocat/subscriptions",
      "organizations_url": "https://api.github.com/users/octocat/orgs",
      "repos_url": "https://api.github.com/users/octocat/repos",
      "events_url": "https://api.github.com/users/octocat/events{/privacy}",
      "received_events_url": "https://api.github.com/users/octocat/received_events",
      "type": "User",
      "site_admin": false
    },
    "author_association": "CONTRIBUTOR"
  }
]
```

Append these two tests to `crates/rupu-scm/tests/github_translation.rs`:

```rust
#[tokio::test]
async fn list_comments_translates() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let body =
        std::fs::read_to_string("tests/fixtures/github/issue_comments_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/issues/42/comments");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let comments = c
        .list_comments(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Github,
                project: "section9labs/rupu".into(),
                number: 42,
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(comments.len(), 2);
    // Oldest-first: GitHub's native order is preserved, which is what a
    // "what changed since my last iteration?" reader depends on.
    assert_eq!(comments[0].id, "1001");
    assert_eq!(comments[0].author, "mrbrutti");
    assert_eq!(comments[0].body, "first comment body");
    assert_eq!(comments[1].author, "octocat");
    assert_eq!(comments[1].body, "second comment body");
    assert!(comments[0].created_at < comments[1].created_at);
}

#[tokio::test]
async fn list_comments_respects_limit() {
    rupu_scm::install_default_crypto_provider();
    let server = MockServer::start();
    let body =
        std::fs::read_to_string("tests/fixtures/github/issue_comments_happy.json").unwrap();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/section9labs/rupu/issues/42/comments");
        then.status(200)
            .header("content-type", "application/json")
            .body(&body);
    });
    let c = common::github_issue_connector_against(&server);
    let comments = c
        .list_comments(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Github,
                project: "section9labs/rupu".into(),
                number: 42,
            },
            Some(1),
        )
        .await
        .unwrap();

    // The server returned two; `limit` truncates client-side so the
    // contract holds regardless of what the API page actually contained.
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "1001");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-scm --test github_translation list_comments`
Expected: FAIL to **compile**, with `no method named 'list_comments' found for reference '&Arc<dyn IssueConnector>'`.

- [ ] **Step 3: Add the trait method with a default implementation**

In `crates/rupu-scm/src/connectors/mod.rs`, inside `pub trait IssueConnector`, immediately after the `comment_issue` line:

```rust
    /// Read an issue's comment thread, oldest-first.
    ///
    /// `limit` caps the number of comments returned; `None` means the
    /// connector's default page size. Default implementation is
    /// unsupported (mirroring `add_pr_labels`) so trackers that have no
    /// comment-read surface — or test mocks — need no change. Only
    /// platforms that override it can read comments.
    async fn list_comments(
        &self,
        i: &IssueRef,
        limit: Option<u32>,
    ) -> Result<Vec<Comment>, ScmError> {
        let _ = (i, limit);
        Err(ScmError::BadRequest {
            message: format!("list_comments is not supported by {}", self.tracker()),
        })
    }
```

- [ ] **Step 4: Implement the GitHub override**

In `crates/rupu-scm/src/connectors/github/issues.rs`, inside `impl IssueConnector for GithubIssueConnector`, immediately after the `comment_issue` method:

```rust
    async fn list_comments(
        &self,
        i: &IssueRef,
        limit: Option<u32>,
    ) -> Result<Vec<Comment>, ScmError> {
        let _permit = self.client.permit().await;
        let (owner, repo) = parse_project(&i.project)?;
        let number = i.number;
        let inner = self.client.inner.clone();
        // GitHub caps per_page at 100; clamp so an absurd `limit` can't
        // produce a 422 from the API.
        let per_page: u8 = limit.unwrap_or(100).clamp(1, 100) as u8;
        let page = self
            .client
            .with_retry_octocrab(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                async move {
                    inner
                        .issues(&owner, &repo)
                        .list_comments(number)
                        .per_page(per_page)
                        .send()
                        .await
                        .map_err(super::client::classify_octocrab_error)
                }
            })
            .await?;

        let mut out: Vec<Comment> = page
            .items
            .into_iter()
            .map(|m| Comment {
                id: m.id.to_string(),
                author: m.user.login,
                // octocrab models an issue comment body as Option<String>
                // (a comment can be body-less after redaction).
                body: m.body.unwrap_or_default(),
                created_at: m.created_at,
            })
            .collect();
        if let Some(n) = limit {
            out.truncate(n as usize);
        }
        Ok(out)
    }
```

If `octocrab` 0.42's builder differs from `list_comments(number).per_page(u8).send()`, keep the same shape and adapt the builder calls — the translation into `Comment` above is the part that matters and is what the tests assert.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-scm --test github_translation list_comments`
Expected: PASS, both tests.

- [ ] **Step 6: Verify nothing else broke**

Run: `cargo test -p rupu-scm`
Expected: PASS. The default trait implementation means the GitLab, Linear, and Jira connectors and `registry.rs`'s mocks compile unchanged — if any of them fail to compile, the default impl was written as a required method by mistake.

Run: `cargo clippy -p rupu-scm --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Format only the touched files and commit**

```bash
cd ~/Code/Oracle/rupu
git checkout -b feat/issues-comments-read-tool
cargo fmt -- crates/rupu-scm/src/connectors/mod.rs crates/rupu-scm/src/connectors/github/issues.rs
git add crates/rupu-scm/src/connectors/mod.rs \
        crates/rupu-scm/src/connectors/github/issues.rs \
        crates/rupu-scm/tests/fixtures/github/issue_comments_happy.json \
        crates/rupu-scm/tests/github_translation.rs
git commit -m "feat(scm): add IssueConnector::list_comments with GitHub impl

The IssueConnector port was write-only for comments (comment_issue with
no reader), so an agent could be woken by github.issue.commented but
could not read what the comment said. Adds list_comments with a default
unsupported impl (same pattern as add_pr_labels) so GitLab/Linear/Jira
and the registry mocks are untouched, plus a GitHub override.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `issues.comments` MCP tool

**Files:**
- Modify: `crates/rupu-mcp/src/tools/issues.rs` (module doc line 1; args structs after `CommentIssueArgs`; `specs()` vec after the `issues.get` entry; dispatchers after `dispatch_get`)
- Modify: `crates/rupu-mcp/src/dispatcher.rs:56` (add a match arm)
- Modify: `crates/rupu-mcp/tests/snapshots/tools_list.json` (regenerate)
- Test: `crates/rupu-mcp/src/tools/issues.rs` (inline `#[cfg(test)]`), `crates/rupu-mcp/tests/schema_snapshot.rs` (existing, regenerated)

**Interfaces:**
- Consumes: `IssueConnector::list_comments(&self, i: &IssueRef, limit: Option<u32>) -> Result<Vec<Comment>, ScmError>` from Task 1; the existing `resolve_tracker(Option<&str>, &Registry) -> Result<IssueTracker, McpError>` helper in the same module; `ToolSpec`, `ToolKind` from `super`.
- Produces: MCP tool `issues.comments`, `ToolKind::Read`, dispatched by `tools::issues::dispatch_comments(args: Value, reg: &Registry) -> Result<String, McpError>`. Returns a JSON array of `Comment` objects.

- [ ] **Step 1: Write the failing test**

Append to `crates/rupu-mcp/src/tools/issues.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_tool_is_in_catalog_and_is_read_kind() {
        let s = specs();
        let spec = s
            .iter()
            .find(|s| s.name == "issues.comments")
            .expect("issues.comments must be in the issues tool catalog");
        // Read-kind matters: a readonly permission mode must still be
        // able to read a comment thread, which is the whole point of the
        // operator control channel.
        assert_eq!(spec.kind, ToolKind::Read);
        let schema = &spec.input_schema;
        let props = &schema["properties"];
        assert!(props.get("project").is_some(), "project is required input");
        assert!(props.get("number").is_some(), "number is required input");
        assert!(props.get("limit").is_some(), "limit is an accepted input");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-mcp comments_tool_is_in_catalog`
Expected: FAIL with a panic on `issues.comments must be in the issues tool catalog`.

(If it instead fails to compile on `assert_eq!(spec.kind, ToolKind::Read)` because `ToolKind` lacks `PartialEq`/`Debug`, add `#[derive(PartialEq, Debug)]` to `ToolKind` in `crates/rupu-mcp/src/tools/mod.rs` — it is a plain enum and this is the minimal fix.)

- [ ] **Step 3: Add the args struct and the tool spec**

In `crates/rupu-mcp/src/tools/issues.rs`, change the module doc on line 1 to:

```rust
//! issues.{list, get, comments, comment, create, update_state} tools.
```

Add after `CommentIssueArgs`:

```rust
#[derive(Deserialize, JsonSchema)]
pub struct ListCommentsArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
    /// Maximum comments to return, oldest-first. Defaults to 100.
    pub limit: Option<u32>,
}
```

Add to the `specs()` vec, immediately after the `issues.get` entry (the catalog has a stable order that a snapshot test asserts, so position matters):

```rust
        ToolSpec {
            name: "issues.comments",
            description: "Read an issue's comment thread, oldest-first. Returns id, author, body, created_at per comment.",
            input_schema: serde_json::to_value(schemars::schema_for!(ListCommentsArgs)).unwrap(),
            kind: ToolKind::Read,
        },
```

- [ ] **Step 4: Add the dispatcher**

Add to `crates/rupu-mcp/src/tools/issues.rs`, after `dispatch_get`:

```rust
pub async fn dispatch_comments(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListCommentsArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let r = IssueRef {
        tracker,
        project: parsed.project,
        number: parsed.number,
    };
    let conn = reg
        .issues(tracker)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {tracker}")))?;
    Ok(serde_json::to_string(&conn.list_comments(&r, parsed.limit).await?).unwrap())
}
```

In `crates/rupu-mcp/src/dispatcher.rs`, after the `"issues.get"` arm at line 56:

```rust
            "issues.comments" => tools::issues::dispatch_comments(args, &self.registry).await,
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p rupu-mcp comments_tool_is_in_catalog`
Expected: PASS.

- [ ] **Step 6: Regenerate the tool catalog snapshot**

Run: `cargo test -p rupu-mcp --test schema_snapshot`
Expected: FAIL — the catalog now has one more tool than `crates/rupu-mcp/tests/snapshots/tools_list.json` records.

Inspect the diff the failure prints. Confirm the **only** change is the addition of `issues.comments` between `issues.get` and `issues.comment`. If anything else moved, the spec was inserted in the wrong position — fix the position rather than accepting the snapshot.

Update the snapshot file to include the new entry, then re-run:

Run: `cargo test -p rupu-mcp --test schema_snapshot`
Expected: PASS.

- [ ] **Step 7: Verify the whole crate and the stdio smoke test**

Run: `cargo test -p rupu-mcp && cargo test -p rupu-cli --test mcp_serve_stdio_smoke`
Expected: PASS. The smoke test asserts specific tool names are present (`crates/rupu-cli/tests/mcp_serve_stdio_smoke.rs:55,60`) rather than a count, so it should pass unchanged — if it fails, read the assertion before editing it.

Run: `cargo clippy -p rupu-mcp --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Format touched files and commit**

```bash
cd ~/Code/Oracle/rupu
cargo fmt -- crates/rupu-mcp/src/tools/issues.rs crates/rupu-mcp/src/dispatcher.rs
git add crates/rupu-mcp/src/tools/issues.rs \
        crates/rupu-mcp/src/dispatcher.rs \
        crates/rupu-mcp/tests/snapshots/tools_list.json
git commit -m "feat(mcp): add issues.comments read tool

Exposes IssueConnector::list_comments as a read-kind MCP tool so agents
can read an issue's comment thread, not just post to it.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Document the tool and open the PR

**Files:**
- Modify: `CLAUDE.md` (the "Native SCM MCP tool catalog" sentence inside the `rupu-orchestrator` bullet)

**Interfaces:**
- Consumes: the completed `issues.comments` tool from Task 2.
- Produces: nothing code-facing. This task exists because `CLAUDE.md` enumerates the native tool catalog verbatim, and a stale catalog there causes agents to shell out to `gh` instead of using the native tool.

- [ ] **Step 1: Update the documented catalog**

In `CLAUDE.md`, find this text inside the `rupu-orchestrator` bullet:

```
Native SCM MCP tool catalog: `issues.{list,get,comment,create,update_state}`
```

Replace it with:

```
Native SCM MCP tool catalog: `issues.{list,get,comments,comment,create,update_state}`
```

- [ ] **Step 2: Update the label-gap note in the same file**

`CLAUDE.md` currently states there is no native issue-label-write tool. That is still true and should stay, but the comment-read gap it implies is now closed. Find:

```
No native issue-**label**-write tool exists (`issues.update_state` is open/closed only).
```

Replace with:

```
No native issue-**label**-write tool exists (`issues.update_state` is open/closed only); comment *reading* is available via `issues.comments` (added 2026-08-28) alongside `issues.comment` for writing.
```

- [ ] **Step 3: Verify the full workspace is green**

Run: `cargo test --workspace`
Expected: PASS.

If `cargo test -p rupu-cp` fails on fixture drift, that means a serde type the macOS app consumes changed. This change touches no `rupu-cp` type, so investigate rather than regenerating fixtures reflexively.

- [ ] **Step 4: Commit and open the PR**

```bash
cd ~/Code/Oracle/rupu
git add CLAUDE.md
git commit -m "docs: record issues.comments in the native tool catalog

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
git push origin feat/issues-comments-read-tool
```

Note: this repo has `push.default = matching`, so always push with an explicit refspec as above — a bare `git push` pushes every matching branch.

```bash
gh pr create --title "feat: issues.comments read tool" --body "$(cat <<'EOF'
## Summary

The `IssueConnector` port was write-only for comments: `comment_issue` posts, but nothing reads. An agent could be woken by a `github.issue.commented` event and still had no way to read what the comment said.

This adds:
- `IssueConnector::list_comments(&IssueRef, Option<u32>) -> Vec<Comment>`, with a default "not supported" impl following the `add_pr_labels` pattern, so GitLab / Linear / Jira and the registry mocks are untouched
- a GitHub override
- the `issues.comments` MCP tool (read kind)

## Why

Prerequisite for Project Chimera, whose entire human control channel is "operators steer autonomous runs by commenting on a GitHub issue." Generally useful beyond that: any issue autoflow that should react to discussion needs it.

## Testing

- Two new httpmock translation tests covering ordering, field mapping, and `limit` truncation
- Catalog test asserting `issues.comments` is present and read-kind
- Tool-catalog snapshot regenerated (single added entry)
- `cargo test --workspace` green

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review

**Spec coverage (§12 of the Chimera design):**
- §12.1 `issues.comments` read tool — Tasks 1 (port + adapter) and 2 (MCP surface). Covered.
- §12.2 issue re-labelling — explicitly out of scope for this plan; the spec marks it optional and not a prerequisite. Intentional gap, tracked in the spec.

**Placeholder scan:** No TBDs. Every code step carries the actual code. The one conditional instruction (Task 1 Step 4, on the octocrab builder shape) names the exact fallback and the invariant the tests pin, rather than deferring a decision.

**Type consistency:** `list_comments(&self, i: &IssueRef, limit: Option<u32>) -> Result<Vec<Comment>, ScmError>` is used identically in the trait default (Task 1 Step 3), the GitHub override (Task 1 Step 4), the tests (Task 1 Step 1), and the dispatcher (Task 2 Step 4). `Comment`'s four fields match `crates/rupu-scm/src/types.rs:252`. `ListCommentsArgs` field names match what `dispatch_comments` reads and what the catalog test asserts.
