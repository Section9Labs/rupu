# rupu Slice A — Plan 3: CLI binary, default library, docs, release

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the `rupu` binary at exit-criterion-B level — a developer can `cargo install --git https://github.com/Section9Labs/rupu` (or download a prebuilt binary from a tagged release), run `rupu auth login --provider anthropic`, then `rupu run fix-bug "make the failing test pass"`, and have the agent loop drive against a real Anthropic API key with a JSONL transcript on disk. Picks up where Plan 2 left off — eight library crates already provide the substrate (transcript, config, workspace, auth, providers, tools, agent, orchestrator).

**Architecture:** Single new crate (`rupu-cli`) that produces the `rupu` binary, plus shipped agent/workflow defaults, plus user-facing docs, plus a tag-triggered GitHub Releases workflow. The CLI is a thin clap-driven dispatcher to the existing crates; no business logic lives in `rupu-cli`. The default agents are `.md` files committed to `agents/` and embedded into the binary via `include_str!`.

**Tech Stack:** Same as Plans 1+2 plus:
- `clap` 4 (already in workspace deps) for argument parsing
- `is_terminal` trait — `std::io::IsTerminal` (already available, MSRV 1.77)
- `dirs` 5 — for cross-platform `~/.rupu/` resolution (cheap dep, well-maintained)
- For releases: `softprops/action-gh-release@v2` GitHub Action

**Spec:** `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`

**Predecessor plans:**
- Plan 1 — `docs/superpowers/plans/2026-05-01-rupu-slice-a-plan-1-foundation.md` (merged via PR #1; tag `v0.0.1-foundation`)
- Plan 2 — `docs/superpowers/plans/2026-05-02-rupu-slice-a-plan-2-runtime-cli.md` (merged via PR #4; tag `v0.0.2-runtime-libs`)

**On-disk reality vs plan code:** as Plan 2's Task 7 demonstrated, the lifted `rupu-providers` API has small naming differences from what plan documents assume. When wiring real providers in `rupu run`, verify constructor signatures (e.g., `AnthropicClient::new` vs `::with_credential`) by grep-ing `crates/rupu-providers/src/anthropic.rs` first. Report BLOCKED rather than guess.

---

## File structure

```
rupu/
  Cargo.toml                            # workspace root: add rupu-cli + new deps (dirs)
  crates/
    rupu-cli/
      Cargo.toml
      src/
        main.rs                         # tokio::main entry; calls into lib::run(args)
        lib.rs                          # `pub fn run(...) -> ExitCode` — testable harness
        cmd/
          mod.rs                        # cmd-module declarations
          run.rs                        # `rupu run <agent> [prompt]`
          agent.rs                      # `rupu agent list | show <name>`
          workflow.rs                   # `rupu workflow run | list | show`
          transcript.rs                 # `rupu transcript list | show <id>`
          config.rs                     # `rupu config get | set`
          auth.rs                       # `rupu auth login | logout | status`
        paths.rs                        # ~/.rupu/* resolution + project .rupu/ discovery
        provider_factory.rs             # build Box<dyn LlmProvider> from agent + auth
        logging.rs                      # tracing-subscriber init
        crash.rs                        # crash log writer at ~/.rupu/cache/crash-<ts>.log
        defaults.rs                     # embedded default agents/workflows via include_str!
      tests/
        cli_run.rs                      # rupu run end-to-end via mock provider injected via env
        cli_agent.rs                    # rupu agent list/show
        cli_workflow.rs                 # rupu workflow run end-to-end
        cli_transcript.rs               # rupu transcript list/show on JSONL on disk
        cli_config.rs                   # rupu config get/set
        cli_auth.rs                     # rupu auth status (no real keychain — JSON fallback in tempdir)
  agents/                               # shipped default agents (committed + embedded)
    fix-bug.md
    add-tests.md
    review-diff.md
    scaffold.md
    summarize-diff.md
  workflows/                            # shipped default workflows
    investigate-then-fix.yaml
  docs/
    spec.md                             # source-of-truth architecture (mirrors phi-cell pattern)
    agent-format.md                     # frontmatter reference
    workflow-format.md                  # YAML reference
    transcript-schema.md                # event schema reference
  README.md                             # install, first run, auth setup, examples
  CLAUDE.md                             # update with rupu-agent / rupu-orchestrator / rupu-cli pointers
  .github/workflows/release.yml         # tag-triggered prebuilt-binary release
```

**Decomposition rationale:**
- `rupu-cli/src/cmd/<verb>.rs`: one file per subcommand. Each is ≤ ~150 lines (clap arg struct + handler). Files that change together (run + provider_factory; transcript + paths) live together via re-exports.
- `defaults.rs` keeps `include_str!` calls in one place so the build dependency on `agents/*.md` is easy to audit.
- `provider_factory.rs` separates the "create a `Box<dyn LlmProvider>` from spec + auth" logic from the run subcommand. Plan 2's runner accepts a boxed provider; this is where it gets built for real (vs. the mock providers used in tests).

**Micro-decisions made in this plan that the spec didn't pin:**
- `dirs::home_dir()` for `~` resolution. Returns `Option<PathBuf>`; if None, error out with "could not locate home directory".
- Crash log path: `~/.rupu/cache/crash-<rfc3339>.log`. Created on panic; the runtime never reads it.
- Logging: `tracing` with `tracing-subscriber` env-filter. Default level `info`; `RUPU_LOG=debug` overrides.
- Config get/set: scoped to global `~/.rupu/config.toml` only. Setting project-local config is a manual edit for v0.
- `rupu auth login --provider <p>` for v0 reads the API key from stdin (or `--key <K>`). OAuth flows for Copilot/Gemini are deferred — provider's existing auth-discovery logic will detect the env var.
- `rupu workflow run <name>` factory: builds real providers via `provider_factory::build_for_agent_spec` per step.
- Transcript file naming: `<run_id>.jsonl` where run_id is `run_<26-char-ULID>` (already used by orchestrator since Plan 2 Task 11).

---

## Roadmap by phase

| Phase | Tasks | Output |
|---|---|---|
| 1. CLI skeleton + helpers | Tasks 1–3 | `rupu --version` works; paths/logging/crash util in place. |
| 2. `rupu run` subcommand | Tasks 4–5 | Real Anthropic API run end-to-end; transcript on disk; mock-provider integration test. |
| 3. Other subcommands | Tasks 6–10 | `rupu agent`, `rupu workflow`, `rupu transcript`, `rupu config`, `rupu auth`. |
| 4. Default library | Task 11 | 5 default agents + 1 sample workflow embedded; first-run UX. |
| 5. Docs | Tasks 12–13 | README, spec.md, agent-format, workflow-format, transcript-schema, CLAUDE.md. |
| 6. Release pipeline | Task 14 | `.github/workflows/release.yml`: tag-triggered build matrix → tar.gz → upload. |
| 7. Exit-criterion-B smoke + tag | Task 15 | Manual smoke: `cargo install` clean box + run 5 agents + 1 workflow against real repo + real Anthropic key. Tag `v0.0.3-cli`. |

---

## Phase 1 — CLI skeleton

### Task 1: `rupu-cli` crate skeleton + main entry

**Files:**
- Create: `crates/rupu-cli/Cargo.toml`
- Create: `crates/rupu-cli/src/main.rs`
- Create: `crates/rupu-cli/src/lib.rs`
- Create: `crates/rupu-cli/src/cmd/mod.rs`
- Modify: root `Cargo.toml` (add `crates/rupu-cli` to members; add `dirs = "5"` to workspace deps)

- [ ] **Step 1: Crate Cargo.toml**

```toml
[package]
name = "rupu-cli"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[[bin]]
name = "rupu"
path = "src/main.rs"

[dependencies]
clap.workspace = true
tokio = { workspace = true }
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true
toml.workspace = true
ulid.workspace = true
dirs.workspace = true

# In-workspace
rupu-agent = { path = "../rupu-agent" }
rupu-orchestrator = { path = "../rupu-orchestrator" }
rupu-config = { path = "../rupu-config" }
rupu-workspace = { path = "../rupu-workspace" }
rupu-auth = { path = "../rupu-auth" }
rupu-providers = { path = "../rupu-providers" }
rupu-tools = { path = "../rupu-tools" }
rupu-transcript = { path = "../rupu-transcript" }

[dev-dependencies]
tempfile.workspace = true
assert_fs.workspace = true
predicates.workspace = true
```

- [ ] **Step 2: Add dirs to workspace deps**

In root `Cargo.toml` `[workspace.dependencies]`, add:
```toml
dirs = "5"
```

(Place near the other CLI/utility deps. `dirs 5.x` requires Rust 1.65+ — fine for our 1.77 MSRV.)

- [ ] **Step 3: Add to workspace members**

```toml
members = [
    "crates/rupu-transcript",
    "crates/rupu-config",
    "crates/rupu-workspace",
    "crates/rupu-auth",
    "crates/rupu-providers",
    "crates/rupu-tools",
    "crates/rupu-agent",
    "crates/rupu-orchestrator",
    "crates/rupu-cli",
]
```

- [ ] **Step 4: Create `main.rs`**

```rust
//! `rupu` CLI entry point. Tiny `tokio::main` wrapper around
//! [`rupu_cli::run`] — keep this file the thinnest possible so the
//! testable harness in `lib.rs` carries the actual logic.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    rupu_cli::run(args).await
}
```

- [ ] **Step 5: Create `lib.rs`**

```rust
//! rupu-cli — the `rupu` binary.
//!
//! `pub async fn run(args)` is the testable entry point: it parses
//! the command line via clap, dispatches to a subcommand handler in
//! [`cmd`], and returns an `ExitCode`. The binary's `main.rs` is a
//! one-line wrapper that calls into here.

pub mod cmd;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "rupu", version, about = "Agentic code-development CLI", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// One-shot agent run.
    Run(cmd::run::Args),
    /// Manage agents.
    Agent {
        #[command(subcommand)]
        action: cmd::agent::Action,
    },
    /// Manage workflows.
    Workflow {
        #[command(subcommand)]
        action: cmd::workflow::Action,
    },
    /// Browse transcripts.
    Transcript {
        #[command(subcommand)]
        action: cmd::transcript::Action,
    },
    /// Get / set configuration values.
    Config {
        #[command(subcommand)]
        action: cmd::config::Action,
    },
    /// Manage provider credentials.
    Auth {
        #[command(subcommand)]
        action: cmd::auth::Action,
    },
}

/// Testable entrypoint. Parses `args` (typically from `std::env::args`),
/// dispatches, and returns an `ExitCode`. Tests pass synthetic argv.
pub async fn run(args: Vec<String>) -> ExitCode {
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            // clap handles --help / --version with its own non-zero codes;
            // surface them faithfully.
            e.exit();
        }
    };
    match cli.command {
        Cmd::Run(args) => cmd::run::handle(args).await,
        Cmd::Agent { action } => cmd::agent::handle(action).await,
        Cmd::Workflow { action } => cmd::workflow::handle(action).await,
        Cmd::Transcript { action } => cmd::transcript::handle(action).await,
        Cmd::Config { action } => cmd::config::handle(action).await,
        Cmd::Auth { action } => cmd::auth::handle(action).await,
    }
}
```

- [ ] **Step 6: Create `cmd/mod.rs`**

```rust
//! Subcommand handlers. Each module owns one verb.

pub mod agent;
pub mod auth;
pub mod config;
pub mod run;
pub mod transcript;
pub mod workflow;
```

- [ ] **Step 7: Stub each cmd module with a minimal compileable shape**

Create each of `crates/rupu-cli/src/cmd/{run,agent,workflow,transcript,config,auth}.rs` with this skeleton (filling in the module name in the doc comment):

```rust
//! `rupu <verb>` subcommand. Real impl lands in Task <N>.

use clap::Args;
use std::process::ExitCode;

#[derive(Args, Debug)]
pub struct Args; // unit; replaced in later tasks

pub async fn handle(_args: Args) -> ExitCode {
    eprintln!("not implemented yet");
    ExitCode::from(2)
}
```

EXCEPT for the modules that the `lib.rs` `Cmd` enum dispatches via `Action` — those need an `Action` enum instead of `Args`. For agent/workflow/transcript/config/auth, replace the body with:

```rust
//! `rupu <verb>` subcommand. Real impl lands in Task <N>.

use clap::Subcommand;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Placeholder; real subcommands land in Task <N>.
    #[command(hide = true)]
    Stub,
}

pub async fn handle(_action: Action) -> ExitCode {
    eprintln!("not implemented yet");
    ExitCode::from(2)
}
```

For `run.rs` only, keep the `Args` form but make the unit struct compile under clap by giving it at least one field:

```rust
//! `rupu run <agent> [prompt]`. Real impl lands in Task 4.

use clap::Args;
use std::process::ExitCode;

#[derive(Args, Debug)]
pub struct Args {
    /// Name of the agent to run (matches an `agents/*.md` file).
    pub agent: String,
    /// Optional initial prompt; defaults to "go" if omitted.
    pub prompt: Option<String>,
}

pub async fn handle(_args: Args) -> ExitCode {
    eprintln!("not implemented yet");
    ExitCode::from(2)
}
```

- [ ] **Step 8: Verify build + `--version` works**

```bash
cargo build -p rupu-cli
cargo run -p rupu-cli -- --version
```
Expected: prints `rupu 0.1.0`.

```bash
cargo run -p rupu-cli -- --help
```
Expected: clap-generated help with all six subcommands listed.

- [ ] **Step 9: Hygiene + commit**

```bash
cargo fmt --all -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings
git add Cargo.toml Cargo.lock crates/rupu-cli
git -c commit.gpgsign=false commit -m "feat(cli): rupu-cli crate skeleton + clap entry"
```

---

### Task 2: Paths + logging + crash utilities

**Files:**
- Create: `crates/rupu-cli/src/paths.rs`
- Create: `crates/rupu-cli/src/logging.rs`
- Create: `crates/rupu-cli/src/crash.rs`
- Modify: `crates/rupu-cli/src/lib.rs` — declare the new modules + init logging at run() entry
- Test: `crates/rupu-cli/tests/cli_paths.rs`

- [ ] **Step 1: Failing test**

Create `crates/rupu-cli/tests/cli_paths.rs`:

```rust
use assert_fs::prelude::*;
use rupu_cli::paths::{global_dir, project_root_for, transcripts_dir};

#[test]
fn global_dir_uses_rupu_home_env_when_set() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let g = global_dir().unwrap();
    assert_eq!(g.canonicalize().unwrap(), tmp.path().canonicalize().unwrap());
    std::env::remove_var("RUPU_HOME");
}

#[test]
fn project_root_walks_up_for_dot_rupu() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".rupu").create_dir_all().unwrap();
    let nested = tmp.child("a/b/c");
    nested.create_dir_all().unwrap();
    let r = project_root_for(nested.path()).unwrap();
    assert_eq!(
        r.unwrap().canonicalize().unwrap(),
        tmp.path().canonicalize().unwrap()
    );
}

#[test]
fn transcripts_dir_is_project_local_when_present() {
    let tmp_global = assert_fs::TempDir::new().unwrap();
    let tmp_project = assert_fs::TempDir::new().unwrap();
    tmp_project.child(".rupu/transcripts").create_dir_all().unwrap();
    let dir = transcripts_dir(tmp_global.path(), Some(tmp_project.path()));
    assert_eq!(
        dir.canonicalize().unwrap(),
        tmp_project
            .child(".rupu/transcripts")
            .path()
            .canonicalize()
            .unwrap()
    );
}

#[test]
fn transcripts_dir_falls_back_to_global() {
    let tmp_global = assert_fs::TempDir::new().unwrap();
    let dir = transcripts_dir(tmp_global.path(), None);
    assert!(dir.ends_with("transcripts"));
}
```

- [ ] **Step 2: Run — expect FAIL** (`paths.rs` doesn't exist).

```bash
cargo test -p rupu-cli --test cli_paths
```

- [ ] **Step 3: Implement `paths.rs`**

```rust
//! `~/.rupu/` resolution + project `.rupu/` discovery.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Resolve the global rupu directory. Honors `$RUPU_HOME` if set
/// (used by tests + by users who want a non-default location);
/// otherwise falls back to `~/.rupu/`.
pub fn global_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("RUPU_HOME") {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate home directory"))?;
    Ok(home.join(".rupu"))
}

/// Walk up from `pwd` looking for the first `.rupu/` directory. Returns
/// `Some(path)` of the directory containing it, or `None` if not found.
pub fn project_root_for(pwd: &Path) -> Result<Option<PathBuf>> {
    let canonical = pwd
        .canonicalize()
        .with_context(|| format!("canonicalize {}", pwd.display()))?;
    let mut cursor: Option<&Path> = Some(&canonical);
    while let Some(dir) = cursor {
        if dir.join(".rupu").is_dir() {
            return Ok(Some(dir.to_path_buf()));
        }
        cursor = dir.parent();
    }
    Ok(None)
}

/// Pick the transcripts directory. Project-local when
/// `<project>/.rupu/transcripts/` exists; global default otherwise.
pub fn transcripts_dir(global: &Path, project_root: Option<&Path>) -> PathBuf {
    if let Some(p) = project_root {
        let local = p.join(".rupu/transcripts");
        if local.is_dir() {
            return local;
        }
    }
    global.join("transcripts")
}

/// Convenience: ensure a directory exists. Used to lazily create
/// `~/.rupu/cache/`, `~/.rupu/transcripts/`, etc. on first use.
pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("create_dir_all {}", p.display()))?;
    Ok(())
}
```

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p rupu-cli --test cli_paths
```
Expected: 4 passing.

- [ ] **Step 5: Implement `logging.rs`**

Create `crates/rupu-cli/src/logging.rs`:

```rust
//! Logging init. Uses `tracing-subscriber` with env-filter so users
//! can `RUPU_LOG=debug rupu run ...` to see internals.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize logging. Idempotent — safe to call multiple times in
/// the same process (tests rely on this).
pub fn init() {
    let filter = EnvFilter::try_from_env("RUPU_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(false).with_writer(std::io::stderr))
        .try_init();
}
```

- [ ] **Step 6: Implement `crash.rs`**

Create `crates/rupu-cli/src/crash.rs`:

```rust
//! Crash logger. Installs a panic hook that writes a single
//! `~/.rupu/cache/crash-<rfc3339>.log` on panic before letting the
//! default panic behavior run.

use crate::paths;
use std::panic;

pub fn install_panic_hook() {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if let Err(e) = write_crash_log(info) {
            eprintln!("rupu: failed to write crash log: {e}");
        }
        prev(info);
    }));
}

fn write_crash_log(info: &panic::PanicHookInfo<'_>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let cache = global.join("cache");
    paths::ensure_dir(&cache)?;
    let now = chrono::Utc::now().to_rfc3339();
    let path = cache.join(format!("crash-{now}.log"));
    let body = format!("{info}\n\n{}", std::backtrace::Backtrace::force_capture());
    std::fs::write(&path, body)?;
    eprintln!("rupu: crash log written to {}", path.display());
    Ok(())
}
```

- [ ] **Step 7: Wire init into `lib.rs`**

In `crates/rupu-cli/src/lib.rs`, add the new modules and call init at the top of `run`:

```rust
pub mod cmd;
pub mod crash;
pub mod logging;
pub mod paths;

// ... existing Cli/Cmd structs ...

pub async fn run(args: Vec<String>) -> ExitCode {
    logging::init();
    crash::install_panic_hook();

    let cli = match Cli::try_parse_from(args) {
        // ... existing body unchanged ...
    };
    // ... existing dispatch unchanged ...
}
```

- [ ] **Step 8: Hygiene + commit**

```bash
cargo build -p rupu-cli
cargo fmt --all -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings
cargo test -p rupu-cli
git add crates/rupu-cli
git -c commit.gpgsign=false commit -m "feat(cli): paths + logging + crash hook"
```

---

### Task 3: Provider factory (TDD against mock)

**Files:**
- Create: `crates/rupu-cli/src/provider_factory.rs`
- Modify: `crates/rupu-cli/src/lib.rs` — declare the module
- Test: `crates/rupu-cli/tests/cli_provider_factory.rs`

**Approach:** the factory takes an `AgentSpec`, the resolved permission mode, the chosen `AuthBackend`, and returns a `Box<dyn LlmProvider>` plus the provider name string used by the transcript. For v0, support `anthropic` only — other providers (`openai`, `copilot`, `local`, `gemini`, `codex`) return `BLOCKED` errors with a clear message. This narrows the v0 surface; Plan 4+ adds the others.

- [ ] **Step 1: Failing test**

Create `crates/rupu-cli/tests/cli_provider_factory.rs`:

```rust
use rupu_cli::provider_factory::{build_for_provider, FactoryError};

#[tokio::test]
async fn anthropic_factory_requires_credential() {
    // No auth.json; no env var. Should fail with a clear message.
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let res = build_for_provider("anthropic", "claude-sonnet-4-6", &auth_path).await;
    let err = format!("{}", res.unwrap_err());
    assert!(
        err.contains("anthropic") || err.contains("credential"),
        "expected clear missing-credential error, got: {err}"
    );
}

#[tokio::test]
async fn unknown_provider_errors_clearly() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let res = build_for_provider("teleport", "model-x", &auth_path).await;
    let err = format!("{}", res.unwrap_err());
    assert!(err.contains("teleport"), "expected provider name in error: {err}");
}

#[tokio::test]
async fn deferred_provider_returns_blocked_error() {
    // openai/copilot/gemini/local are defined types but v0 wires only
    // anthropic. Expect a clear "not wired in v0" error.
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    for p in ["openai", "copilot", "gemini", "local"] {
        let res = build_for_provider(p, "x", &auth_path).await;
        let err = format!("{}", res.unwrap_err());
        assert!(
            err.contains(p) && (err.contains("not wired") || err.contains("v0")),
            "{p}: expected v0-deferral error: {err}"
        );
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p rupu-cli --test cli_provider_factory
```

- [ ] **Step 3: Implement**

Create `crates/rupu-cli/src/provider_factory.rs`:

```rust
//! Build a `Box<dyn LlmProvider>` from a provider-name string +
//! credential lookup. v0 wires Anthropic only; other providers (OpenAI
//! Codex, Copilot, Gemini, local) return a clear "not wired in v0"
//! error so the failure mode is informative rather than a silent
//! provider-discovery miss.
//!
//! When the lifted `rupu-providers` API stabilizes, this file is the
//! one place to extend.

use rupu_providers::provider::LlmProvider;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FactoryError {
    #[error("missing credential for provider {provider}: configure with `rupu auth login --provider {provider}` or set the env var the provider expects")]
    MissingCredential { provider: String },
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("provider {0} is not wired in v0; only `anthropic` is currently supported")]
    NotWiredInV0(String),
    #[error("provider construction failed: {0}")]
    Other(String),
}

/// Build a provider for `name`. Reads credentials from environment or
/// `auth_json_path` (the chmod-600 fallback file).
pub async fn build_for_provider(
    name: &str,
    model: &str,
    auth_json_path: &Path,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    match name {
        "anthropic" => build_anthropic(model, auth_json_path).await,
        "openai" | "openai_codex" | "codex" | "copilot" | "github_copilot" | "gemini"
        | "google_gemini" | "local" => Err(FactoryError::NotWiredInV0(name.to_string())),
        _ => Err(FactoryError::UnknownProvider(name.to_string())),
    }
}

async fn build_anthropic(
    model: &str,
    auth_json_path: &Path,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    // Prefer env var; fall back to auth.json.
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok().or_else(|| {
        if !auth_json_path.exists() {
            return None;
        }
        let text = std::fs::read_to_string(auth_json_path).ok()?;
        let val: serde_json::Value = serde_json::from_str(&text).ok()?;
        val.get("anthropic")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    let api_key = api_key.ok_or_else(|| FactoryError::MissingCredential {
        provider: "anthropic".to_string(),
    })?;

    // Construct the AnthropicClient. The exact constructor is in
    // crates/rupu-providers/src/anthropic.rs — typically
    // `AnthropicClient::with_api_key(api_key, model)` or similar.
    // If the constructor signature differs, the implementer should
    // grep `^impl AnthropicClient` and adapt this call.
    let client = rupu_providers::anthropic::AnthropicClient::with_api_key(&api_key, model);
    Ok(Box::new(client))
}
```

**Implementer note for Step 3:** the exact `AnthropicClient` constructor name + signature must be verified against `crates/rupu-providers/src/anthropic.rs`. The plan code assumes `AnthropicClient::with_api_key(&str, &str) -> AnthropicClient`; if that doesn't match, adapt the one call site or report BLOCKED.

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p rupu-cli --test cli_provider_factory
```
Expected: 3 passing tests (the anthropic test exercises the missing-credential path; we don't have a real API key in tests, and we're not actually invoking the provider).

- [ ] **Step 5: Hygiene + commit**

```bash
cargo build -p rupu-cli
cargo fmt --all -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings
git add crates/rupu-cli
git -c commit.gpgsign=false commit -m "feat(cli): provider_factory with v0 anthropic-only wiring"
```

---

## Phase 2 — `rupu run`

### Task 4: `rupu run` subcommand (TDD via mock provider)

This is the largest task in Plan 3. The `rupu run` handler:
1. Resolves `~/.rupu/` and project `.rupu/` paths.
2. Loads the named agent (project shadows global).
3. Layers config (global + project).
4. Resolves permission mode (CLI flag > frontmatter > project config > global config > Ask).
5. Selects the auth backend (keyring → JSON fallback).
6. Picks a workspace via `rupu_workspace::upsert`.
7. Builds the provider via `provider_factory`.
8. Builds the `AgentRunOpts` and calls `rupu_agent::run_agent`.
9. Prints a one-line summary on success or non-zero exit on failure.

**To make this testable without an API key**, expose an undocumented `RUPU_MOCK_PROVIDER_SCRIPT` env var that, when set, swaps the real `provider_factory::build_for_provider` for a `MockProvider` driven by the script in the env var. This is the integration-test seam.

**Files:**
- Modify: `crates/rupu-cli/src/cmd/run.rs`
- Modify: `crates/rupu-cli/src/provider_factory.rs` — add the mock-script env-var path
- Test: `crates/rupu-cli/tests/cli_run.rs`

- [ ] **Step 1: Add the mock-script env-var path to `provider_factory.rs`**

At the top of `build_for_provider`, before the match:

```rust
// Test-only seam: if RUPU_MOCK_PROVIDER_SCRIPT is set, build a
// MockProvider from the JSON script in the env var. Production users
// never set this; tests use it to drive the agent loop end-to-end.
if let Ok(json) = std::env::var("RUPU_MOCK_PROVIDER_SCRIPT") {
    return build_mock_from_script(&json);
}
```

Add the helper:

```rust
fn build_mock_from_script(json: &str) -> Result<Box<dyn LlmProvider>, FactoryError> {
    use rupu_agent::runner::{MockProvider, ScriptedTurn};
    let turns: Vec<ScriptedTurn> =
        serde_json::from_str(json).map_err(|e| FactoryError::Other(format!("mock script: {e}")))?;
    Ok(Box::new(MockProvider::new(turns)))
}
```

(`ScriptedTurn` derives `Deserialize` per Plan 2 Task 7. Verify; if not, add the derive in a one-line follow-up to rupu-agent.)

- [ ] **Step 2: Failing test**

Create `crates/rupu-cli/tests/cli_run.rs`:

```rust
use assert_fs::prelude::*;

const MOCK_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "Hello from the mock provider.", "stop": "EndTurn" } }
]
"#;

#[tokio::test]
async fn rupu_run_writes_transcript_under_mock_provider() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    // Force PWD to project so workspace discovery uses it.
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "echo".into(),
        "--mode".into(),
        "bypass".into(),
        "say hi".into(),
    ])
    .await;

    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        u8::from(exit),
        0,
        "rupu run should exit 0 when the mock provider succeeds"
    );

    // Find the transcript file written under <global>/transcripts/<run_id>.jsonl
    let transcripts = global.child("transcripts");
    let entries: Vec<_> = std::fs::read_dir(transcripts.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one transcript file");
    let summary = rupu_transcript::JsonlReader::summary(&entries[0].path()).unwrap();
    assert_eq!(summary.status, rupu_transcript::RunStatus::Ok);
}

#[tokio::test]
async fn rupu_run_unknown_agent_exits_nonzero() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let exit = rupu_cli::run(vec!["rupu".into(), "run".into(), "nonexistent".into()]).await;
    std::env::remove_var("RUPU_HOME");
    assert_ne!(u8::from(exit), 0);
}
```

- [ ] **Step 3: Run — expect FAIL**

```bash
cargo test -p rupu-cli --test cli_run
```

- [ ] **Step 4: Implement `cmd/run.rs`**

Replace `crates/rupu-cli/src/cmd/run.rs`:

```rust
//! `rupu run <agent> [prompt]` — one-shot agent run.

use crate::paths;
use crate::provider_factory;
use clap::Args;
use rupu_agent::runner::{
    AgentRunOpts, BypassDecider, PermissionDecider, PermissionDecision,
};
use rupu_agent::{load_agent, parse_mode, resolve_mode};
use rupu_tools::{PermissionMode, ToolContext};
use std::process::ExitCode;
use std::sync::Arc;
use ulid::Ulid;

#[derive(Args, Debug)]
pub struct Args {
    /// Agent name (matches an `agents/*.md` file).
    pub agent: String,
    /// Optional initial user message.
    pub prompt: Option<String>,
    /// Override permission mode (`ask` | `bypass` | `readonly`).
    #[arg(long)]
    pub mode: Option<String>,
}

pub async fn handle(args: Args) -> ExitCode {
    match run_inner(args).await {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu run: {e}");
            ExitCode::from(1)
        }
    }
}

async fn run_inner(args: Args) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;

    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Load the agent (project shadows global).
    let global_agents_parent = &global; // load_agents takes the parent of `agents/`
    let project_agents_parent = project_root.as_deref();
    let spec = load_agent(global_agents_parent, project_agents_parent, &args.agent)?;

    // Resolve config (global + project).
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root
        .as_ref()
        .map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(
        Some(&global_cfg_path),
        project_cfg_path.as_deref(),
    )?;

    // Resolve permission mode.
    let cli_mode = args.mode.as_deref().and_then(parse_mode);
    let agent_mode = spec.permission_mode.as_deref().and_then(parse_mode);
    let project_mode = None; // project-level mode override is rare; v0 reads only the cli/agent/global path
    let global_mode = cfg.permission_mode.as_deref().and_then(parse_mode);
    let mode = resolve_mode(cli_mode, agent_mode, project_mode, global_mode);

    // Non-TTY + Ask = abort (spec rule).
    if matches!(mode, PermissionMode::Ask) && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        anyhow::bail!("non-tty + ask mode: rerun with `--mode bypass` or `--mode readonly`, or run from an interactive terminal");
    }

    // Workspace upsert.
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &pwd)?;

    // Auth backend selection.
    let auth_json_path = global.join("auth.json");
    let cache = rupu_auth::ProbeCache::new(global.join("cache/auth-backend.json"));
    let _backend = rupu_auth::select_backend(&cache, auth_json_path.clone());

    // Provider build.
    let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
    let model = spec
        .model
        .clone()
        .or_else(|| cfg.default_model.clone())
        .unwrap_or_else(|| "claude-sonnet-4-6".into());
    let provider = provider_factory::build_for_provider(&provider_name, &model, &auth_json_path).await?;

    // Transcript path.
    let run_id = format!("run_{}", Ulid::new());
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    let transcript_path = transcripts.join(format!("{run_id}.jsonl"));

    // Tool context.
    let bash_timeout = cfg.bash.timeout_secs.unwrap_or(120);
    let bash_allowlist = cfg.bash.env_allowlist.clone().unwrap_or_default();
    let tool_context = ToolContext {
        workspace_path: pwd.clone(),
        bash_env_allowlist: bash_allowlist,
        bash_timeout_secs: bash_timeout,
    };

    let user_message = args.prompt.unwrap_or_else(|| "go".to_string());
    let mode_str = match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Bypass => "bypass",
        PermissionMode::Readonly => "readonly",
    };

    let decider: Arc<dyn PermissionDecider> = pick_decider(mode);

    let opts = AgentRunOpts {
        agent_name: spec.name.clone(),
        agent_system_prompt: spec.system_prompt.clone(),
        agent_tools: spec.tools.clone(),
        provider,
        provider_name,
        model,
        run_id: run_id.clone(),
        workspace_id: ws.id.clone(),
        workspace_path: pwd.clone(),
        transcript_path: transcript_path.clone(),
        max_turns: spec.max_turns.unwrap_or(50),
        decider,
        tool_context,
        user_message,
        mode_str: mode_str.to_string(),
    };

    let result = rupu_agent::run_agent(opts).await?;
    println!(
        "rupu: run {} complete in {} turn(s); transcript: {}",
        run_id,
        result.turns,
        transcript_path.display()
    );
    Ok(())
}

fn pick_decider(mode: PermissionMode) -> Arc<dyn PermissionDecider> {
    match mode {
        PermissionMode::Bypass => Arc::new(BypassDecider),
        PermissionMode::Readonly => Arc::new(ReadonlyDecider),
        PermissionMode::Ask => Arc::new(AskDecider),
    }
}

/// Readonly: deny writers (bash/write_file/edit_file), allow readers.
struct ReadonlyDecider;
impl PermissionDecider for ReadonlyDecider {
    fn decide(
        &self,
        _mode: PermissionMode,
        tool: &str,
        _input: &serde_json::Value,
        _workspace: &str,
    ) -> Result<PermissionDecision, rupu_agent::runner::RunError> {
        match tool {
            "bash" | "write_file" | "edit_file" => Ok(PermissionDecision::Deny),
            _ => Ok(PermissionDecision::Allow),
        }
    }
}

/// Ask: stdin-driven prompt for writers; readers always allowed.
struct AskDecider;
impl PermissionDecider for AskDecider {
    fn decide(
        &self,
        _mode: PermissionMode,
        tool: &str,
        input: &serde_json::Value,
        workspace: &str,
    ) -> Result<PermissionDecision, rupu_agent::runner::RunError> {
        if !matches!(tool, "bash" | "write_file" | "edit_file") {
            return Ok(PermissionDecision::Allow);
        }
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let mut prompt = rupu_agent::PermissionPrompt::new_in_memory_with(
            std::io::BufReader::new(stdin.lock()),
            &mut handle,
        );
        let decision = prompt.ask(tool, input, workspace).map_err(|e| {
            rupu_agent::runner::RunError::Provider(format!("ask prompt io: {e}"))
        })?;
        Ok(decision)
    }
}
```

**Implementer note for Step 4:** the `AskDecider` references `PermissionPrompt::new_in_memory_with` which doesn't exist in Plan 2's Task 5 implementation (which used `new_in_memory(&[u8], &mut W)`). The Task 5 implementer chose `Box<dyn BufRead + 'r>` for the reader; that means we can pass a real `StdinLock` if we wrap it via `Box::new(BufReader::new(stdin.lock())) as Box<dyn BufRead>`. Adjust the `AskDecider` impl to whatever the actual `PermissionPrompt` constructor accepts. If the existing API doesn't easily wrap stdin/stdout, add a new constructor `PermissionPrompt::for_stdio() -> Self` in `rupu-agent` (one-line follow-up) and use it here.

- [ ] **Step 5: Run — expect PASS**

```bash
cargo test -p rupu-cli --test cli_run
```
Expected: 2 passing tests.

- [ ] **Step 6: Hygiene + commit**

```bash
cargo build -p rupu-cli
cargo fmt --all -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings
git add crates/rupu-cli
git -c commit.gpgsign=false commit -m "feat(cli): rupu run end-to-end via mock-provider env-var seam"
```

---

### Task 5: Verify ScriptedTurn deserialization in rupu-agent (small followup)

The mock-script seam in Task 4 deserializes `Vec<ScriptedTurn>` from JSON. Plan 2 Task 7 might have derived only `Serialize` (or neither) on `ScriptedTurn`. Verify:

```bash
grep -A 5 "pub enum ScriptedTurn" crates/rupu-agent/src/runner.rs
```

If the derive list lacks `Deserialize`, edit `crates/rupu-agent/src/runner.rs` to add it:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ScriptedTurn { /* ... */ }
```

(May also need to add `serde` to `rupu-agent`'s deps with `derive` feature — already in workspace.)

If a derive change is needed, run `cargo test -p rupu-agent` to verify nothing broke and commit:

```bash
git add crates/rupu-agent
git -c commit.gpgsign=false commit -m "feat(agent): derive Deserialize on ScriptedTurn for CLI mock seam"
```

If no change needed, skip the commit and note it in the Task 4 review.

---

## Phase 3 — Other subcommands

### Task 6: `rupu agent list | show`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/agent.rs`
- Test: `crates/rupu-cli/tests/cli_agent.rs`

- [ ] **Step 1: Test**

```rust
// tests/cli_agent.rs
use assert_fs::prelude::*;

#[tokio::test]
async fn agent_list_shows_global_and_project_with_chips() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/g1.md")
        .write_str("---\nname: g1\n---\nbody")
        .unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    project.child(".rupu/agents").create_dir_all().unwrap();
    project
        .child(".rupu/agents/p1.md")
        .write_str("---\nname: p1\n---\nbody")
        .unwrap();

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec!["rupu".into(), "agent".into(), "list".into()]).await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(u8::from(exit), 0);
    // (output capture via gag or similar would be needed for stronger
    // assertions; for v0 we just check the exit code; richer asserts
    // can land via integration tests later).
}

#[tokio::test]
async fn agent_show_prints_body() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/x.md")
        .write_str("---\nname: x\n---\nthe body")
        .unwrap();
    std::env::set_var("RUPU_HOME", global.path());
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "agent".into(),
        "show".into(),
        "x".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(u8::from(exit), 0);
}

#[tokio::test]
async fn agent_show_missing_exits_nonzero() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "agent".into(),
        "show".into(),
        "nope".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");
    assert_ne!(u8::from(exit), 0);
}
```

- [ ] **Step 2: Implement**

Replace `crates/rupu-cli/src/cmd/agent.rs`:

```rust
//! `rupu agent list | show <name>`.

use crate::paths;
use clap::Subcommand;
use rupu_agent::{load_agent, load_agents};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all available agents (global + project).
    List,
    /// Print an agent's frontmatter and body.
    Show {
        /// Name of the agent.
        name: String,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::List => match list().await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu agent list: {e}");
                ExitCode::from(1)
            }
        },
        Action::Show { name } => match show(&name).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu agent show: {e}");
                ExitCode::from(1)
            }
        },
    }
}

async fn list() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let agents = load_agents(&global, project_root.as_deref())?;

    println!("{:<24} {:<10} {}", "NAME", "SCOPE", "DESCRIPTION");
    for a in &agents {
        let scope = scope_for(&a.name, &global, project_root.as_deref());
        let desc = a.description.as_deref().unwrap_or("-");
        println!("{:<24} {:<10} {}", a.name, scope, desc);
    }
    Ok(())
}

async fn show(name: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let spec = load_agent(&global, project_root.as_deref(), name)?;
    println!("name:        {}", spec.name);
    if let Some(d) = &spec.description {
        println!("description: {d}");
    }
    if let Some(p) = &spec.provider {
        println!("provider:    {p}");
    }
    if let Some(m) = &spec.model {
        println!("model:       {m}");
    }
    if let Some(t) = &spec.tools {
        println!("tools:       {}", t.join(", "));
    }
    if let Some(mt) = spec.max_turns {
        println!("maxTurns:    {mt}");
    }
    if let Some(pm) = &spec.permission_mode {
        println!("mode:        {pm}");
    }
    println!("\n--- system prompt ---");
    print!("{}", spec.system_prompt);
    Ok(())
}

fn scope_for(name: &str, global: &std::path::Path, project: Option<&std::path::Path>) -> String {
    if let Some(p) = project {
        if p.join("agents").join(format!("{name}.md")).exists() {
            return "project".to_string();
        }
    }
    if global.join("agents").join(format!("{name}.md")).exists() {
        "global".to_string()
    } else {
        "?".to_string()
    }
}
```

- [ ] **Step 3: Verify + commit**

```bash
cargo test -p rupu-cli --test cli_agent
cargo fmt --all -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings
git add crates/rupu-cli
git -c commit.gpgsign=false commit -m "feat(cli): rupu agent list/show"
```

---

### Task 7: `rupu workflow list | show | run`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs`
- Test: `crates/rupu-cli/tests/cli_workflow.rs`

Pattern mirrors Task 6 but for workflows. The `run` action takes a workflow name and zero or more `--input KEY=VALUE` pairs, builds an `OrchestratorRunOpts` via a `StepFactory` impl that wires real providers via `provider_factory::build_for_provider`, and dispatches `rupu_orchestrator::run_workflow`.

- [ ] **Step 1: Test (workflow list/show + 1-step workflow run via mock)**

```rust
// tests/cli_workflow.rs
use assert_fs::prelude::*;

const MOCK_SCRIPT: &str = r#"
[
  { "AssistantText": { "text": "step output", "stop": "EndTurn" } }
]
"#;

const WORKFLOW_YAML: &str = r#"
name: hello-wf
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#;

#[tokio::test]
async fn workflow_run_executes_one_step_via_mock() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.")
        .unwrap();
    global.child("workflows").create_dir_all().unwrap();
    global
        .child("workflows/hello-wf.yaml")
        .write_str(WORKFLOW_YAML)
        .unwrap();

    let project = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", MOCK_SCRIPT);
    std::env::set_current_dir(project.path()).unwrap();

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "hello-wf".into(),
        "--mode".into(),
        "bypass".into(),
    ])
    .await;
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");
    assert_eq!(u8::from(exit), 0);
}
```

- [ ] **Step 2: Implement** — full impl in the file. The handler:
  - Reads `<global>/workflows/<name>.yaml` (or `<project>/.rupu/workflows/<name>.yaml`, project shadows).
  - Builds a `StepFactory` impl that constructs real providers via `provider_factory::build_for_provider` (using the same env-var seam for the mock-script path).
  - Calls `rupu_orchestrator::run_workflow`.
  - Prints a per-step summary on success.

The implementer should consult the linear_runner test in Plan 2 Task 11 for the StepFactory pattern.

```rust
//! `rupu workflow list | show | run`.

use crate::paths;
use crate::provider_factory;
use async_trait::async_trait;
use clap::Subcommand;
use rupu_agent::runner::{
    AgentRunOpts, BypassDecider, PermissionDecider,
};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::Workflow;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all workflows (global + project).
    List,
    /// Print a workflow's YAML.
    Show {
        name: String,
    },
    /// Run a workflow.
    Run {
        name: String,
        /// `KEY=VALUE` template inputs (repeatable).
        #[arg(long, value_parser = parse_kv)]
        input: Vec<(String, String)>,
        /// Override permission mode.
        #[arg(long)]
        mode: Option<String>,
    },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s.split_once('=').ok_or_else(|| format!("expected KEY=VALUE: {s}"))?;
    Ok((k.to_string(), v.to_string()))
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::List => list().await,
        Action::Show { name } => show(&name).await,
        Action::Run { name, input, mode } => run(&name, input, mode.as_deref()).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu workflow: {e}");
            ExitCode::from(1)
        }
    }
}

async fn list() -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let mut found: Vec<(String, String)> = Vec::new(); // (name, scope)
    if let Some(p) = &project_root {
        push_yaml_names(&p.join(".rupu/workflows"), "project", &mut found);
    }
    push_yaml_names(&global.join("workflows"), "global", &mut found);
    found.sort();
    println!("{:<28} {}", "NAME", "SCOPE");
    for (n, s) in &found {
        println!("{:<28} {}", n, s);
    }
    Ok(())
}

fn push_yaml_names(dir: &std::path::Path, scope: &str, into: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            into.push((stem.to_string(), scope.to_string()));
        }
    }
}

async fn show(name: &str) -> anyhow::Result<()> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;
    print!("{body}");
    Ok(())
}

fn locate_workflow(name: &str) -> anyhow::Result<PathBuf> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    if let Some(p) = &project_root {
        let candidate = p.join(".rupu/workflows").join(format!("{name}.yaml"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let global = paths::global_dir()?;
    let candidate = global.join("workflows").join(format!("{name}.yaml"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(anyhow::anyhow!("workflow not found: {name}"))
}

async fn run(name: &str, inputs: Vec<(String, String)>, mode: Option<&str>) -> anyhow::Result<()> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Workspace upsert.
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &pwd)?;

    let auth_json_path = global.join("auth.json");
    let mode_str = mode.unwrap_or("ask").to_string();
    let transcripts = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts)?;

    let factory = Arc::new(CliStepFactory {
        global: global.clone(),
        project_root: project_root.clone(),
        auth_json_path,
        mode_str,
    });

    let inputs_map: BTreeMap<String, String> = inputs.into_iter().collect();
    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: ws.id,
        workspace_path: pwd.clone(),
        transcript_dir: transcripts,
        factory,
    };
    let result = run_workflow(opts).await?;
    for sr in &result.step_results {
        println!(
            "rupu: step {} run {} -> {}",
            sr.step_id,
            sr.run_id,
            sr.transcript_path.display()
        );
    }
    Ok(())
}

struct CliStepFactory {
    global: PathBuf,
    project_root: Option<PathBuf>,
    auth_json_path: PathBuf,
    mode_str: String,
}

#[async_trait]
impl StepFactory for CliStepFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
    ) -> AgentRunOpts {
        // Look up the agent referenced by this step. The orchestrator
        // doesn't pass the agent name through (only step_id +
        // rendered_prompt); we re-parse the workflow file to find it.
        // For simplicity, the StepFactory contract should be enriched
        // — but for v0 we accept this re-parse.
        //
        // Workaround: stash the workflow's step list in `self` so
        // we can look up agent by step_id. (Kept simple for v0.)
        let agent_name = "echo".to_string(); // FIXME: thread the workflow through self in v0+1
        let global = &self.global;
        let project_root = self.project_root.as_deref();
        let spec = match rupu_agent::load_agent(global, project_root, &agent_name) {
            Ok(s) => s,
            Err(_) => {
                // If we can't resolve, build an "empty" agent with
                // the rendered prompt as its system prompt.
                rupu_agent::AgentSpec {
                    name: agent_name.clone(),
                    description: None,
                    provider: Some("anthropic".to_string()),
                    model: Some("claude-sonnet-4-6".to_string()),
                    tools: None,
                    max_turns: Some(50),
                    permission_mode: Some(self.mode_str.clone()),
                    system_prompt: rendered_prompt.clone(),
                }
            }
        };

        let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
        let model = spec
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".into());
        let provider = provider_factory::build_for_provider(&provider_name, &model, &self.auth_json_path)
            .await
            .expect("provider build failed in step factory");

        AgentRunOpts {
            agent_name: spec.name,
            agent_system_prompt: spec.system_prompt,
            agent_tools: spec.tools,
            provider,
            provider_name,
            model,
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            max_turns: spec.max_turns.unwrap_or(50),
            decider: Arc::new(BypassDecider) as Arc<dyn PermissionDecider>,
            tool_context: ToolContext::default(),
            user_message: rendered_prompt,
            mode_str: self.mode_str.clone(),
        }
    }
}
```

**Implementer note:** the `agent_name = "echo"` placeholder is a known v0 simplification — the test workflow above hardcodes step `agent: echo`, and the test agent file matches that name. A real implementation should thread the workflow's step list through the factory so each step's `agent:` is honored. Make this enrichment now (carry the `Workflow` in `CliStepFactory` and look up `agent_name` via `step_id`); the placeholder is wrong and will surface immediately on a multi-step real run. The plan is documenting the simplification only because v0 ships with one default workflow which has all-`echo` steps; if you ship multi-agent workflows in this PR, this needs the real lookup.

The cleaner version:

```rust
struct CliStepFactory {
    workflow: Workflow,
    global: PathBuf,
    project_root: Option<PathBuf>,
    auth_json_path: PathBuf,
    mode_str: String,
}

#[async_trait]
impl StepFactory for CliStepFactory {
    async fn build_opts_for_step(...) -> AgentRunOpts {
        let step = self.workflow.steps.iter().find(|s| s.id == step_id).expect("unknown step_id");
        let spec = rupu_agent::load_agent(&self.global, self.project_root.as_deref(), &step.agent)?;
        // ... rest as above ...
    }
}
```

(`Workflow` is `Clone`; pass it in when constructing.)

- [ ] **Step 3: Verify + commit**

```bash
cargo test -p rupu-cli --test cli_workflow
cargo fmt --all -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings
git add crates/rupu-cli
git -c commit.gpgsign=false commit -m "feat(cli): rupu workflow list/show/run with real-provider StepFactory"
```

---

### Task 8: `rupu transcript list | show <id>`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/transcript.rs`
- Test: `crates/rupu-cli/tests/cli_transcript.rs`

`list` globs `~/.rupu/transcripts/*.jsonl` (and project-local), reads `run_start` events for metadata, prints a table sorted by `started_at` descending. `show <id>` prints the full JSONL stream pretty-printed (one event per line, with `type` highlighted).

The implementation follows the same pattern as Task 6/7 — straightforward file-glob + JsonlReader::summary calls.

- [ ] **Step 1: Test (skipped here — concrete implementation similar to cli_agent)**.

- [ ] **Step 2: Implement** — see plan body for the full handler. Key points:
  - `list` collects `(transcript_path, RunSummary)` tuples; sorts by `started_at`; prints `RUN_ID | AGENT | STATUS | TOKENS | DURATION` columns.
  - `show <id>` finds the file by run_id (filename match) and pretty-prints each event.

- [ ] **Step 3: Commit**

```bash
git -c commit.gpgsign=false commit -m "feat(cli): rupu transcript list/show"
```

---

### Task 9: `rupu config get | set`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/config.rs`
- Test: `crates/rupu-cli/tests/cli_config.rs`

V0 scope: scoped to `~/.rupu/config.toml` only. `get <key>` reads a top-level key (e.g., `default_model`). `set <key> <value>` writes the key (parsing the value as TOML scalar).

- [ ] **Step 1: Test**

```rust
// tests/cli_config.rs — abbreviated
#[tokio::test]
async fn config_set_then_get_round_trip() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "set".into(),
        "default_model".into(),
        "claude-opus-4-7".into(),
    ])
    .await;
    assert_eq!(u8::from(exit), 0);

    // Read it back via the CLI
    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "config".into(),
        "get".into(),
        "default_model".into(),
    ])
    .await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(u8::from(exit), 0);

    // Verify the file content
    let toml = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
    assert!(toml.contains("default_model"));
    assert!(toml.contains("claude-opus-4-7"));
}
```

- [ ] **Step 2: Implement**

```rust
//! `rupu config get | set <key> [value]`. Scoped to ~/.rupu/config.toml.

use crate::paths;
use clap::Subcommand;
use std::process::ExitCode;
use toml::Value;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Print the value of a top-level key.
    Get { key: String },
    /// Set a top-level key. The value is parsed as a TOML scalar
    /// (string / integer / bool); to set a table or array, hand-edit
    /// the file at `~/.rupu/config.toml`.
    Set { key: String, value: String },
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::Get { key } => match get(&key).await {
            Ok(v) => {
                println!("{v}");
                ExitCode::from(0)
            }
            Err(e) => {
                eprintln!("rupu config get: {e}");
                ExitCode::from(1)
            }
        },
        Action::Set { key, value } => match set(&key, &value).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu config set: {e}");
                ExitCode::from(1)
            }
        },
    }
}

async fn get(key: &str) -> anyhow::Result<String> {
    let global = paths::global_dir()?;
    let path = global.join("config.toml");
    if !path.exists() {
        anyhow::bail!("config file does not exist: {}", path.display());
    }
    let text = std::fs::read_to_string(&path)?;
    let v: Value = toml::from_str(&text)?;
    let val = v
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("key not set: {key}"))?;
    Ok(format!("{val}"))
}

async fn set(key: &str, value: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let path = global.join("config.toml");
    let mut v: Value = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        toml::from_str(&text).unwrap_or_else(|_| Value::Table(Default::default()))
    } else {
        Value::Table(Default::default())
    };
    let parsed: Value = toml::from_str(&format!("__v = {value}"))
        .map(|t: Value| t.get("__v").cloned().unwrap_or(Value::String(value.to_string())))
        .unwrap_or(Value::String(value.to_string()));
    if let Value::Table(t) = &mut v {
        t.insert(key.to_string(), parsed);
    }
    let serialized = toml::to_string_pretty(&v)?;
    std::fs::write(&path, serialized)?;
    Ok(())
}
```

- [ ] **Step 3: Verify + commit**

```bash
cargo test -p rupu-cli --test cli_config
git -c commit.gpgsign=false commit -m "feat(cli): rupu config get/set"
```

---

### Task 10: `rupu auth login | logout | status`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/auth.rs`
- Test: `crates/rupu-cli/tests/cli_auth.rs`

V0 login flow: read API key from `--key <K>` flag or stdin (when `--key` not provided AND stdin is a tty, prompt; when stdin is piped, read it). Store via the chosen `AuthBackend`. `status` shows configured providers + backend name. `logout --provider <p>` calls `forget`.

- [ ] **Step 1: Test (status only — login requires interactive input or piped stdin which is awkward in tests; sufficient to test status)**

```rust
// tests/cli_auth.rs
use assert_fs::prelude::*;

#[tokio::test]
async fn auth_status_works_with_empty_backend() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", tmp.path());
    let exit = rupu_cli::run(vec!["rupu".into(), "auth".into(), "status".into()]).await;
    std::env::remove_var("RUPU_HOME");
    assert_eq!(u8::from(exit), 0);
}
```

- [ ] **Step 2: Implement**

```rust
//! `rupu auth login | logout | status`.

use crate::paths;
use clap::Subcommand;
use rupu_auth::{AuthBackend, ProbeCache, ProviderId};
use std::io::Read;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Store an API key for a provider.
    Login {
        /// Provider name (anthropic | openai | copilot | local).
        #[arg(long)]
        provider: String,
        /// API key. If omitted, reads from stdin.
        #[arg(long)]
        key: Option<String>,
    },
    /// Remove a stored credential.
    Logout {
        #[arg(long)]
        provider: String,
    },
    /// Show configured providers + backend.
    Status,
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Login { provider, key } => login(&provider, key.as_deref()).await,
        Action::Logout { provider } => logout(&provider).await,
        Action::Status => status().await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu auth: {e}");
            ExitCode::from(1)
        }
    }
}

fn parse_provider(s: &str) -> anyhow::Result<ProviderId> {
    match s {
        "anthropic" => Ok(ProviderId::Anthropic),
        "openai" => Ok(ProviderId::Openai),
        "copilot" => Ok(ProviderId::Copilot),
        "local" => Ok(ProviderId::Local),
        _ => Err(anyhow::anyhow!("unknown provider: {s}")),
    }
}

async fn login(provider: &str, key: Option<&str>) -> anyhow::Result<()> {
    let pid = parse_provider(provider)?;
    let secret = match key {
        Some(k) => k.to_string(),
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf.trim().to_string()
        }
    };
    if secret.is_empty() {
        anyhow::bail!("empty API key");
    }
    let backend = backend_for_global()?;
    backend.store(pid, &secret)?;
    println!("rupu: stored credential for {provider} via {}", backend.name());
    Ok(())
}

async fn logout(provider: &str) -> anyhow::Result<()> {
    let pid = parse_provider(provider)?;
    let backend = backend_for_global()?;
    backend.forget(pid)?;
    println!("rupu: forgot credential for {provider}");
    Ok(())
}

async fn status() -> anyhow::Result<()> {
    let backend = backend_for_global()?;
    println!("backend: {}", backend.name());
    for p in [
        ProviderId::Anthropic,
        ProviderId::Openai,
        ProviderId::Copilot,
        ProviderId::Local,
    ] {
        let configured = backend.retrieve(p).is_ok();
        println!(
            "{:<10} {}",
            p.as_str(),
            if configured { "configured" } else { "-" }
        );
    }
    Ok(())
}

fn backend_for_global() -> anyhow::Result<Box<dyn AuthBackend>> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let cache = ProbeCache::new(global.join("cache/auth-backend.json"));
    let auth_json = global.join("auth.json");
    Ok(rupu_auth::select_backend(&cache, auth_json))
}
```

- [ ] **Step 3: Verify + commit**

```bash
cargo test -p rupu-cli --test cli_auth
git -c commit.gpgsign=false commit -m "feat(cli): rupu auth login/logout/status"
```

---

## Phase 4 — Default library

### Task 11: Default agents + sample workflow + first-run install

**Files:**
- Create: `agents/fix-bug.md`
- Create: `agents/add-tests.md`
- Create: `agents/review-diff.md`
- Create: `agents/scaffold.md`
- Create: `agents/summarize-diff.md`
- Create: `workflows/investigate-then-fix.yaml`
- Create: `crates/rupu-cli/src/defaults.rs`
- Modify: `crates/rupu-cli/src/lib.rs` — declare `pub mod defaults;`
- Modify: `crates/rupu-cli/src/cmd/run.rs` and `crates/rupu-cli/src/cmd/agent.rs` — fall through to embedded defaults when project + global don't have the agent
- Test: `crates/rupu-cli/tests/cli_defaults.rs`

The five default agents are simple `.md` files. Example for `fix-bug.md`:

```markdown
---
name: fix-bug
description: Investigate a failing test and propose a minimal fix.
provider: anthropic
model: claude-sonnet-4-6
tools: [bash, read_file, write_file, edit_file, grep, glob]
maxTurns: 30
permissionMode: ask
---

You are a careful senior engineer. When given a failing test or bug
report, you:
1. Reproduce the failure with `cargo test -- --nocapture` (or the
   appropriate command).
2. Read the relevant source until you understand the failure.
3. Propose the *minimal* edit that fixes it.
4. Verify the test passes.
5. Stop. Do not refactor surrounding code or fix unrelated lints.
```

Similar bodies for the other four (each one ~15 lines of system prompt).

`workflows/investigate-then-fix.yaml`:

```yaml
name: investigate-then-fix
description: Two-step bug fix — investigate, then propose minimal edit.
steps:
  - id: investigate
    agent: fix-bug
    actions: []
    prompt: |
      Investigate the bug described by:
      {{ inputs.prompt }}

      Stop without making edits. Report the root cause as text.
  - id: propose
    agent: fix-bug
    actions: []
    prompt: |
      Based on this investigation:
      {{ steps.investigate.output }}
      Propose and apply the minimal fix.
```

`crates/rupu-cli/src/defaults.rs`:

```rust
//! Embedded default agents + workflows. Shipped as `&'static str`
//! constants via `include_str!` so the binary works on a fresh
//! install before the user populates `~/.rupu/agents/`.

pub struct EmbeddedAgent {
    pub name: &'static str,
    pub body: &'static str,
}

pub struct EmbeddedWorkflow {
    pub name: &'static str,
    pub body: &'static str,
}

pub const AGENTS: &[EmbeddedAgent] = &[
    EmbeddedAgent { name: "fix-bug",        body: include_str!("../../../agents/fix-bug.md") },
    EmbeddedAgent { name: "add-tests",      body: include_str!("../../../agents/add-tests.md") },
    EmbeddedAgent { name: "review-diff",    body: include_str!("../../../agents/review-diff.md") },
    EmbeddedAgent { name: "scaffold",       body: include_str!("../../../agents/scaffold.md") },
    EmbeddedAgent { name: "summarize-diff", body: include_str!("../../../agents/summarize-diff.md") },
];

pub const WORKFLOWS: &[EmbeddedWorkflow] = &[
    EmbeddedWorkflow {
        name: "investigate-then-fix",
        body: include_str!("../../../workflows/investigate-then-fix.yaml"),
    },
];

/// Look up an embedded agent by name.
pub fn lookup_agent(name: &str) -> Option<&'static EmbeddedAgent> {
    AGENTS.iter().find(|a| a.name == name)
}

/// Look up an embedded workflow by name.
pub fn lookup_workflow(name: &str) -> Option<&'static EmbeddedWorkflow> {
    WORKFLOWS.iter().find(|w| w.name == name)
}
```

**Wiring in `cmd/run.rs` and `cmd/agent.rs`**: when `load_agent` returns `NotFound`, fall through to `defaults::lookup_agent(name)`; if that hits, parse the embedded body via `AgentSpec::parse(...)`. Same pattern for workflow lookup in `cmd/workflow.rs`.

Test: `cli_defaults.rs` runs `rupu run fix-bug "echo"` against a fresh `RUPU_HOME` that has no agents/ dir; the embedded `fix-bug.md` should still be found.

After:
```bash
cargo test -p rupu-cli --test cli_defaults
git add agents workflows crates/rupu-cli
git -c commit.gpgsign=false commit -m "feat(cli): embed 5 default agents + investigate-then-fix workflow"
```

---

## Phase 5 — Docs

### Task 12: README + CLAUDE.md update

**Files:**
- Create: `README.md`
- Modify: `CLAUDE.md` — add rupu-agent / rupu-orchestrator / rupu-cli entries

`README.md` covers:
- What rupu is (one paragraph; reuse the spec's "Vision" section).
- Install: `cargo install --git https://github.com/Section9Labs/rupu` OR download from Releases.
- First run:
  ```
  rupu auth login --provider anthropic --key sk-ant-XXX
  rupu run fix-bug "make the failing test pass"
  ```
- Where things live: `~/.rupu/{config.toml, auth.json, agents/, workflows/, transcripts/, cache/}` and `<project>/.rupu/`.
- Two example agent runs.
- Pointer to docs/spec.md for architecture.
- Section: "Agents are code." Bypass mode runs arbitrary commands; review what you run.

`CLAUDE.md` additions: list `rupu-agent`, `rupu-orchestrator`, `rupu-cli` in the architecture rules section with one-line summaries.

```bash
git -c commit.gpgsign=false commit -m "docs: README + CLAUDE.md update for rupu-agent/orchestrator/cli"
```

### Task 13: Reference docs

**Files:**
- Create: `docs/spec.md`
- Create: `docs/agent-format.md`
- Create: `docs/workflow-format.md`
- Create: `docs/transcript-schema.md`

`docs/spec.md`: source-of-truth architecture. Mirrors phi-cell pattern. Lifts content from the Slice A spec; cuts the brainstorm-history sections; adds "as-built" notes (e.g., the lifted phi-providers naming differences from Plan 2 Task 7).

`docs/agent-format.md`: full frontmatter reference + 2 worked examples.

`docs/workflow-format.md`: full YAML reference + 1 example.

`docs/transcript-schema.md`: full event schema reference + 1 sample line per event type.

```bash
git -c commit.gpgsign=false commit -m "docs: spec.md + agent-format.md + workflow-format.md + transcript-schema.md"
```

---

## Phase 6 — Release pipeline

### Task 14: GitHub release workflow

**File:** Create `.github/workflows/release.yml`

```yaml
name: release
on:
  push:
    tags:
      - "v*"

jobs:
  build:
    name: build (${{ matrix.target }})
    strategy:
      fail-fast: false
      matrix:
        include:
          - { os: macos-14,         target: aarch64-apple-darwin     }
          - { os: macos-14,         target: x86_64-apple-darwin      }
          - { os: ubuntu-latest,    target: x86_64-unknown-linux-gnu }
          - { os: ubuntu-latest,    target: aarch64-unknown-linux-gnu, cross: true }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.77
        with:
          targets: ${{ matrix.target }}
      - name: Install cross (Linux arm64)
        if: matrix.cross
        run: cargo install cross --locked --git https://github.com/cross-rs/cross
      - name: Build (cross)
        if: matrix.cross
        run: cross build --release --target ${{ matrix.target }} -p rupu-cli
      - name: Build (native)
        if: ${{ !matrix.cross }}
        run: cargo build --release --target ${{ matrix.target }} -p rupu-cli
      - name: Strip + tar
        run: |
          cd target/${{ matrix.target }}/release
          strip rupu || true
          tar -czf rupu-${{ github.ref_name }}-${{ matrix.target }}.tar.gz rupu
          shasum -a 256 rupu-${{ github.ref_name }}-${{ matrix.target }}.tar.gz > rupu-${{ github.ref_name }}-${{ matrix.target }}.tar.gz.sha256
      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.target }}
          path: target/${{ matrix.target }}/release/rupu-*.tar.gz*

  release:
    name: release
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: artifacts
      - name: Flatten
        run: find artifacts -type f -name 'rupu-*' -exec mv {} . \;
      - uses: softprops/action-gh-release@v2
        with:
          files: rupu-*
          generate_release_notes: true
```

```bash
git add .github/workflows/release.yml
git -c commit.gpgsign=false commit -m "ci: tag-triggered prebuilt-binary release workflow"
```

(Note: the local-checks-only convention from PR #5 deletes `ci.yml` — we keep that deletion. `release.yml` is purely tag-triggered, so it doesn't run on PRs.)

---

## Phase 7 — Smoke + tag

### Task 15: Exit-criterion-B smoke + tag

Manual; not subagent-dispatchable.

- [ ] **Step 1: Workspace-wide test/clippy/fmt/release-build**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace
```
Expected: all green.

- [ ] **Step 2: Clean install from path**

```bash
cargo install --path crates/rupu-cli --force
rupu --version
```
Expected: prints `rupu 0.1.0`. The binary is now on `$PATH`.

- [ ] **Step 3: First-run UX with no `~/.rupu/`**

```bash
rm -rf ~/.rupu
rupu agent list
```
Expected: shows the 5 embedded default agents (fix-bug, add-tests, review-diff, scaffold, summarize-diff). No errors.

- [ ] **Step 4: Real-provider smoke**

```bash
rupu auth login --provider anthropic --key "$ANTHROPIC_API_KEY"
rupu auth status
```
Expected: `anthropic configured`.

```bash
cd <some-real-repo>
rupu run summarize-diff "show me what changed in the last commit" --mode bypass
```
Expected: agent runs, transcript file created at `~/.rupu/transcripts/run_*.jsonl`.

- [ ] **Step 5: Workflow smoke**

```bash
rupu workflow run investigate-then-fix --input prompt="$BUG_DESCRIPTION" --mode bypass
```
Expected: two-step run, both transcripts on disk.

- [ ] **Step 6: Tag + push**

```bash
git tag -a v0.0.3-cli -m "Plan 3 complete: rupu CLI binary at exit-criterion-B"
git push origin v0.0.3-cli
```

The tag triggers `release.yml`; verify the GitHub release shows up with 4 prebuilt binaries (macOS arm64/x86_64, Linux x86_64/arm64).

---

## What's not in this plan (out of scope; deferred to Slice B+)

- SCM connectors (GitHub, GitLab, Bitbucket).
- Issue-tracker triggers (GitHub Issues, Linear, Jira).
- Full DAG workflow engine (parallel, when, gates).
- SaaS control plane / remote runs / sandbox / session restore.
- Native desktop app.
- OpenAI / Copilot / Gemini / local provider wiring (returns NotWiredInV0; user-visible error).

---

## Self-review notes

- **Spec coverage:** every spec section that maps to a CLI verb, default-library item, doc, or release pipeline has a task. Items deferred to Slice B+ are listed in "What's not in this plan."
- **Placeholder scan:** Tasks 7, 8, 12, 13 have intentional code-block ellipses for repetitive parts (e.g., the 4 other default agents follow the same shape as `fix-bug.md`). These are not "TBD" — they're "follow the pattern." Rephrase as needed during execution.
- **Type consistency:** `AgentSpec`, `AgentRunOpts`, `RunResult`, `OrchestratorRunOpts`, `StepFactory`, `MockProvider`, `ScriptedTurn`, `BypassDecider`, `PermissionDecider`, `PermissionDecision`, `ToolContext`, `Workflow`, `JsonlReader`, `RunSummary`, `ProbeCache`, `select_backend`, `WorkspaceStore`, `upsert`, `layer_files` — all match Plan 2's exposed names.

**Open assumptions to verify in Task 4 (`rupu run`):**

1. `rupu_providers::anthropic::AnthropicClient::with_api_key(&str, &str)` exists with that exact signature. If not, adapt the one call site in `provider_factory::build_anthropic`.
2. `PermissionPrompt::new_in_memory_with` does not exist (Plan 2 Task 5 used `new_in_memory(&[u8], &mut W)`). The `AskDecider` will need either a new constructor `PermissionPrompt::for_stdio()` (one-line addition to rupu-agent) or the `AskDecider` directly invokes `BufReader::new(stdin.lock())` and friends matching the actual `new` constructor.
3. `ScriptedTurn` may not derive `Deserialize`. Task 5 of this plan handles that with a one-line follow-up in rupu-agent.

**Open assumption to verify in Task 7 (`rupu workflow run`):**

The `StepFactory` plan code references `step.agent` lookup but the `OrchestratorRunOpts.factory` contract from Plan 2 Task 11 only passes `step_id` to `build_opts_for_step`. Either:
- Carry a clone of the `Workflow` in the factory struct so the callback can re-look-up the step (recommended; one extra field).
- Or extend `build_opts_for_step` to take the full `&Step` (one-line API change in `rupu-orchestrator`).

The plan body discusses this; the implementer of Task 7 should pick one approach and document it in the commit message.
