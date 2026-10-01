# Engagement profiles — Plan 2 (wiring + end-to-end) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the Plan 1 engagement-profile contract into the running system so a `binary` engagement can be selected, its findings validated against the `binary` profile (per-origin routed), its assets persisted, and a real binary workflow run end-to-end — with `code` behavior unchanged.

**Architecture:** The active profile set rides inside `FindingWriteOptions` (already carried through every tool path), so selection threads the same route `FindingProfile` already does: CLI flag / workflow `defaults` / agent frontmatter, resolved most-specific-first. `report_finding` resolves a finding's *owning* profile from its asset-kind namespace (per-origin, not the composite union) and gates it on that profile's completeness. Assets persist as an append-only `assets.jsonl` in the workspace-scoped `CoveragePaths`, folded on read. Remote/CP delivery and composites-at-scale are deliberately out of scope (fail-closed refused).

**Tech Stack:** Rust 2021, serde, toml, tokio, thiserror. TDD with `#[test]` + mock-provider integration (`MockProvider`/`BypassDecider` from `rupu_agent::runner`).

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-engagement-profiles-asset-model-design.md`
**Builds on:** Plan 1 contract (PR Section9Labs/rupu#687) — must be merged to `main` first.

## Global Constraints

- MSRV pinned; `#![deny(clippy::all)]`/`#![forbid(unsafe_code)]` workspace-wide; workspace deps only.
- **Additive & backwards-compatible.** New fields on `FindingWriteOptions`, `ReportFindingInput`, `FindingRecord`, `RunManifest`, `Frontmatter`/`AgentSpec`, `WorkflowDefaults`/`Step` are `#[serde(default, skip_serializing_if=…)]`; `code` runs behave identically. Legacy records/frontmatter/YAML still load.
- **The schema lockstep stays untouched:** put the asset on `ReportFindingInput`/`FindingRecord`, NOT inside `FindingReport` — so `schema/finding_report.schema.json` ↔ `validate_report` ↔ `report_schema_lockstep` need no change. Profile completeness is a *separate* gate run after `validate_report`.
- **Per-origin routing (matt's decision):** a finding validates against the profile that OWNS its asset kind (by namespace), never a composite's merged union. Agents may set an explicit `engagement_profile` to disambiguate; inference-by-kind is the default, explicit wins.
- **Resolution precedence** mirrors `FindingProfile::resolve`: step → workflow `defaults` → agent frontmatter → run/session flag → built-in default (`code`). An agent's `engagementProfiles` is the agent slot (3rd), NOT a strict disjoint-check (that would reject every binary agent under a `code` default).
- **Fail-closed, resolved at LAUNCH (not `Workflow::parse`, which is pure):** unknown profile id, kind owned by no active profile, a step widening beyond the run set, or a malformed selected profile from `discover` → hard error at launch/first-write.
- **Deferred to a later plan (refuse, don't half-wire):** remote/placed/CP delivery of a non-default engagement set (refuse on `host:`/`distribute:` units — `AgentLaunchRequest`/connectors/capability untouched); the CP web workflow-editor engagement field. A `rupu run`/`session` CLI flag is NOT persisted into resume (resume rebuilds from `workflow.yaml`), so workflow-level selection is YAML-only.
- No assessment-derived sample data (public repo) — the sample binary agent/workflow are synthetic.
- Gate before each commit: `cargo test` for the touched crate(s) + `cargo clippy -p <crate> --all-targets -- -D warnings`; a workspace `--locked` check at the end.

---

## File Structure

**`crates/rupu-coverage/`**
- `src/report/options.rs` — add `engagement: Option<Arc<ActiveSet>>` + `with_engagement()`.
- `src/profile/registry.rs` — per-origin routing (preserve sub-profile namespace); derive `Debug/Clone/PartialEq/Eq` on `ActiveSet`/owned profile data.
- `src/profile/mod.rs` — `EngagementProfile::resolve(...) -> Vec<String>`; `builtin_profiles()` + overlay-with-`discover` constructor (fail-closed).
- `src/profile/predicate.rs` — add `unsatisfied(checks, report, locator) -> Result<Vec<&CompletenessCheck>, PredicateError>`.
- `src/asset/graph.rs` — add `iter()` + `from_assets()`.
- Create `src/asset/store.rs` — append/fold `assets.jsonl` (`AssetLine { asset, declared_by }`).
- `src/ledger/paths.rs` — add `assets` path.
- `src/ledger/events.rs` — `FindingRecord.asset: Option<AssetRef>`; `AssetRef { id, kind }`.
- `src/ledger/manifest.rs` — `RunManifest.engagement_profiles: Vec<String>`.
- `src/tools/report_finding.rs` — `ReportFindingInput.asset`; kind resolution + per-origin routing + completeness gate + asset upsert.
- Create `src/tools/asset_mark.rs` — register asset / set depth; `src/tools/mod.rs` + `lib.rs` exports.

**`crates/rupu-agent/`** — `src/spec.rs` (`engagementProfiles`), `src/coverage_tools.rs` (asset in schema + asset tool + register), `src/runner.rs` (guidance + register-without-concerns).

**`crates/rupu-orchestrator/`** — `src/workflow.rs` (`defaults.engagement_profiles`, `Step.engagement_profiles`, legality, `action_engagement`), `src/step_factory.rs` (narrow), `src/runner.rs` (pass + fail-closed remote guard).

**`crates/rupu-mcp/`** — `src/tools/findings.rs` (asset arg), `src/dispatcher.rs` (narrowed set), bless `tests/snapshots/tools_list.json`.

**`crates/rupu-cli/`** — `src/cmd/run.rs` + new `src/engagement_opts.rs` + `src/cmd/session.rs` + `src/cmd/dispatch.rs`.

**Docs/samples** — `.rupu/agents/binary-analyst.md`, `.rupu/workflows/binary-assessment.yaml` (run-time samples), `docs/{coverage,workflow-format,agent-format}.md`.

---

## Task 1: `engagement` in FindingWriteOptions + ActiveSet derives

**Files:** Modify `rupu-coverage/src/report/options.rs`, `src/profile/registry.rs`. Test: inline.

**Interfaces:**
- Produces: `FindingWriteOptions.engagement: Option<std::sync::Arc<crate::profile::ActiveSet>>`, `FindingWriteOptions::with_engagement(self, Option<Arc<ActiveSet>>) -> Self`. `ActiveSet` + its owned `EngagementProfile` data derive `Debug, Clone, PartialEq, Eq` (so `FindingWriteOptions`'s existing `Eq`/`PartialEq` still hold).

- [ ] **Step 1: failing test** — in registry.rs assert `ActiveSet` is `Clone + PartialEq`; in options.rs assert `FindingWriteOptions::default().with_engagement(Some(set)).engagement.is_some()` and that two equal options compare equal.
- [ ] **Step 2: run, expect fail** (`ActiveSet: !Clone`, no field).
- [ ] **Step 3: implement.** Add `#[derive(Debug, Clone, PartialEq, Eq)]` to `ActiveSet` and any owned struct it holds that lacks them (`EngagementProfile`, `AssetKindDef`, `CoverageSpec`, `Bundle`, `CompletenessCheck`, `Predicate` already derive `PartialEq, Eq`; add `Clone` where missing). Add to `FindingWriteOptions`:
  ```rust
  #[serde(skip)]                                   // runtime-only; never serialized
  pub engagement: Option<std::sync::Arc<crate::profile::ActiveSet>>,
  ```
  `#[serde(skip)]` keeps the wire form of options unchanged. Add the builder:
  ```rust
  pub fn with_engagement(mut self, e: Option<std::sync::Arc<crate::profile::ActiveSet>>) -> Self { self.engagement = e; self }
  ```
  `Arc<ActiveSet>` keeps `Clone` cheap and `PartialEq` by pointer-or-value (derive compares `Option<Arc<_>>` by value — fine).
- [ ] **Step 4: run, expect pass.** `cargo test -p rupu-coverage report::options profile::registry`
- [ ] **Step 5: commit** `feat(coverage): carry active engagement set in FindingWriteOptions`

---

## Task 2: Per-origin routing + resolve + discover overlay

**Files:** Modify `rupu-coverage/src/profile/registry.rs`, `src/profile/mod.rs`, `src/profile/loader.rs`. Test: inline.

**Interfaces:**
- Consumes: `expand_includes` (Plan 1).
- Produces: registry preserves each included profile's own namespace (an included `service` from `network` stays `network:service` even inside composite `pentest`), so `ActiveSet::profile_for_kind("network:service")` resolves to the `network` profile whether `network` or `pentest` is the active id. `EngagementProfile::resolve(step: Option<&str>, workflow: &[String], agent: &[String]) -> Vec<String>` (most-specific non-empty wins; empty → `[DEFAULT_PROFILE]`). `fn registry_with_overlay(global_dir, project_dir) -> Result<ProfileRegistry, RegistryError>` folding built-ins under `discover` overlays, failing closed if a discovered file errored.

- [ ] **Step 1: failing test** — `composite_routes_to_origin_subprofile`:
  ```rust
  // network{service}, web{route}, pentest=includes[network,web]
  let reg = ProfileRegistry::from_profiles(map).unwrap();
  let set = reg.active_set(&["pentest".into()]).unwrap();
  assert_eq!(set.profile_for_kind("network:service").unwrap().id, "network");
  assert_eq!(set.profile_for_kind("web:route").unwrap().id, "web");
  ```
  Plus `resolve_prefers_most_specific` and `overlay_fails_closed_on_bad_profile`.
- [ ] **Step 2: run, expect fail** (today composite kinds become `pentest:service`).
- [ ] **Step 3: implement.** In `from_profiles`, when flattening a composite, namespace each kind by the **profile it originated from**, not `flat.id`. `expand_includes` currently loses origin; extend it to tag each merged `AssetKindDef` with an `origin: String` (the source profile id), then `from_profiles` namespaces `origin:kind` and registers every origin sub-profile as individually routable (store sub-profiles in the registry map, not only the composite). `active_set(["pentest"])` expands `pentest` to the set of origin ids `{network, web}` for routing purposes while keeping `pentest` as the selected label. `profile_for_kind` keys on the namespace → origin profile. Add `EngagementProfile::resolve` and `registry_with_overlay` (discover → union over built-ins → on a selected id whose discovered source errored, return `RegistryError`).
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): per-origin kind routing + engagement resolve + discovery overlay`

---

## Task 3: `assets.jsonl` store in CoveragePaths

**Files:** Modify `rupu-coverage/src/ledger/paths.rs`, `src/asset/graph.rs`; create `src/asset/store.rs`; export in `asset/mod.rs`. Test: inline + a tempdir round-trip.

**Interfaces:**
- Consumes: `Asset`, `AssetGraph` (Plan 1), `Attribution` (ledger).
- Produces: `CoveragePaths.assets: PathBuf` (= `<root>/assets.jsonl`); `AssetGraph::iter()`, `AssetGraph::from_assets(impl IntoIterator<Item=Asset>)`; `asset::store::{append_asset(paths, &Asset, &Attribution) -> io::Result<()>, read_asset_graph(paths) -> AssetGraph}`. Line shape: `AssetLine { #[serde(flatten)] asset: Asset, declared_by: Attribution }`, append-only, folded on read via `AssetGraph::insert` (last-write-wins — order independent, safe for parallel `for_each` writers).

- [ ] **Step 1: failing test** — write two assets (one re-inserted with a new depth) via `append_asset` to a tempdir; `read_asset_graph` returns a graph whose re-inserted node has the latest depth and whose roots/children match.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement.** `paths.rs`: `assets: root.join("assets.jsonl")` in `CoveragePaths::new`. `graph.rs`: `pub fn iter(&self) -> impl Iterator<Item=&Asset>` (over `order`→`nodes`), `pub fn from_assets(it) -> Self` (insert each). `store.rs`: append one JSON line (create-dir-all + `OpenOptions::append`), and `read_asset_graph` (missing file → empty graph; fold each line via `insert`).
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): append-only asset store (assets.jsonl) folded on read`

---

## Task 4: asset fields on ReportFindingInput + FindingRecord

**Files:** Modify `rupu-coverage/src/tools/report_finding.rs` (input), `src/ledger/events.rs` (`FindingRecord`, `AssetRef`). Test: inline serde.

**Interfaces:**
- Produces: `AssetInput { kind: String, locator: crate::asset::Locator, #[serde(default)] parent: Option<String>, #[serde(default)] label: Option<String> }`; `ReportFindingInput.asset: Option<AssetInput>` (`#[serde(default, skip_serializing_if=Option::is_none)]`); `AssetRef { id: String, kind: String }`; `FindingRecord.asset: Option<AssetRef>` (`#[serde(default, skip_serializing_if=Option::is_none)]`). `FindingRecordWire` + `From` kept in step.

- [ ] **Step 1: failing test** — a `ReportFindingInput` JSON with an `asset` round-trips; a legacy `FindingRecord` line without `asset` deserializes to `asset: None`; a record with `asset` serializes it.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement.** Add the structs/fields with the serde attrs above; update `FindingRecordWire`/`From` if present (keep byte-compat for legacy). ~7 `ReportFindingInput` + ~30 `FindingRecord` literals exist (mostly tests) — all compile via `..Default::default()`/field add since new fields default.
- [ ] **Step 4: run, expect pass** incl. existing `report_schema_lockstep` (unchanged — asset is not on `FindingReport`).
- [ ] **Step 5: commit** `feat(coverage): optional asset on ReportFindingInput + FindingRecord`

---

## Task 5: profile-aware recording (routing + completeness gate + asset upsert)

**Files:** Modify `rupu-coverage/src/tools/report_finding.rs`, `src/profile/predicate.rs`. Test: inline.

**Interfaces:**
- Consumes: Tasks 1–4; `ActiveSet::profile_for_kind`; `predicate::score`.
- Produces: `predicate::unsatisfied(checks: &[CompletenessCheck], r: &FindingReport, loc: &Locator) -> Result<Vec<String>, PredicateError>` (ids of required+unsatisfied checks). In `report_finding`: when `opts.engagement` is `Some`, (a) resolve the finding's kind — explicit `input.asset.kind`, else map legacy `scope`/`file_path` to `code:file`; (b) `profile_for_kind(kind)` → reject with a clear error naming the active ids if `None`; (c) under `Full`, append to `problems` any unsatisfied profile completeness checks + any evidence block kind / classification system not permitted by the profile; (d) upsert the asset to `assets.jsonl` (`append_asset`) and stamp `record.asset = AssetRef`. When `opts.engagement` is `None` (today's `code` path), behavior is unchanged.

- [ ] **Step 1: failing tests** — with a `binary`-only `ActiveSet`: a finding whose `asset.kind="binary:function"` and evidence has a `disasm` block PASSES; the same finding with no disasm block FAILS with `evidence_has_listing` in the error; a finding with `asset.kind="web:route"` (not in the active set) is REJECTED naming the active profiles; a finding with no asset under a `code` active set maps to `code:file` and passes (code completeness empty). Assert the asset line was appended.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** `unsatisfied()` + the gate + routing + upsert, guarded by `if let Some(engagement) = &opts.engagement`. Keep all existing `validate_report` logic and the `problems` aggregation pattern (one error listing everything).
- [ ] **Step 4: run, expect pass** + full `cargo test -p rupu-coverage`.
- [ ] **Step 5: commit** `feat(coverage): per-origin profile routing + completeness gate + asset upsert in report_finding`

---

## Task 6: `asset_mark` coverage tool

**Files:** Create `rupu-coverage/src/tools/asset_mark.rs`; modify `src/tools/mod.rs`, `src/lib.rs`. Test: inline.

**Interfaces:**
- Produces: `AssetMarkInput { kind, locator, #[serde(default)] parent, #[serde(default)] label, depth: String }`, `asset_mark(paths, attribution, input, &FindingWriteOptions) -> Result<AssetMarkOutput, AssetMarkError>`: validates `kind` is owned by the active set and `depth` is in that profile's `coverage.depth_ladder` (fail-closed), then upserts the asset with that depth. Mirrors `report_finding`'s no-`concerns:` registration path (binary runs have no catalog).

- [ ] **Step 1: failing test** — marking `binary:function` depth `analyzed` succeeds and the stored asset carries it; an unknown depth or a kind outside the active set errors.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** + re-export from `lib.rs`.
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): asset_mark tool (coverage depth for assets)`

---

## Task 7: agent frontmatter + tool registration + guidance

**Files:** Modify `rupu-agent/src/spec.rs`, `src/coverage_tools.rs`, `src/runner.rs`. Test: inline + a spec-parse test.

**Interfaces:**
- Produces: `Frontmatter.engagement_profiles` (`#[serde(default, rename="engagementProfiles")]` `Vec<String>`), `AgentSpec.engagement_profiles`; `coverage_tools`: `asset` advertised in `summary_schema`/`full_schema`, the new `asset_mark` tool registered, and `register`/the no-`concerns:` path (runner.rs:1085-1110) register asset tooling when `tools:` lists `report_finding`/`asset_mark`; `report::guidance` gains an engagement section (kinds/coordinates/blocks/systems/ladder) when `opts.engagement` is set.

- [ ] **Step 1: failing test** — an agent `.md` with `engagementProfiles: [binary]` parses to `AgentSpec.engagement_profiles == ["binary"]`; `deny_unknown_fields` still rejects a typo.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the frontmatter field + thread into `AgentSpec` (3 literals), advertise `asset` in the tool schemas, register `asset_mark`, add the guidance section.
- [ ] **Step 4: run, expect pass** `cargo test -p rupu-agent`.
- [ ] **Step 5: commit** `feat(agent): engagementProfiles frontmatter + asset tool registration + guidance`

---

## Task 8: CLI selection (`rupu run` / session / dispatch)

**Files:** Modify `rupu-cli/src/cmd/run.rs`, `src/cmd/session.rs`, `src/cmd/dispatch.rs`; create `src/engagement_opts.rs`. Test: CLI parse + resolution.

**Interfaces:**
- Consumes: `EngagementProfile::resolve`, `registry_with_overlay`, `FindingWriteOptions::with_engagement`.
- Produces: `rupu run --engagement-profile <id>` / `--engagement-profiles a,b` (`Args.engagement_profiles: Vec<String>`, raw-argv pre-pass already re-parses `Args`); `engagement_opts::active_set(global, project_root, selected) -> Result<Option<Arc<ActiveSet>>>` (None when selected resolves to just `code`, so the `code` path keeps `engagement: None` and is byte-identical); stored via `.with_engagement(..)` into the `ToolContext.findings` built at run.rs:848 and cloned into `CliAgentDispatcher.findings_base` (so sub-agents inherit). Session: `StartArgs` flag → snapshot into `SessionRecord.engagement_profiles` → rebuilt each turn in `run_turn` (session.rs:7632).

- [ ] **Step 1: failing test** — `Args` parses `--engagement-profiles binary,web`; `engagement_opts::active_set` with `[]`/`["code"]` returns `None`; with `["binary"]` returns `Some` whose `profile_for_kind("binary:function")` is `binary`; an unknown id errors.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the flags, `engagement_opts.rs`, and the wiring at the `FindingWriteOptions` construction sites (run.rs:848/814; session start + `run_turn`; dispatch child ctx inherits from `findings_base`).
- [ ] **Step 4: run, expect pass** `cargo test -p rupu-cli`.
- [ ] **Step 5: commit** `feat(cli): --engagement-profile(s) on run + session; sub-agents inherit`

---

## Task 9: workflow selection + step narrowing + fail-closed guards

**Files:** Modify `rupu-orchestrator/src/workflow.rs`, `src/step_factory.rs`, `src/runner.rs`. Test: inline + a workflow-parse test.

**Interfaces:**
- Produces: `WorkflowDefaults.engagement_profiles: Vec<String>` + `Step.engagement_profiles: Vec<String>` (both `#[serde(default)]`); `Step` legality mirroring the `findings_profile` block (workflow.rs:1877-1920); `DefaultStepFactory::build_opts_for_step` resolves the set (step narrows the workflow set — narrow-only; a widening step uses the existing `agent_load_error_stub` path since `build_opts_for_step` is infallible) and calls `.with_engagement(..)`; `Workflow::action_engagement` for an `action: findings.record` step; `runner.rs` passes the narrowed set at the three `execute_action_step` sites (4725/5594/5841) and **refuses a non-default engagement set on remote/placed units** at 6194/7053/7131 (fail-closed — remote delivery deferred).

- [ ] **Step 1: failing test** — a workflow YAML with `defaults.engagement_profiles: [binary]` resolves a step's options to a binary `ActiveSet`; a step narrowing to `[code]` works; a step widening to `[web]` beyond the run set errors at launch.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** the YAML fields + legality + `step_factory` narrow + `action_engagement` + the remote-refusal guard.
- [ ] **Step 4: run, expect pass** `cargo test -p rupu-orchestrator`.
- [ ] **Step 5: commit** `feat(orchestrator): workflow engagement_profiles + step narrowing + fail-closed remote guard`

---

## Task 10: MCP `findings.record` asset arg + dispatcher

**Files:** Modify `rupu-mcp/src/tools/findings.rs`, `src/dispatcher.rs`; bless `tests/snapshots/tools_list.json`. Test: inline + snapshot.

**Interfaces:**
- Produces: `RecordArgs.asset: Option<AssetInput>` forwarded into `ReportFindingInput`; `dispatcher` carries the per-step narrowed engagement set alongside the findings profile (generalize `call_with_findings_profile`); the FindingsContext already carries `options`, so the engagement rides in `options.engagement` — no `FindingsContext` struct change.

- [ ] **Step 1: failing test** — `findings.record` accepts an `asset` arg and routes it through; the tools-list snapshot reflects the new optional arg.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** + `BLESS=1 cargo test -p rupu-mcp --test schema_snapshot`.
- [ ] **Step 4: run, expect pass** `cargo test -p rupu-mcp`.
- [ ] **Step 5: commit** `feat(mcp): asset arg on findings.record + engagement-aware dispatch`

---

## Task 11: RunManifest engagement + rerun

**Files:** Modify `rupu-coverage/src/ledger/manifest.rs`, `src/rerun.rs`. Test: inline.

**Interfaces:**
- Produces: `RunManifest.engagement_profiles: Vec<String>` (`#[serde(default)]`); `plan_rerun` carries the engagement selection forward so a rerun uses the same profiles.

- [ ] **Step 1: failing test** — a manifest without the field loads (`[]`); `plan_rerun` preserves a recorded engagement selection.
- [ ] **Step 2: run, expect fail.**
- [ ] **Step 3: implement** (8 `RunManifest` literals via field-add/default).
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `feat(coverage): record engagement profiles in RunManifest + rerun`

---

## Task 12: End-to-end binary engagement

**Files:** Create `.rupu/agents/binary-analyst.md`, `.rupu/workflows/binary-assessment.yaml`; create an integration test (`rupu-agent/tests/engagement_binary_e2e.rs` using `MockProvider`/`BypassDecider`). Test: the integration test IS the deliverable.

**Interfaces:**
- Consumes: the whole stack.
- Produces: a synthetic `binary-analyst` agent (`engagementProfiles: [binary]`, `tools: [report_finding, asset_mark]`) and a `binary-assessment` workflow (`defaults.engagement_profiles: [binary]`); an integration test that runs the agent against a `MockProvider` scripted to call `report_finding` with a `binary:function` asset + a `disasm` evidence block, and asserts: the finding is recorded, routed to the `binary` profile, passes completeness, `record.asset` is stamped, and `assets.jsonl` holds the function asset. A negative case (missing disasm) asserts the completeness rejection.

- [ ] **Step 1: write the failing integration test** (scripted MockProvider turn emitting the report_finding call; assert on the ledger + assets.jsonl).
- [ ] **Step 2: run, expect fail** (asset not persisted / routing not wired until the agent+workflow+config exist).
- [ ] **Step 3: author the sample agent + workflow; wire any missing config discovery so a `binary` engagement activates end-to-end.**
- [ ] **Step 4: run, expect pass.**
- [ ] **Step 5: commit** `test(agent): end-to-end binary engagement — routed finding + persisted asset`

---

## Task 13: Docs

**Files:** Modify `docs/coverage.md`, `docs/workflow-format.md`, `docs/agent-format.md`, `CLAUDE.md`. Test: none (docs); verify links/anchors.

**Interfaces:** Document engagement profiles: selecting (`--engagement-profile(s)`, `defaults.engagement_profiles`, `engagementProfiles` frontmatter), per-origin routing, `assets.jsonl`, the built-in `binary` profile + authoring a profile under `~/.rupu/profiles`/`.rupu/profiles`, and the explicit deferrals (remote, CP web editor).

- [ ] **Step 1: write the docs sections** referencing the real flags/fields/paths from Tasks 2–12.
- [ ] **Step 2: verify** no dangling references; `rg` the new flag names resolve.
- [ ] **Step 3: commit** `docs: engagement profiles — selection, routing, authoring`

---

## Self-Review

**Spec coverage:** selection on every non-remote path (T7/T8/T9/T10) ✔; per-origin routing (T2/T5) ✔; completeness gate (T5) ✔; asset persistence in the correct place — `CoveragePaths`, not the run dir (T3) ✔; `code` unchanged — `engagement: None` path (T1/T5/T8) ✔; schema lockstep untouched — asset on input/record not report (T4) ✔; fail-closed at launch (T5/T9) ✔; end-to-end (T12) ✔.
**Deferred (by ruling, carried from recon):** remote/placed/CP delivery (refused in T9); CP web workflow-editor engagement field; composites-at-scale beyond routing; resume-surviving CLI flag (YAML-only). All stated in Global Constraints.
**Type consistency:** `ActiveSet`/`EngagementProfile` derives (T1) used by `FindingWriteOptions` (T1), `report_finding` (T5), CLI/orchestrator (T8/T9); `AssetInput`/`AssetRef` (T4) used by T5/T6/T10; `unsatisfied` (T5) used only in T5; `append_asset`/`read_asset_graph` (T3) used by T5/T6.
**Placeholder scan:** every task carries real signatures + recon file:line anchors; no "TBD"/"similar to".

## Follow-on

- **Plan 3 — network** (separate doc): `network` profile as data, `http_exchange`/`scan_output`/`pcap_ref` render polish, scope-as-root RoE enforcement, and `pentest = includes[network, web]` exercising the per-origin routing landed here.
- Later: remote/placed engagement delivery (`AgentLaunchRequest` + connectors + capability); CP web workflow-editor engagement field.
