# rupu Slice B-2 — Plan 3: CLI surface + docs + nightly extension

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface the unified SCM/MCP machinery to end-users via the CLI and documentation. Adds three CLI changes (`rupu repos list`, `rupu mcp serve`, optional `target` arg on `rupu run` / `rupu workflow run`), the canonical docs (`docs/scm.md`, `docs/scm/github.md`, `docs/scm/gitlab.md`, `docs/mcp.md`), README "SCM & issue trackers" section, the CHANGELOG entry, and the nightly-live-tests workflow extension. After this plan: a user can `rupu auth login --provider github --mode sso`, then `rupu run review-pr github:owner/repo#42`, and an external Claude Desktop instance can spawn `rupu mcp serve` and call `scm.repos.list` against the same keychain.

**Architecture:** Pure CLI plumbing on top of Plans 1 & 2. The `target` parser is a small standalone module in `rupu-cli` (no business logic — just text parsing). `rupu mcp serve` is ~30 lines: build a `Registry` from the `KeychainResolver`, hand it to `rupu_mcp::McpServer::new(registry, StdioTransport::new(), allow_all)`, run. `rupu repos list` builds the same `Registry`, iterates `repo(p)?.list_repos()` per platform, prints a table.

**Tech Stack:** Rust 2021 (MSRV 1.88), `clap` (already pinned), `comfy-table` for table rendering (added as a dep this plan). No new MCP / SCM crate work — Plans 1 + 2 already shipped that.

**Spec:** `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`

---

## File Structure

```
crates/
  rupu-cli/
    src/
      cmd/
        mod.rs                                     # MODIFY: add `repos` and `mcp` modules
        repos.rs                                   # NEW: rupu repos list
        mcp.rs                                     # NEW: rupu mcp serve
        run.rs                                     # MODIFY: optional `target` positional arg
        workflow.rs                                # MODIFY: same for `rupu workflow run`
      run_target.rs                                # NEW: RunTarget parser (github:owner/repo#42 etc.)
      lib.rs                                       # MODIFY: pub use new modules
      main.rs                                      # MODIFY: clap dispatch for `repos` / `mcp`
    tests/
      run_target_parse.rs                          # NEW
      repos_list_e2e.rs                            # NEW
      mcp_serve_stdio_smoke.rs                     # NEW
      run_target_e2e.rs                            # NEW

docs/
  scm.md                                           # NEW (canonical SCM reference)
  scm/
    github.md                                      # NEW (per-platform walkthrough)
    gitlab.md                                      # NEW
  mcp.md                                           # NEW (external-client wiring)
  providers/
    github.md                                      # MODIFY: cross-ref to docs/scm/github.md

README.md                                          # MODIFY: SCM & issue trackers section
CHANGELOG.md                                       # MODIFY: B-2 release entry
.github/workflows/nightly-live-tests.yml           # MODIFY: add GitHub + GitLab live env vars
CLAUDE.md                                          # MODIFY: mark Plan 3 in progress, point to docs
```

## Conventions to honor

- `rupu-cli` stays thin: parse args + dispatch to `rupu-scm` / `rupu-mcp`. No business logic in CLI handlers (CLAUDE.md rule 2).
- Workspace deps only.
- `#![deny(clippy::all)]`.
- Per the "no mock features" memory: `rupu repos list` against a missing platform credential reports a clear "no credential for github (run `rupu auth login --provider github`)" line, not a silent success.
- Docs grammar: lowercase command names, fenced code blocks for examples, real working invocations (cross-checked against actual binary output).

## Important pre-existing state (read before starting)

- `crates/rupu-cli/src/cmd/mod.rs` (Slice A Plan 3) currently has seven submodules (`agent` / `auth` / `config` / `models` / `run` / `transcript` / `workflow`). Plan 3 adds two more (`repos`, `mcp`).
- `crates/rupu-cli/src/main.rs` dispatches via `clap::Subcommand`. Adding subcommands requires updating both the enum and the dispatcher.
- `crates/rupu-cli/src/cmd/run.rs::Args` has fields `agent: String`, `prompt: Option<String>`, `mode: Option<String>`, `no_stream: bool`. Plan 3 adds `target: Option<String>` *between* `agent` and `prompt` to keep the `<agent> [<target>] [<prompt>]` ordering intuitive — see Task 4 for the parsing rules that disambiguate `target` from `prompt`.
- `docs/RELEASING.md` describes the manual release runbook. Plan 3 doesn't change releases; the v0.2.0-cli release happens after this plan ships.
- `docs/providers.md` is the canonical LLM-provider reference (added in Slice B-1 Plan 3 Task 10). `docs/scm.md` mirrors that doc's structure but for SCM/issue platforms.
- `.github/workflows/nightly-live-tests.yml` runs nightly with provider tokens. Plan 3 Task 13 extends the env-var matrix.

---

## Phase 0 — Workspace deps + run-target parser

### Task 1: Add `comfy-table` to the workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add to `[workspace.dependencies]`**

```toml
comfy-table = "7"
```

- [ ] **Step 2: Verify**

```
cargo metadata --no-deps --format-version 1 > /dev/null
```

Expected: exit 0.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
deps: add comfy-table to workspace

Used by `rupu repos list` (Plan 3 Task 5) for the multi-column
table render. Same dep is reusable for future list-style
subcommands (`rupu models list` etc., though those currently
hand-roll the alignment).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `RunTarget` parser

**Files:**
- Create: `crates/rupu-cli/src/run_target.rs`
- Create: `crates/rupu-cli/tests/run_target_parse.rs`
- Modify: `crates/rupu-cli/src/lib.rs` (re-export)

- [ ] **Step 1: Write failing tests first**

Create `crates/rupu-cli/tests/run_target_parse.rs`:

```rust
use rupu_cli::run_target::{parse_run_target, RunTarget};
use rupu_scm::{IssueTracker, Platform};

#[test]
fn parses_repo_only_form() {
    let t = parse_run_target("github:section9labs/rupu").unwrap();
    assert_eq!(t, RunTarget::Repo {
        platform: Platform::Github,
        owner: "section9labs".into(),
        repo: "rupu".into(),
        ref_: None,
    });
}

#[test]
fn parses_pr_form() {
    let t = parse_run_target("github:section9labs/rupu#42").unwrap();
    assert_eq!(t, RunTarget::Pr {
        platform: Platform::Github,
        owner: "section9labs".into(),
        repo: "rupu".into(),
        number: 42,
    });
}

#[test]
fn parses_issue_form() {
    let t = parse_run_target("github:section9labs/rupu/issues/123").unwrap();
    assert_eq!(t, RunTarget::Issue {
        tracker: IssueTracker::Github,
        project: "section9labs/rupu".into(),
        number: 123,
    });
}

#[test]
fn parses_gitlab_mr_with_bang() {
    let t = parse_run_target("gitlab:group/sub/project!7").unwrap();
    assert_eq!(t, RunTarget::Pr {
        platform: Platform::Gitlab,
        owner: "group/sub".into(),
        repo: "project".into(),
        number: 7,
    });
}

#[test]
fn parses_gitlab_issue() {
    let t = parse_run_target("gitlab:group/project/issues/9").unwrap();
    assert_eq!(t, RunTarget::Issue {
        tracker: IssueTracker::Gitlab,
        project: "group/project".into(),
        number: 9,
    });
}

#[test]
fn rejects_unknown_platform() {
    assert!(parse_run_target("bitbucket:foo/bar").is_err());
}

#[test]
fn rejects_missing_separator() {
    assert!(parse_run_target("github-foo-bar").is_err());
}

#[test]
fn rejects_missing_owner() {
    assert!(parse_run_target("github:repo-only").is_err());
}
```

Run: `cargo test -p rupu-cli --test run_target_parse`
Expected: FAIL — `parse_run_target` doesn't exist yet.

- [ ] **Step 2: Implement the parser**

Create `crates/rupu-cli/src/run_target.rs`:

```rust
//! Parse the `target` positional arg of `rupu run` / `rupu workflow run`.
//!
//! Grammar (matches docs/scm.md §"Target syntax"):
//!
//! ```text
//!   github:owner/repo                          # Repo
//!   github:owner/repo#42                       # PR
//!   github:owner/repo/issues/123               # Issue
//!   gitlab:group/project                       # Repo (gitlab.com)
//!   gitlab:group/sub/project!7                 # MR (uses `!` per gitlab convention)
//!   gitlab:group/project/issues/9              # Issue
//! ```
//!
//! Returns `RunTarget` for the runner to preload into the agent's
//! system prompt as a `## Run target` section.

use rupu_scm::{IssueTracker, Platform};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTarget {
    Repo {
        platform: Platform,
        owner: String,
        repo: String,
        ref_: Option<String>,
    },
    Pr {
        platform: Platform,
        owner: String,
        repo: String,
        number: u32,
    },
    Issue {
        tracker: IssueTracker,
        project: String,
        number: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RunTargetParseError {
    #[error("expected `<platform>:<owner>/<repo>[#N | !N | /issues/N]`, got `{0}`")]
    BadShape(String),
    #[error("unknown platform `{0}`")]
    UnknownPlatform(String),
    #[error("invalid number in target: {0}")]
    BadNumber(String),
}

pub fn parse_run_target(s: &str) -> Result<RunTarget, RunTargetParseError> {
    let (platform_str, rest) = s
        .split_once(':')
        .ok_or_else(|| RunTargetParseError::BadShape(s.into()))?;
    let platform = Platform::from_str(platform_str)
        .map_err(|_| RunTargetParseError::UnknownPlatform(platform_str.into()))?;

    // Issue form: <owner>/<repo>/issues/<N>
    if let Some((project, num_part)) = rest.rsplit_once("/issues/") {
        let number: u64 = num_part.parse().map_err(|_| RunTargetParseError::BadNumber(num_part.into()))?;
        let tracker = match platform {
            Platform::Github => IssueTracker::Github,
            Platform::Gitlab => IssueTracker::Gitlab,
        };
        return Ok(RunTarget::Issue {
            tracker,
            project: project.to_string(),
            number,
        });
    }

    // PR form: <owner>/<repo>#<N> (github) or <group/project>!<N> (gitlab MR)
    let (path, number_opt): (&str, Option<u32>) = if let Some((p, n)) = rest.split_once('#') {
        (p, Some(n.parse().map_err(|_| RunTargetParseError::BadNumber(n.into()))?))
    } else if let Some((p, n)) = rest.split_once('!') {
        (p, Some(n.parse().map_err(|_| RunTargetParseError::BadNumber(n.into()))?))
    } else {
        (rest, None)
    };

    // Split path into owner+repo. GitLab supports nested namespaces: take
    // the LAST segment as the repo, everything before as the owner.
    let (owner, repo) = path
        .rsplit_once('/')
        .ok_or_else(|| RunTargetParseError::BadShape(s.into()))?;
    if owner.is_empty() || repo.is_empty() {
        return Err(RunTargetParseError::BadShape(s.into()));
    }

    Ok(match number_opt {
        Some(n) => RunTarget::Pr {
            platform,
            owner: owner.to_string(),
            repo: repo.to_string(),
            number: n,
        },
        None => RunTarget::Repo {
            platform,
            owner: owner.to_string(),
            repo: repo.to_string(),
            ref_: None,
        },
    })
}
```

In `crates/rupu-cli/src/lib.rs`:

```rust
pub mod run_target;
```

- [ ] **Step 3: Run tests**

```
cargo test -p rupu-cli --test run_target_parse
```

Expected: 8 pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/src/run_target.rs crates/rupu-cli/tests/run_target_parse.rs crates/rupu-cli/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-cli: RunTarget parser for rupu run [target] grammar

Pure parser — no I/O, no Registry lookup. Handles the four valid
shapes documented in spec §7d: repo-only, PR (#N for github,
!N for gitlab MRs), and issues (/issues/N for both). Returns
typed RunTarget that the runner preloads into the agent system
prompt as a `## Run target` section.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — `rupu run [target]` argument

### Task 3: Optional `target` positional arg on `rupu run` and `rupu workflow run`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`
- Modify: `crates/rupu-cli/src/cmd/workflow.rs`
- Create: `crates/rupu-cli/tests/run_target_e2e.rs`

- [ ] **Step 1: Add `target` field to `Args` struct**

```rust
#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Agent name (matches an `agents/*.md` file).
    pub agent: String,

    /// Optional run target, e.g. `github:owner/repo#42`.
    /// See `docs/scm.md#target-syntax` for the full grammar.
    /// Distinguished from `prompt` by containing a colon AND
    /// matching one of the known platform prefixes.
    pub target: Option<String>,

    /// Optional initial user message. Defaults to "go" if omitted.
    pub prompt: Option<String>,

    #[arg(long)]
    pub mode: Option<String>,
    #[arg(long)]
    pub no_stream: bool,
}
```

- [ ] **Step 2: Disambiguate target vs prompt at handle-time**

```rust
async fn run_inner(args: Args) -> anyhow::Result<()> {
    // ... existing setup ...

    // If `target` parses as a RunTarget, use it; otherwise treat it as
    // the leading words of the prompt.
    let (run_target, user_message) = match args.target.as_deref() {
        None => (None, args.prompt.unwrap_or_else(|| "go".into())),
        Some(s) => match crate::run_target::parse_run_target(s) {
            Ok(t) => (Some(t), args.prompt.unwrap_or_else(|| "go".into())),
            Err(_) => {
                // Fallback: not a target → it's part of the prompt.
                let combined = match args.prompt {
                    Some(p) => format!("{s} {p}"),
                    None => s.to_string(),
                };
                (None, combined)
            }
        },
    };
    // ...
}
```

- [ ] **Step 3: Preload target into the system prompt**

Where the system prompt is built (currently `spec.system_prompt`), append:

```rust
let system_prompt = match run_target {
    Some(ref t) => format!(
        "{}\n\n## Run target\n\n{}",
        spec.system_prompt,
        format_run_target_for_prompt(t),
    ),
    None => spec.system_prompt.clone(),
};
```

`format_run_target_for_prompt` lives next to the parser (in `run_target.rs`):

```rust
pub fn format_run_target_for_prompt(t: &RunTarget) -> String {
    match t {
        RunTarget::Repo { platform, owner, repo, .. } => format!(
            "Repo: {platform}:{owner}/{repo}\n\nUse the SCM tools (scm.repos.get, scm.files.read, scm.prs.list) to explore."
        ),
        RunTarget::Pr { platform, owner, repo, number } => format!(
            "PR: {platform}:{owner}/{repo}#{number}\n\nUse scm.prs.get + scm.prs.diff to read it. Use scm.prs.comment to post a review."
        ),
        RunTarget::Issue { tracker, project, number } => format!(
            "Issue: {tracker}:{project}/issues/{number}\n\nUse issues.get to read it. If asked to fix, branch + scm.prs.create when done."
        ),
    }
}
```

- [ ] **Step 4: Clone-into-tmpdir for remote targets**

When `run_target` is `Some(_)` AND the cwd is not already a checkout of the target repo, clone it before invoking the agent. Hold the `TempDir` in an outer binding so it stays alive (and is cleaned up) for the whole run:

```rust
// Bind the TempDir at function scope so its Drop runs on return.
let _clone_guard: Option<tempfile::TempDir>;
let workspace_path: std::path::PathBuf = match &run_target {
    Some(RunTarget::Repo { platform, owner, repo, .. })
    | Some(RunTarget::Pr { platform, owner, repo, .. }) => {
        let r = rupu_scm::RepoRef {
            platform: *platform,
            owner: owner.clone(),
            repo: repo.clone(),
        };
        let conn = scm_registry
            .repo(*platform)
            .ok_or_else(|| anyhow::anyhow!(
                "no {} credential — run `rupu auth login --provider {}`",
                platform, platform
            ))?;
        let tmp = tempfile::tempdir()?;
        conn.clone_to(&r, tmp.path()).await?;
        let path = tmp.path().to_path_buf();
        _clone_guard = Some(tmp);  // keep alive until run_inner returns
        path
    }
    _ => {
        _clone_guard = None;
        pwd.clone()
    }
};
```

`_clone_guard` is named with a leading underscore so clippy doesn't flag it, but it's deliberately bound (not `_`) — `let _ = tmp` would drop it immediately, defeating the point.

- [ ] **Step 5: E2E test**

In `crates/rupu-cli/tests/run_target_e2e.rs`:

```rust
//! Spawn `rupu run` with a target arg using a mock provider script.
//! Asserts the system prompt the runner sent to MockProvider contains
//! the "## Run target" section.

#[test]
fn run_with_pr_target_preloads_run_target_section() {
    let temp_workspace = tempfile::tempdir().unwrap();
    // ... write a sample agent + a RUPU_MOCK_PROVIDER_SCRIPT env var ...
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(&["run", "demo-agent", "github:section9labs/rupu#1"])
        .env("RUPU_MOCK_PROVIDER_SCRIPT", "...")
        .current_dir(temp_workspace.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    // ... read transcript, confirm Event::AgentSystemPrompt (or whatever Slice A emits) contains "## Run target" ...
}
```

(If Slice A doesn't already emit a system-prompt event, instead grep the test's `RUPU_MOCK_CAPTURE` capture file for `"## Run target"`.)

- [ ] **Step 6: Commit per file (run.rs, workflow.rs, e2e test)**

```bash
git add crates/rupu-cli/src/cmd/run.rs crates/rupu-cli/src/run_target.rs crates/rupu-cli/tests/run_target_e2e.rs
git commit -m "$(cat <<'EOF'
rupu-cli: rupu run [target] argument + tmpdir clone

The `target` positional is optional and disambiguates from `prompt`
by parsing successfully as a RunTarget. When set, the runner
clones the repo into a tempdir (or uses the cwd if it's already
a checkout) and preloads a `## Run target` section into the
agent's system prompt with the typed reference and a hint at
which SCM tools to use.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

`rupu workflow run [target]` mirrors the same — task includes both.

---

## Phase 2 — `rupu repos list` and `rupu mcp serve`

### Task 4: `rupu repos list` subcommand

**Files:**
- Create: `crates/rupu-cli/src/cmd/repos.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs`
- Modify: `crates/rupu-cli/src/main.rs`
- Create: `crates/rupu-cli/tests/repos_list_e2e.rs`

- [ ] **Step 1: Implement the handler**

```rust
//! `rupu repos list [--platform <name>]` — list repos via Registry.

use crate::paths;
use clap::{Args as ClapArgs, Subcommand};
use comfy_table::{ContentArrangement, Table};
use rupu_scm::{Platform, Registry};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all repositories accessible via configured platforms.
    List(ListArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ListArgs {
    /// Filter to one platform (`github` | `gitlab`). Default: all.
    #[arg(long)]
    pub platform: Option<String>,
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::List(args) => match list_inner(args).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu repos list: {e}");
                ExitCode::from(1)
            }
        },
    }
}

async fn list_inner(args: ListArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;

    let resolver = rupu_auth::KeychainResolver::new();
    let registry = Arc::new(Registry::discover(&resolver, &cfg).await);

    let platforms: Vec<Platform> = match args.platform.as_deref() {
        Some(s) => vec![s.parse().map_err(|e: String| anyhow::anyhow!(e))?],
        None => vec![Platform::Github, Platform::Gitlab],
    };

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["Platform", "Owner/Repo", "Default branch", "Visibility"]);

    let mut any_listed = false;
    for p in platforms {
        let Some(conn) = registry.repo(p) else {
            eprintln!("(skipped {p}: no credential — run `rupu auth login --provider {p}`)");
            continue;
        };
        let repos = conn.list_repos().await?;
        for r in repos {
            table.add_row(vec![
                p.to_string(),
                format!("{}/{}", r.r.owner, r.r.repo),
                r.default_branch,
                if r.private { "private".into() } else { "public".into() },
            ]);
            any_listed = true;
        }
    }
    if !any_listed {
        eprintln!("No repos to list. Run `rupu auth login --provider github` or `--provider gitlab`.");
        return Ok(());
    }
    println!("{table}");
    Ok(())
}
```

- [ ] **Step 2: Wire into `main.rs` clap dispatch**

```rust
#[derive(Subcommand)]
enum Cmd {
    // ... existing variants ...
    /// SCM repository operations.
    Repos {
        #[command(subcommand)]
        action: cmd::repos::Action,
    },
    /// MCP server operations.
    Mcp {
        #[command(subcommand)]
        action: cmd::mcp::Action,
    },
}

// dispatcher:
Cmd::Repos { action } => cmd::repos::handle(action).await,
Cmd::Mcp { action } => cmd::mcp::handle(action).await,
```

And in `cmd/mod.rs`:

```rust
pub mod mcp;
pub mod repos;
```

- [ ] **Step 3: E2E test (no live network)**

In `crates/rupu-cli/tests/repos_list_e2e.rs`:

```rust
#[test]
fn repos_list_prints_skip_message_when_no_credentials() {
    let temp_global = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(&["repos", "list"])
        .env("RUPU_GLOBAL_DIR", temp_global.path())
        // no keychain / explicit empty resolver path
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("skipped github") || stderr.contains("No repos"));
}
```

(If `RUPU_GLOBAL_DIR` env var doesn't exist yet, this test demonstrates why it should — same harness pattern as Slice B-1's auth tests. If it doesn't exist, this task adds one in `crates/rupu-cli/src/paths.rs`.)

- [ ] **Step 4: Run gates**

```
cargo test -p rupu-cli --test repos_list_e2e
cargo clippy -p rupu-cli -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/cmd/repos.rs crates/rupu-cli/src/cmd/mod.rs crates/rupu-cli/src/main.rs crates/rupu-cli/tests/repos_list_e2e.rs
git commit -m "$(cat <<'EOF'
rupu-cli: rupu repos list [--platform N]

Builds Registry::discover from KeychainResolver+Config, iterates
configured platforms, prints a comfy-table with platform,
owner/repo, default branch, visibility. Platforms without a
credential are reported on stderr as "skipped" with the recovery
command. Honors --platform <name> for single-platform queries.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `rupu mcp serve` subcommand

**Files:**
- Create: `crates/rupu-cli/src/cmd/mcp.rs`
- Create: `crates/rupu-cli/tests/mcp_serve_stdio_smoke.rs`

- [ ] **Step 1: Implement**

```rust
//! `rupu mcp serve [--transport stdio|http]` — JSON-RPC MCP server
//! for external clients (Claude Desktop, Cursor, etc).

use crate::paths;
use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use rupu_mcp::{McpPermission, McpServer, StdioTransport};
use rupu_scm::Registry;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Run the MCP server.
    Serve(ServeArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ServeArgs {
    /// Transport. v0 only stdio; http returns NotWiredInV0.
    #[arg(long, value_enum, default_value = "stdio")]
    pub transport: TransportKind,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum TransportKind { Stdio, Http }

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::Serve(args) => match serve_inner(args).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu mcp serve: {e}");
                ExitCode::from(1)
            }
        },
    }
}

async fn serve_inner(args: ServeArgs) -> anyhow::Result<()> {
    if matches!(args.transport, TransportKind::Http) {
        anyhow::bail!("http transport not wired in v0; use --transport stdio (the default)");
    }

    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;

    let resolver = rupu_auth::KeychainResolver::new();
    let registry = Arc::new(Registry::discover(&resolver, &cfg).await);

    // External-client mode: trust the upstream client's permission model.
    // Bypass mode + allow-all listing; the MCP-aware client (Claude Desktop)
    // is responsible for prompting the user before calling Write tools.
    let permission = McpPermission::allow_all();
    let server = McpServer::new(registry, StdioTransport::new(), permission);
    server.run().await.map_err(|e| anyhow::anyhow!("mcp server: {e}"))
}
```

- [ ] **Step 2: Stdio smoke test**

In `crates/rupu-cli/tests/mcp_serve_stdio_smoke.rs`:

```rust
//! Spawn `rupu mcp serve --transport stdio`, send tools/list, parse
//! the response, confirm the catalog contains the expected names.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn mcp_serve_stdio_returns_tools_list() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args(&["mcp", "serve", "--transport", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let stdin = child.stdin.as_mut().expect("stdin");
    writeln!(stdin, "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}}").unwrap();
    let stdout = child.stdout.as_mut().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 1);
    let tools = v["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "scm.repos.list"));
    assert!(tools.iter().any(|t| t["name"] == "issues.get"));

    let _ = child.kill();
    let _ = child.wait();
}
```

- [ ] **Step 3: Run gates**

```
cargo test -p rupu-cli --test mcp_serve_stdio_smoke
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/src/cmd/mcp.rs crates/rupu-cli/tests/mcp_serve_stdio_smoke.rs
git commit -m "$(cat <<'EOF'
rupu-cli: rupu mcp serve --transport stdio

Builds Registry from KeychainResolver+Config and hands it to
McpServer with allow-all permissions (the upstream MCP client is
responsible for confirmation prompts in this mode). --transport
http surfaces a clear NotWiredInV0 message instead of failing
silently. Stdio smoke test spawns the binary and asserts a
tools/list response includes the expected names.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Documentation

### Task 6: `docs/scm.md` (canonical SCM reference)

**Files:**
- Create: `docs/scm.md`

- [ ] **Step 1: Write the doc**

Structure mirrors `docs/providers.md` (Slice B-1 Plan 3 Task 10):

```markdown
# SCM & issue trackers

rupu integrates with SCM (source-code management) platforms and issue trackers
through a single embedded MCP server. Agents call typed tools (`scm.repos.list`,
`scm.prs.diff`, `issues.get`, ...) regardless of which platform the call resolves
to. Per-platform connectors handle vendor-specific quirks (GitLab MR vs GitHub
PR, nested namespaces, rate-limit headers).

## At a glance

| Capability                  | GitHub | GitLab | Linear | Jira |
|----------------------------|:------:|:------:|:------:|:----:|
| Repos (list, get, branch)  |   ✅   |   ✅   |   —    |   —  |
| PRs / MRs (read, comment, create) |   ✅   |   ✅   |   —    |   —  |
| Issues (read, comment, create, transition) |   ✅   |   ✅   |  v0.3  | v0.3 |
| Workflow / pipeline trigger |   ✅   |   ✅   |   —    |   —  |
| `clone_to` (local checkout) |   ✅   |   ✅   |   —    |   —  |
| File read by ref            |   ✅   |   ✅   |   —    |   —  |
| API surface                 | REST  | REST  |   —    |   —  |

## Auth

`rupu auth login --provider <github|gitlab> --mode <api-key|sso>` stores tokens
in the OS keychain. Same flow as Slice B-1's LLM-provider auth; same `rupu auth
status` table picks up SCM rows automatically.

## Target syntax

The optional positional arg on `rupu run` and `rupu workflow run`:

| Form                                     | Means                          |
|------------------------------------------|--------------------------------|
| `github:owner/repo`                      | repo (working tree)            |
| `github:owner/repo#42`                   | PR 42                          |
| `github:owner/repo/issues/123`           | issue 123                      |
| `gitlab:group/project`                   | repo (working tree)            |
| `gitlab:group/sub/project!7`             | MR 7 (gitlab uses `!` not `#`) |
| `gitlab:group/project/issues/9`          | issue 9                        |

## MCP tool catalog

Full JSON-Schema-typed catalog. All tools accept `platform?` (or `tracker?`)
that falls back to `[scm.default]` / `[issues.default]` from config when omitted.

(table covering the 17 tools from spec §6b: name | description | kind | typical use)

## Configuration

```toml
[scm.default]
platform = "github"
owner = "section9labs"
repo = "rupu"

[issues.default]
tracker = "github"
project = "section9labs/rupu"

[scm.github]
base_url = "https://api.github.com"
timeout_ms = 30000
max_concurrency = 8
clone_protocol = "https"

[scm.gitlab]
base_url = "https://gitlab.com/api/v4"
timeout_ms = 30000
max_concurrency = 6
clone_protocol = "https"
```

## Concurrency, caching, retry

(reproduce spec §9 tables — defaults + override knobs)

## Error classification

(reproduce spec §9d table — HTTP signal → ScmError variant)

## Troubleshooting

| Symptom                                              | Likely cause                            | Fix |
|-----------------------------------------------------|-----------------------------------------|-----|
| `MissingScope { scope: "repo" }`                    | PAT was issued without `repo` scope     | `rupu auth logout --provider github` then `rupu auth login --provider github --mode sso` |
| `RateLimited` after a few calls                     | Hit GitHub's secondary rate limit       | Drop `[scm.github].max_concurrency` to 4 |
| `Unauthorized` after a token rotation               | Keychain still has the old token        | `rupu auth logout --provider github --mode api-key` |
| `Network` from inside a container                   | Container can't reach api.github.com    | Confirm DNS + outbound TCP/443 |
| `tool not in agent's `tools:` list`                 | Agent forgot to allowlist the tool      | Add `scm.*` (or specific tool name) to frontmatter |
| `gitlab: 403 + insufficient_scope`                  | PAT missing `read_repository`           | Re-issue PAT with full scope set; `rupu auth login --provider gitlab --mode sso` re-prompts |

## See also

- `docs/scm/github.md` — GitHub-specific walkthrough (PAT, OAuth, GHES)
- `docs/scm/gitlab.md` — GitLab-specific walkthrough (PAT, OAuth, self-hosted)
- `docs/mcp.md` — wiring `rupu mcp serve` into Claude Desktop / Cursor
- `docs/providers.md` — LLM-provider reference (separate auth surface)
```

- [ ] **Step 2: Commit**

```bash
git add docs/scm.md
git commit -m "$(cat <<'EOF'
docs: docs/scm.md canonical SCM/issue-tracker reference

Mirrors docs/providers.md's structure: capability matrix, auth,
target-syntax grammar, full MCP tool catalog, config schema,
concurrency/cache/retry tables, error classification, and a
troubleshooting matrix. Cross-references docs/scm/<platform>.md
walkthroughs and docs/mcp.md for external-client wiring.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: `docs/scm/github.md` (per-platform walkthrough)

**Files:**
- Create: `docs/scm/github.md`

Structure parallels `docs/providers/anthropic.md` (Slice B-1 Plan 3 Task 11):

- [ ] **Step 1: Sections**

```markdown
# GitHub

## Auth modes

### API key (PAT)
1. github.com/settings/tokens → "Generate new token (classic)"
2. Scopes: `repo`, `workflow`, `gist`, `read:org`, `read:user`
3. `rupu auth login --provider github --mode api-key --key ghp_xxx`

### OAuth (device-code SSO)
`rupu auth login --provider github --mode sso` → opens github.com/login/device,
prompts for the user-code printed to stderr, stores the access token in keychain.

## Sample agent

```yaml
---
name: review-pr
provider: anthropic
model: claude-sonnet-4-6
tools: [scm.prs.get, scm.prs.diff, scm.prs.comment]
permission_mode: ask
---
You are a code reviewer. Read the PR via scm.prs.diff and post a single
summary review with scm.prs.comment.
```

Run: `rupu run review-pr github:section9labs/rupu#42`

## Known quirks

- **Fine-grained PATs**: less surface than classic PATs (no `gist`, less `read:org`).
  rupu uses classic PATs by default; document this in the tool description.
- **GraphQL**: rupu uses REST only in v0; some queries (e.g. cross-org search)
  aren't reachable. Filed as out-of-scope in spec §12.
- **GHES**: set `[scm.github].base_url = "https://ghes.example.com/api/v3"`.
  No code changes required, but error messages will reference api.github.com
  unless overridden.
- **Workflow dispatch**: requires `workflow` scope on the PAT *and* the workflow
  file must contain `on: workflow_dispatch:`. rupu surfaces 422 as
  `BadRequest { message: "workflow not configured for dispatch" }`.

## See also

- `docs/scm.md` — canonical reference
- `docs/providers/github.md` — Copilot LLM provider (separate keychain entry)
```

- [ ] **Step 2: Cross-link from `docs/providers/github.md`**

Append a one-liner:

```markdown
## See also

- `docs/scm/github.md` — GitHub repo + issues integration (separate from this LLM-provider doc).
```

- [ ] **Step 3: Commit**

```bash
git add docs/scm/github.md docs/providers/github.md
git commit -m "$(cat <<'EOF'
docs: docs/scm/github.md walkthrough + cross-link

Follows docs/providers/<name>.md structure: auth modes (PAT + SSO),
working sample agent, known quirks (fine-grained vs classic PATs,
GHES override, workflow_dispatch requirements). docs/providers/
github.md gets a one-line cross-reference so the Copilot LLM page
points readers at the SCM page.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: `docs/scm/gitlab.md`

**Files:**
- Create: `docs/scm/gitlab.md`

Same structure as github.md, swapping vocabulary. Cover:

- PAT acquisition (gitlab.com/-/user_settings/personal_access_tokens), required scope set (`api`, `read_user`, `read_repository`, `write_repository`).
- OAuth (browser-callback PKCE flow) — note that gitlab.com OAuth apps must be registered (TODO.md item).
- Sample agent (mirror review-pr but use `gitlab:group/project!7` form).
- Known quirks:
  - **MR vs PR vocabulary**: rupu translates internally — agents see `scm.prs.*`.
  - **Nested groups**: `group/sub/project` parses with `owner = "group/sub"`, `repo = "project"`.
  - **Self-hosted**: `[scm.gitlab].base_url` override works but is not formally tested in nightly CI.
  - **Trigger tokens**: `gitlab.pipeline_trigger` uses the project access token, not a separate trigger token. PATs with `api` scope work.

- [ ] **Step 1: Write the doc.**
- [ ] **Step 2: Commit.**

---

### Task 9: `docs/mcp.md` (external-client wiring)

**Files:**
- Create: `docs/mcp.md`

- [ ] **Step 1: Sections**

```markdown
# rupu's MCP server

`rupu mcp serve` exposes the unified SCM + issue tool catalog over
JSON-RPC stdio per the [MCP spec](https://spec.modelcontextprotocol.io/).
Any MCP-aware client can spawn it as a subprocess and call the same
tools rupu's own agents call.

## Wiring into Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "rupu": {
      "command": "/usr/local/bin/rupu",
      "args": ["mcp", "serve", "--transport", "stdio"]
    }
  }
}
```

After restart, Claude Desktop's tool catalog includes every `scm.*` and
`issues.*` tool from `docs/scm.md`. Authentication is shared with rupu's CLI
via the OS keychain — running `rupu auth login --provider github --mode sso`
once unlocks the catalog for both.

## Wiring into Cursor

(Cursor's MCP config layout — `cursor.json` snippet, same args.)

## Tool catalog

(Same matrix as docs/scm.md §"MCP tool catalog", reframed for the MCP-client
audience: input schemas, sample arguments, sample responses.)

## Permissions

`rupu mcp serve` runs with permission mode `bypass` and an allow-all
allowlist. The upstream MCP client (Claude Desktop, Cursor) is responsible
for prompting the user before invoking write tools. This is consistent
with the rest of the MCP ecosystem; rupu does NOT prompt from the server.

For `rupu run` invocations from the CLI, the agent's frontmatter
`tools:` list and the `--mode` flag enforce per-tool gating; the MCP
server enforces both.

## Troubleshooting

(...)
```

- [ ] **Step 2: Commit.**

---

## Phase 4 — README + CHANGELOG

### Task 10: README "SCM & issue trackers" section

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a section after "Configuring providers"**

```markdown
## SCM & issue trackers

rupu integrates with GitHub and GitLab through a single embedded MCP
server. Agents call typed tools (`scm.prs.diff`, `issues.get`, ...) and
the right per-platform connector dispatches the call. See `docs/scm.md`
for the full reference.

```bash
# 1. Authenticate
rupu auth login --provider github --mode sso

# 2. List your repos
rupu repos list

# 3. Run an agent against a PR
rupu run review-pr github:section9labs/rupu#42

# 4. Or expose the same surface to Claude Desktop / Cursor:
rupu mcp serve --transport stdio
```

| Capability      | GitHub | GitLab |
|-----------------|:------:|:------:|
| Repos / branches |   ✅   |   ✅   |
| PRs / MRs        |   ✅   |   ✅   |
| Issues           |   ✅   |   ✅   |
| Workflows / pipelines |   ✅   |   ✅   |
| Clone to local   |   ✅   |   ✅   |

Linear and Jira issue trackers are designed-in but not shipped in this
release; see [TODO.md](TODO.md) for the deferred-feature list.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs(README): SCM & issue trackers section

Quick-start matrix + four-line code blob (login / list / run /
mcp serve) + capability matrix. Points at docs/scm.md for the
full reference and TODO.md for the deferred-tracker list.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: CHANGELOG entry for B-2

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add v0.2.0 entry at the top**

```markdown
## v0.2.0 — Slice B-2: SCM + issue trackers (2026-05-XX)

### Added

- **GitHub + GitLab connectors** (`rupu-scm`). RepoConnector + IssueConnector
  trait families with per-platform impls; ETag-cached + retry-with-backoff
  + per-platform Semaphore. classify_scm_error pure function gives every
  error a recoverable/unrecoverable verdict.
- **Embedded MCP server** (`rupu-mcp`). 17 tools (`scm.repos.*`, `scm.prs.*`,
  `scm.files.read`, `scm.branches.*`, `issues.*`, `github.workflows_dispatch`,
  `gitlab.pipeline_trigger`) auto-attached to every `rupu run` /
  `rupu workflow run`. JSON-Schema-typed via schemars; permission gating
  honors the agent's frontmatter `tools:` list AND `--mode` flag.
- **`rupu auth login --provider github|gitlab`** with both api-key and SSO
  flows. SSO uses GitHub's device-code flow / GitLab's browser-callback PKCE
  flow.
- **`rupu repos list [--platform <name>]`** — table-rendered list of repos
  the user can access on configured platforms.
- **`rupu mcp serve [--transport stdio]`** — MCP server for external
  clients (Claude Desktop, Cursor, etc.).
- **`rupu run <agent> [<target>]`** — optional positional arg; `target` is
  a `<platform>:<owner>/<repo>[#N | !N | /issues/N]` reference. The runner
  clones the repo to a tmpdir (or reuses the cwd) and preloads a
  `## Run target` section into the agent's system prompt.

### Architecture

- Two new crates: `rupu-scm` (connectors) and `rupu-mcp` (MCP kernel).
- `rupu-agent`'s `run_agent` now spins up `rupu_mcp::serve_in_process`
  before the first turn and tears it down before returning. SCM tools
  appear alongside the six built-in tools through a thin `McpToolAdapter`.
- `Registry::discover` builds connectors from the same `KeychainResolver`
  + `Config` that LLM-provider auth uses; missing credentials skip the
  platform silently with an INFO log.

### Internal

- `Platform` and `IssueTracker` enums in `rupu-scm` cover GitHub + GitLab
  today; `IssueTracker::Linear` and `IssueTracker::Jira` exist so future
  adapters slot in without reshaping call sites.
- New workspace deps: `octocrab`, `gitlab`, `git2` (vendored libgit2 +
  vendored OpenSSL), `lru`, `schemars`, `comfy-table`, `jsonschema`.

### Docs

- `docs/scm.md` — canonical reference (capabilities, auth, target syntax,
  full tool catalog, config schema, error classification, troubleshooting).
- `docs/scm/github.md` + `docs/scm/gitlab.md` — per-platform walkthroughs.
- `docs/mcp.md` — Claude Desktop / Cursor wiring + sample config.
- README + CHANGELOG updates.
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "$(cat <<'EOF'
CHANGELOG: Slice B-2 release entry

Documents the rupu-scm + rupu-mcp introduction, the three new CLI
subcommands (rupu repos list, rupu mcp serve, rupu run [target]),
new workspace deps, and the docs additions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Nightly live-tests workflow + final gates

### Task 12: Extend `.github/workflows/nightly-live-tests.yml`

**Files:**
- Modify: `.github/workflows/nightly-live-tests.yml`

- [ ] **Step 1: Add SCM env vars + a second job step**

```yaml
name: nightly-live-tests
on:
  schedule:
    - cron: "0 8 * * *"
  workflow_dispatch: {}

jobs:
  live:
    runs-on: ubuntu-latest
    timeout-minutes: 25
    env:
      RUPU_LIVE_TESTS: "1"
      RUPU_LIVE_ANTHROPIC_KEY: ${{ secrets.RUPU_LIVE_ANTHROPIC_KEY }}
      RUPU_LIVE_OPENAI_KEY: ${{ secrets.RUPU_LIVE_OPENAI_KEY }}
      RUPU_LIVE_GEMINI_KEY: ${{ secrets.RUPU_LIVE_GEMINI_KEY }}
      RUPU_LIVE_COPILOT_TOKEN: ${{ secrets.RUPU_LIVE_COPILOT_TOKEN }}
      RUPU_LIVE_GITHUB_TOKEN: ${{ secrets.RUPU_LIVE_GITHUB_TOKEN }}
      RUPU_LIVE_GITLAB_TOKEN: ${{ secrets.RUPU_LIVE_GITLAB_TOKEN }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: "1.88"
      - name: Cargo build (providers + scm)
        run: cargo build -p rupu-providers -p rupu-scm --tests
      - name: Live smoke (LLM providers)
        run: cargo test -p rupu-providers --test live_smoke -- --nocapture
      - name: Live smoke (SCM connectors)
        run: cargo test -p rupu-scm --test live_smoke -- --nocapture
```

- [ ] **Step 2: Document the new secrets**

In `docs/RELEASING.md` (or a new section if one doesn't fit), append the secret-name list so a future maintainer knows what to set in the GitHub repo settings:

```markdown
### Nightly-test secrets (live-API smokes)

Set under repo Settings → Secrets and variables → Actions:
- `RUPU_LIVE_ANTHROPIC_KEY`
- `RUPU_LIVE_OPENAI_KEY`
- `RUPU_LIVE_GEMINI_KEY`
- `RUPU_LIVE_COPILOT_TOKEN`
- `RUPU_LIVE_GITHUB_TOKEN`           # PAT, scopes: repo + read:user + read:org
- `RUPU_LIVE_GITLAB_TOKEN`           # PAT, scopes: api + read_user + read_repository
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/nightly-live-tests.yml docs/RELEASING.md
git commit -m "$(cat <<'EOF'
ci: nightly-live-tests now runs SCM smokes (github + gitlab)

Two new env vars (RUPU_LIVE_GITHUB_TOKEN, RUPU_LIVE_GITLAB_TOKEN)
plumb through to the existing matrix; second `cargo test` step
runs `rupu-scm/tests/live_smoke.rs`. Workflow timeout bumped from
15 to 25 minutes to absorb the new tests. RELEASING.md updated
with the full list of nightly secrets so future maintainers can
set them in repo settings without reading the workflow.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: Workspace gates + cargo build smoke + CLAUDE.md update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Run all gates from the workspace root**

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --workspace
```

All four exit 0.

- [ ] **Step 2: Smoke the binary**

```
./target/release/rupu --version
./target/release/rupu --help                          # Should list `repos` and `mcp` subcommands
./target/release/rupu repos --help
./target/release/rupu mcp serve --help
./target/release/rupu run --help                      # Should mention TARGET positional
```

- [ ] **Step 3: Update `CLAUDE.md`**

```markdown
## Read first
- Slice A spec: `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`
- Slice B-1 spec: `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`
- Slice B-2 spec: `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`
- Plan 1 (foundation + GitHub, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-1-foundation-and-github.md`
- Plan 2 (GitLab + MCP server, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-2-gitlab-and-mcp.md`
- Plan 3 (CLI + docs + nightly, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-3-cli-and-docs.md`
```

Update the `### Crates` section so `rupu-cli` mentions the two new subcommands:

```markdown
- **`rupu-cli`** — the `rupu` binary. Thin clap dispatcher. Subcommands:
  `run` / `agent` / `workflow` / `transcript` / `config` / `auth` / `models`
  / `repos` / `mcp`.
```

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
CLAUDE.md: Slice B-2 plans complete; subcommand list updated

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Plan 3 success criteria

After all 13 tasks complete:

- `cargo fmt --all -- --check` exits 0.
- `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- `cargo test --workspace` exits 0.
- `cargo build --release --workspace` produces a `rupu` binary that lists `repos` and `mcp` in `--help`.
- `rupu repos list` against a fresh keychain prints "skipped github" / "skipped gitlab" guidance; with credentials, prints a comfy-table.
- `rupu mcp serve --transport stdio | jq` (with `tools/list` piped in) returns the 17-tool catalog.
- `rupu run review-pr github:section9labs/rupu#1` clones into a tempdir and preloads the `## Run target` section into the system prompt.
- `docs/scm.md`, `docs/scm/github.md`, `docs/scm/gitlab.md`, `docs/mcp.md` all exist and cross-reference each other.
- README has the "SCM & issue trackers" section; CHANGELOG has the v0.2.0 entry.
- Nightly workflow exercises both GitHub + GitLab live smokes when the corresponding secrets are set.

## Ready-to-release checklist

After Plan 3 passes:

- [ ] `cargo build --release --workspace` (signed via `make release` per RELEASING.md).
- [ ] Nightly workflow run on `main` is green (or the previous run is green and the only new commits are docs).
- [ ] CHANGELOG entry's date is updated to the actual release date.
- [ ] `rupu --version` reports the bumped version.
- [ ] Tag + GitHub release per `docs/RELEASING.md` runbook.
- [ ] Smoke a brand-new install (`/tmp` checkout) end-to-end: `rupu auth login --provider github`, `rupu repos list`, `rupu mcp serve | jq` (tools/list).

## Out of scope (deferred to follow-up slices)

- Linear / Jira / Asana issue-tracker adapters (`IssueConnector` trait absorbs them; v0 ships GitHub + GitLab).
- Bitbucket / Codeberg / Forgejo SCM adapters.
- Hosted MCP server (`rupu mcp serve --transport http`).
- PR review threads (line-level comments / suggestions).
- Branch protection / merge button (`merge_pr`, `enable_auto_merge`).
- GraphQL surfaces; v0 is REST only.
- `rupu repos search` / cross-platform search.
- Webhook / poll-based triggers (Slice D concern).
