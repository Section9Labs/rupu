# AI-generated agent & workflow definitions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user describe an agent (`.md`) or workflow (`.yaml`) in natural language and have a configured provider/model draft a *validated* definition file, exposed identically on the `rupu` CLI (`create --describe`) and the CP web create modals.

**Architecture:** One generation core in `rupu-orchestrator` (`generate` module) runs a bounded validate→repair loop and returns parseable content or fails loudly. The CLI calls it directly inside the existing `create` flow. CP reaches it through a new optional `DefinitionGenerator` adapter port wired only under `rupu cp serve` (501 when absent); the CP generate endpoints return content without writing, and the existing `POST /api/agents|workflows` write endpoints persist after the user reviews.

**Tech Stack:** Rust 2021 (tokio, thiserror, async-trait, axum), `rupu-providers`/`rupu-auth`/`rupu-runtime` provider stack, React 18 + TypeScript + Vite + Tailwind (CP frontend), vitest.

## Global Constraints

- Workspace deps only — versions pinned in root `Cargo.toml`; never in crate `Cargo.toml` files.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- Errors: `thiserror` for libraries; `anyhow` for the CLI binary.
- `rupu-cli` stays thin: arg parsing + delegation, no business logic.
- Hexagonal separation: `rupu-cp` must NOT gain a direct provider/credential dependency; it reaches generation through the adapter port.
- Provider name strings are `"anthropic"`, `"openai"`, `"gemini"`, `"copilot"` (as `build_for_provider` and the `models` command use).
- Never run package-wide `cargo fmt`; format only files you touched (`cargo fmt -p <crate>` or per-file).
- Per repo convention: branch → commit → PR. Rebuild CP UI with `make cp-web` before any release that should ship the new UI.
- Tests that set `RUPU_MOCK_PROVIDER_SCRIPT` MUST serialize on a shared lock (the env var is process-global) and remove the var afterward — mirror `crates/rupu-cli/tests/cli_workflow.rs`.

---

## File Structure

**Slice 1 — core (`rupu-orchestrator`)**
- Create `crates/rupu-orchestrator/src/generate.rs` — types, errors, prompt builders, fence-strip, validate, `generate_definition`, `pick_default_gen_model`.
- Modify `crates/rupu-orchestrator/src/lib.rs` — `pub mod generate;` + re-exports.

**Slice 2 — CLI**
- Modify `crates/rupu-cli/src/cmd/agent.rs` — `--describe`/`--gen-provider`/`--gen-model`/`--host` on `Create`; generation branch.
- Modify `crates/rupu-cli/src/cmd/workflow.rs` — same, plus available-agent gathering.
- Create `crates/rupu-cli/tests/cli_generate.rs` — end-to-end via the mock-provider env seam.

**Slice 3 — CP backend**
- Create `crates/rupu-cp/src/definition_generator.rs` — `DefinitionGenerator` port + DTOs.
- Modify `crates/rupu-cp/src/lib.rs` — `pub mod definition_generator;`, `ServeOpts.generator` field, wire into `AppState`.
- Modify `crates/rupu-cp/src/state.rs` — `generator` field + `with_generator`.
- Modify `crates/rupu-cp/src/api/agents.rs` + `.../workflows.rs` — `POST /generate` routes + handlers + tests.
- Modify `crates/rupu-cp/src/api/mod.rs` (or wherever shared routes mount) — `GET /api/generate/models` route.
- Create `crates/rupu-cli/src/cp_definition_generator.rs` — `RuntimeDefinitionGenerator` adapter.
- Modify `crates/rupu-cli/src/cmd/cp.rs` — build + pass the adapter; `crates/rupu-cli/src/lib.rs` — `mod cp_definition_generator;`.

**Slice 4 — CP frontend**
- Modify `crates/rupu-cp/web/src/lib/api.ts` — `generateAgent`, `generateWorkflow`, `generateModels` + types.
- Modify `crates/rupu-cp/web/src/pages/Agents.tsx` — Describe/Edit toggle in `NewAgentModal`.
- Modify `crates/rupu-cp/web/src/pages/Workflows.tsx` — Describe/Edit toggle in `NewWorkflowModal`.

---

## Slice 1 — Generation core (`rupu_orchestrator::generate`)

### Task 1: Core types, prompt builder, fence-strip, validate

**Files:**
- Create: `crates/rupu-orchestrator/src/generate.rs`
- Modify: `crates/rupu-orchestrator/src/lib.rs`
- Test: inline `#[cfg(test)]` in `generate.rs`

**Interfaces:**
- Consumes: `rupu_agent::AgentSpec::parse(&str)`, `crate::Workflow::parse(&str)`.
- Produces: `GenKind`, `GenerateRequest`, `GenerateOutcome`, `GenerateError`, `build_system_prompt(GenKind, &[String]) -> String`, `strip_fences(&str) -> &str`, `validate(GenKind, &str) -> Result<(), String>`, `DEFAULT_GEN_MODELS`, `MAX_ATTEMPTS`.

- [ ] **Step 1: Add the module to the crate**

In `crates/rupu-orchestrator/src/lib.rs`, add alongside the other `pub mod` lines:

```rust
pub mod generate;
```

And add to the public re-exports (near the existing `pub use` block):

```rust
pub use generate::{
    generate_definition, pick_default_gen_model, GenKind, GenerateError, GenerateOutcome,
    GenerateRequest,
};
```

- [ ] **Step 2: Write the failing tests for the pure helpers**

Create `crates/rupu-orchestrator/src/generate.rs` with ONLY the test module first (so it fails to compile → "not found"):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fences_removes_lang_fence() {
        let input = "```yaml\nname: hi\n```";
        assert_eq!(strip_fences(input), "name: hi");
    }

    #[test]
    fn strip_fences_leaves_bare_content() {
        assert_eq!(strip_fences("name: hi"), "name: hi");
    }

    #[test]
    fn validate_accepts_good_workflow() {
        let yaml = "name: wf\nsteps:\n  - id: main\n    agent: rev\n    prompt: do it\n";
        assert!(validate(GenKind::Workflow, yaml).is_ok());
    }

    #[test]
    fn validate_rejects_workflow_missing_agent() {
        let yaml = "name: wf\nsteps:\n  - id: main\n    prompt: do it\n";
        let err = validate(GenKind::Workflow, yaml).unwrap_err();
        assert!(err.contains("agent"), "got: {err}");
    }

    #[test]
    fn system_prompt_lists_available_agents_for_workflows() {
        let p = build_system_prompt(GenKind::Workflow, &["reviewer".to_string(), "fixer".to_string()]);
        assert!(p.contains("reviewer"));
        assert!(p.contains("fixer"));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p rupu-orchestrator generate::tests`
Expected: FAIL — `cannot find function strip_fences` / `GenKind`.

- [ ] **Step 4: Implement the types and pure helpers**

Prepend to `crates/rupu-orchestrator/src/generate.rs` (above the test module):

```rust
//! Generate validated agent (`.md`) / workflow (`.yaml`) definition files
//! from a natural-language description via a configured provider/model.
//!
//! The core lives here (not in `rupu-runtime`) because validating a
//! generated workflow needs [`crate::Workflow::parse`], and orchestrator
//! already depends on `rupu-runtime` for `build_for_provider` — putting it
//! in runtime would cycle.

use rupu_providers::types::{LlmRequest, Message};
use rupu_runtime::provider_factory::build_for_provider;

/// Total send attempts (1 first try + repairs) before giving up.
pub const MAX_ATTEMPTS: u8 = 3;

/// Output token ceiling for a generated definition.
const MAX_TOKENS: u32 = 8192;

/// Provider preference order + the model each one generates with. Seeded
/// from each provider adapter's own `default_model()`. One place to bump.
pub const DEFAULT_GEN_MODELS: &[(&str, &str)] = &[
    ("anthropic", "claude-sonnet-4-6"),
    ("openai", "gpt-5.4"),
    ("gemini", "gemini-2.5-pro"),
    ("copilot", "claude-sonnet-4-6"),
];

/// What kind of definition to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenKind {
    Agent,
    Workflow,
}

impl GenKind {
    fn noun(self) -> &'static str {
        match self {
            GenKind::Agent => "agent",
            GenKind::Workflow => "workflow",
        }
    }
}

/// A request to generate one definition file.
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub kind: GenKind,
    pub description: String,
    pub provider: String,
    pub model: String,
    /// Real agent names available in scope. Empty for agents; for
    /// workflows these are injected so generated steps reference agents
    /// that actually exist.
    pub available_agents: Vec<String>,
}

/// A successfully generated, validated definition.
#[derive(Debug, Clone)]
pub struct GenerateOutcome {
    pub content: String,
    pub provider: String,
    pub model: String,
    pub attempts: u8,
}

#[derive(thiserror::Error, Debug)]
pub enum GenerateError {
    #[error("no authenticated provider available; run `rupu auth login`")]
    NoCredentials,
    #[error("provider error: {0}")]
    Provider(#[from] rupu_providers::ProviderError),
    #[error("model returned empty output")]
    Empty,
    #[error("generated {kind} did not parse after {attempts} attempt(s): {last_error}")]
    Invalid {
        kind: &'static str,
        attempts: u8,
        last_error: String,
    },
}

/// Strip a single wrapping Markdown code fence (```` ```lang … ``` ````)
/// if the model wrapped its output, and trim surrounding whitespace.
pub fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop the (optional) language tag on the opening fence line.
        let after_lang = rest.split_once('\n').map(|(_, body)| body).unwrap_or("");
        if let Some(inner) = after_lang.trim_end().strip_suffix("```") {
            return inner.trim();
        }
    }
    t
}

/// Parse-validate generated content for the kind. Returns the parse error
/// text on failure (fed back into the repair prompt).
pub fn validate(kind: GenKind, content: &str) -> Result<(), String> {
    match kind {
        GenKind::Agent => rupu_agent::AgentSpec::parse(content)
            .map(|_| ())
            .map_err(|e| e.to_string()),
        GenKind::Workflow => crate::Workflow::parse(content)
            .map(|_| ())
            .map_err(|e| e.to_string()),
    }
}

/// Build the system prompt teaching the model the target file format.
pub fn build_system_prompt(kind: GenKind, available_agents: &[String]) -> String {
    match kind {
        GenKind::Agent => AGENT_SYSTEM_PROMPT.to_string(),
        GenKind::Workflow => {
            let agents = if available_agents.is_empty() {
                "(none defined yet — you may reference an agent the user will create)".to_string()
            } else {
                available_agents.join(", ")
            };
            format!("{WORKFLOW_SYSTEM_PROMPT}\n\nAvailable agent names (use ONLY these for `agent:` and panelist fields): {agents}")
        }
    }
}

const AGENT_SYSTEM_PROMPT: &str = r#"You generate a rupu AGENT definition file. Output ONLY the file content — no Markdown code fences, no commentary.

Format: YAML frontmatter delimited by `---` lines, then a Markdown body that is the agent's system prompt.

Required frontmatter:
  name: <kebab-case identifier>
  description: <one short line>
  provider: anthropic   # one of: anthropic | openai | google | github-copilot | broker
  model: <a model id for that provider, e.g. claude-sonnet-4-6>

Optional frontmatter: tools (a YAML list, e.g. [bash, read, grep]), permissionMode (ask|bypass|readonly), maxTurns (integer).

The Markdown body after the closing `---` is the system prompt: role, voice, boundaries. Be specific and useful.
"#;

const WORKFLOW_SYSTEM_PROMPT: &str = r#"You generate a rupu WORKFLOW definition file. Output ONLY the YAML content — no Markdown code fences, no commentary.

Top-level keys:
  name: <kebab-case identifier>
  description: <one short line>
  inputs: (optional map) each input has type (string|int|bool), required (bool), description, optional default. Reference them in prompts as {{ inputs.<key> }}.
  steps: (required list)

Each linear step needs:
  - id: <unique id>
    agent: <one of the available agent names>
    prompt: |
      <multi-line instruction; may reference {{ inputs.x }} and {{ steps.<id>.output }}>
    actions: []        # optional allow-list of tool actions

Other step shapes: `parallel:` (a list of sub-steps each with id/agent/prompt), and `panel:` (panelists list + subject + prompt, optional gate). Keep it minimal unless the description calls for fan-out.

Never leave `agent:` empty — every linear/for_each step must name a real agent.
"#;
```

Add the workspace dep if missing — check `crates/rupu-orchestrator/Cargo.toml` already lists `rupu-agent`, `rupu-providers`, `rupu-runtime` (it does) and `thiserror` (it does). No Cargo change expected.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-orchestrator generate::tests`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/generate.rs crates/rupu-orchestrator/src/lib.rs
git commit -m "feat(orchestrator): generation core types, prompt builder, validation"
```

### Task 2: `generate_definition` with the validate→repair loop

**Files:**
- Modify: `crates/rupu-orchestrator/src/generate.rs`
- Test: inline `#[cfg(test)]` integration test using `RUPU_MOCK_PROVIDER_SCRIPT`

**Interfaces:**
- Consumes: `build_for_provider`, `build_system_prompt`, `strip_fences`, `validate`, `MAX_ATTEMPTS`.
- Produces: `pub async fn generate_definition(&GenerateRequest, &dyn rupu_auth::CredentialResolver) -> Result<GenerateOutcome, GenerateError>`.

- [ ] **Step 1: Write the failing integration tests**

Append to the `tests` module in `crates/rupu-orchestrator/src/generate.rs`:

```rust
use rupu_auth::InMemoryResolver;
use tokio::sync::Mutex as AsyncMutex;

// Env-var seam is process-global; serialize.
static ENV_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

const VALID_AGENT_MD: &str = "---\nname: gen-agent\ndescription: a test agent\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nYou are a helpful test agent.\n";

#[tokio::test]
async fn generate_returns_valid_content_first_try() {
    let _g = ENV_LOCK.lock().await;
    let script = serde_json::json!([
        { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
    ])
    .to_string();
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

    let resolver = InMemoryResolver::new();
    let req = GenerateRequest {
        kind: GenKind::Agent,
        description: "a helpful test agent".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        available_agents: vec![],
    };
    let out = generate_definition(&req, &resolver).await.expect("ok");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

    assert_eq!(out.attempts, 1);
    assert!(out.content.contains("name: gen-agent"));
}

#[tokio::test]
async fn generate_repairs_invalid_then_succeeds() {
    let _g = ENV_LOCK.lock().await;
    // First turn: invalid (no frontmatter). Second turn: valid.
    let script = serde_json::json!([
        { "AssistantText": { "text": "not a valid agent file", "stop": "end_turn" } },
        { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
    ])
    .to_string();
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

    let resolver = InMemoryResolver::new();
    let req = GenerateRequest {
        kind: GenKind::Agent,
        description: "x".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        available_agents: vec![],
    };
    let out = generate_definition(&req, &resolver).await.expect("ok");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

    assert_eq!(out.attempts, 2);
    assert!(out.content.contains("name: gen-agent"));
}

#[tokio::test]
async fn generate_errors_when_never_valid() {
    let _g = ENV_LOCK.lock().await;
    let script = serde_json::json!([
        { "AssistantText": { "text": "junk", "stop": "end_turn" } },
        { "AssistantText": { "text": "still junk", "stop": "end_turn" } },
        { "AssistantText": { "text": "junk again", "stop": "end_turn" } }
    ])
    .to_string();
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

    let resolver = InMemoryResolver::new();
    let req = GenerateRequest {
        kind: GenKind::Agent,
        description: "x".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        available_agents: vec![],
    };
    let err = generate_definition(&req, &resolver).await.unwrap_err();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

    match err {
        GenerateError::Invalid { attempts, .. } => assert_eq!(attempts, MAX_ATTEMPTS),
        other => panic!("expected Invalid, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator generate::tests::generate_`
Expected: FAIL — `cannot find function generate_definition`.

- [ ] **Step 3: Implement `generate_definition`**

Add to `crates/rupu-orchestrator/src/generate.rs` (above the test module):

```rust
/// Generate a validated definition, repairing up to [`MAX_ATTEMPTS`].
pub async fn generate_definition(
    req: &GenerateRequest,
    resolver: &dyn rupu_auth::CredentialResolver,
) -> Result<GenerateOutcome, GenerateError> {
    let (_mode, mut provider) =
        build_for_provider(&req.provider, &req.model, None, resolver)
            .await
            .map_err(|_| GenerateError::NoCredentials)?;

    let system = build_system_prompt(req.kind, &req.available_agents);
    let mut messages = vec![Message::user(&format!(
        "Create a rupu {} from this description:\n\n{}",
        req.kind.noun(),
        req.description
    ))];

    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let llm_req = LlmRequest {
            model: req.model.clone(),
            system: Some(system.clone()),
            messages: messages.clone(),
            max_tokens: MAX_TOKENS,
            ..Default::default()
        };
        let resp = provider.send(&llm_req).await?;
        let raw = resp.text().ok_or(GenerateError::Empty)?.to_string();
        let content = strip_fences(&raw).to_string();

        match validate(req.kind, &content) {
            Ok(()) => {
                return Ok(GenerateOutcome {
                    content,
                    provider: req.provider.clone(),
                    model: req.model.clone(),
                    attempts: attempt,
                });
            }
            Err(e) => {
                last_error = e;
                messages.push(Message::assistant(&raw));
                messages.push(Message::user(&format!(
                    "Your previous output failed to parse as a valid rupu {}: {last_error}\n\nReturn the corrected full file. Output ONLY the file content.",
                    req.kind.noun()
                )));
            }
        }
    }

    Err(GenerateError::Invalid {
        kind: req.kind.noun(),
        attempts: MAX_ATTEMPTS,
        last_error,
    })
}
```

Note: `build_for_provider` under `RUPU_MOCK_PROVIDER_SCRIPT` ignores `provider`/`model`/`resolver` and returns a `MockProvider`, so the loop drives real repair behavior. Mapping every factory error to `NoCredentials` is acceptable here — the only production failure before send is a missing/invalid credential; deeper errors surface as `Provider(..)` from `send`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p rupu-orchestrator generate::tests::generate_`
Expected: PASS (3 tests).

- [ ] **Step 5: Verify clippy is clean for the crate**

Run: `cargo clippy -p rupu-orchestrator --all-targets`
Expected: no warnings from `generate.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/generate.rs
git commit -m "feat(orchestrator): generate_definition with validate->repair loop"
```

### Task 3: `pick_default_gen_model`

**Files:**
- Modify: `crates/rupu-orchestrator/src/generate.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `DEFAULT_GEN_MODELS`, `rupu_auth::CredentialResolver::get`.
- Produces: `pub async fn pick_default_gen_model(&dyn rupu_auth::CredentialResolver) -> Option<(String, String)>`.

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module (it already imports `InMemoryResolver`; add the credential imports):

```rust
use rupu_auth::stored::StoredCredential;
use rupu_providers::{auth_mode::AuthMode, provider_id::ProviderId};

#[tokio::test]
async fn pick_default_returns_none_when_nothing_authed() {
    let resolver = InMemoryResolver::new();
    assert!(pick_default_gen_model(&resolver).await.is_none());
}

#[tokio::test]
async fn pick_default_prefers_anthropic_then_openai() {
    // Only openai authed → openai wins.
    let resolver = InMemoryResolver::new();
    resolver
        .put(
            ProviderId::Openai,
            AuthMode::ApiKey,
            StoredCredential::api_key("sk-test-openai"),
        )
        .await;
    let (provider, model) = pick_default_gen_model(&resolver).await.expect("some");
    assert_eq!(provider, "openai");
    assert_eq!(model, "gpt-5.4");

    // Add anthropic → anthropic now wins (higher preference).
    resolver
        .put(
            ProviderId::Anthropic,
            AuthMode::ApiKey,
            StoredCredential::api_key("sk-test-anthropic"),
        )
        .await;
    let (provider, model) = pick_default_gen_model(&resolver).await.expect("some");
    assert_eq!(provider, "anthropic");
    assert_eq!(model, "claude-sonnet-4-6");
}
```

(`ProviderId` here is `rupu_auth`'s — confirm the variant name is `Openai`/`Anthropic` per `crates/rupu-auth/src/in_memory.rs::parse_provider`; the `InMemoryResolver::put` signature takes the `rupu_providers` re-exported `ProviderId`/`AuthMode` as in `crates/rupu-runtime/src/provider_factory.rs` tests — mirror those imports exactly.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-orchestrator generate::tests::pick_default`
Expected: FAIL — `cannot find function pick_default_gen_model`.

- [ ] **Step 3: Implement `pick_default_gen_model`**

Add to `crates/rupu-orchestrator/src/generate.rs`:

```rust
/// First authenticated provider (in [`DEFAULT_GEN_MODELS`] order) paired
/// with its default generation model. `None` when nothing is authed.
pub async fn pick_default_gen_model(
    resolver: &dyn rupu_auth::CredentialResolver,
) -> Option<(String, String)> {
    for (provider, model) in DEFAULT_GEN_MODELS {
        if resolver.get(provider, None).await.is_ok() {
            return Some((provider.to_string(), model.to_string()));
        }
    }
    None
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p rupu-orchestrator generate::tests::pick_default`
Expected: PASS (2 tests).

- [ ] **Step 5: Format + full crate test**

Run: `cargo fmt -p rupu-orchestrator && cargo test -p rupu-orchestrator`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-orchestrator/src/generate.rs
git commit -m "feat(orchestrator): pick_default_gen_model provider preference"
```

---

## Slice 2 — CLI surface

### Task 4: `rupu agent create --describe`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/agent.rs`
- Test: `crates/rupu-cli/tests/cli_generate.rs` (created here; workflow case added in Task 5)

**Interfaces:**
- Consumes: `rupu_orchestrator::{generate_definition, pick_default_gen_model, GenerateRequest, GenKind}`, `create_common::{prompt_scope, prompt_name, target_dir, validate_name}`, `rupu_auth::KeychainResolver`.
- Produces: extended `Action::Create` variant + a `create` flow that branches on `describe`.

- [ ] **Step 1: Extend the `Create` clap variant**

In `crates/rupu-cli/src/cmd/agent.rs`, replace the `Create { name, scope, editor }` variant fields with:

```rust
    /// Scaffold a new agent file, then open it for editing. Prompts
    /// interactively for scope and name when omitted. With `--describe`,
    /// a model drafts the definition before you review it.
    Create {
        /// Name for the new agent (no `.md` extension).
        name: Option<String>,
        /// Target scope (`global` or `project`). Prompts when omitted.
        #[arg(long, value_parser = ["global", "project"])]
        scope: Option<String>,
        /// Override the editor (e.g. `--editor "code --wait"`).
        #[arg(long)]
        editor: Option<String>,
        /// Natural-language description — a model drafts the agent.
        #[arg(long)]
        describe: Option<String>,
        /// Provider for generation (default: first authenticated).
        #[arg(long)]
        gen_provider: Option<String>,
        /// Model for generation (default: provider's default).
        #[arg(long)]
        gen_model: Option<String>,
        /// Host to create on (only `local` available today).
        #[arg(long, default_value = "local")]
        host: String,
    },
```

- [ ] **Step 2: Thread the new fields through `handle`**

Update the `Action::Create { .. }` arm in `handle` to pass the new fields:

```rust
        Action::Create {
            name,
            scope,
            editor,
            describe,
            gen_provider,
            gen_model,
            host,
        } => match create(name, scope, editor.as_deref(), describe, gen_provider, gen_model, &host).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
        },
```

- [ ] **Step 3: Write the failing CLI test**

Create `crates/rupu-cli/tests/cli_generate.rs`:

```rust
//! `rupu agent create --describe` end-to-end via the
//! `RUPU_MOCK_PROVIDER_SCRIPT` seam. Hold `ENV_LOCK` while the var is set.

use std::process::Command;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

const VALID_AGENT_MD: &str = "---\nname: gen-agent\ndescription: a test agent\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nYou are a helpful test agent.\n";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_rupu")
}

#[tokio::test]
async fn agent_create_describe_writes_valid_file() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let script = serde_json::json!([
        { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
    ])
    .to_string();

    let out = Command::new(bin())
        .args([
            "agent", "create", "gen-agent",
            "--scope", "global",
            "--describe", "a helpful test agent",
            "--editor", "true", // `true` exits 0 without opening anything
        ])
        .env("RUPU_HOME", home)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", &script)
        .env("EDITOR", "true")
        .output()
        .expect("run");

    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read_to_string(home.join("agents/gen-agent.md")).expect("file written");
    assert!(written.contains("name: gen-agent"));
}
```

Confirm the global-dir env override name: check `crates/rupu-cli/src/paths.rs` for how `global_dir()` resolves (e.g. `RUPU_HOME` or `RUPU_GLOBAL_DIR`). Use whatever existing CLI tests use (grep `RUPU_` in `crates/rupu-cli/tests/`). If `--editor true` is not honored for generation, the generation path should skip opening the editor when `--describe` ran non-interactively; see Step 4. Add `tempfile` to `crates/rupu-cli/Cargo.toml` `[dev-dependencies]` only if not already present (it is used by other CLI tests — confirm first).

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p rupu-cli --test cli_generate agent_create_describe`
Expected: FAIL — `create` signature mismatch / file not written.

- [ ] **Step 5: Implement the generation branch in `create`**

In `crates/rupu-cli/src/cmd/agent.rs`, change the `create` signature and add the branch. Keep the existing template path intact for the no-describe case:

```rust
async fn create(
    name: Option<String>,
    scope: Option<String>,
    editor_override: Option<&str>,
    describe: Option<String>,
    gen_provider: Option<String>,
    gen_model: Option<String>,
    host: &str,
) -> anyhow::Result<()> {
    if host != "local" {
        anyhow::bail!("host `{host}` is not available (only `local` today)");
    }
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let scope = match scope {
        Some(s) => s,
        None => crate::cmd::create_common::prompt_scope("agent", project_root.as_deref())?,
    };
    let name = match name {
        Some(n) => {
            crate::cmd::create_common::validate_name(n.trim())?;
            n.trim().to_string()
        }
        None => crate::cmd::create_common::prompt_name("agent")?,
    };

    let dir =
        crate::cmd::create_common::target_dir(&scope, &global, project_root.as_deref(), "agents")?;
    let target = dir.join(format!("{name}.md"));
    if target.exists() {
        anyhow::bail!(
            "agent `{name}` already exists at {} — use `rupu agent edit {name}` to modify",
            target.display()
        );
    }
    std::fs::create_dir_all(&dir)?;

    let contents = match describe {
        Some(desc) => {
            let resolver = rupu_auth::KeychainResolver::new();
            let (provider, model) = match (gen_provider, gen_model) {
                (Some(p), Some(m)) => (p, m),
                (Some(p), None) => {
                    // provider given, model defaults from the table
                    let m = rupu_orchestrator::generate::DEFAULT_GEN_MODELS
                        .iter()
                        .find(|(name, _)| *name == p)
                        .map(|(_, m)| m.to_string())
                        .ok_or_else(|| anyhow::anyhow!("unknown --gen-provider `{p}`"))?;
                    (p, m)
                }
                (None, _) => rupu_orchestrator::pick_default_gen_model(&resolver)
                    .await
                    .ok_or_else(|| {
                        anyhow::anyhow!("no authenticated provider; run `rupu auth login` or pass --gen-provider/--gen-model")
                    })?,
            };
            println!("generating agent `{name}` via {provider}/{model}…");
            let req = rupu_orchestrator::GenerateRequest {
                kind: rupu_orchestrator::GenKind::Agent,
                description: desc,
                provider,
                model,
                available_agents: vec![],
            };
            let outcome = rupu_orchestrator::generate_definition(&req, &resolver).await?;
            outcome.content
        }
        None => AGENT_TEMPLATE.replace("{{name}}", &name),
    };

    std::fs::write(&target, &contents)?;
    println!("created {} ({scope})", target.display());

    editor::open_for_edit(editor_override, &target)?;

    match AgentSpec::parse_file(&target) {
        Ok(_) => {
            println!("✓ {name}: frontmatter parses cleanly");
            Ok(())
        }
        Err(e) => {
            eprintln!("⚠ {name}: failed to re-parse after save:\n  {e}");
            Ok(())
        }
    }
}
```

Add `use rupu_orchestrator;` is unnecessary (path-qualified). Ensure `rupu-auth` is a dep of `rupu-cli` (it is, per `Cargo.toml`).

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p rupu-cli --test cli_generate agent_create_describe`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/cmd/agent.rs crates/rupu-cli/tests/cli_generate.rs
git commit -m "feat(cli): rupu agent create --describe generates via a model"
```

### Task 5: `rupu workflow create --describe`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs`
- Test: add a case to `crates/rupu-cli/tests/cli_generate.rs`

**Interfaces:**
- Consumes: same as Task 4 plus `rupu_agent::load_agents` to gather available agent names.
- Produces: extended `Action::Create` + `create` branch for workflows.

- [ ] **Step 1: Extend the `Create` variant + `handle` arm**

Mirror Task 4 Step 1/2 in `crates/rupu-cli/src/cmd/workflow.rs`: add `describe`, `gen_provider`, `gen_model`, `host` to the `Create` variant and thread them into `create(...)` from the `handle` `Action::Create` arm.

- [ ] **Step 2: Write the failing test case**

Append to `crates/rupu-cli/tests/cli_generate.rs`:

```rust
const VALID_WF_YAML: &str = "name: gen-wf\ndescription: a test workflow\nsteps:\n  - id: main\n    agent: gen-agent\n    prompt: do the thing\n";

#[tokio::test]
async fn workflow_create_describe_writes_valid_file() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    // Seed an agent so the workflow can reference a real name.
    std::fs::create_dir_all(home.join("agents")).unwrap();
    std::fs::write(home.join("agents/gen-agent.md"), VALID_AGENT_MD).unwrap();

    let script = serde_json::json!([
        { "AssistantText": { "text": VALID_WF_YAML, "stop": "end_turn" } }
    ])
    .to_string();

    let out = Command::new(bin())
        .args([
            "workflow", "create", "gen-wf",
            "--scope", "global",
            "--describe", "a workflow that does the thing",
            "--editor", "true",
        ])
        .env("RUPU_HOME", home)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", &script)
        .env("EDITOR", "true")
        .output()
        .expect("run");

    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read_to_string(home.join("workflows/gen-wf.yaml")).expect("file written");
    assert!(written.contains("agent: gen-agent"));
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p rupu-cli --test cli_generate workflow_create_describe`
Expected: FAIL — signature mismatch / not written.

- [ ] **Step 4: Implement the workflow `create` branch**

In `crates/rupu-cli/src/cmd/workflow.rs`, update `create` like Task 4's, with two differences: kind is `Workflow`, and gather available agent names. Replace the `describe` match arm body's request build with:

```rust
            // Real agent names so generated steps reference agents that exist.
            let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
            let available_agents = rupu_agent::load_agents(&global, project_agents_parent.as_deref())
                .map(|specs| specs.into_iter().map(|s| s.name).collect::<Vec<_>>())
                .unwrap_or_default();
            println!("generating workflow `{name}` via {provider}/{model}…");
            let req = rupu_orchestrator::GenerateRequest {
                kind: rupu_orchestrator::GenKind::Workflow,
                description: desc,
                provider,
                model,
                available_agents,
            };
            let outcome = rupu_orchestrator::generate_definition(&req, &resolver).await?;
            outcome.content
```

Keep the no-describe arm producing `WORKFLOW_TEMPLATE.replace("{{name}}", &name)`, and the post-write tail using `Workflow::parse_file`. The file extension/target is `{name}.yaml` (existing code).

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p rupu-cli --test cli_generate workflow_create_describe`
Expected: PASS.

- [ ] **Step 6: Format, clippy, full CLI generate test**

Run: `cargo fmt -p rupu-cli && cargo clippy -p rupu-cli --all-targets && cargo test -p rupu-cli --test cli_generate`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/cmd/workflow.rs crates/rupu-cli/tests/cli_generate.rs
git commit -m "feat(cli): rupu workflow create --describe generates via a model"
```

---

## Slice 3 — CP backend

### Task 6: `DefinitionGenerator` port + DTOs

**Files:**
- Create: `crates/rupu-cp/src/definition_generator.rs`
- Modify: `crates/rupu-cp/src/lib.rs` (module decl + `ServeOpts` field), `crates/rupu-cp/src/state.rs` (field + wither)
- Test: inline `#[cfg(test)]` in `definition_generator.rs`

**Interfaces:**
- Produces: `DefinitionGenerator` trait, `GenerateDefRequest`, `GeneratedDef`, `GenDefError`, `ProviderModels`.

- [ ] **Step 1: Write the failing test (DTO + trait object compiles & dispatches)**

Create `crates/rupu-cp/src/definition_generator.rs`:

```rust
//! Port: drafts agent/workflow definition content from a description via a
//! model. rupu-cp defines it; rupu-cli's `cp serve` provides the adapter
//! backed by `rupu_orchestrator::generate`. Read-only `rupu-cp` runs with
//! `None` → the generate endpoints return 501.

use async_trait::async_trait;

/// Which kind of definition to draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    Agent,
    Workflow,
}

#[derive(Debug, Clone)]
pub struct GenerateDefRequest {
    pub kind: DefKind,
    pub description: String,
    /// Provider override; `None` → adapter picks the default.
    pub provider: Option<String>,
    /// Model override; `None` → adapter picks the default.
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GeneratedDef {
    pub raw: String,
    pub provider: String,
    pub model: String,
    pub attempts: u8,
}

/// Providers/models offered in the CP dropdown.
#[derive(Debug, Clone)]
pub struct ProviderModels {
    pub provider: String,
    pub models: Vec<String>,
    pub is_default: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum GenDefError {
    #[error("no authenticated provider; connect one to use AI generation")]
    NoCredentials,
    #[error("generation failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait DefinitionGenerator: Send + Sync {
    async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError>;
    async fn available_models(&self) -> Vec<ProviderModels>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Stub;
    #[async_trait]
    impl DefinitionGenerator for Stub {
        async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError> {
            Ok(GeneratedDef {
                raw: format!("kind={:?}", req.kind),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                attempts: 1,
            })
        }
        async fn available_models(&self) -> Vec<ProviderModels> {
            vec![ProviderModels { provider: "anthropic".into(), models: vec!["claude-sonnet-4-6".into()], is_default: true }]
        }
    }

    #[tokio::test]
    async fn trait_object_dispatches() {
        let g: Arc<dyn DefinitionGenerator> = Arc::new(Stub);
        let out = g
            .generate(GenerateDefRequest { kind: DefKind::Agent, description: "x".into(), provider: None, model: None })
            .await
            .unwrap();
        assert!(out.raw.contains("Agent"));
        assert_eq!(g.available_models().await.len(), 1);
    }
}
```

- [ ] **Step 2: Declare the module + state field**

In `crates/rupu-cp/src/lib.rs`, add near the other `pub mod` lines:

```rust
pub mod definition_generator;
```

Add to `ServeOpts` (after `session_starter`):

```rust
    /// Adapter that drafts agent/workflow definitions from a description.
    /// `None` (read-only cp) → the generate endpoints return 501.
    pub generator: Option<std::sync::Arc<dyn crate::definition_generator::DefinitionGenerator>>,
```

In `crates/rupu-cp/src/state.rs`, add the field to `AppState` (after `session_starter`):

```rust
    /// Optional definition generator; `rupu cp serve` installs the
    /// orchestrator-backed adapter via [`AppState::with_generator`].
    pub generator: Option<Arc<dyn crate::definition_generator::DefinitionGenerator>>,
```

Initialize it to `None` in `AppState::new` (next to `session_starter: None`), and add a wither mirroring `with_session_starter`:

```rust
    pub fn with_generator(
        mut self,
        generator: Option<Arc<dyn crate::definition_generator::DefinitionGenerator>>,
    ) -> Self {
        self.generator = generator;
        self
    }
```

Finally, in `crates/rupu-cp/src/lib.rs` where `AppState` is built from `ServeOpts` (the `.with_*` chain in `serve`), add `.with_generator(opts.generator.clone())` and pass `opts.generator` through if the builder takes positional adapters (mirror exactly how `session_starter` is threaded — grep `with_session_starter` in `lib.rs`).

- [ ] **Step 3: Run to verify the test passes**

Run: `cargo test -p rupu-cp definition_generator`
Expected: PASS (`trait_object_dispatches`).

- [ ] **Step 4: Build the crate to confirm wiring compiles**

Run: `cargo build -p rupu-cp`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/definition_generator.rs crates/rupu-cp/src/lib.rs crates/rupu-cp/src/state.rs
git commit -m "feat(cp): DefinitionGenerator port + AppState wiring"
```

### Task 7: CP generate endpoints (agents, workflows, models)

**Files:**
- Modify: `crates/rupu-cp/src/api/agents.rs`, `crates/rupu-cp/src/api/workflows.rs`
- Test: inline `#[cfg(test)]` in `agents.rs` (mirror the existing `start_session` tests)

**Interfaces:**
- Consumes: `AppState.generator`, `DefinitionGenerator`, `GenerateDefRequest`, `DefKind`, `ApiError::not_available`.
- Produces routes: `POST /api/agents/generate`, `POST /api/workflows/generate`, `GET /api/generate/models`.

- [ ] **Step 1: Write the failing handler tests**

Append to the `tests` module in `crates/rupu-cp/src/api/agents.rs`:

```rust
    use crate::definition_generator::{
        DefKind, DefinitionGenerator, GenDefError, GenerateDefRequest, GeneratedDef, ProviderModels,
    };

    struct StubGen;
    #[async_trait::async_trait]
    impl DefinitionGenerator for StubGen {
        async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError> {
            assert_eq!(req.kind, DefKind::Agent);
            Ok(GeneratedDef {
                raw: VALID_MD.to_string(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                attempts: 1,
            })
        }
        async fn available_models(&self) -> Vec<ProviderModels> {
            vec![ProviderModels { provider: "anthropic".into(), models: vec!["claude-sonnet-4-6".into()], is_default: true }]
        }
    }

    #[tokio::test]
    async fn generate_agent_returns_content_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()).with_generator(Some(std::sync::Arc::new(StubGen)));
        let body = GenerateAgentBody { description: "x".into(), provider: None, model: None };
        let Json(out) = generate_agent(State(state), Json(body)).await.expect("ok");
        assert!(out.raw.contains("name:"));
        // Nothing persisted by generate.
        assert!(!tmp.path().join("agents").exists() || std::fs::read_dir(tmp.path().join("agents")).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn generate_agent_without_adapter_is_not_available() {
        let tmp = tempfile::tempdir().unwrap();
        let state = test_state(tmp.path()); // generator = None
        let body = GenerateAgentBody { description: "x".into(), provider: None, model: None };
        let err = generate_agent(State(state), Json(body)).await.unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
```

If a `test_state(path)` helper / `VALID_MD` const does not already exist in this test module, reuse what the existing `start_session` tests use (grep `fn test_state`/`VALID_MD` in `agents.rs`); if absent, construct `AppState::new(path.to_path_buf(), Default::default())` inline.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p rupu-cp --lib api::agents::tests::generate_agent`
Expected: FAIL — `generate_agent` / `GenerateAgentBody` not found.

- [ ] **Step 3: Add the agent generate route + handler**

In `crates/rupu-cp/src/api/agents.rs`, add the route in `routes()`:

```rust
        .route("/api/agents/generate", post(generate_agent))
```

Add the body + response DTOs and handler:

```rust
#[derive(Deserialize)]
struct GenerateAgentBody {
    description: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Serialize)]
struct GeneratedDefDto {
    raw: String,
    provider: String,
    model: String,
    attempts: u8,
}

async fn generate_agent(
    State(s): State<AppState>,
    Json(body): Json<GenerateAgentBody>,
) -> ApiResult<Json<GeneratedDefDto>> {
    use crate::definition_generator::{DefKind, GenDefError, GenerateDefRequest};
    let gen = s
        .generator
        .clone()
        .ok_or_else(|| ApiError::not_available("AI generation requires `rupu cp serve`"))?;
    let out = gen
        .generate(GenerateDefRequest {
            kind: DefKind::Agent,
            description: body.description,
            provider: body.provider,
            model: body.model,
        })
        .await
        .map_err(|e| match e {
            GenDefError::NoCredentials => ApiError::bad_request(e.to_string()),
            GenDefError::Failed(m) => ApiError::internal(m),
        })?;
    Ok(Json(GeneratedDefDto {
        raw: out.raw,
        provider: out.provider,
        model: out.model,
        attempts: out.attempts,
    }))
}
```

- [ ] **Step 4: Run to verify the agent tests pass**

Run: `cargo test -p rupu-cp --lib api::agents::tests::generate_agent`
Expected: PASS (2 tests).

- [ ] **Step 5: Add the workflow generate route + handler**

In `crates/rupu-cp/src/api/workflows.rs`, add the route:

```rust
        .route("/api/workflows/generate", post(generate_workflow))
```

And the handler (mirrors `generate_agent`, `DefKind::Workflow`; reuse a shared `GeneratedDefDto` — either duplicate the small struct here or move it to `api/mod.rs`; duplicating a 4-field DTO is fine):

```rust
#[derive(serde::Deserialize)]
struct GenerateWorkflowBody {
    description: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(serde::Serialize)]
struct GeneratedWfDto {
    raw: String,
    provider: String,
    model: String,
    attempts: u8,
}

async fn generate_workflow(
    State(s): State<AppState>,
    Json(body): Json<GenerateWorkflowBody>,
) -> ApiResult<Json<GeneratedWfDto>> {
    use crate::definition_generator::{DefKind, GenDefError, GenerateDefRequest};
    let gen = s
        .generator
        .clone()
        .ok_or_else(|| ApiError::not_available("AI generation requires `rupu cp serve`"))?;
    let out = gen
        .generate(GenerateDefRequest {
            kind: DefKind::Workflow,
            description: body.description,
            provider: body.provider,
            model: body.model,
        })
        .await
        .map_err(|e| match e {
            GenDefError::NoCredentials => ApiError::bad_request(e.to_string()),
            GenDefError::Failed(m) => ApiError::internal(m),
        })?;
    Ok(Json(GeneratedWfDto {
        raw: out.raw,
        provider: out.provider,
        model: out.model,
        attempts: out.attempts,
    }))
}
```

The adapter gathers the available agent names server-side (Task 8), so the workflow body needs no agent list.

- [ ] **Step 6: Add the `GET /api/generate/models` route**

Add to `crates/rupu-cp/src/api/workflows.rs` `routes()` (or agents.rs — either; pick workflows.rs):

```rust
        .route("/api/generate/models", get(generate_models))
```

Handler:

```rust
#[derive(serde::Serialize)]
struct ProviderModelsDto {
    provider: String,
    models: Vec<String>,
    is_default: bool,
}

async fn generate_models(State(s): State<AppState>) -> Json<Vec<ProviderModelsDto>> {
    let list = match &s.generator {
        Some(g) => g
            .available_models()
            .await
            .into_iter()
            .map(|p| ProviderModelsDto { provider: p.provider, models: p.models, is_default: p.is_default })
            .collect(),
        None => Vec::new(),
    };
    Json(list)
}
```

(Empty list when no adapter — the frontend treats empty as "AI generation unavailable".)

- [ ] **Step 7: Format, clippy, full crate test**

Run: `cargo fmt -p rupu-cp && cargo clippy -p rupu-cp --all-targets && cargo test -p rupu-cp`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/rupu-cp/src/api/agents.rs crates/rupu-cp/src/api/workflows.rs
git commit -m "feat(cp): /api/{agents,workflows}/generate + /api/generate/models endpoints"
```

### Task 8: `cp serve` adapter (`RuntimeDefinitionGenerator`)

**Files:**
- Create: `crates/rupu-cli/src/cp_definition_generator.rs`
- Modify: `crates/rupu-cli/src/lib.rs` (module decl), `crates/rupu-cli/src/cmd/cp.rs` (build + pass adapter)
- Test: inline `#[cfg(test)]` in the adapter (mock-provider seam)

**Interfaces:**
- Consumes: `rupu_orchestrator::{generate_definition, pick_default_gen_model, GenerateRequest, GenKind, generate::DEFAULT_GEN_MODELS}`, `rupu_agent::load_agents`, `rupu_auth::KeychainResolver`, `rupu_cp::definition_generator::*`.
- Produces: `RuntimeDefinitionGenerator { global_dir: PathBuf }` implementing `DefinitionGenerator`.

- [ ] **Step 1: Write the failing adapter test**

Create `crates/rupu-cli/src/cp_definition_generator.rs`:

```rust
//! `rupu cp serve` adapter for rupu-cp's `DefinitionGenerator` port. Calls
//! the orchestrator generation core with the real credential resolver and
//! gathers available agent names for workflow generation.

use std::path::PathBuf;

use rupu_cp::definition_generator::{
    DefKind, DefinitionGenerator, GenDefError, GenerateDefRequest, GeneratedDef, ProviderModels,
};

pub struct RuntimeDefinitionGenerator {
    pub global_dir: PathBuf,
}

#[async_trait::async_trait]
impl DefinitionGenerator for RuntimeDefinitionGenerator {
    async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError> {
        let resolver = rupu_auth::KeychainResolver::new();
        let (provider, model) = match (req.provider, req.model) {
            (Some(p), Some(m)) => (p, m),
            _ => rupu_orchestrator::pick_default_gen_model(&resolver)
                .await
                .ok_or(GenDefError::NoCredentials)?,
        };
        let (kind, available_agents) = match req.kind {
            DefKind::Agent => (rupu_orchestrator::GenKind::Agent, vec![]),
            DefKind::Workflow => {
                let agents = rupu_agent::load_agents(&self.global_dir, None)
                    .map(|specs| specs.into_iter().map(|s| s.name).collect())
                    .unwrap_or_default();
                (rupu_orchestrator::GenKind::Workflow, agents)
            }
        };
        let gen_req = rupu_orchestrator::GenerateRequest {
            kind,
            description: req.description,
            provider,
            model,
            available_agents,
        };
        let out = rupu_orchestrator::generate_definition(&gen_req, &resolver)
            .await
            .map_err(|e| GenDefError::Failed(e.to_string()))?;
        Ok(GeneratedDef {
            raw: out.content,
            provider: out.provider,
            model: out.model,
            attempts: out.attempts,
        })
    }

    async fn available_models(&self) -> Vec<ProviderModels> {
        let resolver = rupu_auth::KeychainResolver::new();
        let default = rupu_orchestrator::pick_default_gen_model(&resolver).await;
        let mut out = Vec::new();
        for (provider, model) in rupu_orchestrator::generate::DEFAULT_GEN_MODELS {
            if resolver.get(provider, None).await.is_ok() {
                out.push(ProviderModels {
                    provider: provider.to_string(),
                    models: vec![model.to_string()],
                    is_default: default.as_ref().map(|(p, _)| p == provider).unwrap_or(false),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());
    const VALID_AGENT_MD: &str = "---\nname: a\ndescription: d\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nbody\n";

    #[tokio::test]
    async fn adapter_generates_agent_via_mock() {
        let _g = ENV_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let script = serde_json::json!([
            { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
        ])
        .to_string();
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

        let adapter = RuntimeDefinitionGenerator { global_dir: tmp.path().to_path_buf() };
        let out = adapter
            .generate(GenerateDefRequest {
                kind: DefKind::Agent,
                description: "x".into(),
                provider: Some("anthropic".into()),
                model: Some("claude-sonnet-4-6".into()),
            })
            .await
            .expect("ok");
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        assert!(out.raw.contains("name: a"));
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/rupu-cli/src/lib.rs`, add with the other `mod` lines:

```rust
mod cp_definition_generator;
```

- [ ] **Step 3: Run to verify the test passes**

Run: `cargo test -p rupu-cli --lib cp_definition_generator`
Expected: PASS (adapter compiles + generates). If it fails to compile because `pick_default_gen_model` is invoked with `Some` provider but `Some` model (so the `_ =>` arm isn't hit), that is fine — the test passes both, hitting the first match arm.

- [ ] **Step 4: Wire the adapter into `cp serve`**

In `crates/rupu-cli/src/cmd/cp.rs`, where the other adapters are built (next to `session_starter`), add:

```rust
            // Adapter for rupu-cp's DefinitionGenerator port: calls the
            // orchestrator generation core with the real resolver.
            let generator: Option<Arc<dyn rupu_cp::definition_generator::DefinitionGenerator>> =
                Some(Arc::new(crate::cp_definition_generator::RuntimeDefinitionGenerator {
                    global_dir: global_dir.clone(),
                }));
```

And add `generator,` to the `rupu_cp::ServeOpts { … }` literal. Confirm `global_dir` is still in scope at that point (it is used for `repos` just above); if it was moved, clone earlier.

- [ ] **Step 5: Build to confirm wiring**

Run: `cargo build -p rupu-cli`
Expected: success.

- [ ] **Step 6: Format, clippy**

Run: `cargo fmt -p rupu-cli && cargo clippy -p rupu-cli --all-targets`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cli/src/cp_definition_generator.rs crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/cp.rs
git commit -m "feat(cli): wire RuntimeDefinitionGenerator into cp serve"
```

---

## Slice 4 — CP frontend

> Run frontend commands from `crates/rupu-cp/web`. Build with `npm run build`; test with `npx vitest run`.

### Task 9: API client methods + types

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`
- Test: `crates/rupu-cp/web/src/lib/api.generate.test.ts` (create)

**Interfaces:**
- Produces: `api.generateAgent(body)`, `api.generateWorkflow(body)`, `api.generateModels()`, and the `GeneratedDef` / `ProviderModels` / `GenerateBody` types.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cp/web/src/lib/api.generate.test.ts`:

```ts
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

describe('generate api', () => {
  it('posts a description to generateAgent and returns raw', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({ raw: 'name: x', provider: 'anthropic', model: 'claude-sonnet-4-6', attempts: 1 }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    );
    const out = await api.generateAgent({ description: 'a helpful agent' });
    expect(out.raw).toContain('name: x');
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toContain('/api/agents/generate');
    expect(init?.method).toBe('POST');
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run (from `crates/rupu-cp/web`): `npx vitest run src/lib/api.generate.test.ts`
Expected: FAIL — `api.generateAgent is not a function`.

- [ ] **Step 3: Add types + methods**

In `crates/rupu-cp/web/src/lib/api.ts`, add exported types near the other interface declarations:

```ts
export interface GeneratedDef {
  raw: string;
  provider: string;
  model: string;
  attempts: number;
}

export interface GenerateBody {
  description: string;
  provider?: string;
  model?: string;
}

export interface ProviderModels {
  provider: string;
  models: string[];
  is_default: boolean;
}
```

Add methods inside the `export const api = { … }` object (next to `createAgent` / `createWorkflow`):

```ts
  /** Draft an agent definition from a description. 501 when `rupu cp serve` is not running. */
  generateAgent(body: GenerateBody): Promise<GeneratedDef> {
    return request<GeneratedDef>('/api/agents/generate', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },
  /** Draft a workflow definition from a description. 501 when `rupu cp serve` is not running. */
  generateWorkflow(body: GenerateBody): Promise<GeneratedDef> {
    return request<GeneratedDef>('/api/workflows/generate', {
      method: 'POST',
      body: JSON.stringify(body),
    });
  },
  /** Providers/models available for AI generation (empty when unavailable). */
  generateModels(): Promise<ProviderModels[]> {
    return request<ProviderModels[]>('/api/generate/models');
  },
```

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run src/lib/api.generate.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts crates/rupu-cp/web/src/lib/api.generate.test.ts
git commit -m "feat(cp/web): api.generateAgent/Workflow/Models client methods"
```

### Task 10: Describe/Edit toggle in `NewAgentModal`

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Agents.tsx`
- Test: `crates/rupu-cp/web/src/pages/NewAgentModal.test.tsx` (create)

**Interfaces:**
- Consumes: `api.generateAgent`, `api.generateModels`, existing `CodeEditor`, `Button`, `Sparkles` (already imported in `Agents.tsx`).

- [ ] **Step 1: Write the failing component test**

Create `crates/rupu-cp/web/src/pages/NewAgentModal.test.tsx`:

```tsx
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Agents from './Agents';
import { api } from '../lib/api';

afterEach(() => vi.restoreAllMocks());

describe('NewAgentModal describe mode', () => {
  it('generates a draft into the editor', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([
      { provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true },
    ]);
    const gen = vi
      .spyOn(api, 'generateAgent')
      .mockResolvedValue({ raw: 'name: drafted', provider: 'anthropic', model: 'claude-sonnet-4-6', attempts: 1 });

    render(
      <MemoryRouter>
        <Agents />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByText('New agent'));
    fireEvent.click(await screen.findByRole('button', { name: /describe/i }));
    fireEvent.change(screen.getByLabelText(/describe the agent/i), {
      target: { value: 'a code reviewer' },
    });
    fireEvent.click(screen.getByRole('button', { name: /generate/i }));

    await waitFor(() => expect(gen).toHaveBeenCalled());
    expect(await screen.findByDisplayValue(/name: drafted/)).toBeInTheDocument();
  });
});
```

(Adjust `api.getAgents` to the real list method name used by `Agents.tsx` — grep the page; it may be `getAgents`.)

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/pages/NewAgentModal.test.tsx`
Expected: FAIL — no Describe button.

- [ ] **Step 3: Add Describe/Edit mode to `NewAgentModal`**

In `crates/rupu-cp/web/src/pages/Agents.tsx`, extend `NewAgentModal` with a mode toggle and generation. Add state at the top of the component:

```tsx
  const [mode, setMode] = useState<'describe' | 'edit'>('describe');
  const [description, setDescription] = useState('');
  const [models, setModels] = useState<ProviderModels[]>([]);
  const [genProvider, setGenProvider] = useState<string>('');
  const [generating, setGenerating] = useState(false);

  useEffect(() => {
    api.generateModels().then((m) => {
      setModels(m);
      const def = m.find((x) => x.is_default) ?? m[0];
      if (def) setGenProvider(def.provider);
      // No providers → fall back to raw editing.
      if (m.length === 0) setMode('edit');
    }).catch(() => setMode('edit'));
  }, []);

  async function generate() {
    if (generating || !description.trim()) return;
    setGenerating(true);
    setError(null);
    try {
      const sel = models.find((m) => m.provider === genProvider);
      const out = await api.generateAgent({
        description,
        provider: genProvider || undefined,
        model: sel?.models[0],
      });
      setRaw(out.raw);
      setMode('edit'); // drop into the editor for review
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to generate agent');
    } finally {
      setGenerating(false);
    }
  }
```

Import the type at the top of the file: add `ProviderModels` to the existing `import { api } from '../lib/api';` line → `import { api, type ProviderModels } from '../lib/api';`.

Replace the modal body (`<div className="space-y-3 px-5 py-4"> … </div>`) with a mode switch — Describe shows the textarea + provider `<select>` + Generate button; Edit shows the existing `CodeEditor`:

```tsx
        <div className="space-y-3 px-5 py-4">
          <div className="flex gap-1 rounded-lg border border-border p-1 text-ui">
            <button
              type="button"
              onClick={() => setMode('describe')}
              disabled={models.length === 0}
              className={cn('flex-1 rounded-md px-3 py-1.5', mode === 'describe' ? 'bg-panel-2 text-ink' : 'text-ink-dim')}
            >
              Describe
            </button>
            <button
              type="button"
              onClick={() => setMode('edit')}
              className={cn('flex-1 rounded-md px-3 py-1.5', mode === 'edit' ? 'bg-panel-2 text-ink' : 'text-ink-dim')}
            >
              Edit raw
            </button>
          </div>

          {mode === 'describe' ? (
            <>
              <label htmlFor="agent-desc" className="block text-ui text-ink-dim">
                Describe the agent you want
              </label>
              <textarea
                id="agent-desc"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                rows={5}
                className="w-full rounded-lg border border-border bg-panel-2 p-2 text-ui text-ink"
                placeholder="e.g. a security reviewer that flags high/critical vulnerabilities"
              />
              <div className="flex items-center gap-2">
                <select
                  value={genProvider}
                  onChange={(e) => setGenProvider(e.target.value)}
                  className="rounded-lg border border-border bg-panel-2 px-2 py-1.5 text-ui text-ink"
                  aria-label="Generation provider"
                >
                  {models.map((m) => (
                    <option key={m.provider} value={m.provider}>
                      {m.provider} · {m.models[0]}
                    </option>
                  ))}
                </select>
                <Button onClick={generate} disabled={generating || !description.trim()}>
                  <Sparkles size={14} />
                  {generating ? 'Generating…' : 'Generate'}
                </Button>
              </div>
            </>
          ) : (
            <CodeEditor value={raw} onChange={setRaw} language="markdown" ariaLabel="New agent definition" />
          )}

          {error && (
            <p role="alert" className="text-ui font-medium text-err">
              {error}
            </p>
          )}
        </div>
```

The footer Create button is unchanged; it still calls `create()` → `api.createAgent(raw)`. Ensure `cn` is imported in this file (it is used elsewhere; grep — if not, import from `../lib/cn` or wherever `Agents.tsx` siblings import it).

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run src/pages/NewAgentModal.test.tsx`
Expected: PASS.

- [ ] **Step 5: Typecheck + build**

Run: `npm run build`
Expected: `tsc -b` clean, `vite build` succeeds.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/pages/Agents.tsx crates/rupu-cp/web/src/pages/NewAgentModal.test.tsx
git commit -m "feat(cp/web): Describe/Edit toggle + AI generate in NewAgentModal"
```

### Task 11: Describe/Edit toggle in `NewWorkflowModal`

**Files:**
- Modify: `crates/rupu-cp/web/src/pages/Workflows.tsx`
- Test: `crates/rupu-cp/web/src/pages/NewWorkflowModal.test.tsx` (create)

**Interfaces:**
- Consumes: `api.generateWorkflow`, `api.generateModels`, existing modal components.

- [ ] **Step 1: Write the failing component test**

Create `crates/rupu-cp/web/src/pages/NewWorkflowModal.test.tsx`, mirroring Task 10's test but for workflows:

```tsx
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Workflows from './Workflows';
import { api } from '../lib/api';

afterEach(() => vi.restoreAllMocks());

describe('NewWorkflowModal describe mode', () => {
  it('generates a draft into the editor', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([
      { provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true },
    ]);
    const gen = vi
      .spyOn(api, 'generateWorkflow')
      .mockResolvedValue({ raw: 'name: drafted-wf', provider: 'anthropic', model: 'claude-sonnet-4-6', attempts: 1 });

    render(
      <MemoryRouter>
        <Workflows />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByText(/new workflow/i));
    fireEvent.click(await screen.findByRole('button', { name: /describe/i }));
    fireEvent.change(screen.getByLabelText(/describe the workflow/i), {
      target: { value: 'review then fix' },
    });
    fireEvent.click(screen.getByRole('button', { name: /generate/i }));

    await waitFor(() => expect(gen).toHaveBeenCalled());
    expect(await screen.findByDisplayValue(/name: drafted-wf/)).toBeInTheDocument();
  });
});
```

(Adjust `api.getWorkflows` and the "new workflow" trigger text to match `Workflows.tsx`.)

- [ ] **Step 2: Run to verify failure**

Run: `npx vitest run src/pages/NewWorkflowModal.test.tsx`
Expected: FAIL — no Describe button.

- [ ] **Step 3: Apply the same Describe/Edit changes to `NewWorkflowModal`**

In `crates/rupu-cp/web/src/pages/Workflows.tsx`, apply the identical pattern as Task 10 Step 3 to `NewWorkflowModal`, with these substitutions:
- `api.generateAgent` → `api.generateWorkflow`
- `language="markdown"` → `language="yaml"`
- `ariaLabel="New agent definition"` → `ariaLabel="New workflow definition"`
- label text `Describe the agent you want` → `Describe the workflow you want`, textarea `id="workflow-desc"`, placeholder e.g. `"e.g. review changed files, then fix anything high severity"`
- import `Sparkles` from `lucide-react` if not already imported in this file (grep; add to the import if missing), and `type ProviderModels` from `../lib/api`, and `cn`.

The error/footer handling and `create()` → `api.createWorkflow(raw)` are unchanged.

- [ ] **Step 4: Run to verify pass**

Run: `npx vitest run src/pages/NewWorkflowModal.test.tsx`
Expected: PASS.

- [ ] **Step 5: Full frontend test + build**

Run: `npx vitest run && npm run build`
Expected: all tests pass; build clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cp/web/src/pages/Workflows.tsx crates/rupu-cp/web/src/pages/NewWorkflowModal.test.tsx
git commit -m "feat(cp/web): Describe/Edit toggle + AI generate in NewWorkflowModal"
```

---

## Final verification (after all slices)

- [ ] **Workspace build + targeted tests**

Run: `cargo build --workspace && cargo test -p rupu-orchestrator -p rupu-cp -p rupu-cli`
Expected: green (note the known pre-existing toolchain caveat for rupu-cli baseline on the worktree's Homebrew toolchain — compare against `main`, don't attribute pre-existing red to this work).

- [ ] **Clippy on touched crates**

Run: `cargo clippy -p rupu-orchestrator -p rupu-cp -p rupu-cli --all-targets`
Expected: no new warnings.

- [ ] **Frontend**

Run (from `crates/rupu-cp/web`): `npx vitest run && npm run build`
Expected: green.

- [ ] **Rebuild embedded UI before any release**

Run: `make cp-web`
Expected: `crates/rupu-cp/web/dist` refreshed so the binary embeds the new modals.

- [ ] **Manual smoke (matt, optional but recommended for the CP UI)**

`rupu cp serve`, open the web UI → Agents → New agent → Describe → enter a description → Generate → review the drafted `.md` in the editor → Create. Repeat for Workflows. Confirm a bare read-only deploy (no `cp serve`) surfaces the unavailable state gracefully.

---

## Notes / open follow-ups (out of scope here)

- **Project scope on CP:** CP create writes to the global dir only today; a project-scope picker on CP rides on the broader projects feature, not this slice (the CLI already supports `--scope project`).
- **Host on CP:** the host selector is intentionally absent from the modals until multi-host lands and more than one host exists; the CLI `--host` flag already rejects non-`local` hosts with a clear message.
- **Richer model lists:** `available_models` returns one model per authed provider (the default). Wiring the full `ModelRegistry` list into the dropdown is a later enhancement.
