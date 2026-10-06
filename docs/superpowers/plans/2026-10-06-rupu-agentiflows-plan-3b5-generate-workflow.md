# Agentiflows Plan 3b-5 — `generate_workflow` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the agentiflow lead a Tier-2 `generate_workflow` tool that authors a NEW workflow (taught the full format) via the lead's own injected provider, validates it fail-closed against the pool, stamps the active engagement profile, materializes it into the run dir, and runs it as a process-isolated unit — without persisting anything into the user's catalog.

**Architecture:** Reuse the existing LLM-backed generator (`rupu_orchestrator::generate_definition`) rather than invent a second one. Split its credential-building shell from its generate-and-repair core so the agentiflow crate can drive the core with the **same provider closure the lead already uses** — keeping `rupu-auth` out of `rupu-agentiflow` (hexagonal, consistent with the existing `make_provider` injection). Add a real `rupu workflow run --file <path>` primitive so a generated workflow living in the run dir can run as a unit without touching the on-disk catalog. The new tool mounts in the existing `fleet_unit_tools` constructor beside `dispatch`/`join`/`run_workflow`, sharing their participant counter, and is **absent entirely when no generation capability is injected** (fail-closed, no-op-when-absent).

**Tech Stack:** Rust 2021, tokio, async-trait, thiserror (libs), serde/serde_yaml, clap (CLI), ULID. Crates touched: `rupu-orchestrator`, `rupu-cli`, `rupu-agentiflow`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` — §6 (lead tools incl. `generate_workflow`), §7 (engagement set propagation; a generated workflow may narrow but never widen), §13 Tier-2 (`generate_workflow` role-gated to the lead/orchestrators), §23 Plan 3 decomposition ("`generate_workflow` taught the full format (loops/panels/splits) and made to stamp the active engagement profile on generated workflows; pool validation (incl. pool ⊇ the workflow's dispatched agents)").

## Global Constraints

- **Hexagonal / no new crate dep.** `rupu-agentiflow` MUST NOT gain a `rupu-auth` dependency. Generation runs through a caller-injected provider closure, exactly as the lead's `make_provider: ProviderFactory` already works (`crates/rupu-agentiflow/src/lead.rs:220`). `rupu-orchestrator` keeps `rupu-auth` for its credential-building wrapper only.
- **Workspace deps only.** New deps (if any) pinned in root `Cargo.toml`; internal crates referenced as `{ path = "../x" }`. Do not add versions to crate `Cargo.toml` files.
- **`#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden; `thiserror` for library errors, no `anyhow` in library crates** (`anyhow` only in `rupu-cli`).
- **Fail-closed, nothing-spawned-on-refusal.** Every validation failure in the tool returns `Ok(ToolOutput::error(reason))` (the lead sees the reason) with no unit dispatched and no file left behind — mirroring `run_workflow`'s five checks (`crates/rupu-agentiflow/src/dispatch_tools.rs:329-402`). A refusal is never a Rust `Err`.
- **No-op-when-absent.** When no generation capability is injected, `generate_workflow` is not added to the lead's toolset at all. A plain run without the capability behaves exactly as today.
- **No catalog pollution.** Generated workflows materialize under `<run_dir>/generated/`, never into `<global|project>/workflows/`. They run via `--file`, not by catalog id.
- **No mock features.** The generation capability is a real injection seam. Its production wiring is deferred to Plan 4 (Surfaces), identical to `make_provider` today ("Plan 4 wires the real resolver; tests hand in a mock", `crates/rupu-agentiflow/src/run.rs:154`). This is injection, not a silent no-op.
- **Integration tests: one binary per crate.** New tests go in modules under `crates/<c>/tests/it/` listed in `main.rs` — never a new top-level `tests/*.rs`. `rupu-cli` env/cwd-mutating tests go in `tests/serial/` holding `ENV_LOCK`.
- **Engagement stamping = launch parameter, not YAML.** The workflow model carries NO engagement field (confirmed: zero `engagement` in `workflow.rs`). Stamping means dispatching the generated unit with `UnitSpec.engagement = active_set`, identical to `run_workflow` (`dispatch_tools.rs:233,392`). Widening is structurally impossible; per-step narrowing is a future profile-spec feature, explicitly out of scope for v1.

---

## Design rulings (read before Task 1)

These reconcile the spec's one-line decomposition with the code as it exists; they are binding for this plan.

1. **Reuse, don't duplicate, the generator.** The "linear/parallel/panel only" limitation is purely in `WORKFLOW_SYSTEM_PROMPT` (`crates/rupu-orchestrator/src/generate.rs:137-147`); the validator (`Workflow::parse`) already accepts the full format. So "teach the full format" = expand that prompt (Task 2), and `generate_workflow` wraps the existing, tested generate-and-repair loop (Task 1 exposes a provider-taking entry point).

2. **Generation uses the lead's own provider.** The tool is handed a `GenerationCapability { provider, model, factory }` built the same way (and by the same Plan-4 caller) as `make_provider`. No `rupu-auth` in agentiflow; no separate credential path. The capability is `Option` — absent ⇒ no tool.

3. **Run generated workflows via `--file`, in the run dir.** `rupu workflow run --file <path>` (Task 3) is a real, documented primitive — the "run an unsaved workflow" affordance the spec gestures at. The unit launcher emits `--file <materialized path>` (Task 4). Nothing is written to the catalog.

4. **v1 rejects the same shapes `run_workflow` rejects.** A generated workflow with an approval gate or a `host:`/`distribute:` step is refused post-generation (`has_approval_gate()` / `has_placed_step()`), AND the generator is steered away from them via a constraint appended to the description. Generated units are therefore always local, unattended, single-process — same safety envelope as 3b-4.

5. **Pool validation = `dispatched_agents() ⊆ pool`.** Reuse `Workflow::dispatched_agents()` (`workflow.rs:1269`) and the lead's pool agent set (`Arc<Vec<String>>`, the same one `run_workflow` checks). There is no "workflow is in the pool" check — the workflow is brand-new by definition.

---

## File Structure

- `crates/rupu-orchestrator/src/generate.rs` — **modify.** Extract the generate-and-repair core into `generate_definition_with_provider(req, provider: &mut dyn LlmProvider)`; `generate_definition` becomes a thin build-provider-then-delegate wrapper. Expand `WORKFLOW_SYSTEM_PROMPT`.
- `crates/rupu-cli/src/cmd/workflow.rs` — **modify.** `Run` clap variant gains `file: Option<PathBuf>`; `name` becomes `Option<String>` (`required_unless_present = "file"`). `run`/`run_with_outcome` take an explicit-file override.
- `crates/rupu-agentiflow/src/unit.rs` — **modify.** `UnitSpec` gains `workflow_file: Option<PathBuf>`.
- `crates/rupu-agentiflow/src/subprocess.rs` — **modify.** `rupu_workflow_argv` emits `--file <path>` (omitting the positional name) when `workflow_file` is set.
- `crates/rupu-agentiflow/src/lead.rs` — **modify.** Define `GenerationCapability` + `GenerationProviderFactory` next to `ProviderFactory`.
- `crates/rupu-agentiflow/src/dispatch_tools.rs` — **modify.** Add the `GenerateWorkflowTool`; `fleet_unit_tools` conditionally mounts it.
- `crates/rupu-agentiflow/src/run.rs` — **modify.** `RunAgentiflowOpts` gains `generation: Option<GenerationCapability>`; threaded into `fleet_unit_tools`.
- Test modules: `crates/rupu-orchestrator/tests/it/` (generator), `crates/rupu-cli/src/cmd/workflow.rs` unit tests + `crates/rupu-cli/tests/serial/` (--file), `crates/rupu-agentiflow/src/dispatch_tools.rs` + `src/subprocess.rs` unit tests + `src/run.rs` integration tests.

---

### Task 1: Expose a provider-taking generator core (`rupu-orchestrator`)

**Files:**
- Modify: `crates/rupu-orchestrator/src/generate.rs:162-242`
- Test: `crates/rupu-orchestrator/src/generate.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `GenerateRequest` (`generate.rs:44`), `GenerateOutcome` (`:57`), `GenerateError` (`:65`), `build_system_prompt` (`:109`), `validate` (`:97`), `strip_fences` (`:83`), `rupu_providers::LlmProvider` (`send(&mut self, &LlmRequest)`), `LlmRequest`/`Message` (`generate.rs:9`).
- Produces: `pub async fn generate_definition_with_provider(req: &GenerateRequest, provider: &mut dyn rupu_providers::LlmProvider) -> Result<GenerateOutcome, GenerateError>` — the prompt-build + send + parse-repair loop, no credential/provider construction. `generate_definition` keeps its current signature and delegates.

- [ ] **Step 1: Write the failing test** — a mock provider drives the new core directly (no resolver), succeeding on the first attempt.

```rust
// in generate.rs tests. Reuse the crate's existing mock-provider helper if one
// exists in tests; otherwise a minimal inline mock:
struct ScriptedProvider { replies: std::sync::Mutex<Vec<String>> }
#[async_trait::async_trait]
impl rupu_providers::LlmProvider for ScriptedProvider {
    async fn send(&mut self, _req: &rupu_providers::types::LlmRequest)
        -> Result<rupu_providers::types::LlmResponse, rupu_providers::ProviderError> {
        let text = self.replies.lock().unwrap().remove(0);
        Ok(rupu_providers::types::LlmResponse::assistant_text(text)) // use the crate's real constructor
    }
    // name()/model()/etc.: delegate to the smallest real impl the trait needs.
}

#[tokio::test]
async fn with_provider_returns_validated_workflow_on_first_try() {
    let wf = "name: gen\nsteps:\n  - id: a\n    agent: writer\n    prompt: hi\n";
    let mut p = ScriptedProvider { replies: std::sync::Mutex::new(vec![wf.to_string()]) };
    let req = GenerateRequest {
        kind: GenKind::Workflow, description: "x".into(),
        provider: "anthropic".into(), model: "claude-sonnet-4-6".into(),
        available_agents: vec!["writer".into()],
    };
    let out = generate_definition_with_provider(&req, &mut p).await.unwrap();
    assert_eq!(out.attempts, 1);
    assert!(out.content.contains("agent: writer"));
}
```

- [ ] **Step 2: Run it, confirm it fails** — `cargo test -p rupu-orchestrator --lib generate::tests::with_provider_returns_validated_workflow_on_first_try` → fails: `generate_definition_with_provider` not found. (Inspect the crate's existing mock-provider test helper at `generate.rs:296` and REUSE it rather than hand-rolling `LlmResponse` — match the real constructor.)

- [ ] **Step 3: Extract the core.** Move `generate.rs:197-241` (system prompt build, message seeding, the `for attempt in 1..=MAX_ATTEMPTS` loop, the final `Err(Invalid)`) into:

```rust
/// Generate a validated definition using an already-built provider,
/// repairing up to [`MAX_ATTEMPTS`]. The caller owns credential/provider
/// construction (see [`generate_definition`] for the config-aware shell).
pub async fn generate_definition_with_provider(
    req: &GenerateRequest,
    provider: &mut dyn rupu_providers::LlmProvider,
) -> Result<GenerateOutcome, GenerateError> {
    let system = build_system_prompt(req.kind, &req.available_agents);
    let mut messages = vec![Message::user(&format!(
        "Create a rupu {} from this description:\n\n{}",
        req.kind.noun(), req.description
    ))];
    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let llm_req = LlmRequest {
            model: req.model.clone(),
            system: Some(system.clone()),
            messages: messages.clone(),
            max_tokens: Some(MAX_TOKENS),
            ..Default::default()
        };
        let resp = provider.send(&llm_req).await?;
        let raw = resp.text().ok_or(GenerateError::Empty)?.to_string();
        let content = strip_fences(&raw).to_string();
        match validate(req.kind, &content) {
            Ok(()) => return Ok(GenerateOutcome {
                content, provider: req.provider.clone(), model: req.model.clone(), attempts: attempt,
            }),
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
    Err(GenerateError::Invalid { kind: req.kind.noun(), attempts: MAX_ATTEMPTS, last_error })
}
```

Then rewrite `generate_definition` (`:163`) to keep building the provider (`:186-195`) and delegate:

```rust
pub async fn generate_definition(
    req: &GenerateRequest,
    resolver: &dyn rupu_auth::CredentialResolver,
    provider_config: &rupu_runtime::ProviderConfig,
) -> Result<GenerateOutcome, GenerateError> {
    let (_mode, mut provider) = build_for_provider_with_config(
        &req.provider, &req.model, None, resolver, provider_config,
        std::sync::Arc::new(rupu_netflow::NullSink),
    ).await.map_err(|_| GenerateError::NoCredentials)?;
    generate_definition_with_provider(req, provider.as_mut()).await
}
```

Keep the existing `// ISSUES.md I-74` / netflow-sink comment block attached to the `build_for_provider_with_config` call in the wrapper.

- [ ] **Step 4: Run tests, confirm pass** — `cargo test -p rupu-orchestrator --lib generate::` — the new test passes AND the three existing mock-provider end-to-end tests (`generate.rs:296,322,350`) still pass unchanged (they exercise `generate_definition` via `RUPU_MOCK_PROVIDER_SCRIPT`, which still flows through the wrapper). Expected: all green.

- [ ] **Step 5: Commit** — `git add crates/rupu-orchestrator/src/generate.rs && git commit -m "refactor(orchestrator): expose generate_definition_with_provider core"`

---

### Task 2: Teach the generator the full workflow format (`rupu-orchestrator`)

**Files:**
- Modify: `crates/rupu-orchestrator/src/generate.rs:137-147` (`WORKFLOW_SYSTEM_PROMPT`)
- Test: `crates/rupu-orchestrator/src/generate.rs` tests (extend `system_prompt_lists_available_agents_for_workflows`, `generate.rs:273`)

**Interfaces:**
- Consumes: nothing new.
- Produces: an expanded `WORKFLOW_SYSTEM_PROMPT` const covering the full runnable format. No signature change.

- [ ] **Step 1: Write the failing assertion** — extend the existing prompt test to assert the new constructs are taught:

```rust
#[test]
fn workflow_prompt_teaches_full_format() {
    let p = build_system_prompt(GenKind::Workflow, &["writer".to_string()]);
    for kw in ["for_each", "parallel", "panel", "branch", "split", "join",
               "depends_on", "loops", "until"] {
        assert!(p.contains(kw), "prompt must teach `{kw}`");
    }
    assert!(p.contains("writer"), "lists available agents");
}
```

- [ ] **Step 2: Run it, confirm it fails** — `cargo test -p rupu-orchestrator --lib generate::tests::workflow_prompt_teaches_full_format` → fails (current prompt omits `for_each`, `branch`, `split`, `loops`, …).

- [ ] **Step 3: Expand `WORKFLOW_SYSTEM_PROMPT`.** Replace the "Other step shapes" paragraph (`:144-147`) with teaching for the full runnable surface. Keep it concise and accurate to `workflow.rs`. Cover, each with one line:
  - `for_each: <minijinja list expr>` + `agent`/`prompt` — fan one agent over a list; `max_parallel`.
  - `parallel:` — list of sub-steps (`id`/`agent`/`prompt`); `max_parallel`.
  - `panel:` — `panelists` + `subject` + `prompt`, optional `gate`.
  - `branch:` — `condition` (minijinja) + `then:`/`else:` (lists of step ids).
  - `split: [ids]` + `join:` — fan into concurrent tracks then barrier.
  - `next: [ids]` / `depends_on: [ids]` — explicit DAG edges (default is linear order).
  - `when: <minijinja>` — skip-guard; `continue_on_error: true`.
  - Top-level `loops:` map, each `{ nodes: [ids], until: <minijinja>, max_iterations: <int>, on_max: ... }` — note loops are a TOP-LEVEL map keyed by name, NOT a step field.
  - A closing line: "Prefer the simplest shape that fits. Every `agent:`/panelist/`for_each` step must name a real available agent."
  - Do NOT teach `approval:`/`host:`/`distribute:`/`action:`/`run:` as the default — these are valid in the format but the common generated workflow omits them. (The `generate_workflow` tool, Task 5, additionally constrains against gates/placement; this prompt serves the human `rupu workflow generate` path too, so simply don't foreground them.)

- [ ] **Step 4: Run tests, confirm pass** — `cargo test -p rupu-orchestrator --lib generate::` — new test green; the `validate_accepts_good_workflow`/`validate_rejects_*` tests unaffected.

- [ ] **Step 5: Commit** — `git add crates/rupu-orchestrator/src/generate.rs && git commit -m "feat(orchestrator): teach workflow generator the full format (for_each/branch/split/join/loops)"`

---

### Task 3: `rupu workflow run --file <path>` primitive (`rupu-cli`)

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` — `Run` clap variant (`:401-452`), its handler (`:667-696`), `run` (`:4634`), `run_with_outcome` (`:4656-4670`)
- Test: `crates/rupu-cli/tests/serial/` (new module, listed in `tests/serial/main.rs`) — env/cwd-sensitive, so serial.

**Interfaces:**
- Consumes: `locate_workflow` (`:4446`), `Workflow::parse` (`:4670`), `FleetRunOverlay`.
- Produces: `Run.file: Option<PathBuf>`; `run`/`run_with_outcome` gain a leading `file: Option<&Path>` parameter. When `Some`, the workflow is read from that path and the run's display name is the parsed workflow's `name:`; when `None`, behavior is exactly as today.

- [ ] **Step 1: Write the failing test** — a workflow file OUTSIDE any catalog runs via `--file`. Drive the public `run_with_outcome` (or `run`) directly with a temp file; assert it loads and starts (status/run-id returned), and that a bogus `--file` path errors clearly.

```rust
// crates/rupu-cli/tests/serial/run_from_file.rs  (hold ENV_LOCK; set HOME/RUPU_HOME to a tempdir)
#[tokio::test]
async fn run_file_loads_workflow_outside_catalog() {
    let _g = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    // isolate RUPU_HOME + cwd so no catalog workflow named `offcat` exists
    let wf = tmp.path().join("offcat.yaml");
    std::fs::write(&wf, "name: offcat\nsteps:\n  - id: a\n    agent: echo\n    prompt: hi\n").unwrap();
    // call the crate-internal run entry with file=Some(&wf), name=None-equivalent.
    // assert: Ok, and the started run's workflow name resolves to "offcat".
}
```

(Exact harness: match the pattern already used by `rupu-cli` serial workflow-run tests — reuse their RUPU_HOME/tempdir setup and whatever lightweight agent/no-op the suite uses so the run can start without a live provider. If none exists, assert up to the point the workflow is parsed and inputs resolve, before the agent call.)

- [ ] **Step 2: Run it, confirm it fails** — the new parameter doesn't exist yet. `cargo test -p rupu-cli --test serial run_from_file::` → compile error / fail.

- [ ] **Step 3: Implement.**
  1. `Run` variant: change `name: String` → `name: Option<String>` with `#[arg(required_unless_present = "file", add = ...)]`; add `#[arg(long, value_name = "PATH", conflicts_with = "target")] file: Option<PathBuf>` with help: "Run a workflow directly from a file path instead of resolving a name from the catalog. The workflow is not added to the catalog."
  2. Handler (`:667`): destructure `file`; pass `file.as_deref()` and `name.as_deref()` into `run(...)`.
  3. `run` + `run_with_outcome`: add a leading `file: Option<&Path>` param (thread `run` → `run_with_outcome`). In `run_with_outcome`, replace `:4668-4670`:

```rust
let (path, body) = match file {
    Some(f) => (f.to_path_buf(), std::fs::read_to_string(f)
        .map_err(|e| anyhow::anyhow!("--file {}: {e}", f.display()))?),
    None => {
        let name = name.ok_or_else(|| anyhow::anyhow!("workflow name required"))?;
        let p = locate_workflow(name)?;
        let body = std::fs::read_to_string(&p)?;
        (p, body)
    }
};
let workflow = Workflow::parse(&body)?;
let name = file.map(|_| workflow.name.as_str()).or(name).unwrap_or(&workflow.name);
// use `name` (now always a &str) for the rest of run_with_outcome unchanged.
```

  (Adjust the downstream `name` references to the computed `&str`. Keep `target` conflicting with `file` — a generated workflow has no clone/PR/issue target.)
  4. Update the other in-crate callers of `run`/`run_with_outcome` (autoflow, webhook receiver, `run_by_*`) to pass `None` for `file` — a one-line edit each; grep `run_with_outcome(` and `run(` call sites.

- [ ] **Step 4: Run tests, confirm pass** — `cargo test -p rupu-cli --test serial run_from_file::`; then `cargo test -p rupu-cli` (the whole suite, since clap-shape and call-site changes are broad). Expected: green.

- [ ] **Step 5: Commit** — `git add crates/rupu-cli/src/cmd/workflow.rs crates/rupu-cli/tests/serial/ && git commit -m "feat(cli): rupu workflow run --file <path> — run a workflow outside the catalog"`

---

### Task 4: File-backed workflow units (`rupu-agentiflow` subprocess)

**Files:**
- Modify: `crates/rupu-agentiflow/src/unit.rs` (`UnitSpec`, `:36-53`; `MockUnitLauncher`/test ctor defaults, `:204-207`)
- Modify: `crates/rupu-agentiflow/src/subprocess.rs` (`rupu_workflow_argv`, `:105-121`)
- Test: `crates/rupu-agentiflow/src/subprocess.rs` tests

**Interfaces:**
- Consumes: `UnitSpec`, `UnitKind::Workflow`.
- Produces: `UnitSpec.workflow_file: Option<PathBuf>` (None for agent units and catalog workflow units). When `Some`, `rupu_workflow_argv` emits `workflow run --file <path> --run-id <id> …` (no positional name).

- [ ] **Step 1: Write the failing test** — a workflow unit with `workflow_file: Some(p)` produces a `--file`-shaped argv.

```rust
#[test]
fn workflow_argv_uses_file_when_set() {
    let spec = UnitSpec {
        agent: "gen-abc".into(), prompt: String::new(), engagement: vec!["network".into()],
        kind: UnitKind::Workflow, inputs: vec![("k".into(), "v".into())],
        workflow_file: Some(PathBuf::from("/runs/x/generated/gen-abc.yaml")),
        participant: "p3".into(), // match the real field name
    };
    let argv: Vec<String> = rupu_workflow_argv(&spec, "run-1", Path::new("/runs/x"))
        .iter().map(|s| s.to_string_lossy().into_owned()).collect();
    assert!(argv.windows(2).any(|w| w[0] == "--file" && w[1] == "/runs/x/generated/gen-abc.yaml"));
    assert!(!argv.iter().any(|a| a == "gen-abc"), "no positional name when --file is set");
    assert!(argv.windows(2).any(|w| w[0] == "--engagement-profile" && w[1] == "network"));
    assert!(argv.windows(2).any(|w| w[0] == "--input" && w[1] == "k=v"));
}
```

- [ ] **Step 2: Run it, confirm it fails** — `cargo test -p rupu-agentiflow --lib subprocess::tests::workflow_argv_uses_file_when_set` → fails (field missing / positional still emitted).

- [ ] **Step 3: Implement.**
  1. `UnitSpec`: add `/// For a generated workflow unit: the materialized file to run via\n/// `--file` instead of a catalog id. `None` for agent and pool-workflow units.\npub workflow_file: Option<PathBuf>,`. Add `workflow_file: None` to the test-helper ctor default (`:204`) and any `UnitSpec { … }` literal in-crate (grep; `dispatch_tools.rs:229`, `:388`).
  2. `rupu_workflow_argv` (`:105`): when `spec.workflow_file` is `Some(p)`, build `vec!["workflow".into(), "run".into(), "--file".into(), p.clone().into(), "--run-id".into(), run_id.into(), "--mode".into(), "bypass".into(), "--plain".into()]`; else keep the current positional-name form. Both then fall through to the existing `--input` loop + `push_unit_tail`.

- [ ] **Step 4: Run tests, confirm pass** — `cargo test -p rupu-agentiflow --lib subprocess::` plus the existing `rupu_workflow_argv` catalog-id test (unchanged shape when `workflow_file` is `None`). Expected: green.

- [ ] **Step 5: Commit** — `git add crates/rupu-agentiflow/src/unit.rs crates/rupu-agentiflow/src/subprocess.rs && git commit -m "feat(agentiflow): file-backed workflow units (UnitSpec.workflow_file → --file argv)"`

---

### Task 5: The `generate_workflow` tool (`rupu-agentiflow`)

**Files:**
- Modify: `crates/rupu-agentiflow/src/lead.rs` — add `GenerationProviderFactory` + `GenerationCapability` (near `ProviderFactory`, `:220`)
- Modify: `crates/rupu-agentiflow/src/dispatch_tools.rs` — add `GenerateWorkflowTool`; `fleet_unit_tools` conditionally mounts it
- Test: `crates/rupu-agentiflow/src/dispatch_tools.rs` tests

**Interfaces:**
- Consumes: `rupu_orchestrator::generate::{generate_definition_with_provider, GenerateRequest, GenKind}`; `rupu_orchestrator::Workflow` (`parse`, `dispatched_agents`, `has_approval_gate`, `has_placed_step`, `name`); `rupu_orchestrator::runner::resolve_inputs`; `FleetSupervisor::dispatch`; `UnitSpec` (fields: `agent`, `prompt`, `engagement`, `participant`, `kind`, `inputs`, + the new `workflow_file`); `rupu_tools::{Tool, ToolContext, ToolError}`; `rupu_providers::LlmProvider`. **Reuse the existing in-module helpers `done(stdout)` / `failed(msg)` (NOT `ToolOutput::text`/`::error`, which don't exist here) and `req_str(&input, key)`, and reuse the shared `DispatchCtx` (`sup` / `pool` / `engagement` / `depth` / `mint_participant(agent)`) exactly as `RunWorkflowTool` does — do NOT re-plumb pool/sup/engagement/counter onto the new tool.**
- Produces:
  - `lead.rs`: `pub type GenerationProviderFactory = std::sync::Arc<dyn Fn() -> Box<dyn rupu_providers::LlmProvider> + Send + Sync>;` and
    ```rust
    /// How the lead authors new workflows: the provider/model to generate
    /// with and a factory that mints a provider for each generation call.
    /// Built by the launch site exactly like `make_provider` (Plan 4).
    #[derive(Clone)]
    pub struct GenerationCapability {
        pub provider: String,
        pub model: String,
        pub factory: GenerationProviderFactory,
    }
    ```
  - `dispatch_tools.rs`: `GenerateWorkflowTool` implementing `Tool` (name `"generate_workflow"`); `fleet_unit_tools` grows params to accept `run_dir: PathBuf` and `generation: Option<GenerationCapability>`, and pushes the tool only when `generation.is_some()`.

- [ ] **Step 1: Write the failing tests** — use a scripted mock provider via the `GenerationProviderFactory`, plus the existing `MockUnitLauncher`/supervisor test rig already used by the `run_workflow` tests (`dispatch_tools.rs:941`). Cover:

```rust
#[tokio::test]
async fn generate_workflow_dispatches_a_file_backed_unit_with_engagement() {
    // factory mints a provider scripted to return a valid single-step workflow
    // using a pool agent ("writer"); pool = ["writer"], engagement = ["network"].
    // invoke generate_workflow { description: "..." } with a ToolContext.
    // assert: Ok(non-error); exactly one unit spawned; kind == Workflow;
    //         workflow_file == Some(<run_dir>/generated/*.yaml) and that file exists & parses;
    //         engagement == ["network"]; participant distinct from a prior dispatch.
}

#[tokio::test]
async fn generate_workflow_refuses_out_of_pool_agent() {
    // scripted workflow references agent "haxor" not in pool ["writer"].
    // assert: Ok(ToolOutput::error) mentioning the offending agent; NO unit spawned; NO file left.
}

#[tokio::test]
async fn generate_workflow_refuses_gated_or_placed_workflow() {
    // scripted workflow contains an `approval:` (then a second case: a `host:` step).
    // assert: Ok(error) "cannot run a gated or host/distribute workflow as a unit"; nothing spawned.
}

#[tokio::test]
async fn generate_workflow_surfaces_generator_failure() {
    // provider scripted to always return unparseable YAML (exhausts MAX_ATTEMPTS).
    // assert: Ok(error) carrying the generator's parse error; nothing spawned.
}
```

- [ ] **Step 2: Run them, confirm they fail** — `cargo test -p rupu-agentiflow --lib dispatch_tools::tests::generate_workflow_` → fail (tool absent).

- [ ] **Step 3: Implement `GenerateWorkflowTool`.** Shape it on `RunWorkflowTool` (`dispatch_tools.rs:262`): `struct GenerateWorkflowTool { ctx: Arc<DispatchCtx>, gen: GenerationCapability, run_dir: PathBuf }`. All of pool/engagement/depth/sup/participant come from `ctx` (`let c = &self.ctx;`). `name()` = `"generate_workflow"`. `description()`: "Author a NEW workflow for a described task and run it as an independent unit, in its own process. The workflow may only dispatch agents in this flow's pool; it runs unattended (no approval gates, no remote placement)." `input_schema`: required string `description`; optional `inputs` object (string→string). `invoke(&self, input, _ctx)`:
  1. `let c = &self.ctx;` `let description = req_str(&input, "description")?;` parse optional `inputs` → `Vec<(String, String)>` (a `{}` map in insertion order; reuse whatever helper `run_workflow` uses for its `inputs`, else map over `as_object()`).
  2. Depth guard first (like `run_workflow`): `if c.depth >= MAX_DEPTH { return Ok(failed(format!("generate_workflow refused: maximum dispatch depth ({MAX_DEPTH}) reached"))); }`.
  3. Build `let req = GenerateRequest { kind: GenKind::Workflow, description: format!("{description}{CONSTRAINT}"), provider: self.gen.provider.clone(), model: self.gen.model.clone(), available_agents: (*c.pool).clone() };` where `const CONSTRAINT: &str = "\n\nConstraints: the workflow runs unattended as a single local process. Do NOT use approval gates (`approval:`), `host:`, or `distribute:`.";`.
  4. `let mut provider = (self.gen.factory)();` then `let outcome = match generate_definition_with_provider(&req, provider.as_mut()).await { Ok(o) => o, Err(e) => return Ok(failed(format!("workflow generation failed: {e}"))) };`.
  5. `let wf = match Workflow::parse(&outcome.content) { Ok(w) => w, Err(e) => return Ok(failed(format!("generated workflow did not parse: {e}"))) };` (belt-and-braces; the generator already validated).
  6. Checks — each `return Ok(failed(..))`, nothing written/spawned — in order:
     - `if wf.has_approval_gate() || wf.has_placed_step() { return Ok(failed("generated workflow cannot run as a unit: it uses an approval gate or host/distribute placement")); }`
     - `let pool: std::collections::BTreeSet<String> = c.pool.iter().cloned().collect(); let missing: Vec<String> = wf.dispatched_agents().difference(&pool).cloned().collect(); if !missing.is_empty() { return Ok(failed(format!("generated workflow dispatches agents not in this flow's pool: {missing:?}"))); }`
     - `if let Err(e) = rupu_orchestrator::runner::resolve_inputs(&wf, &inputs) { return Ok(failed(format!("generated workflow inputs did not resolve: {e}"))); }`
  7. Materialize into the run dir: `let dir = self.run_dir.join("generated"); if let Err(e) = std::fs::create_dir_all(&dir) { return Ok(failed(format!("could not create generated/ dir: {e}"))); }` filename collision-free, e.g. `format!("{}-{}.yaml", slug(&wf.name), ulid::Ulid::new())` (`slug` = lowercase, non-alnum → `-`); `let path = dir.join(filename); if let Err(e) = std::fs::write(&path, &outcome.content) { return Ok(failed(format!("could not write generated workflow: {e}"))); }` — write `outcome.content` verbatim (the exact validated text; no re-serialize).
  8. Dispatch, exactly as `run_workflow` does (`dispatch_tools.rs:388-398`): `let spec = UnitSpec { agent: wf.name.clone(), prompt: String::new(), engagement: c.engagement.clone(), participant: c.mint_participant(&wf.name), kind: UnitKind::Workflow, inputs, workflow_file: Some(path.clone()) }; let participant = spec.participant.clone(); Ok(match c.sup.dispatch(spec) { Ok(handle) => done(json!({ "handle": handle, "participant": participant, "generated_file": path.display().to_string() }).to_string()), Err(e) => failed(format!("generated workflow failed to start: {e}")) })`.
  9. `fleet_unit_tools`: add params `run_dir: PathBuf, generation: Option<GenerationCapability>`. Change the two existing `ctx.clone()` moves so `RunWorkflowTool` takes `ctx.clone()`, then build the vec and, when `generation` is `Some`, push `Arc::new(GenerateWorkflowTool { ctx: ctx.clone(), gen, run_dir })`. The tool shares the SAME `Arc<DispatchCtx>` (hence the SAME `next` counter) as dispatch/join/run_workflow — that is the participant-uniqueness guarantee.

- [ ] **Step 4: Run tests, confirm pass** — `cargo test -p rupu-agentiflow --lib dispatch_tools::` (incl. the existing `run_workflow` + participant-uniqueness tests, which must still pass; adjust their `fleet_unit_tools(...)` calls for the two new params — pass a tempdir + `None` where generation isn't under test). Expected: green.

- [ ] **Step 5: Commit** — `git add crates/rupu-agentiflow/src/lead.rs crates/rupu-agentiflow/src/dispatch_tools.rs && git commit -m "feat(agentiflow): generate_workflow tool — author, validate, materialize, run a workflow unit"`

---

### Task 6: Wire the generation capability through `run_agentiflow` (`rupu-agentiflow`)

**Files:**
- Modify: `crates/rupu-agentiflow/src/run.rs` — `RunAgentiflowOpts` (`:180` area); `run_agentiflow` assembly (`:427`, `:591-600`)
- Test: `crates/rupu-agentiflow/src/run.rs` tests (or `tests/it/`)

**Interfaces:**
- Consumes: `GenerationCapability` (Task 5); `fleet_unit_tools` (new signature, Task 5).
- Produces: `RunAgentiflowOpts.generation: Option<crate::lead::GenerationCapability>` — threaded into `fleet_unit_tools(..., run_dir.clone(), opts.generation.clone())`. Absent ⇒ no `generate_workflow` tool (no-op-when-absent).

- [ ] **Step 1: Write the failing test** — an end-to-end run where the lead calls `generate_workflow` once and a workflow unit is launched. Reuse the `run_agentiflow` test harness (mock lead provider scripted to emit a `generate_workflow` tool call; a mock `GenerationCapability.factory` returning a valid pool-only workflow; `RunAgentiflowOpts.unit_launcher` = a recording `MockUnitLauncher`). Assert: a `UnitKind::Workflow` unit with `workflow_file: Some(...)` and `engagement` = the flow's set was launched. Add a second assertion that with `generation: None`, no `generate_workflow` tool is advertised (e.g. the lead's tool list omits it — or the scripted call yields an "unknown tool" path).

- [ ] **Step 2: Run it, confirm it fails** — field/threading absent.

- [ ] **Step 3: Implement.** Add `pub generation: Option<crate::lead::GenerationCapability>,` to `RunAgentiflowOpts` (doc it like `make_provider`: production wiring in Plan 4; tests inject a mock). Destructure it in `run_agentiflow` and pass `run_dir.clone()` + `opts.generation.clone()` into the `fleet_unit_tools(...)` call at `:591`. Ensure `run_dir` (the agentiflow dir, already used for board/mailbox) is the value passed as the tool's materialization root. Update any other `fleet_unit_tools(...)` call site and all `RunAgentiflowOpts { … }` literals in tests to set `generation: None` by default.

- [ ] **Step 4: Run tests, confirm pass** — `cargo test -p rupu-agentiflow` (whole crate — the opts-struct change touches every test constructing `RunAgentiflowOpts`). Expected: green. Then `cargo clippy -p rupu-orchestrator -p rupu-cli -p rupu-agentiflow --all-targets -- -D warnings`.

- [ ] **Step 5: Commit** — `git add crates/rupu-agentiflow/src/run.rs && git commit -m "feat(agentiflow): thread generation capability into run_agentiflow (no-op when absent)"`

---

## Self-Review

**Spec coverage:** §23 "taught the full format" → Task 2; "stamp the active engagement profile" → Task 5 step 6 (`engagement = active_set` on the unit, the only stamping the model supports — see Global Constraints); "pool validation (incl. pool ⊇ dispatched agents)" → Task 5 step 4; Tier-2 lead-held tool (§13) → Task 5/6 (mounted on the lead's `extra_tools`, role-gating to pool orchestrators is a Plan-4 concern as with the other Tier-2 tools). §7 narrow-never-widen → structurally satisfied (no YAML engagement knob) + noted deferred for per-step narrowing.

**Placeholder scan:** none — every code step carries real signatures/bodies; test harness steps point at the exact existing rigs to reuse (`generate.rs:296`, `dispatch_tools.rs:941`, the `rupu-cli` serial suite) rather than inventing fixtures.

**Type consistency:** `generate_definition_with_provider(&GenerateRequest, &mut dyn LlmProvider)` (Task 1) is the exact entry Task 5 calls. `UnitSpec.workflow_file: Option<PathBuf>` (Task 4) is set by Task 5 and read by Task 4's argv. `GenerationCapability { provider, model, factory }` (Task 5, defined in `lead.rs`) is the type of `RunAgentiflowOpts.generation` (Task 6). `fleet_unit_tools` gains `(run_dir: PathBuf, generation: Option<GenerationCapability>)` in Task 5 and is called with them in Task 6 — both tasks must use that exact pair. The participant `Arc<AtomicU32>` shared across dispatch/join/run_workflow/generate_workflow is the same handle (Task 5 step 7).

**Verify-before-commit:** every task ends green on its own `cargo test -p <crate>` scope; Task 6 ends on the cross-crate clippy gate. The full workspace suite is the release gate on `main` (not required per-PR).
