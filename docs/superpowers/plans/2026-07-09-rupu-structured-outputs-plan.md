# rupu structured outputs (agent JSON schemas) — Implementation Plan

> **For agentic workers:** subagent-driven-development. Steps use `- [ ]`.

**Goal:** Make `outputFormat: json` agents produce *guaranteed* schema-conforming JSON via Anthropic structured outputs, by letting an agent carry a JSON Schema (`outputSchema`) that rupu emits as `output_config.format = {type:"json_schema", schema}`. Fixes the 400 (`output_config.format: Input does not match the expected shape`) properly, and gives the reviewer agents reliable JSON.

**Background (approved decisions):** Anthropic mandates a schema — `format.type` only accepts `"json_schema"`, `schema` required, no schema-less JSON mode (verified against platform.claude.com docs). So the correct fix carries a real schema. Schema is declared **inline** in agent frontmatter (`outputSchema:`) — keeps rupu's "agent = one self-contained `.md`" model; `serde_yaml` deserializes the mapping straight into `serde_json::Value`. The current HEAD already has the **floor** (commit 2b1ea9b, PR #469: no schema → don't emit the invalid `output_config.format`) — this plan builds the schema-carrying path on top, on the same branch.

## Scope
- **Part A (this PR, rupu repo):** `outputSchema` plumbing (`rupu-agent` spec + runner → `rupu-providers` request → `anthropic.rs` emission) + findings schemas for the 3 in-repo reviewers.
- **Part B (live config, NOT this PR — controller handles + matt reviews):** a canonical TVM findings JSON Schema derived from `~/Security/tvm_reporting_prompt.md`, referenced by `~/.rupu/agents/oracle-assessor.md`. oracle-assessor keeps writing prose reports (NOT structured-outputs). Documented here; delivered as live-config edits, not part of the repo PR.

## Global Constraints
- Correct Anthropic shape ONLY: `output_config.format = { "type": "json_schema", "schema": <the outputSchema value> }`. Emit it **only** when the request carries a schema; otherwise the #469 floor (no `output_config.format`). No schema-less JSON mode exists — never send a bare string.
- Backward compatible: an agent with `outputFormat: json` and NO `outputSchema` behaves exactly as today's floor (prompt-driven JSON, no `output_config.format`); an agent with neither is unchanged. The OpenAI/codex `text.format` path is untouched (leave it).
- `code-reviewer` / `review-diff` emit PROSE findings (no `outputFormat: json`) — do NOT give them schemas.
- Hexagonal: `rupu-agent` threads a schema value through the port; `rupu-providers` decides the wire shape per provider. `#![deny(clippy::all)]`; no `unsafe`; `thiserror`; workspace deps only. Per-file rustfmt only (never lib.rs/mod.rs; `--skip-children` absent in rustfmt 1.9.0 → hand-format). NOTE `rupu-providers` cold-compiles slowly (2min+).

## Grounded shapes (verified)
- `crates/rupu-agent/src/spec.rs`: `struct Frontmatter` (:29) has `#[serde(default, rename = "outputFormat")] output_format: Option<OutputFormat>` (:73); `pub struct AgentSpec` (:130) has `pub output_format` (:142), set from `fm.output_format` (:198).
- `crates/rupu-providers/src/types.rs`: request struct has `pub output_format: Option<OutputFormat>` (:134). MANY constructors across the crate init `output_format: None` (broker_client/local/provider/github_copilot/smart_router/task_classifier/broker_types/openai_codex, etc.) — a new sibling field needs the same `..: None` at each, OR use `#[serde(default)]` + `..Default::default()` where structs derive Default to minimize churn.
- `crates/rupu-providers/src/anthropic.rs` ~:1252-1276: builds `output_config` (currently only `task_budget`; `format` intentionally NOT emitted per the #469 floor, with a `TODO(structured-outputs)`). This is where the schema-carrying `format` object gets emitted.
- Reviewer output: `.rupu/agents/maintainability-reviewer.md` (:9 `outputFormat: json`; :26-36 the `{"findings":[{severity,title,…}]}` shape); same pattern in `security-reviewer.md` + `performance-reviewer.md`.
- Agent → request: the runner (`crates/rupu-agent/src/runner.rs`) builds the provider request from `AgentRunOpts`/spec; find where `output_format` is set on the request and set `output_schema` alongside (grep `output_format` in runner + spec-to-opts wiring).

---

## Task 1: `outputSchema` plumbing + correct Anthropic emission (rupu-agent + rupu-providers)

**Files:** `crates/rupu-agent/src/spec.rs`; `crates/rupu-agent/src/runner.rs` (+ wherever the request is built from the spec); `crates/rupu-providers/src/types.rs`; all request constructors that init `output_format`; `crates/rupu-providers/src/anthropic.rs`. Tests: spec.rs + anthropic.rs.

**Interfaces — Produces:** an `output_schema: Option<serde_json::Value>` on the provider request; `AgentSpec.output_schema: Option<serde_json::Value>`; `anthropic.rs` emits `output_config.format = {type:"json_schema", schema}` when present.

- [ ] **Step 1: Failing tests.**
  - `spec.rs`: `AgentSpec::parse` reads a frontmatter `outputSchema:` YAML mapping into `output_schema: Some(<serde_json::Value object>)`; absent → `None`.
  - `anthropic.rs`: build_body with `output_schema: Some(json!({"type":"object",...}))` → `body["output_config"]["format"] == json!({"type":"json_schema","schema":{"type":"object",...}})`. And with `output_schema: None` (even if `output_format: Some(Json)`) → NO `output_config.format` (the floor holds). And a schema + task_budget both present → both under output_config.
- [ ] **Step 2:** `cargo test -p rupu-agent -p rupu-providers --lib` → FAIL.
- [ ] **Step 3: Implement.**
  - `spec.rs`: add `#[serde(default, rename = "outputSchema")] output_schema: Option<serde_json::Value>` to `Frontmatter`; `pub output_schema: Option<serde_json::Value>` to `AgentSpec`; set from `fm.output_schema`.
  - `types.rs`: add `pub output_schema: Option<serde_json::Value>` to the request struct (next to `output_format`); add `output_schema: None` (or `..Default::default()`) to every constructor that inits `output_format`.
  - runner/spec-to-request wiring: set `output_schema` on the request from `spec.output_schema` wherever `output_format` is set.
  - `anthropic.rs`: in the `output_config` block, when `request.output_schema` is `Some(schema)`, insert `format = json!({ "type": "json_schema", "schema": schema })`. Keep `task_budget`. Keep the floor (no schema → no `format`). Update the comment/TODO.
- [ ] **Step 4:** tests pass; `cargo test -p rupu-agent -p rupu-providers --lib` green.
- [ ] **Step 5:** rustfmt the changed non-root files; `cargo clippy -p rupu-agent -p rupu-providers --no-deps`; commit `feat(providers,agent): outputSchema → Anthropic structured outputs (output_config.format json_schema)`.

## Task 2: Findings schemas for the in-repo reviewers (data)

**Files:** `.rupu/agents/maintainability-reviewer.md`, `.rupu/agents/security-reviewer.md`, `.rupu/agents/performance-reviewer.md`. No test (data), but the workflow/panel that parses their output is the consumer — keep the schema a faithful superset of what each prompt already documents.

- [ ] **Step 1:** For each reviewer, READ the JSON output shape its own prompt documents (e.g. maintainability's `{"findings":[{severity,title,…}]}` block, :26-36 — read the FULL block for every field it lists). Author an `outputSchema:` in the frontmatter that exactly matches: `type: object`, `properties: { findings: { type: array, items: { type: object, properties: {…each documented field with its type/enum…}, required: [...], additionalProperties: false } } }`, `required: [findings]`, `additionalProperties: false`. Severity is the documented enum (`low|medium|high|critical`). Do NOT invent fields the prompt doesn't produce; do NOT omit ones it does.
- [ ] **Step 2:** Sanity: the schema is valid JSON Schema (draft the API accepts) and its `required`/enums match the prompt. Confirm `rupu agent` still loads these files (`cargo test -p rupu-agent` covers spec parse; a malformed frontmatter would fail).
- [ ] **Step 3:** commit `feat(agents): findings outputSchema for maintainability/security/performance reviewers`.

---

## Part B (live config — controller-delivered, matt reviews; NOT in this PR)
Create `~/Security/tvm_finding.schema.json` — a canonical JSON Schema codifying the TVM finding fields from `~/Security/tvm_reporting_prompt.md` (Identifier, Owner, Product, Affected Component, Source Repository, Existing Ticket References [structured], Impact enum, Category, Likelihood enum, Description, Location {Input,Output}, Evidence, Remediation, Cross-References, References, CVSS v3 Base Score, plus the patch/detection/regression guidance fields). Edit `~/.rupu/agents/oracle-assessor.md` to reference it as the authoritative finding definition (prose reference — oracle-assessor keeps writing prose reports, NOT `outputFormat: json`). Present both to matt for review before finalizing (his security reporting standard). Faithful codification of the existing template — no invented fields.

## Self-Review
Coverage: engine (T1) + reviewer schemas (T2) + TVM schema/oracle-assessor (Part B, live). Correct Anthropic shape + floor preserved (T1 tests both). Backward compat: no-schema agents unchanged. Type flow: `outputSchema` → `AgentSpec.output_schema` → request `output_schema` → anthropic emission. code-reviewer/review-diff excluded (prose). 

## Execution
Subagent-driven on branch `fix-anthropic-output-config` (has the #469 floor). T1 → review → T2 → review → final whole-branch review → update PR #469 to the full structured-outputs fix (no self-merge; matt reviews). Part B delivered as live-config edits, presented to matt separately.
