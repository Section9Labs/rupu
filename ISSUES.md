# rupu — known issues

Bugs and correctness defects in shipped code. Distinct from [`TODO.md`](TODO.md),
which tracks *deferred features*; this file tracks *things that are wrong*.

**Two tiers.** The triage table below lists every open defect. Full write-ups
(**symptom** / **root cause** with `file:line` / **impact** / **fix**) follow for
items in the active arc; an item is promoted to a write-up when its arc starts.
Move an entry to `## Fixed` with the PR number and the validation that proves it.

Arcs, sequencing, and the tracking rules are defined in
[`docs/superpowers/specs/2026-07-24-rupu-integrity-remediation-charter.md`](docs/superpowers/specs/2026-07-24-rupu-integrity-remediation-charter.md).

Severity: **P0** breaks/misleads/destroys data today (including "user follows our
own docs and gets a wrong result") · **P1** wrong or missing, workaroundable ·
**P2** cosmetic, internal, or latent.

Most entries below came from the 2026-07-24 four-sweep audit; each was verified
against the code before being recorded.

---

## Triage

### Arc 1 — config integrity

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-9 | P1 | rupu-providers | `[providers.*].timeout_ms` has no consumer | open |
| I-10 | P1 | rupu-providers | `[providers.*].max_retries` has no consumer; real retry budget is a hardcoded 1 | open |
| I-11 | P1 | rupu-providers | `[providers.*].max_concurrency` never throttles LLM calls (semaphore is SCM-only) | open |
| I-12 | P1 | rupu-providers | `[providers.*].org_id` and `.region` have no consumers | open |
| I-13 | P1 | rupu-config | The entire `[retry]` section is inert | open |
| I-14 | P1 | rupu-cli | `log_level` has no consumer; logging reads only `RUPU_LOG` | open |
| I-15 | P1 | rupu-scm | `[scm.default]` / `[issues.default]` are inert; platform choice is hardcoded GitHub-then-GitLab | open |
| I-16 | P1 | rupu-scm | `[scm.*].clone_protocol` is inert — clones always use HTTPS despite the UI dropdown | open |
| I-17 | P2 | rupu-scm | `[scm.*].timeout_ms` has no consumer | open |
| I-18 | P1 | rupu-orchestrator | `[bash]` config is dropped on the workflow path (works under `run`/`session`) | open |
| I-19 | P1 | rupu-app | The desktop app passes `Config::default()` — all user config is inert there | open |
| I-20 | P2 | rupu-config | `resolve()`'s env-override tier is never populated; `KeySource::Env` is unreachable | open |
| I-21 | P2 | rupu-cli | A malformed `config.toml` is silently swallowed on the pricing paths, printing wrong costs | open |

### Arc 2 — safety

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-22 | P1 | rupu-tools | `grep` and `ast_grep` escape the workspace — no `path_scope` containment | open |
| I-23 | P0 | docs/autoflow | The autoflow author allowlist is undocumented and defaults to no restriction | open |
| I-24 | P1 | rupu-cli | `on_reject` cleanup runs at `ask` mode regardless of the run's original mode | open |
| I-25 | P1 | rupu-cli | `rupu workflow runs` — a list command — executes `on_reject` chains as a side effect | open |
| I-26 | P1 | rupu-cli | Action steps get allowlist `["*"]`; the code comment claims parity with agent `tools:` | open |
| I-27 | P2 | rupu-orchestrator | `action_protocol::validate_actions` is dead code; three docs describe a check that never runs | open |

### Arc 3 — single UI path

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-28 | P1 | rupu-cp web | Delete the classic agent-authoring UI; drop `[cp].agent_authoring_ui` | open |
| I-29 | P1 | rupu-cp web | Delete the classic workflow-editor UI; drop `[cp].workflow_editor_ui` | open |
| I-30 | P1 | rupu-cp web | Collapse the run-graph classic/next dual paths to one | open |
| I-31 | P2 | rupu-cp web | Remove both UI hooks and their localStorage overrides | open |
| I-32 | P2 | rupu-cp web | Fan-out / fan-in / branch node silhouettes are placeholders | open |

### Arc 4 — gate/action correctness

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-33 | P0 | rupu-orchestrator | Action steps cannot take a templated number — the headline use case is unexpressible | open |
| I-34 | P0 | rupu-orchestrator | `{{ steps.<action>.output }}` is an unindexable JSON string; no `fromjson` filter exists | open |
| I-35 | P0 | rupu-cp | CP web reject (and host-connector, TUI, cancel) skips the `on_reject` chain entirely | open |
| I-36 | P1 | rupu-orchestrator | A reject with an empty `on_reject` chain records no gate decision at all | open |
| I-37 | P1 | rupu-cli | `on_timeout: approve` never resumes without `cp serve` — the lazy path only prints a hint | open |
| I-38 | P1 | rupu-orchestrator | A timeout-driven approval is recorded as `via: "human"` | open |
| I-39 | P1 | rupu-orchestrator | `StepKind` has no `#[serde(other)]`; old binaries die on new `events.jsonl` | open |
| I-40 | P1 | rupu-cp web | The CP transcript viewer discards `action_emitted` — action steps show an empty transcript | open |
| I-41 | P2 | rupu-cli | `rupu workflow show` renders gate and action steps as `linear` with a blank primary column | open |
| I-42 | P2 | rupu-orchestrator | A `when:`-skipped gate/action loses its kind and persists as `Linear` | open |
| I-43 | P2 | rupu-cli | The gate sweep can re-spawn `workflow approve` every tick forever, with no backoff | open |
| I-44 | P2 | rupu-orchestrator | `notify` hooks write orphan transcript files no `StepResult` references | open |

### Arc 5 — provider wire correctness

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-45 | P0 | rupu-providers | Gemini 3 gets a guaranteed 400 — `thinkingLevel` and `thinkingBudget` are sent together | open |
| I-46 | P1 | rupu-providers | Gemini thinking levels are sent uppercase; the API expects lowercase | open |
| I-47 | P1 | rupu-providers | `xhigh` / `minimal` are rejected outright by DeepSeek, Groq and xAI | open |
| I-48 | P1 | rupu-providers | `usageMetadata.thoughtsTokenCount` is never read — Gemini reasoning tokens are missing from cost | open |
| I-49 | P2 | rupu-providers | `parse_retry_after` is a stub returning `None`; every 429 backs off a flat 60s | open |

### Arc 6 — docs truth pass

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-50 | P0 | README | MSRV is stated as 1.77; the workspace requires 1.88, so `cargo install` fails | open |
| I-51 | P0 | docs | Three docs still say `actions:` is *not* a tool allowlist — since #533/#537 it narrows tools | open |
| I-52 | P0 | docs/providers | `rupu run --agent <name>` is documented twice; the flag does not exist | open |
| I-53 | P0 | docs/workflow-format | Approval **gate nodes** are entirely undocumented | open |
| I-54 | P0 | docs/workflow-format | `action:` connector steps are entirely undocumented | open |
| I-55 | P0 | docs/workflow-format | `branch:` is undocumented, including its silent-wrong-result transitive-arm rule | open |
| I-56 | P0 | docs/workflow-format | The `wake_on:` example uses `github.pull_request.closed`, which never fires | open |
| I-57 | P0 | docs/workflow-format | The `when:` event example uses a path that renders empty, silently skipping the step forever | open |
| I-58 | P1 | docs | There is no config reference page; ~25 shipped keys are documented nowhere | open |
| I-59 | P1 | docs/workflow-format | Non-linear constructs (`next`/`depends_on`/`split`/`join`/`loops`/`max_concurrency`) are undocumented | open |
| I-60 | P1 | docs/workflow-format | Remote placement (`host:`/`distribute:`/`workspace:`) is referenced but never defined | open |
| I-61 | P1 | docs/workflow-format | "Timeouts are enforced lazily" is stale — the gate sweep enforces them by default | open |
| I-62 | P1 | docs/workflow-format | `autoflow.entity` is documented as issue-only; `pull_request` ships | open |
| I-63 | P1 | docs/agent-format | Six accepted frontmatter keys and three built-in tools are missing from the reference | open |
| I-64 | P1 | docs/transcript-schema | `action_emitted` semantics are wrong and two shipped event types are undocumented | open |
| I-65 | P1 | docs/scm | The capability matrix says Linear/Jira have no `issues.*` support; both work today | open |
| I-66 | P1 | docs/providers | "Workflow steps and subagents don't support openai-compatible" is stale | open |
| I-67 | P1 | docs | `rupu cp`, `host`, `node`, `update`, `session`, `autoflow`, `cleanup` are undocumented | open |
| I-68 | P1 | docs | `rupu workflow approve\|reject --gate <step-id>` is unmentioned but required for multi-gate runs | open |
| I-69 | P1 | rupu-cli | `rupu cp serve --help` describes an HTTP server; it also runs autoflow, cron and gate-sweep daemons | open |
| I-70 | P1 | docs/specs | The gate/action design spec's own YAML examples fail schema validation | open |
| I-71 | P2 | README | The subcommand table omits 7 shipped commands; provider count and "not in this binary" are stale | open |
| I-72 | P2 | docs/providers | `[pricing]` is recommended but its schema is documented nowhere | open |

---

## Open

### I-9 … I-14, I-17, I-19, I-20 — dead configuration

Nine keys parse, are documented, and in several cases are editable in the CP
Settings UI, yet have **no runtime consumer**. Proof for each is a workspace-wide
grep (excluding `crates/rupu-config/` and tests) returning zero hits.

| ID | Key | Declared | Documented as working | Reality |
|---|---|---|---|---|
| I-9 | `[providers.*].timeout_ms` | `provider_config.rs:23` | `docs/providers.md:111` ("Default: `120000`") + `ConfigEditor.tsx:220` | vendor default always used |
| I-10 | `[providers.*].max_retries` | `provider_config.rs:25` | `docs/providers.md:112` ("Default: `5`") | real budget is a hardcoded 1 (`anthropic.rs:213`) |
| I-11 | `[providers.*].max_concurrency` | `provider_config.rs:27` | `docs/providers.md:113` + `concurrency.rs:6` | `semaphore_for` is called only by SCM clients; **no LLM call ever acquires a permit** |
| I-12 | `[providers.*].org_id`, `.region` | `provider_config.rs:19,21` | `docs/providers/openai.md:20`, `gemini.md:23` | org-scoped keys and non-default Vertex regions are unreachable |
| I-13 | `[retry]` (whole section) | `config.rs:113-116` | — | inert top-level section |
| I-14 | `log_level` | `config.rs:27` | `ConfigEditor.tsx:199-207`, with a lock toggle | logging reads only `RUPU_LOG` (`logging.rs:25`) |
| I-17 | `[scm.*].timeout_ms` | `scm_config.rs:46` | `docs/scm.md:109-119` | no consumer (sibling `base_url`/`max_concurrency` *are* consumed) |
| I-19 | all of it, in rupu-app | — | — | `executor/mod.rs:210` passes `Config::default()`; self-admitted at `:109-115` |
| I-20 | `resolve()` env tier | `resolve.rs:144-171` | — | both callers pass an empty map; `KeySource::Env` is unreachable |

**Impact.** Exactly the I-1 shape, ~9×. A user reads the docs, sets a value,
sees it accepted (and in four cases sees it in the web UI), and gets different
behavior than documented — with no error.

**Fix.** For each: wire it to its consumer, or delete the key and its
documentation and UI field. Deleting is a legitimate outcome — an honestly absent
knob beats a knob that lies. Validation must observe the value *at the consumer*;
a parse test cannot see this class of defect.

---

### I-15 — `[scm.default]` / `[issues.default]` are inert

**Symptom.** A user with both GitHub and GitLab credentials sets
`[scm.default] platform = "gitlab"`. Every tool call that omits `platform` still
goes to GitHub.

**Root cause.** `Registry::default_platform` / `default_tracker`
(`crates/rupu-scm/src/registry.rs:206-230`) implement a hardcoded
GitHub-then-GitLab preference over registered connectors and never read
`ScmDefault` / `IssuesDefault` (`crates/rupu-config/src/scm_config.rs:24-38`),
which have no consumer anywhere. The code says so: *"Wiring to `[scm.default]`
config lands in Task 19; this is the v0 'first registered' fallback"*
(`registry.rs:207-208`).

**Impact.** The key is written into every `rupu init` config
(`crates/rupu-cli/src/templates.rs:155`), documented as functional
(`docs/scm.md:100-107`), and *named in the error message users see when it is
missing* (`crates/rupu-mcp/src/tools/scm_repos.rs:73`: "no platform arg and no
`[scm.default]` configured"). A GitLab-primary shop silently operates against
GitHub. `default_tracker` additionally ignores Linear even when registered.

**Fix.** Read the config values, falling back to the current preference only when
unset. Include `IssueTracker::Linear` in the fallback.

---

### I-16 — `[scm.*].clone_protocol` is inert

**Symptom.** A user on SSH-only infrastructure selects "ssh" from the
`clone_protocol` dropdown in CP Settings. Clones still go over HTTPS.

**Root cause.** `clone_protocol` (`crates/rupu-config/src/scm_config.rs:51`) has
no consumer. The clone paths hardcode HTTPS with an embedded token —
`connectors/gitlab/repo.rs:477-479`, `connectors/github/repo.rs:442`.

**Impact.** Documented at `docs/scm.md:109-119` and given a dedicated
`https`/`ssh` dropdown at `ConfigEditor.tsx:374,463`. Clones fail or use the
wrong credentials on hosts that only permit SSH.

**Fix.** Honor the setting in both connectors' clone paths. Note this overlaps
the self-hosted-host defect (clone URLs also ignore `base_url`), tracked
separately in `TODO.md`.

---

### I-18 — `[bash]` config is dropped on the workflow path

**Symptom.** `[bash].timeout_secs` and `[bash].env_allowlist` apply under
`rupu run` and `rupu session`, and are silently ignored under
`rupu workflow run`.

**Root cause.** `crates/rupu-orchestrator/src/step_factory.rs:245-246` hardcodes
`bash_env_allowlist: Vec::new(), bash_timeout_secs: 120`. `DefaultStepFactory`
carries no `Config` (`step_factory.rs:36-50`). The values are read correctly at
`cmd/session.rs:6705-6706` and `cmd/run.rs:574-575`.

**Impact.** Precisely the I-2 shape: the same agent behaves differently depending
on how it is invoked. A workflow step gets a 120s bash timeout and an empty env
allowlist no matter what the user configured.

**Fix.** Thread the `[bash]` config into `DefaultStepFactory` alongside the
provider/model values I-2 already added.

---

### I-21 — a malformed `config.toml` silently yields wrong cost figures

**Symptom.** A typo in `[pricing]` makes `rupu run list` / `rupu run show` print
costs computed from default rates, with no warning.

**Root cause.** `crates/rupu-cli/src/cmd/run.rs:339,404` —
`layer_files(...).unwrap_or_default()` discards a real `LayerError::Parse`
(`crates/rupu-config/src/layer.rs:23-28`) and feeds `cfg.pricing` into
`query_run_detail`. The fallback is deliberate per the comment at `run.rs:335`,
but it was reasoned about for UI preferences, not for numbers the user reads.

**Impact.** Wrong dollar figures presented as authoritative. The same pattern at
`cmd/workflow.rs:993` and `cmd/cron.rs:281` affects only UI preferences and is
harmless.

**Fix.** On the pricing paths, surface the parse error (or at minimum warn that
default rates are in use). Leave the UI-preference sites as they are.

---

### I-5 — `rust-toolchain.toml` is not honored on this box; clippy is red under 1.95

**Symptom.** `cargo clippy` fails on a clean `main` in crates unrelated to any
change, e.g.:

- `crates/rupu-config/src/config.rs:150` — `unnecessary_map_or` (fixed in
  passing, see I-1/I-2 PR)
- `crates/rupu-orchestrator/src/runner.rs:3262` — `items_after_test_module`
- `crates/rupu-cp/src/host/ssh.rs:193` — `type_complexity`
- `crates/rupu-cp/src/host/ssh.rs:1041` — `single_match`

**Root cause.** `rustup` is not installed, so `rust-toolchain.toml`'s
`channel = "1.88"` pin is silently ignored and the Homebrew `rustc 1.95.0` is
used instead. These lints post-date 1.88, so CI (which does honor the pin) stays
green while local clippy is red.

**Impact.** Real, and it compounds: because clippy lints workspace path
dependencies, a red crate blocks linting of every crate that depends on it —
`rupu-cp` being red means `rupu-cli` cannot be linted locally at all. Local
clippy is therefore not usable as a pre-push gate.

**Fix.** Either install `rustup` so the pin applies, or bump the pinned
toolchain and clear the new lints in one sweep. The second is probably better
long-term, but it is a workspace-wide change and wants its own PR.

---

### I-4 — Four `linear_runner` tests fail on a clean `main`

**Symptom.** `cargo test -p rupu-orchestrator --test linear_runner` reports
`24 passed; 4 failed` on an unmodified checkout of `main` (verified at `6dffeb5`):

- `run_store_marks_run_failed_with_error_message`
- `for_each_without_continue_on_error_aborts_workflow_on_first_failure`
- `parallel_without_continue_on_error_aborts_with_sub_step_id_in_message`
- `resume_reruns_only_failed_fanout_units`

All four assert on an error message propagating out of a failed step, e.g.
`linear_runner.rs:915`:
`assert!(rec.error_message.as_ref().is_some_and(|m| m.contains("simulated failure")))`.

**Root cause.** Not yet diagnosed. The suite takes a suspicious ~48s wall-clock,
which suggests the failing steps are retrying against a real network endpoint
and ultimately failing with a timeout/transport message instead of the
simulated one — i.e. the mock isn't intercepting, or a retry wrapper is
rewriting the error. Worth checking against the retry config and whether these
tests are toolchain-sensitive (this box runs Homebrew Rust 1.95 vs. the pinned
1.88).

**Impact.** The orchestrator test baseline is red, so a real regression in step
error propagation would be invisible — it looks like the pre-existing noise.

**Fix.** Unassigned. Diagnose before trusting this suite as a gate.

---

### I-3 — Global `default_model` shadows a provider-scoped `default_model`

**Symptom.** With a global `default_model` set and an agent pinned to an
openai-compatible provider that has its own `[providers.<name>].default_model`,
the *global* value wins and is sent to the custom endpoint — which typically
rejects it as an unknown model.

**Root cause.** The fallback chain in `crates/rupu-cli/src/cmd/run.rs` orders
`cfg.default_model` *before* the provider-scoped `oai_params.default_model`:

```rust
spec.model → cfg.default_model → oai_params.default_model → "claude-sonnet-4-6"
```

The provider-scoped default is the more specific value and arguably should win
whenever the resolved provider is that provider.

**Impact.** Only bites when both a global `default_model` and a custom provider
are configured. The documented config in `docs/providers.md` sets no global
`default_model`, so the documented path is unaffected.

**Fix.** Probably reorder to `spec.model → oai_params.default_model →
cfg.default_model → hardcoded`. Deliberately **not** fixed alongside I-1/I-2:
it is a behavior change to a currently-consistent path rather than a
silent-noop, and it deserves its own decision. Needs a call from matt on
whether global `default_model` is meant to be provider-agnostic.

---

## Fixed

### I-8 — `dispatch_agent` hardcoded provider and model (the unfixed 4th I-1/I-2 site)

**Symptom.** A sub-agent dispatched via `dispatch_agent` /
`dispatch_agents_parallel` ignored `default_provider` and `default_model`, and
could not use a config-declared openai-compatible provider.

**Root cause.** `crates/rupu-cli/src/cmd/dispatch.rs:186-190` did
`spec.provider.clone().unwrap_or_else(|| "anthropic".into())` and
`unwrap_or_else(|| "claude-sonnet-4-6".into())`, calling
`provider_factory::build_for_provider` without any config. `CliAgentDispatcher`
(`:26-30`) had no config-derived field at all.

**Impact.** I-1 and I-2 were recorded as fixed "at all three call sites"
(`run.rs`, `session.rs`, `step_factory.rs`). There was a fourth. The same
silent-noop the original fix set out to eliminate was still live on the
sub-agent path, and this file's own "Fixed" section overstated the coverage
(both entries are corrected below).

**Fix.** PR (branch `arc1/config-integrity`).

1. `CliAgentDispatcher` gained the three config-derived fields
   `DefaultStepFactory` already carries — `default_provider: Option<String>`,
   `default_model: Option<String>`, and the `openai_compatible:
   HashMap<String, OpenAiCompatibleParams>` map. Nothing re-reads
   `config.toml` inside `dispatch()`: the values are threaded in from the
   caller, which has already loaded config through `layer_files_locked`
   (I-7), so the policy lock applies to sub-agent dispatch too.
2. The hardcoded block was replaced with the exact sequence `cmd/run.rs` and
   `step_factory.rs` use: `provider_factory::resolve_provider_name` →
   `openai_compatible` lookup → `provider_factory::resolve_model` →
   `build_for_provider_with_config` with a `ProviderConfig` carrying
   `spec.anthropic_oauth_prefix` + the resolved openai-compatible params.
   A build failure still surfaces as `DispatchError::ProviderBuild` — the
   unknown-provider case stays loud rather than falling back to Anthropic.
3. All **four** construction sites were updated to pass
   `cfg.default_provider` / `cfg.default_model` /
   `provider_factory::openai_compatible_map(&cfg.providers)`:
   `crates/rupu-cli/src/cmd/run.rs:686` (bare `rupu run`),
   `crates/rupu-cli/src/cmd/workflow.rs:4022` (`rupu workflow run`),
   `crates/rupu-cli/src/cmd/workflow.rs:2639` and
   `crates/rupu-cli/src/resume.rs:218` (both resume paths). Each of the
   latter three already computed the same `openai_compatible` map for its
   `DefaultStepFactory`; the map was hoisted above the dispatcher build and
   shared, so a dispatched sub-agent and a workflow step now resolve from
   byte-identical inputs.

**Validation.** `cargo test -p rupu-cli --lib cmd::dispatch` — 8 tests (4
pre-existing, 3 new). The seam is real persisted output, not a mock internal:
`run_agent` writes the resolved pair into the child's own transcript as
`Event::RunStart { provider, model }` (`crates/rupu-agent/src/runner.rs:739`),
so the new tests dispatch a child for real under `RUPU_MOCK_PROVIDER_SCRIPT`
and read that event back off disk.

- `dispatch_honors_config_default_provider_and_model` — the binding assertion.
  A child agent whose frontmatter pins **neither** `provider:` nor `model:`,
  dispatched with `default_provider = "cfg-provider"` / `default_model =
  "cfg-model"`, must record `cfg-provider` / `cfg-model`. On the pre-fix code
  it fails with `left: "claude-sonnet-4-6", right: "cfg-model"` — the exact
  hardcoded fallback.
- `dispatch_agent_frontmatter_overrides_config_defaults` — the control:
  a pinned `provider:`/`model:` still wins over the config defaults.
- `dispatch_resolves_openai_compatible_provider_default_model` — a child on a
  config-declared `oracle` provider with no `model:` resolves that provider's
  `default_model`, proving the openai-compatible map is actually consulted
  (this is the capability the sub-agent path could not reach at all before).

`cargo build --workspace` is clean after the constructor-signature change, and
`grep -rn "CliAgentDispatcher::new(" --include="*.rs" crates` returns exactly the
four production sites plus the six in-file test constructions — there is no
fifth production site left un-threaded.

### I-7 — `[policy].lock` is not enforced anywhere outside the web UI

**Symptom.** An administrator locks `permission_mode` globally. The CP Settings
UI shows the field locked and refuses to edit it. A project-level
`.rupu/config.toml` still overrides it for every `rupu run`, `rupu session` and
`rupu workflow run`.

**Root cause.** Lock enforcement lives entirely inside `rupu_config::resolve`
(`crates/rupu-config/src/resolve.rs:180-197`, `is_locked` → global-wins
precedence). `resolve()` has **6 call sites, all in rupu-cp**
(`crates/rupu-cp/src/state.rs`, `crates/rupu-cp/src/api/config.rs`). Every CLI
path loads configuration through `rupu_config::layer_files` instead — **43 call
sites** — which performs ordinary project-over-global layering and never
consults `[policy].lock`.

**Impact.** The one mechanism presented as a governance control is a UI-only
affordance. `ConfigEditor.tsx:7-14` tells the operator "a field whose resolved
value is enforced by the global policy lock cannot be edited", which is true of
the web form and false of the tool.

**Fix.** PR (branch `arc1/config-integrity`), in two commits:

1. `crates/rupu-config/src/layer.rs` gained `layer_files_locked(global,
   project)` — same signature as `layer_files`, but it delegates to
   `resolve()` (empty env map) and returns `resolved.config`, so the
   global-wins-when-locked precedence and the dotted-key encoding are the
   single implementation in `resolve.rs`, not a second copy.
2. Every **policy-bearing** `layer_files` call site in `rupu-cli` moved to
   `layer_files_locked` — 30 of the 42 non-test sites: `resume.rs`, `cmd/run.rs`
   (×3), `cmd/session.rs` (×5), `cmd/workflow.rs` (×4), `cmd/cp.rs` (×2),
   `cmd/cron.rs` (×2), `cmd/issues.rs` (×2), `cmd/auth.rs` (×2),
   `cmd/repos.rs`, `cmd/mcp.rs`, `cmd/update.rs`, `cmd/webhook.rs`,
   `cmd/agent.rs`, `cmd/usage.rs`, `cmd/autoflow.rs`, `cmd/transcript.rs`,
   `cmd/editor.rs`.
   The rule applied: **migrate** whenever the loaded `Config` escapes the
   function or feeds a non-`[ui]` key (permission mode, provider/model,
   `[scm]`/`[issues]`, `[triggers]`, `[cp]`, `[storage]` retention,
   `[pricing]`, `[update]`); **leave** only where the `Config` is local to the
   function and nothing but `cfg.ui` is read.

   The 12 deliberate leaves are UI-preference-only loads (theme / color /
   pager / live-view) and each now carries the marker comment
   `// UI prefs only — lock does not apply (I-7)`: `cmd/ui.rs`,
   `cmd/session.rs:show`, `cmd/cron.rs:ui_prefs`,
   `cmd/workflow.rs:show`, `cmd/transcript.rs` (×2), `cmd/autoflow.rs`,
   `cmd/auth.rs:auth_ui_prefs`, `output/diag.rs`, `cmd/watch.rs`,
   `output/workflow_printer.rs`, `cmd/repos.rs:tracked`.

   **Follow-up (2026-07-27).** `cmd/editor.rs` was originally left on
   `layer_files` under the same "UI prefs only" annotation, on the mistaken
   premise that `[ui].editor` is a display preference. It is not:
   `resolve_editor` (`editor.rs:49-73`) returns it as the **program name**
   `open_for_edit` spawns as a subprocess for `rupu agent edit` / `rupu
   workflow create`, so an unlocked project config could choose which binary
   executes on a locked installation — exactly the governance hole this issue
   closes. Moved to `layer_files_locked`; regression test
   `layer_files_locked_keeps_a_locked_global_ui_editor` added to
   `crates/rupu-cli/tests/policy_lock.rs` (now 4 tests).

**Validation.** `cargo test -p rupu-cli --test policy_lock` — 3 tests. The two
that prove the migration drive a **real CLI command** rather than the config
crate, so they would still fail if any `rupu run` call site had been left on
`layer_files`: `cli_run_honors_a_locked_global_permission_mode` writes a global
`permission_mode = "readonly"` + `lock = ["permission_mode"]` against a project
`permission_mode = "bypass"`, invokes `rupu_cli::run(["rupu","run","writer","go"])`
(no `--mode` flag) under the mock provider, and asserts the agent's `write_file`
call was **denied** — i.e. the effective mode stayed `readonly`. It fails on the
pre-fix code with the file present on disk. `cli_run_lets_an_unlocked_project_permission_mode_win`
is the control: with no `[policy].lock`, ordinary project-over-global layering
still applies and the write goes through.
`layer_files_locked_keeps_the_locked_global_value` pins the library contract.

Completeness is enforced by grep: `grep -rn "layer_files(" --include="*.rs"
crates/rupu-cli | grep -v test` returns only the 12 UI-preference sites, every
one of them carrying the `// UI prefs only — lock does not apply (I-7)`
comment.

**Notes — known behavior deltas from routing a site through `layer_files_locked`
(i.e. through `resolve()`) instead of `layer_files`:** (a) a project-declared
`[policy].lock` no longer lands in the resolved config — the lock list is
pinned to the global one (this is the existing CP behavior); (b) a
scalar-vs-table structural conflict between layers now surfaces as
`LayerError::Invalid` instead of a serde `Layered` error — both were errors
before and after; (c) `resolve()`'s `flatten` treats a `Value::Table` with no
leaf keys as contributing nothing, so a config section declared but left empty
(e.g. `[providers.foo]` with no keys under it) — which survived plain
`layer_files` layering as a defaulted map entry — disappears from the config
produced by a migrated site; this is believed harmless in practice (an empty
section carries no settings to act on) but is an undisclosed behavior change
and is recorded here for completeness.

### I-6 — `rupu config set` corrupts `config.toml`, then silently wipes it

**Symptom.** Two commands destroy a user's configuration:

```
$ rupu config set ui.theme dracula     # writes a literal "ui.theme" top-level key
$ rupu config set log_level debug      # config.toml is now an empty file
```

**Root cause.** Two independent defects in `crates/rupu-cli/src/cmd/config.rs`:

1. `set` operates on the top-level table only — `t.insert(key.to_string(), parsed)`
   (`config.rs:66`) inserts the dotted string `"ui.theme"` as a key rather than
   descending into `[ui]`. `Config` is `#[serde(default, deny_unknown_fields)]`
   (`crates/rupu-config/src/config.rs:22`), so the file no longer loads.
2. The *next* `set` reads that now-invalid file with
   `toml::from_str(&text).unwrap_or_else(|_| Value::Table(Default::default()))`
   (`config.rs:54`) — on a parse failure it starts from an **empty table** and
   writes it back, discarding every remaining setting.

`get` has the same top-level-only limitation (`config.rs:42`) but is read-only.

**Impact.** Data loss on the most natural input. `README.md:283` advertises the
command as "Read / write rupu configuration"; only the clap help mentions the
top-level restriction. Downstream, the corrupted file either hard-errors
(`cmd/run.rs:473`) or is silently swallowed (`cmd/run.rs:339`), so the user may
just see all their settings quietly stop applying.

**Fix.** PR (branch `arc1/config-integrity`) — added `get_path`/`set_path`
helpers in `crates/rupu-cli/src/cmd/config.rs` that split a dotted key on `.`
and descend/create intermediate tables, replacing the top-level-only
`t.insert(key.to_string(), parsed)`. `set_path` refuses to overwrite an
existing non-table with a table, and (follow-up) also refuses to overwrite an
existing **table** with a scalar — both directions were the same
silent-subtree-loss shape and both now name the offending key in the error.
`set`'s read path no longer falls back to an empty table on a parse error — a
malformed `config.toml` now aborts the write with `anyhow::bail!` instead of
being replaced.

**Validation.** `cargo test -p rupu-cli --lib cmd::config` —
`cmd::config::tests`, 7 tests, all observing real `Value` mutation/traversal
(not just parsing): `set_path_descends_into_a_nested_table`,
`set_path_creates_missing_intermediate_tables`,
`set_path_refuses_to_overwrite_a_scalar_with_a_table`,
`set_path_refuses_to_overwrite_an_existing_table_with_a_scalar`,
`set_path_still_overwrites_an_existing_scalar_with_a_scalar`,
`get_path_reads_a_nested_key`, `a_top_level_key_still_works`.

`cargo test -p rupu-cli --test cli_config` exercises the real `async fn set()`
read path end-to-end (writes files under a temp `RUPU_HOME`, invokes
`rupu_cli::run([...])`), closing the gap the unit tests alone left open —
defect (2) above (fall back to an empty table on a parse error) was only
proven at the pure-helper level until now:
`config_set_on_a_malformed_file_fails_and_leaves_the_file_untouched` writes a
deliberately invalid `config.toml`, asserts it actually fails
`toml::from_str` first (fixture sanity check), then asserts `config set`
exits non-zero AND that the file's bytes on disk are byte-for-byte identical
to what was written before the call — the anti-wipe guarantee.
`config_set_on_a_valid_file_adds_a_key_without_disturbing_existing_ones` is
the mirror-image positive case: a valid file with a top-level key and a
nested table both survive a `config set` that adds an unrelated new key.

### I-1 — `default_provider` in `config.toml` was dead config

**Symptom.** Setting `default_provider = "oracle"` in `~/.rupu/config.toml` or
`<repo>/.rupu/config.toml` parsed cleanly and did nothing. Every agent still ran
on `anthropic` unless its frontmatter pinned `provider:` explicitly.

**Root cause.** `Config.default_provider` was declared at
`crates/rupu-config/src/config.rs:24` and covered by parse/layering tests, but
had **no runtime consumer**. All three provider-resolution call sites hardcoded
the fallback:

- `crates/rupu-cli/src/cmd/run.rs:292` — `rupu run`
- `crates/rupu-cli/src/cmd/session.rs:1337` — `rupu session`
- `crates/rupu-orchestrator/src/step_factory.rs:176` — workflow steps

all as `spec.provider.clone().unwrap_or_else(|| "anthropic".into())`.

**Impact.** High, and user-facing: `default_provider = "oracle"` is the
*documented* way to point rupu at an OpenAI-compatible endpoint
(`docs/providers.md:126`, `docs/providers/openai-compatible.md:17`), and the
rupu-cp web UI exposes an editor field for it
(`crates/rupu-cp/web/src/components/ConfigEditor.tsx:74`). Users following the
docs silently got billed Anthropic traffic instead of hitting their own
endpoint. A textbook silent-noop.

**Fix.** PR — extracted `provider_factory::resolve_provider_name()` as the
single resolution point (`spec.provider → cfg.default_provider → "anthropic"`)
and routed all three call sites through it.

**Correction (I-8).** "All three call sites" was wrong: there was a **fourth**,
`crates/rupu-cli/src/cmd/dispatch.rs` (sub-agent dispatch via `dispatch_agent` /
`dispatch_agents_parallel`), which kept the hardcoded
`unwrap_or_else(|| "anthropic".into())` and never received a config. It was
closed separately as **I-8** (see above); provider resolution is only genuinely
single-pointed as of that fix.

### I-2 — `default_model` was ignored on the workflow path

**Symptom.** The same agent resolved to a different model under `rupu run` than
as a workflow step.

**Root cause.** `rupu run` (`crates/rupu-cli/src/cmd/run.rs:305`) and
`rupu session` (`crates/rupu-cli/src/cmd/session.rs:1341`) consulted
`cfg.default_model`; the workflow `StepFactory`
(`crates/rupu-orchestrator/src/step_factory.rs:180-183`) skipped it, going
straight from `spec.model` to the openai-compatible default to the hardcoded
`"claude-sonnet-4-6"`. `DefaultStepFactory` never received the value — it only
carried the `openai_compatible` map, not the rest of the config.

**Impact.** Medium. An agent with no `model:` pin silently ran on a different
model depending on how it was invoked, which also makes cost attribution
misleading.

**Fix.** PR — extracted `provider_factory::resolve_model()` as the single
resolution point and threaded `default_provider` / `default_model` into
`DefaultStepFactory` so the workflow path resolves identically to `rupu run`.

**Correction (I-8).** As with I-1, the recorded three call sites were not all of
them. `crates/rupu-cli/src/cmd/dispatch.rs` was a **fourth**, still going
straight from `spec.model` to a hardcoded `"claude-sonnet-4-6"` with no
`cfg.default_model` and no openai-compatible fallback, so the "same agent, same
model regardless of invocation" guarantee did not hold for a dispatched
sub-agent. Closed as **I-8** (see above), which threads the same three
config-derived values into `CliAgentDispatcher` that `DefaultStepFactory`
carries.
