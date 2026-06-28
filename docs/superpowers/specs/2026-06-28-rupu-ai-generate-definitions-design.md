# AI-generated agent & workflow definitions — design

- **Status:** Approved (2026-06-28)
- **Author:** rupu agent (paired with matt)
- **Scope:** Generate agent (`.md`) and workflow (`.yaml`) definition files from a
  natural-language description, using a configured provider/model, surfaced
  identically on the `rupu` CLI and the CP web control panel.

## 1. Motivation

Today an agent or workflow is created from a static template
(`AgentSpec` / `Workflow` scaffolds in `rupu-cli`) that the user must fill in by
hand. The template ships with placeholder fields — and unfilled placeholders are
a real failure mode: a workflow whose `agent:` field is still the template
comment parses to `null` and the autoflow loader rejects the whole file with
`step \`main\`: missing required field \`agent\``.

Letting a model draft the file from a description removes the blank-page step and,
for workflows, structurally avoids the empty-`agent:` class of error by feeding
the model the live list of real agent names to reference.

The user wants this on **two surfaces**:

1. The CLI `create` commands (a `--describe` flag).
2. The CP web pages that already create agents/workflows (a "describe it" option
   in the existing create modals).

And in both cases the user keeps control over **scope** (global vs project) and
**host** (when more than one host is available — see §7).

## 2. Goals / non-goals

**Goals**

- A single generation core that, given a kind + description + provider/model,
  returns **validated** file content or fails loudly. No silent half-output.
- Smart default model selection (first authed provider, strong pinned model),
  with explicit override.
- CLI `--describe` on `agent create` and `workflow create`, preserving the
  existing post-create UX (write → open `$EDITOR` → re-validate on save).
- CP "describe it" mode in the existing `NewAgentModal` / `NewWorkflowModal`,
  pre-filling the existing code editor for review before the user clicks Create.
- Scope selection reused from existing helpers; host selection wired as a
  dormant-but-ready seam.

**Non-goals**

- Multi-host execution. Host selection resolves to `local` until the multi-host
  slice lands (see `docs/superpowers/specs/2026-06-28-rupu-multi-host-slice-1-design.md`).
- Editing/refactoring an existing definition with AI. This is create-only.
- Generating anything other than agent and workflow definition files.
- A bespoke prompt-engineering UI (temperature, system-prompt tweaking, etc.).

## 3. The generation core (`rupu_runtime::generate`)

Lives in `rupu-runtime` because that crate already owns `provider_factory`
(`build_for_provider`) and depends on `rupu-providers` / `rupu-auth`. Keeping the
core here means `rupu-cp` does **not** grow a provider/credential dependency — it
reaches generation through an adapter port (§5), consistent with the existing
`RunLauncher` / `SessionStarter` pattern.

### 3.1 Public API

```rust
pub enum GenKind { Agent, Workflow }

pub struct GenerateRequest {
    pub kind: GenKind,
    pub description: String,
    pub provider: String,
    pub model: String,
    /// Real agent names available in scope. Empty for agents; for
    /// workflows this is injected into the prompt so generated steps
    /// reference agents that actually exist.
    pub available_agents: Vec<String>,
}

pub struct GenerateOutcome {
    pub content: String,   // validated .md or .yaml, ready to write
    pub provider: String,
    pub model: String,
    pub attempts: u8,      // 1 = first try valid; >1 = needed repair
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
    Invalid { kind: &'static str, attempts: u8, last_error: String },
}

/// One-shot generation with a bounded validate→repair loop.
pub async fn generate_definition(
    req: &GenerateRequest,
    resolver: &dyn rupu_auth::CredentialResolver,
) -> Result<GenerateOutcome, GenerateError>;

/// Preference-ordered pick of the first authenticated provider and a
/// strong pinned model for it. Returns None when nothing is authed.
pub async fn pick_default_gen_model(
    resolver: &dyn rupu_auth::CredentialResolver,
) -> Option<(String, String)>;
```

### 3.2 Algorithm

1. Build a **system prompt** that:
   - States the target format (agent = YAML frontmatter + Markdown body;
     workflow = YAML matching the `Workflow` schema).
   - Embeds a compact field cheat-sheet derived from the canonical templates and
     the orchestrator's parse rules (linear/`for_each` steps require `agent:` and
     `prompt:`; `panel`/`parallel` shapes; common optional fields). The cheat-sheet
     lives next to the templates so it stays in sync.
   - For workflows, lists the available agent names and instructs the model to use
     only those for `agent:` / panelist references.
   - Instructs: **output only the file content**, no Markdown fences, no prose.
2. `build_for_provider(provider, model, None, resolver)` → `LlmProvider::send`
   with the description as the user message.
3. Post-process: trim, strip a wrapping ```` ```lang … ``` ```` fence if present.
4. **Validate**: `AgentSpec::parse` or `Workflow::parse`.
5. On parse failure, if `attempts < MAX_ATTEMPTS` (default **3** total = 1 + 2
   repairs), re-send with the prior output + the parse error appended
   ("your previous output failed to parse: <err>; return the corrected full file")
   and loop to step 3.
6. Return `GenerateOutcome` on success; `GenerateError::Invalid` once attempts are
   exhausted, carrying the last parse error.

`MAX_ATTEMPTS` is a module constant; the repair loop is the core guarantee that a
returned file is parseable.

### 3.3 Default-model preference order

`pick_default_gen_model` checks, in order, `anthropic` → `openai` → `gemini` →
`copilot`, via `resolver.peek(pid, ApiKey)` (and the SSO peek where relevant). The
first authed provider wins, paired with a strong pinned model constant per
provider (e.g. anthropic → `claude-opus-4-8`). The pinned models live in one table
so they are easy to bump.

## 4. CLI surface

Extend the existing `Create` actions in `crates/rupu-cli/src/cmd/agent.rs` and
`crates/rupu-cli/src/cmd/workflow.rs`:

```
rupu agent create   [name] --describe "<text>" [--scope global|project]
                           [--gen-provider <p>] [--gen-model <m>] [--host <h>]
rupu workflow create [name] --describe "<text>" [--scope global|project]
                           [--gen-provider <p>] [--gen-model <m>] [--host <h>]
```

Behavior with `--describe`:

1. Resolve **scope** and **name** exactly as today (prompt when omitted via
   `create_common::prompt_scope` / `prompt_name`).
2. Resolve **host** (§7) — default/only `local` today.
3. Resolve the generation model: `--gen-provider`/`--gen-model` if given, else
   `pick_default_gen_model`. None authed → exit with the `auth login` hint.
4. For workflows, gather available agent names (`load_agents`, project + global).
5. Call `generate_definition`. On `GenerateError`, exit non-zero with the message.
6. Write the validated content to the target path (`create_common::target_dir`),
   print `generated <path> (<scope>) via <provider>/<model>`, then open it in
   `$EDITOR` and re-validate on save — **identical** to the manual create tail, so
   the existing editor/validate code is reused unchanged.

Without `--describe` the commands behave exactly as today (static template). The
CLI stays thin: it does arg-parsing, scope/name/host resolution, and delegates the
model call to `rupu_runtime::generate`.

### 4.1 Test seam

To unit-test the CLI path without a live provider, the generation call is reached
through a small indirection (a function pointer / trait object the tests can swap
for a stub returning canned content). The default wiring calls
`generate_definition`.

## 5. CP backend

### 5.1 Adapter port

CP reaches generation through a new port, mirroring the existing optional adapters
(`RunLauncher`, `AgentLauncher`, `SessionStarter`, …) that are `Some` only under
`rupu cp serve`:

```rust
#[async_trait]
pub trait DefinitionGenerator: Send + Sync {
    async fn generate(&self, req: GenerateRequestDto)
        -> Result<GenerateOutcomeDto, GenError>;
    /// Providers/models the user can choose from (authed only) — backs
    /// the CP dropdown and marks the default.
    async fn available_models(&self) -> Vec<ProviderModelsDto>;
}
```

Stored on `AppState` as `Option<Arc<dyn DefinitionGenerator>>`. When `None` (bare
read-only `rupu-cp`), the generate endpoints return **501 Not Available**, exactly
like `start_session` does today.

The concrete impl is wired in `crates/rupu-cli/src/cmd/cp.rs` (the full runtime),
calling `rupu_runtime::generate::generate_definition` with the real
`CredentialResolver`, and `pick_default_gen_model` + the model registry for
`available_models`.

### 5.2 Routes

Added to `crates/rupu-cp/src/api/agents.rs` and `…/workflows.rs`:

- `POST /api/agents/generate` — body
  `{ description, provider?, model? }` → `{ raw, provider, model, attempts }`.
- `POST /api/workflows/generate` — same shape; the handler gathers available
  agent names server-side and passes them into the request.
- `GET /api/generate/models` — `[{ provider, models: [...], default: bool }]` for
  the dropdown.

Generation endpoints **do not write files.** They return content; the existing
`POST /api/agents` / `POST /api/workflows` write endpoints persist the file after
the user reviews and clicks Create. This keeps generate and write cleanly
separated and gives the review step for free.

## 6. CP frontend

Extend the existing `NewAgentModal` (`web/src/pages/Agents.tsx`) and
`NewWorkflowModal` (`web/src/pages/Workflows.tsx`):

- Add a mode toggle at the top of the modal: **Describe** ⇄ **Edit raw**.
- **Describe** mode shows: a description `<textarea>`, a model dropdown populated
  from `GET /api/generate/models` (default pre-selected), a host selector that
  renders only when more than one host is available (§7), and a ✨ **Generate**
  button.
- Generate calls `api.generateAgent({ description, provider, model })` /
  `api.generateWorkflow(...)`, then **switches the modal to Edit-raw mode with the
  returned content loaded into the existing `CodeEditor`**. The user reviews/edits
  and clicks **Create**, which hits the unchanged write path.
- Errors (501 / no-credentials / invalid) surface inline in the modal.

New API client methods in `web/src/lib/api.ts`: `generateAgent`,
`generateWorkflow`, `generateModels`. No change to `createAgent` / `createWorkflow`.

## 7. Scope and host

- **Scope** reuses `create_common::prompt_scope` / `target_dir` (CLI) and the
  modal's existing scope handling (CP). No new mechanism.
- **Host**: a `--host` flag (CLI) and a selector (CP) are added now but resolve
  against the host registry, which today contains only the implicit `local`
  host. The selector is **hidden/skipped when only one host exists**, matching the
  user's "if more than one is available" requirement. When the multi-host slice
  ships, the same selector begins offering remote hosts; the generated file still
  lands locally (generation is a local authoring action) — host attribution is for
  where the definition is *registered*, handled by that future slice. No host
  execution logic is added here.

## 8. Error handling

| Condition | Core | CLI | CP |
|---|---|---|---|
| No authed provider | `NoCredentials` | non-zero exit + `auth login` hint | `GET /generate/models` empty; modal shows "connect a provider"; generate → 4xx |
| Provider/network error | `Provider(..)` | non-zero exit, message | 502/4xx, inline error |
| Empty model output | `Empty` | non-zero exit | 4xx, inline error |
| Unparseable after repairs | `Invalid{..}` | non-zero exit, shows last parse error | 4xx with parse error, inline |
| Generator adapter absent (bare `rupu-cp`) | n/a | n/a | **501 Not Available** |

In every failure case nothing is written. On the CLI, partial files are never left
behind; on CP, generate never writes, so there is nothing to clean up.

## 9. Testing

- **Core (`rupu-runtime`)**, with a `MockProvider`:
  - valid first-try output → `attempts == 1`, content passed through;
  - invalid-then-valid → repair loop runs, `attempts == 2`, valid content;
  - always-invalid → `GenerateError::Invalid` after `MAX_ATTEMPTS`;
  - fence-wrapped output is stripped before validation;
  - workflow request injects the agent-name list into the prompt;
  - `pick_default_gen_model` honors preference order and returns `None` when
    nothing is authed (mock resolver).
- **CLI**: `--describe` with a stub generator writes a file that re-parses; without
  `--describe` the manual template path is unchanged.
- **CP backend**: handler test with a stub `DefinitionGenerator` (200 + body);
  `None` adapter → 501; `available_models` shape.
- **Frontend (vitest)**: modal toggle renders Describe/Edit; Generate populates the
  editor from a mocked API; error states render.

## 10. Build / release

No new build step. The React changes flow through the existing `make cp-web` →
`web/dist` → `rust-embed` path; the spec's CLI/runtime changes are ordinary Rust.
The implementation plan must remember `make cp-web` before `make release` so the
embedded UI is current.

## 11. Implementation slices (for the plan)

1. **Core** — `rupu_runtime::generate` (API, prompt builder, repair loop,
   default-model picker) + unit tests. No surface yet.
2. **CLI** — `--describe` on `agent create` / `workflow create`, gen-model flags,
   host flag (local-only), test seam + tests.
3. **CP backend** — `DefinitionGenerator` port, `cp serve` wiring, the three
   routes, 501-when-absent, handler tests.
4. **CP frontend** — modal Describe/Edit toggle, model dropdown, host seam, API
   client methods, vitest.

Each slice is independently reviewable; slice 1 is a prerequisite for 2–4.
