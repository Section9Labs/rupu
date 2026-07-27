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
| I-6 | P0 | rupu-cli | `rupu config set` writes dotted keys literally, then wipes the whole config on the next write | open |
| I-7 | P0 | rupu-config | `[policy].lock` is enforced only inside rupu-cp; all 43 CLI config loads ignore it | open |
| I-8 | P0 | rupu-cli | `dispatch_agent` hardcodes provider/model — the 4th I-1/I-2 site, never fixed | open |
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

**Fix.** Descend dotted paths on both `get` and `set` (the CP web already does
this — `crates/rupu-cp/src/api/config.rs` handles dotted keys). Never fall back
to an empty table on a parse error: fail loudly and leave the file untouched.

---

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

**Fix.** Route CLI config loading through a single lock-aware resolver so
`layer_files` is no longer used directly for policy-bearing keys. Validate with a
test that a locked global key survives a conflicting project config on a CLI
code path — not only through `resolve()`.

---

### I-8 — `dispatch_agent` hardcodes provider and model (the unfixed 4th I-1/I-2 site)

**Symptom.** A sub-agent dispatched via `dispatch_agent` /
`dispatch_agents_parallel` ignores `default_provider` and `default_model`, and
cannot use a config-declared openai-compatible provider.

**Root cause.** `crates/rupu-cli/src/cmd/dispatch.rs:157-162` still does
`spec.provider.clone().unwrap_or_else(|| "anthropic".into())` and
`unwrap_or_else(|| "claude-sonnet-4-6".into())`, calling
`provider_factory::build_for_provider` without a `Config`. The struct has no
`Config` field at all.

**Impact.** I-1 and I-2 are recorded as fixed "at all three call sites"
(`run.rs`, `session.rs`, `step_factory.rs`). There is a fourth. The same
silent-noop the original fix set out to eliminate is still live on the
sub-agent path, and this file's own "Fixed" section overstates the coverage.

**Fix.** Thread `Config` into the dispatch tool and route through
`provider_factory::resolve_provider_name()` / `resolve_model()` like the other
three sites.

---

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
rewriting the error. Worth checking against the retry config. **Not a
toolchain gap:** I-5 bumped the pin to 1.95, so this box's Homebrew rustc now
matches the pinned/CI toolchain exactly, and the same 24-passed/4-failed
split reproduces unchanged under it — these four failures have some other
cause and still need diagnosis.

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

---

### I-5 — `rust-toolchain.toml` is not honored on this box; clippy is red under 1.95

**Symptom.** `cargo clippy` failed on a clean `main` in crates unrelated to any
change (`rupu-config`, `rupu-orchestrator`, `rupu-cp`, `rupu-cli`, `rupu-app`),
because the pinned toolchain was silently ignored locally.

**Root cause.** `rustup` is not installed, so `rust-toolchain.toml`'s
`channel = "1.88"` pin was silently ignored and the Homebrew `rustc 1.95.0` was
used instead. Lints that post-date 1.88 made CI (which honored the pin) stay
green while local clippy was red.

**Fix.** (PR: pin-toolchain-1.95) Bumped the pinned toolchain to 1.95 at all
four sites (`rust-toolchain.toml`, workspace `Cargo.toml` `rust-version`,
`.github/workflows/nightly-live-tests.yml`, `docs/RELEASING.md`) so declared
== actual, then cleared every clippy lint 1.95 surfaces workspace-wide in the
same sweep — including a real `MutexGuard`-held-across-`.await` hazard in
`rupu-auth`'s test-only `ENV_LOCK` (switched to `tokio::sync::Mutex`, which is
built to be held across an await point, instead of papering over it).
`cargo clippy --workspace --all-targets` is clean; `cargo build --workspace`
is clean; `rupu-auth`/`rupu-config`/`rupu-orchestrator`/`rupu-cp` `--lib`
suites are green.
