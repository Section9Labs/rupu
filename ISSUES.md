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
| I-9 | P1 | rupu-providers | `[providers.*].timeout_ms` has no consumer | fixed |
| I-10 | P1 | rupu-providers | `[providers.*].max_retries` has no consumer; real retry budget is a hardcoded 1 | fixed |
| I-11 | P1 | rupu-providers | `[providers.*].max_concurrency` never throttles LLM calls (semaphore is SCM-only) | fixed |
| I-12 | P1 | rupu-providers | `[providers.*].org_id` and `.region` have no consumers | fixed |
| I-13 | P1 | rupu-config | The entire `[retry]` section is inert | fixed |
| I-14 | P1 | rupu-cli | `log_level` has no consumer; logging reads only `RUPU_LOG` | fixed |
| I-16 | P1 | rupu-scm | `[scm.*].clone_protocol` is inert — clones always use HTTPS despite the UI dropdown | fixed |
| I-17 | P2 | rupu-scm | `[scm.*].timeout_ms` has no consumer | fixed |
| I-19 | P1 | rupu-app | The desktop app passes `Config::default()` — all user config is inert there | fixed |
| I-20 | P2 | rupu-config | `resolve()`'s env-override tier is never populated; `KeySource::Env` is unreachable | fixed |
| I-21 | P2 | rupu-cli | A malformed `config.toml` is silently swallowed on the pricing paths, printing wrong costs | fixed |

### Found during Arc 1 (unscheduled)

Discovered while fixing Arc 1 and recorded here rather than carried in a report —
the anti-orphan rule applies to findings made *during* the work, not just to
planned deferrals. None are regressions from Arc 1; all pre-date it.

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-73 | P1 | rupu-scm | `[scm.default].owner`/`.repo` and `[issues.default].project` are still inert — I-15 wired only `platform`/`tracker` | open |
| I-74 | P1 | rupu-orchestrator | `generate.rs:167` calls `build_for_provider` without `ProviderConfig`, so `generate` ignores all `[providers.*]` tuning | open |
| I-75 | P2 | rupu-providers | Anthropic's in-client 429 loop sleeps while holding its concurrency permit (it isn't wrapped in `RetryingProvider`) | open |
| I-76 | P2 | rupu-providers | The provider decorators don't forward `list_models`; a factory-built `Box<dyn LlmProvider>` would silently return `vec![]` | open |
| I-77 | P2 | rupu-scm | `insert_repo_connector`/`insert_issue_connector` are documented "test/internal" but carry no `#[cfg]` gate, unlike `empty()` | open |

### Arc 2 — safety

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-22 | P1 | rupu-tools | `grep` and `ast_grep` escape the workspace — no `path_scope` containment | fixed |
| I-23 | P0 | docs/autoflow | The autoflow author allowlist is undocumented and defaults to no restriction | fixed |
| I-24 | P1 | rupu-cli | `on_reject` cleanup runs at `ask` mode regardless of the run's original mode | fixed |
| I-25 | P1 | rupu-cli | `rupu workflow runs` — a list command — executes `on_reject` chains as a side effect | fixed |
| I-26 | P1 | rupu-cli | Action steps get allowlist `["*"]`; the code comment claims parity with agent `tools:` | fixed |
| I-27 | P2 | rupu-orchestrator | `action_protocol::validate_actions` is dead code; three docs describe a check that never runs | fixed |
| I-78 | P1 | rupu-orchestrator | A workflow step at `--mode ask` still gets `BypassDecider` — `ask` grants full tool access | open |
| I-79 | P2 | rupu-cli | The action dispatcher's `["*"]` allowlist is sound only by invariant, not by construction | open |
| I-80 | P2 | rupu-cli/docs | Reject-timeout gates now resolve only via `cp serve`'s sweep; CLI-only operators lose auto-resolution | open |
| I-81 | P2 | rupu-mcp | `tools_list_matches_snapshot` fails in this worktree — schemars field-order drift under Homebrew 1.95 vs pinned 1.88 | open |
| I-82 | P2 | rupu-cp | The CP web approve path discards the resolved decision, so a web approve's true actor is lost before the resume worker spawns | open |
| I-83 | P2 | rupu-providers | `RetryingProvider` ignores server-supplied `Retry-After` by design; nothing in production reads it | open |
| I-84 | P1 | rupu-providers | Anthropic's nested idle-retry × 429-retry loops multiply attempts | open |
| I-85 | P2 | rupu-providers | Four hand-written reasoning-effort ladders, two of them byte-identical duplicates | open |
| I-86 | P2 | docs/spec.md | Three stale claims: `gate_requested` "not emitted in v0", an `actions:` allowlist check on `action_emitted` that never existed, and "v0 logs `action_emitted` only" | open |

### Arc 3 — single UI path

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-28 | P1 | rupu-cp web | Delete the classic agent-authoring UI; drop `[cp].agent_authoring_ui` | fixed |
| I-29 | P1 | rupu-cp web | Delete the classic workflow-editor UI; drop `[cp].workflow_editor_ui` | fixed |
| I-30 | P1 | rupu-cp web | Collapse the run-graph classic/next dual paths to one | fixed |
| I-31 | P2 | rupu-cp web | Remove both UI hooks and their localStorage overrides | fixed |
| I-32 | P2 | rupu-cp web | Fan-out / fan-in node silhouettes are provisional art (`branch` is not — mis-scoped) | moved → TODO.md |

### Arc 4 — gate/action correctness

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-33 | P0 | rupu-orchestrator | Action steps cannot take a templated number — the headline use case is unexpressible | fixed |
| I-34 | P0 | rupu-orchestrator | `{{ steps.<action>.output }}` is an unindexable JSON string; no `fromjson` filter exists | fixed |
| I-35 | P0 | rupu-cp | Web, local/http connector, desktop-app and **both cancel** paths skip the `on_reject` chain (no TUI exists; SSH/tunnel are fine) | fixed |
| I-36 | P1 | rupu-orchestrator | A reject with an empty `on_reject` chain records no gate decision at all | fixed |
| I-37 | P1 | rupu-cli | `on_timeout: approve` never resumes without `cp serve` — the lazy path only prints a hint | fixed (docs) |
| I-38 | P1 | rupu-orchestrator | A timeout-driven approval is recorded as `via: "human"` | fixed |
| I-39 | P1 | rupu-orchestrator | `StepKind` has no `#[serde(other)]`; an unknown kind **silently drops the whole step result** on resume (readers skip, they do not die) | fixed |
| I-40 | P2 | rupu-cp web | The CP transcript never shows an action step's rendered `with:` args (the node itself *does* render via `tool_audit`) | fixed |
| I-41 | P2 | rupu-cli | `rupu workflow show`'s **steps table** lacks gate/action arms (the graph, the primary view, is correct) | fixed |
| I-42 | P2 | rupu-orchestrator | A `when:`-skipped gate/action loses its kind and persists as `Linear` | fixed |
| I-43 | P2 | rupu-cli | The gate sweep can re-spawn `workflow approve` every tick forever, with no backoff | fixed |
| I-44 | P2 | rupu-orchestrator | `notify` hooks write orphan transcript files no `StepResult` references | fixed |

### Arc 5 — provider wire correctness

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-45 | P1 | rupu-providers | `thinkingLevel` + `thinkingBudget` are always co-sent, and there is **no model gate** — the Gemini-3-only key also goes to 2.5 (the "400" is doc-derived, never observed) | fixed |
| I-46 | P2 | rupu-providers | Gemini thinking levels are sent uppercase; Google documents lowercase (our own spec says tolerance is **unverified**) | fixed |
| I-47 | P2 | rupu-providers | `minimal`/`xhigh` are forwarded to any openai-compatible endpoint with no allowlist ("rejected outright" is **unproven**; none of the named vendors ship as providers) | fixed (docs) |
| I-48 | **P0** | rupu-providers | `usageMetadata.thoughtsTokenCount` is never read — the **only default-reachable** defect here; Gemini spend is silently under-billed and run totals under-report | fixed |
| I-49 | P2 | rupu-providers | `classify.rs` is dead code (zero production callers) — the filed "flat 60s" claim is **false at every constant**; real default is one 2s retry | fixed |

### Arc 6 — docs truth pass

| ID | Sev | Area | Title | Status |
|---|---|---|---|---|
| I-50 | P0 | README | MSRV is stated as 1.77; the workspace requires 1.88, so `cargo install` fails | fixed |
| I-51 | P0 | docs | Three docs still say `actions:` is *not* a tool allowlist — since #533/#537 it narrows tools | fixed |
| I-52 | P0 | docs/providers | `rupu run --agent <name>` is documented twice; the flag does not exist | fixed |
| I-53 | P0 | docs/workflow-format | Approval **gate nodes** are entirely undocumented | fixed (in Arc 4) |
| I-54 | P0 | docs/workflow-format | `action:` connector steps are entirely undocumented | open |
| I-55 | P0 | docs/workflow-format | `branch:` is undocumented, including its silent-wrong-result transitive-arm rule | open |
| I-56 | P0 | docs/workflow-format | The `wake_on:` example uses `github.pull_request.closed`, which never fires | open |
| I-57 | P0 | docs/workflow-format | The `when:` event example uses a path that renders empty, silently skipping the step forever | open |
| I-58 | P1 | docs | There is no config reference page; ~25 shipped keys are documented nowhere | fixed |
| I-59 | P1 | docs/workflow-format | Non-linear constructs (`next`/`depends_on`/`split`/`join`/`loops`/`max_concurrency`) are undocumented | open |
| I-60 | P1 | docs/workflow-format | Remote placement (`host:`/`distribute:`/`workspace:`) is referenced but never defined | open |
| I-61 | P1 | docs/workflow-format | "Timeouts are enforced lazily" is stale — the gate sweep enforces them by default | fixed (in Arc 4) |
| I-62 | P1 | docs/workflow-format | `autoflow.entity` is documented as issue-only; `pull_request` ships | open |
| I-63 | P1 | docs/agent-format | Six accepted frontmatter keys and three built-in tools are missing from the reference | fixed |
| I-64 | P1 | docs/transcript-schema | `action_emitted` semantics are wrong and two shipped event types are undocumented | fixed |
| I-65 | P1 | docs/scm | The capability matrix says Linear/Jira have no `issues.*` support; both work today | fixed |
| I-66 | P1 | docs/providers | "Workflow steps and subagents don't support openai-compatible" is stale | fixed |
| I-67 | P1 | docs | `rupu cp`, `host`, `node`, `update`, `session`, `autoflow`, `cleanup` are undocumented | fixed |
| I-68 | P1 | docs | `rupu workflow approve\|reject --gate <step-id>` is unmentioned but required for multi-gate runs | fixed |
| I-69 | P1 | rupu-cli | `rupu cp serve --help` describes an HTTP server; it also runs autoflow, cron and gate-sweep daemons | fixed |
| I-70 | P1 | docs/specs | The gate/action design spec's own YAML examples fail schema validation | fixed |
| I-71 | P2 | README | The subcommand table omits 7 shipped commands; provider count and "not in this binary" are stale | fixed |
| I-72 | P2 | docs/providers | `[pricing]` is recommended but its schema is documented nowhere | fixed |

---

## Open

### I-86 — three stale claims in `docs/spec.md`

Surfaced during the Arc 6 truth pass and filed rather than fixed, because `docs/spec.md`
was outside that sweep's assigned scope.

- `:230` — lists `gate_requested` as *"reserved; not emitted in v0"*. Gate nodes ship and
  the CP transcript viewer has an explicit arm for the event, so this needs checking against
  the current emitters.
- `:250` — *"For each `action_emitted` event, check the step's `actions:` allowlist"*
  describes a check that **never existed in shipped code**. This is the same phantom
  validator [[I-27]] deleted and [[I-51]] corrected elsewhere; `docs/spec.md` was missed
  because it wasn't in either issue's file list.
- `:65` — *"v0 logs `action_emitted` only"* predates action steps actually executing.

**Fix.** Verify each against the current `Event` emitters and correct, or add a dated
correction banner if `docs/spec.md` is meant to read as a historical record rather than live
reference — decide which it is first, since that determines the treatment (compare the
banner approach taken for [[I-70]]).

---

### I-83 — `RetryingProvider` ignores server-supplied `Retry-After`

**Symptom.** When a provider returns 429 with a `Retry-After` header telling us exactly how
long to wait, rupu ignores it and uses its own exponential backoff instead.

**Root cause.** `RetryingProvider` (`tuned.rs:151-174`) calls `(self.backoff)(attempt)`
unconditionally. Nothing in production ever parsed the header — see [[I-49]], which deleted
the inert stub that was *supposed* to.

**Impact.** Low today: the default is one 2s retry, so we rarely retry long enough for the
server's hint to matter. It becomes real for anyone raising `max_retries`, where ignoring an
explicit `Retry-After: 30` means hammering a provider that told us to back off — which is
how rate-limit bans escalate.

**Fix.** Parse `Retry-After` from the response headers at the client boundary (rupu-scm
already does this correctly at `error.rs:144` and is the model to copy), surface it via the
already-existing `ProviderError::RateLimited { retry_after }`, and have `RetryingProvider`
prefer it over its computed backoff when present. Related: [[I-49]].

---

### I-84 — Anthropic's nested retry loops multiply attempts

**Symptom.** Anthropic requests can retry far more times than any single configured limit
suggests.

**Root cause.** Two loops nest: an outer `MAX_STREAM_IDLE_RETRIES` loop (`anthropic.rs:1088`)
wraps an inner `max_rate_limit_retries` 429 loop (`:1101`). Attempts **multiply** rather than
add, so the effective ceiling is the product of the two.

**Impact.** Worst case is a long stall against a provider that is already rate-limiting us.
Note this is *not* the decorator-stacking problem — that one is correctly handled:
`provider_factory.rs:331-337` excludes Anthropic from `RetryingProvider` precisely because it
has native retry, pinned by `only_anthropic_skips_the_retry_decorator`. This is a defect
*inside* the native implementation.

**Fix.** Give the two loops a shared attempt budget, or make the outer loop not re-enter the
inner one after a 429-driven exhaustion. Needs a decision about intended total attempts.

---

### I-85 — four hand-written reasoning-effort ladders, two duplicated verbatim

**Symptom.** `ThinkingLevel` is a single shared enum, but every provider hand-writes its own
translation with no shared helper: Anthropic → `budget_tokens`
(`anthropic.rs:1338-1349`), Gemini → level + budget (`google_gemini.rs:357-371`),
openai_wire → `reasoning_effort` string (`:178-185`), Codex → `reasoning.effort` string
behind a model gate (`openai_codex.rs:537-546`).

**Root cause.** Incremental provider addition, each pass adding a `match` rather than
extending a shared mapping.

**Impact.** The last two are **byte-identical duplicated `match` blocks in two files**, so
any per-provider constraint (see [[I-47]]) has to be written twice or it silently diverges.
Maintenance risk rather than a live defect.

**Fix.** Factor the translation into one place keyed by wire format. Do this **before**
adding any per-provider allowlist, not after, or the allowlist becomes a third copy.

---

### I-82 — a web approve's true actor is lost before the resume worker spawns

**Symptom.** Approving a gate from the CP web UI records the decision, but the *identity*
of whoever approved does not reach the gate decision row that [[I-36]]/[[I-38]] added.

**Root cause.** `request_resume_approval` resolves an approval decision, but the CP's
`approve_run` handler discards the returned value, so the actor is gone before the
cross-process resume worker spawns and re-enters the runner. The same false comment
I-36 corrected on `approve_gate`/`reject_gate` — *"identity recorded in transcript via
runner re-entry"* — also sat on `request_resume_approval`; the comment was fixed there,
but unlike the other two the plumbing behind it was **not** built, because it crosses a
process boundary rather than staying inside one call chain.

**Impact.** Audit-quality, not correctness: the decision, reason, timestamp and `via` are
all recorded correctly; only the *who* is missing, and only for web-initiated approvals.
CLI and sweep-driven paths carry the actor properly after I-36/I-38. Filed rather than
folded into I-36 because it needs the resume-worker handoff to carry the actor across the
spawn, which is a different piece of work from the in-process threading.

**Fix.** Have `approve_run` persist the resolved decision's actor (the marker the resume
worker already reads is the natural carrier), and have the worker thread it into
`resume_run`'s existing `approver` parameter — which now exists, so the receiving end is
already in place.

---

### I-81 — `tools_list_matches_snapshot` fails in this worktree

**Symptom.** `cargo test -p rupu-mcp` reports
`tools_list_matches_snapshot ... FAILED — tools/list snapshot drift`
(`crates/rupu-mcp/tests/schema_snapshot.rs:36`).

**Not caused by this program.** Verified: `git diff origin/main..HEAD --name-only` shows
**zero** `rupu-mcp` files changed across Arcs 1–4. The last commit to touch that snapshot
was itself `8cea2495 test: bless tools_list snapshot (field order)`, i.e. it has drifted on
field ordering before.

**Root cause (probable).** Same class as [[I-4]] and [[I-5]]: this worktree runs Homebrew
Rust 1.95 against a `rust-toolchain.toml` pinned at 1.88, and `schemars` emits schema
properties in a different order under the newer compiler. Not confirmed by bisect — filed
so the known-red baseline is written down rather than rediscovered each arc.

**Fix.** Confirm it reproduces on a clean `main` under 1.95 and passes under 1.88. If so it
is an environment artifact, and the durable fix is making the snapshot order-insensitive
(compare parsed JSON with sorted keys) rather than blessing it again — a blessed snapshot
will just re-drift for the next person on a different toolchain.

---

### I-80 — reject-timeout gates now resolve only via `cp serve`'s sweep

**Symptom.** After I-25, a gate with `on_timeout: reject` whose deadline has passed
stays parked at `AwaitingApproval` indefinitely unless `rupu cp serve` is running.
Before I-25, the next `rupu workflow runs` would resolve it.

**Root cause.** Deliberate, and the correct trade for I-25: a listing command must
not execute cleanup chains, and because `sweep_decision` only acts on runs still
`AwaitingApproval`, the listing cannot finalize the run either without silently
losing the chain. Resolution therefore moved entirely to the `cp serve` gate sweep
(`[cp].gate_sweep_enabled`, default on at 60s).

**Impact.** Affects a real class of user: anyone driving rupu purely from the CLI who
never starts `rupu cp serve`, plus anyone who has set `[cp].gate_sweep_enabled = false`.
Their reject-timeout gates no longer auto-resolve.

**Scope widened by [[I-35]] (Arc 4).** The sweep is now also the executor of `on_reject`
cleanup for **web, desktop-app and cancel** rejects, which previously ran no chain at all.
So the same "needs `cp serve`" dependency now covers materially more behavior than
timeout routing alone. This is still a strict improvement over the prior state for those
paths (cleanup eventually runs, versus never), but it raises the stakes on documenting the
dependency: an operator who never starts `cp serve` now has *both* unresolved
reject-timeout gates *and* unexecuted cleanup chains, with nothing surfacing either. This is a behavior change rather
than a safety regression — the gate stays parked (fail-safe) rather than firing
unexpectedly — and `rupu workflow reject <id>` still resolves it manually at any time.
It is filed because the dependency is currently invisible: nothing tells such an
operator that timeout routing needs a daemon.

**Fix.** Options, in rough order of preference: document the dependency plainly in the
gate/timeout docs and in `rupu workflow runs` output when it sees an overdue reject
gate it is deliberately not resolving; or add an explicit opt-in
`rupu workflow runs --resolve-expired` so the side effect is requested rather than
implicit; or have the CLI resolve overdue reject gates on the `reject`/`approve`
paths only, which is already true. Related: [[I-25]].

---

### I-78 — a workflow step at `--mode ask` still gets `BypassDecider`

**Symptom.** `rupu workflow run --mode ask` grants every agent step unrestricted
tool access. `bash`, `write_file` and `edit_file` all execute without a prompt and
without a denial, which is precisely `bypass` behavior under a different name.

**Root cause.** `DefaultStepFactory::build` (`crates/rupu-orchestrator/src/step_factory.rs`)
hardcoded `decider: Arc::new(BypassDecider)` for every workflow step regardless of
the run's mode. `BypassDecider`'s own doc comment describes it as a "Test/CI
decider… always Allow". I-24's fix introduced `ReadonlyDecider` and wired it in for
`readonly`, but deliberately left `ask` on `BypassDecider`, because the agent
runtime's interactive `ask` decider blocks on stdin and a workflow step has no
operator present to answer it.

**Impact.** Discovered while building I-24's validation test — without it, that fix
would have had no observable effect at the tool layer. Two live gaps remain. First,
an operator choosing `ask` over `bypass` reasonably expects *more* restriction and
silently gets none; this is the "silent no-op config" class the whole program
exists to eliminate. Second, `ask` is the **default** when `--mode` is omitted, so
every workflow run that doesn't pass a mode is effectively running at bypass.

**Fix.** Decide what `ask` means in an unattended context — there is no operator to
prompt, so it must resolve to something non-interactive. The plausible options are
to treat it as `readonly` (deny writers), to gate it behind the existing approval
machinery so the *workflow* asks rather than the tool layer, or to reject
`--mode ask` on a workflow run and force an explicit choice. Whichever is chosen,
the current state — `ask` silently meaning `bypass` — is not defensible, and the
default-mode case makes it P1 rather than P2. Related: [[I-24]].

---

### I-79 — the action dispatcher's `["*"]` allowlist is sound only by invariant

**Symptom.** `action_dispatcher_for` (`crates/rupu-cli/src/resume.rs:27`) builds its
`McpPermission` with a wildcard tool allowlist, `vec!["*".into()]`.

**Root cause.** The dispatcher is constructed once per run, while the tool it may
call is per-step, so the construction site has no single tool name to narrow to.

**Impact.** No exploitable hole **today**, and this was verified rather than
assumed: `opts.action_dispatcher` has exactly three production consumers
(`runner.rs:4293`, `:4700`, `:4923`), all of which funnel into `execute_action_step`,
whose only dispatch is `dispatcher.call(tool, …)` with `tool = step.action`. That
tool is validated against the live MCP catalog at parse time by
`validate_action_step`, and a step may not carry a non-empty `actions:` alongside
`action:` (`ActionsOnActionStep`). Agent-step tool calls never touch this dispatcher.
So the wildcard is currently unreachable — but its safety rests on an invariant
enforced three modules away, and any future code that hands this dispatcher to a
less constrained caller turns it into a real hole with no local signal.

**Fix.** Narrow the allowlist to the single tool being invoked. This needs
`execute_action_step` to build (or be handed) a per-step dispatcher, which means
threading the registry rather than `&ToolDispatcher` through three call sites —
mechanical but not free, hence P2. Defense in depth, not a live defect. Related:
[[I-26]].

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

### I-50 … I-52, I-58, I-63 … I-69, I-71, I-72 — the docs truth pass (batch)

Closed together as one sweep across every doc except `docs/workflow-format.md`. Each was
verified against the code rather than against the filed text, and **two filed items were
themselves wrong**.

- **I-50** — README claimed "Rust 1.77+" in two places; the real pin is **1.95**
  (`rust-toolchain.toml:2`, `Cargo.toml:32`). Note the *issue* said 1.88 — **the tracker
  entry describing a doc-truth bug had itself gone stale**, which is a fair illustration of
  why this arc exists.
- **I-51** — README ×2, `docs/agent-format.md` and `docs/triggers.md` all still said
  `actions:` is *not* a tool allowlist. It has been a real connector-subset narrowing since
  #533/#537. The nuance now stated correctly: **builtins are never narrowed** — it
  constrains connector/MCP tools only (confirmed in `step_factory.rs`'s
  `narrow_agent_tools`).
- **I-52** — `rupu run --agent <name>` was documented in two live pages and **that flag does
  not exist**; `rupu run --help` offers only `--format`. The real form is positional:
  `rupu run <agent> [target] [prompt]`. Files under `docs/superpowers/plans/` were left
  alone as dated records.
- **I-58 / I-72** — new `docs/configuration.md` documenting the real `~/.rupu/config.toml`
  schema with **verified** defaults (e.g. `max_retries` = 1, sweep defaults on/60s,
  `MAX_WORKSPACE_BYTES` = 256 MiB), including `[pricing]`. Deprecated-but-accepted keys
  (`[retry]`, `[cp].agent_authoring_ui`, `[cp].workflow_editor_ui`) are marked as inert
  shims, so nobody deletes them from a config expecting a behavior change or, worse, adds
  them expecting one.
- **I-63** — added exactly six frontmatter keys (`outputSchema`, `dispatchableAgents`,
  `concerns`, `maxTokens`, `contextWindowTokens`, `compactAtPercent`) and three builtin
  tools (`ast_grep`, `dispatch_agent`, `dispatch_agents_parallel`), verified against
  `AgentSpec`/`Frontmatter` and `default_tool_registry()`.
- **I-64** — documented **both** `action_emitted` shapes (the dead legacy finding shape and
  the live action-node shape — see [[I-40]]). The filed issue said *two* event types were
  undocumented; **there were three**: `assistant_delta`, `usage` and `tool_audit`, all
  confirmed to have real production writers. All three documented.
- **I-65** — the capability matrix claimed Linear and Jira have no `issues.*` support. Both
  implement the full `IssueConnector` (`list/get/comment/create/update_state`) and are wired
  into the registry. Matrix corrected.
- **I-66** — "workflow steps and subagents don't support openai-compatible" was stale; they
  resolve it today.
- **I-67** — documented `rupu cp`, `host`, `node`, `update`. The filed issue **overstated
  scope**: `session`, `autoflow` and `cleanup` were already thoroughly documented.
- **I-68** — `--gate <step-id>` on `workflow approve|reject` confirmed to exist and now
  documented, including why it is required when several gates are parked at once.
- **I-69** (the only code change) — `rupu cp serve --help` described an HTTP server only.
  It now names all three background loops (autoflow reconciler, cron/event tick, gate
  sweep) and their gating `[cp]` keys. **Verified by running `--help`.**
- **I-71** — subcommand table gained the seven missing commands; provider count corrected
  4 → 5. `google-antigravity` was deliberately *excluded* as an internal Gemini SSO variant
  rather than a separate user-facing provider.

**Validation.** `cargo build --workspace` clean; README MSRV now reads 1.95 in both spots;
`grep` for `run --agent` across live docs returns nothing; `rupu cp serve --help` verified
by execution. Follow-up filed as [[I-86]].

---

### I-70 — the gate/action design spec is flagged as drifted

**This is the drift that started the whole program.** The operator spotted that the spec's
`with:` examples would fail schema validation and that it lists `scm.prs.add_labels`, which
isn't in `tool_catalog()` — which prompted the audit that produced this tracker.

**Verified.** `scm.prs.add_labels` appears in the spec's §3 v1 action set and **nowhere in
the code** — a workflow copying it fails at parse time with `ActionsUnknownTool`. The real
shipped catalog is 17 tools: `issues.create|comment|get|list|update_state`,
`scm.prs.create|comment|get|list|diff`, `scm.branches.create|list`, `scm.repos.get|list`,
`scm.files.read`, `github.workflows_dispatch`, `gitlab.pipeline_trigger`. The spec also
predates two rules it therefore violates: `ActionsOnActionStep` and Arc 4's templated-value
coercion.

**Fix: a dated correction banner, not a rewrite.** Specs in this repo are records of what
was approved on a date; editing the body would falsify the record. This is the same
reasoning Arc 3 used to leave 18 historical plan/spec files untouched. The banner states
plainly that the YAML must not be copied, lists the real catalog, and points at the
authoritative references (`docs/workflow-format.md` and `rupu mcp` / `GET /api/tools`) — so
the document stays honest as history without misleading anyone who reads it as guidance.

---

### I-53 + I-61 — closed by Arc 4, recorded here for the ledger

Both were fixed while closing [[I-37]] in Arc 4 and are recorded rather than re-worked.

**I-53 — gate nodes were entirely undocumented.** Found while writing I-37's `cp serve`
dependency note: standalone `approval:` gate nodes had **no user-facing documentation at
all** — `on_timeout`, `auto_approve`, `on_reject` and `notify` appeared nowhere in `docs/`.
A shipped feature with no docs. `docs/workflow-format.md` now has a "Gate nodes (standalone
`approval:` steps)" section with the field table, the `steps.<gate-id>.decision` note, and
the unattended-routing subsection.

**I-61 — "timeouts are enforced lazily on the next run-store interaction" was stale.** True
when written; **false since Arc 2** made the runs listing side-effect-free ([[I-25]]).
Removed in the same edit, and replaced with the accurate statement that timeout routing is
performed by `cp serve`'s gate sweep — which is what [[I-80]] tracks the operator
consequence of.

**Validation.** `grep -c "Gate nodes (standalone" docs/workflow-format.md` → 1;
`grep -c "enforced lazily" docs/workflow-format.md` → 0.

---

### I-45 + I-46 — the Gemini thinking config is model-gated and lowercase

Closed together: both lived in the same `serde_json::json!` literal, so they were one edit.

**Symptom.** For every non-`Auto` reasoning level, the Gemini request set **both**
`thinkingLevel` and `thinkingBudget` in `generationConfig.thinkingConfig`, with the level
string UPPERCASE.

**Root cause, wider than filed.** `thinkingLevel` is a **Gemini-3-only** key, but there was
**no model gate anywhere in `build_request_body`** — so the identical body went to
`gemini-2.5-pro`, this provider's `ModelTier::Default`, sending it an unknown key. The filed
issue framed this as Gemini-3-specific; the blast radius was actually *wider*. Separately
the level strings were hardcoded uppercase; serde's `rename_all = "snake_case"` on
`ThinkingLevel` governs config *parsing*, not the wire, so it never applied here.

**Fix.** A model gate on `request.model.starts_with("gemini-3")` — a prefix match, so it is
robust to `-preview` and future Gemini-3 variants while never matching `gemini-2.5-*` or
`gemini-2.0-*`. Gemini 3 gets `thinkingLevel` (lowercase) only; 2.5 and earlier get
`thinkingBudget` only. Never both. `Auto`'s existing `thinkingBudget: -1` sentinel is
unchanged.

**Honesty note — this is conformance, not a repaired failure.** The "guaranteed 400" in the
original issue is **doc-derived and was never observed in this repo**; the sole source is
our own design doc reading Google's documentation, which also says of the casing
*"Unverified whether the API tolerates both."* The fix conforms to Google's documented
contract. No commit message, comment or test claims an observed rejection, and the code
comment records the uncertainty explicitly.

**Validation.** RED: `test_build_request_body_with_thinking` previously asserted
`thinkingLevel == "MEDIUM"` **and** `thinkingBudget == 8192` on a `gemini-2.5-pro` request —
a test actively pinning both defects. Three such tests were revised. Two added, including
`test_thinking_config_never_sends_both_keys`, which loops over **both model families × all
five non-`Auto` levels** and asserts mutual exclusion — that invariant, rather than any
single body shape, is the thing worth pinning. `rupu-providers` 511 passed / 0 failed.

---

### I-47 — the effort→wire translation is documented, deliberately not clamped

**The filed claim was not supported.** It said `minimal`/`xhigh` are *"rejected outright by
DeepSeek, Groq and xAI"*. Verification found:

- Forwarding without an allowlist **is** real: `openai_wire.rs` maps `Minimal → "minimal"`
  and `Max → "xhigh"` and passes them through with no per-provider or per-model gate.
- **"Rejected outright" is unproven.** This repo's own spec says vendor behavior on an
  unknown `reasoning_effort` — 400 versus silent ignore — is *"undocumented across
  vendors."*
- The per-vendor ladders in the title were garbled, and **none of DeepSeek, Groq or xAI ship
  as providers** — no preset, no `ProviderId`. They exist only if an operator declares an
  openai-compatible entry pointing at them.

**Documented rather than clamped, deliberately.** Clamping `minimal`/`max` for
openai-compatible endpoints was considered and rejected: it would **silently downgrade an
explicit `effort: max`** on endpoints that *do* support `xhigh` (OpenAI's own gpt-5.5 does),
to guard against a rejection we have no evidence occurs. Silently overriding a user's
explicit setting is the failure mode this program exists to remove, not add.

**A wider gap surfaced while writing it:** the per-provider translation was documented
**nowhere**. `docs/agent-format.md` listed the accepted `effort` values but never said what
any of them become on the wire. Now documents all five wire forms (Anthropic budget tokens,
Gemini 3 level, Gemini 2.5 budget, Codex `reasoning.effort`, openai-compatible
`reasoning_effort`), the `auto` special case, and the caveat with a concrete fallback — drop
to `low`/`medium`/`high` — for anyone hitting an endpoint that rejects the ends of the
ladder.

Unifying the four hand-written ladders is tracked separately as [[I-85]], and must be done
**before** any per-provider allowlist or the allowlist becomes a third copy.

---

### I-48 — Gemini reasoning tokens are counted in usage and cost

**Promoted P1 → P0 during verification:** this was the **only** defect in Arc 5 reachable
from a default config, and the only one that costs users money. `gemini-2.5-pro` thinks by
default, so every stock Gemini run was affected.

**Symptom.** Gemini spend was silently under-billed and run token totals under-reported,
with no `cost_partial` marker — so a user had no way to tell.

**Root cause.** Both Gemini usage sites read exactly two `usageMetadata` fields
(`promptTokenCount`, `candidatesTokenCount`). `thoughtsTokenCount` appeared **nowhere in the
workspace**, and `Usage` had no field to hold it. Gemini reports thinking tokens *outside*
`candidatesTokenCount` while Google bills them at the output rate, so those tokens were
multiplied by zero. The same `output_tokens` feeds `total_out`/`tokens_out`, so run totals
and the context-budget arithmetic under-reported too.

**Provider-specific, not a general gap** — worth stating because it looks like one: no
provider in rupu reads a reasoning-token field, but Anthropic and the openai-compatible path
**don't need one**, since their `output_tokens`/`completion_tokens` already include
reasoning. Gemini is the only provider where the omission loses tokens.

**Fix.** `Usage.reasoning_tokens`, populated from `thoughtsTokenCount` at both the
non-streaming and streaming parse sites. `rupu-agent`'s runner folds it once into
`billable_output_tokens = output_tokens + reasoning_tokens`, which feeds `total_out`,
`Event::Usage.output_tokens` and `Event::TurnEnd.tokens_out` — so the number reaches the
persisted transcript and every downstream cost call, rather than stopping at the struct
boundary. Anthropic and openai-wire sites set `reasoning_tokens: 0` explicitly with a
comment, so no path double-counts.

**Validation.** RED observed with real numbers: `thoughtsTokenCount: 200` parsed to
`Usage { input: 15, output: 8 }` — 200 tokens dropped — and billed identically to a
non-thinking response. The binding test is
`gemini_reasoning_tokens_increase_run_totals_and_cost` (rupu-agent), which drives the **full
pipeline** and reads the persisted JSONL back: run total 8 → 208 tokens, cost strictly
greater. Plus parse tests on both Gemini paths.

**A dead second mechanism was removed rather than left in.** The fix initially also added
`ModelPricing::cost_usd_with_reasoning`, whose test was cited as the binding cost test — but
it had **no callers outside its own module**, so that test exercised a function nothing
calls: precisely the validation-bar failure the charter exists to prevent. It was also a
double-billing hazard, since every production caller sources `output_tokens` from the
already-folded transcript. Deleted, with the contract documented on `cost_usd` so it is not
reintroduced.

---

### I-49 — the dead `classify` module is deleted

**The filed statement was false at every constant.** It claimed `parse_retry_after` being a
stub meant "every 429 backs off a flat 60s". In fact:

- `classify_anthropic`/`openai`/`gemini`/`copilot` had **zero production callers** — only
  `crates/rupu-providers/tests/classify.rs`. Every real client builds `ProviderError::Api`
  directly, and `ProviderError::RateLimited` was constructed **only inside `#[cfg(test)]`**.
  The stub was inert; nothing in production ever reached it.
- The real default backoff is `RetryingProvider` with `DEFAULT_MAX_RETRIES = 1` and
  `2000ms << attempt` — **one 2-second retry**. 60s is the cap after ~5 retries, and the
  only literal 60s is a `ModelPool` availability window whose sole caller passes `None` and
  whose type has no production constructor at all.

**The stub could never have worked as written.** It takes `body: &str`, but `Retry-After` is
an HTTP **header**. `rupu-scm` gets this right (`crates/rupu-scm/src/error.rs:144` parses it
from a `HeaderMap`) — so the correct implementation already exists elsewhere in the
workspace, against a different input type.

**The module docstring was also false**: *"Each adapter calls its corresponding `classify_*`
at the boundary between raw HTTP and the agent loop."* No adapter did.

**Fix: deleted**, matching how [[I-27]] was handled in Arc 2. Wiring these in would be a
*behavior change* — honouring server-supplied `Retry-After` — and that deserves its own
scope rather than arriving as a side effect of removing dead code. Filed as [[I-83]].

`ProviderError::RateLimited` is deliberately **kept**: it is the natural target once
`Retry-After` is honoured, and removing a public error variant is a breaking change for no
benefit.

**Validation.** `cargo build --workspace` clean and `rupu-providers` green after removing
133 lines of source, its test file, and the `pub mod` — the compiler is the proof for a
deletion, and nothing referenced it.

---

### I-36 + I-38 — gate decision provenance is recorded on every path

Closed together because they were one gap: **the gate audit record could not tell you who
decided, or whether a human decided at all.**

**I-36.** An **empty** `on_reject` chain short-circuited past `run_reject_cleanup` — which
is the **only caller of `emit_gate_result`** — so no gate decision row was written at all.
Separately, `RunStore::reject_gate` and `approve_gate` both explicitly discarded the actor
(`let _ = approver;`), under a comment claiming *"identity recorded in transcript via runner
re-entry"*. **That comment was false**: `emit_gate_result` had no approver field.

**I-38.** When the `cp serve` sweep resolved an `on_timeout: approve` gate it spawned
`rupu workflow approve`, which landed in the normal approve path and emitted
`via: "human"` — a machine-initiated approval was indistinguishable from an operator's.
Reject already threaded `"timeout"` correctly; the asymmetry was the bug.

**Fix.** The decision JSON gains an `approver` field, and the plumbing the false comments
*claimed* existed was actually built: approver → `ApprovalDecision` →
`ResumeState::from_approval_with_actor` → `run_reject_cleanup`'s new `approver` param →
`emit_gate_result`. Timeout provenance is threaded through the approve path so a
sweep-driven approval records `via: "timeout"`. The I-36 unconditional-cleanup fix was
applied to **all three** sites sharing that guard — CLI reject, CLI approve's
`ExpiredRejected` arm, and the cp-serve sweep — not only the one named in the plan;
`cheap_on_reject_chain_len` thereby became dead and was removed.

**Validation.** Three tests, all RED pre-fix for the right reasons (no gate row /
`via:"human"` / `approver:null`): empty-chain reject records a decision with reason **and**
actor; a sweep timeout-approve records `via == "timeout"`; and a genuine operator approve
still records `via == "human"` with the operator's identity — that third one is the guard
that stops the timeout fix over-applying.

**A third false comment** of the same wording was found on `request_resume_approval` and
corrected, but its cross-process resume-worker plumbing was **not** wired — see [[I-82]].

---

### I-39 — an unknown `StepKind` no longer drops the whole step result

**The filed title was wrong and the real defect is worse.** It said old binaries "die" on a
new `events.jsonl`. Nothing dies: every reader is tolerant and **silently skips the line**
(`api/events.rs`, `api/graph.rs`, `transcript_tail.rs`, `executor/file_tail.rs`,
`agent/runner.rs` ×3, and `RunStore::read_step_results`, whose own comment says *"Skip
malformed rows rather than failing the read"*).

The severe case is **`step_results.jsonl`**: skipping a row means an older binary resuming a
run **silently loses a prior step's output** — the template context loses values and steps
re-run, with no error surfaced anywhere. `StepKind` had 10 variants and no `#[serde(other)]`;
`#[serde(default)]` on `StepResultRecord.kind` covers only a *missing* field, not an unknown
value. The `Split`/`Join`/`Loop` doc comments narrate **three separate rounds** of exactly
this variant-addition, so this had already bitten repeatedly.

**Fix.** `#[serde(other)] Unknown` as the final variant (serde requires it last), plus the
11 resulting exhaustive-match arms across `cmd/autoflow.rs`, `output/live_run.rs` and
`output/workflow_printer.rs` — falling back to the generic linear rendering, or
`unreachable!()` only where an outer match had already narrowed the kind.

**Validation.** RED verified by toggling `#[serde(other)]` off (the row was silently
dropped — `rows.len() == 1` instead of 2) and back on. The binding assertion is that the
**data survives**, not the label: `future.output == "future output survives"`.

**`events.jsonl` deliberately not covered.** It is internally-tagged with struct-only
variants, so the same treatment would require new no-op arms in GUI-adjacent `rupu-app` and
CLI live-view matches — which this repo's rules say need runtime rendering validation a
subagent cannot perform. Since resume reads `step_results.jsonl` exclusively, `events.jsonl`
is diagnostic-only and did not clear the "cheap" bar. A defensible scope call, recorded
rather than silently skipped.

---

### I-42 — a `when:`-skipped gate or action keeps its kind

**Symptom.** A gate or action step skipped by a false `when:` guard persisted as
`kind: "linear"` in `step_results.jsonl`, losing the fact that it was ever a gate or an
action.

**Root cause.** Of the four skip sites, **two set `kind` and two did not**. The `when:`-skip
sites (scheduler and linear loop) built their `StepResult` with `..Default::default()`, and
`StepKind::default()` is `Linear`. The `when:` check runs *before* the gate/action checks,
so the kind was never derived. The two sites that do it correctly — prune/cancel and
branch-not-taken — sit ~60 lines away in the same file, which is what makes this an
omission rather than a decision.

**Fix.** Both `when:`-skip sites now set `kind: step_kind_for_run_record(step)`, exactly as
their neighbours do. Two lines.

**Validation.** Three tests observed RED (`Linear` vs expected `ApprovalGate`/`Action`),
covering both the scheduler and linear-loop paths for a gate, plus an action step.

---

### I-43 — the gate sweep no longer re-spawns `approve` forever

**Symptom.** For an `on_timeout: approve` gate the sweep spawns a detached
`rupu workflow approve`. If that spawn failed — or the run was still `AwaitingApproval` and
overdue on the next tick — it spawned another. Every 60 seconds. Indefinitely.

**Root cause.** The sweep claims a lease via `claim_resume`, then calls `clear_resume`
**unconditionally** after spawning, including when the spawn itself failed. `clear_resume`
nulls `resume_claimed_at`, so the next tick's `claim_resume` returns `true` again. The
in-memory `rec.awaiting` mutation is explicitly in-memory only and the next tick reloads
from disk. There was no marker, no backoff and no attempt counter; the only thing that
stopped repetition was the child successfully flipping the status — which the sweep never
verified.

**Fix.** Do not clear the resume claim when the spawn failed, so the existing 5-minute
`RESUME_LEASE` TTL acts as the backoff. No new marker, counter or config knob — the
infrastructure was already there for the web-approve race, and reusing it keeps one
concept rather than two.

**Validation.** `run_gate_sweep_does_not_respawn_forever_after_spawn_failure`. The binding
assertion is on the **second** tick — `resume_claimed_at` is unchanged between tick 1 and
tick 2, proving no re-claim and therefore no re-spawn. Asserting only on the first tick
would have proved nothing.

---

### I-44 — notify hook transcripts are no longer orphaned

**Symptom.** Every `notify:` hook on a gate wrote a transcript `.jsonl` that **nothing
referenced** — a ULID-named file, unrecoverable by path, invisible to `show-run` and the CP,
never collected.

**Root cause.** `fire_notify_hooks` built a synthetic step and called
`execute_action_step`, then **discarded the returned `Ok(StepResult)`**. Meanwhile
`execute_action_step` unconditionally writes its transcript. And because
`continue_on_error: true` is passed, even a *failed* hook returns `Ok` — so the file was
written and dropped on **every** path, not just the error path.

**Fix.** Persist the notify hook's `StepResult` (id `<step>.notify`) like every other step,
threading `run_id`/`step_results` into `fire_notify_hooks`. Suppressing the transcript was
the alternative and was rejected: the audit trail is the entire point of a notify hook —
these are the calls that tell an outside system a gate is waiting.

**Validation.** `notify_hook_transcript_is_referenced_by_a_persisted_step_result`, observed
RED.

---

### I-41 — the steps table now renders gate and action nodes

**Re-scoped from the filed title.** It claimed `rupu workflow show` renders gates and
actions as `linear`. Only the **steps table** does. The graph renderer
(`rupu-app-canvas`'s `git_graph.rs:278,308`) already emits `gate` and `action · <tool>`
correctly, and the graph is the primary view — so this was never "show is broken".

**Root cause.** `workflow_step_table_summary` had arms for `parallel`, `panel` and
`for_each`, then fell through to `linear`. A gate printed `KIND=linear` with a **blank**
PRIMARY column (gates have no `agent:`), and an action printed the same while **never
naming the tool it calls** — which is the entire identity of an action step.

**Fix.** A gate arm (prompt in PRIMARY; `auto_approve`/`timeout`/`on_timeout`/`on_reject`/
`notify` in DETAIL) and an action arm (tool in PRIMARY, sorted `with:` keys in DETAIL —
sorted so the column is stable across runs).

**Validation.** Three tests, two **observed RED** by disabling the new arms
(`left: "linear"`, `right: "gate"` / `"action"`). The third is a guard against the new arms
over-matching: an agent step that merely *carries* an inline `approval:` is **not** a gate
node and must still render `linear` with its agent name — it stayed green throughout,
proving the guard is real rather than incidental.

---

### I-37 — the `cp serve` dependency for timeout routing is documented

**Fixed as documentation, deliberately — and the code was left alone.** Making the runs
listing resume an `on_timeout: approve` gate would **directly re-break [[I-25]]** from
Arc 2, whose entire point is that a read command has no external side effects. The
hint-only behavior is correct; what was missing is that nothing told the operator there is
no automatic non-`cp serve` resume path.

**A larger gap was found while fixing it:** standalone `approval:` **gate nodes were
entirely undocumented** — `on_timeout`, `auto_approve`, `on_reject` and `notify` appeared
nowhere in `docs/`. A whole shipped feature had no user-facing documentation.

**Fix.** `docs/workflow-format.md` gains a gate-node section: the field table, the
`steps.<gate-id>.decision` note, and an explicit subsection stating that
`timeout_seconds`/`on_timeout` are acted on **only** by a running `rupu cp serve`, with the
concrete manual fallback for each case. It also folds in the widened [[I-35]]/[[I-80]]
consequence: `on_reject` chains triggered from the web UI, the desktop app or
`workflow cancel` are recorded as pending and executed by the same sweep, so they wait on
`cp serve` too.

Removed the stale line *"timeouts are enforced lazily on the next run-store interaction"* —
true when written, false since Arc 2 made the listing side-effect-free.

---

### I-35 — every reject and cancel path now runs the `on_reject` chain

**Symptom.** Rejecting a gate from the CP web UI, the desktop app, a local/http host
connector, or **cancelling** an awaiting run silently skipped the gate's `on_reject`
cleanup chain. The operator saw a rejected run and reasonably believed cleanup had run.
It never had, and nothing would ever retry it.

**Root cause.** `run_reject_cleanup` is not merely "cleanup" — it is **the only caller of
`emit_gate_result`**, so a path that skips it also records *no gate decision at all*.
Five paths skipped it: CP web `POST /api/runs/:id/reject`, `LocalHostConnector::reject_run`,
`HttpHostConnector::reject_run`, the `rupu-app` desktop reject, and — the sneakiest —
`RunStore::cancel`, whose `AwaitingApproval` arm calls `self.reject(...)`, driving **both**
CLI `workflow cancel` and CP web `/cancel`.

**Neither `cp serve` worker covered it**, verified rather than assumed: the resume worker
filters on `resume_requested_at.is_some()`, which a reject never sets; and the gate sweep
matched only `AwaitingApproval`/`Running`/`Pending`, so an already-terminal `Rejected` run
fell to `_ => {}` forever. The library documented the gap against itself at
`runs.rs:1627-1634`.

**Corrections to the filed issue** (it would have misdirected the work): there is **no
TUI** — the affected GUI is the GPUI desktop app. And SSH/tunnel connectors were wrongly
blamed: they **do** run the chain, because they re-enter the CLI.

**Fix — marker + sweep, respecting the read-only boundary.** `rupu-cp` is deliberately
read-only ("record-in-CP, resume-in-cp-serve") and must not grow a workflow runtime, so
the chain is *not* called from `rupu-cp`. Instead `RunStore::reject_gate` records a
`reject_cleanup_pending` marker when it finalizes a run `Rejected`, and the `cp serve`
gate sweep grew an arm that picks those up, runs `build_reject_cleanup_opts` +
`run_reject_cleanup`, and clears the marker.

Because the marker lives in `RunStore::reject_gate`, the desktop app
(`InProcessExecutor::reject` → `run_store.reject`) and **both** cancel paths
(`RunStore::cancel` → `self.reject`) are covered with no caller-specific wiring — confirmed,
not assumed. The CLI's own `reject` clears the marker after its existing synchronous run,
so the sweep never double-executes.

**The sweep arm matches on the MARKER, never on `status == Rejected`.** That is deliberate:
matching on status is exactly the failure class Arc 2 hit in [[I-25]], where finalizing a
run before the chain ran caused the sweep to skip it forever.

**Validation.** RED observed by commenting out the marker assignment — all three tests
failed at the marker assertion, then passed when restored.
`web_reject_leaves_marker_sweep_runs_cleanup_and_records_gate_decision` stands up a **real**
`rupu_cp::server::router` + `axum::serve` and rejects over genuine HTTP rather than calling
the handler directly; `cancel_on_awaiting_approval_run_leaves_marker_sweep_runs_cleanup`
covers the cancel path; and
`empty_on_reject_chain_clears_marker_and_does_not_reprocess_on_next_tick` proves a second
tick is a silent no-op (asserted by the absence of a duplicate gate row in
`step_results.jsonl`), so an empty chain cannot spin. Real chain execution goes through the
`RUPU_MOCK_PROVIDER_SCRIPT` seam with an actual `write_file` call.

**Operator consequence — tracked, not buried.** Cleanup for web/app/cancel rejects now
depends on `cp serve` running, the same dependency [[I-80]] already tracks for
reject-timeout gates. I-80's write-up is widened rather than a duplicate being filed.

---

### I-40 — an action step's rendered `with:` args are now visible

**The filed title was wrong**, and correcting it changed the work. It claimed action steps
show an *empty transcript*. They do not: `execute_action_step` writes **two** lines
(`ActionEmitted` and `ToolAudit`), and the viewer already had a dedicated standalone-action
fallback for `tool_audit` (`transcriptView.ts:364`) with a passing test at
`transcriptView.test.ts:369`. The node rendered fine.

**The real gap** was the `action_emitted` payload — the **rendered `with:` args**, plus
`allowed`/`applied`/`reason`. So an operator could see *that* an action ran but never
*what it sent*, which for a tool that comments on issues or opens PRs is the part that
matters.

**Root cause.** `action_emitted` fell through to `default: break`, under a comment calling
it a "dead/legacy shape". That was half true: **two distinct shapes share the event name.**
The legacy finding shape (`{action: 'report_finding', severity, summary}`) genuinely is
dead. The action-node shape (`{kind, payload, allowed, applied}`) is live and carries the
args. Treating both as dead discarded the live one.

**Fix.** The action-node shape's payload is stashed (queued by tool name, since a step may
call the same tool twice) and picked up by the `tool_audit` that immediately follows,
producing a **single merged entry** with `input` populated instead of `undefined`. The
legacy shape stays ignored.

**Why this respects the existing regression test rather than overriding it.** Two tests
deliberately asserted `action_emitted` stays ignored; the plan flagged that they must be
revised deliberately, not deleted. On inspection the real invariant that test protects is
*"an action call is surfaced exactly once, never twice"* — and merging preserves it
exactly. **The test passes unchanged.** Only its name and comment were corrected, because
"action_emitted stays dead" no longer describes what it guards. The stale
`transcriptView.ts` comment was corrected for the same reason — still true of
`user_message`, no longer true of `action_emitted`.

**Validation.** Two new tests: the args are visible on the merged entry, and a legacy
`report_finding` payload is **not** mistaken for action args. Suite 1854 → 1856 across 179
files, `npm run build` clean.

---

### I-33 — a templated scalar now reaches a typed tool parameter

**Symptom.** The `action:` feature's headline use case — *"comment on issue #N"*, where N
comes from a workflow input or a trigger event — was **literally unexpressible**. Writing
`number: "{{ inputs.number }}"` failed at dispatch with
`invalid type: string "42", expected u64`.

**Root cause.** Three layers each behaving reasonably, combining into a dead end:
`validate_action_step` (`workflow.rs:1319`) deliberately checks **keys only** — its own
doc comment says *"VALUES are not checked — they may be minijinja templates rendered at
runtime (the dispatcher's typed serde parse re-validates then)"*. `render_action_args`
(`runner.rs:4528`) rendered a string leaf to `serde_json::Value::String`
**unconditionally**. The dispatcher then did a typed serde parse (e.g.
`CommentIssueArgs { number: u64, .. }`) which rejects `"42"`. No coercion existed anywhere
(`deserialize_with`/`as_u64` in `crates/rupu-mcp/src`: zero hits).

Aggravating: a declared `type: int` input could not help, because `StepContext.inputs` is
`BTreeMap<String, String>` and `render_step_prompt` returns `String` regardless of source.
The same applied to `{{ event.issue.number }}`.

**Scope.** Every integer parameter in the catalog — `issues.get/comment/update_state.number`
(u64), `scm.prs.get/diff/comment.number` (u32), every `.limit`. The only workaround was a
literal unquoted `number: 7`, which is exactly what the repo's own fixture used
(`tests/action_step.rs:259`) — **every pre-existing test templated only string fields**,
which is why this survived.

**Fix.** `render_action_args` now coerces against the tool's declared JSON-schema type,
resolved through a shared `schema_scalar_kind` helper so "which types are coercible" has
exactly one definition, used by both the renderer and the parser. Two deliberate judgment
calls:
- **Coercion fires only when the schema declares `integer`/`number`/`boolean`** — never
  inferred from the value's shape. Inferring would mangle `"007"` and version strings in
  genuine string fields.
- **A *partial* template into a typed field** (`"issue-{{ n }}"`) is an author error and is
  rejected, not coerced.

Failures raise `RenderError::ActionArgType` naming the parameter and expected type, rather
than surfacing a bare downstream serde message. Parse-time checking was also added for
*literal* wrong-type values, so authors fail fast at `Workflow::parse` instead of mid-run.

**Validation.** RED observed as the exact predicted error. Tests:
`templated_numeric_field_reaches_the_connector_as_a_json_number` (asserts the connector
received a JSON **number**, not a string — the binding assertion),
`non_numeric_template_into_a_numeric_field_fails_naming_step_and_param`,
`numeric_looking_string_in_a_string_field_is_not_mangled` (the over-coercion guard), and
`action_step_literal_string_for_numeric_param_fails_parse`. A new `RecordingIssueConnector`
fixture was needed — the existing harness only had a `RepoConnector`, which is itself a
sign of how little the issue-tool path was exercised.

No tool's schema shape defeated the lookup. `workflows.dispatch.inputs` and
`pipeline_trigger.variables` are correctly left uncoerced by design.

---

### I-34 — an action step's output is now indexable

**Symptom.** `{{ steps.<action-id>.output }}` interpolated a raw JSON string that could
not be indexed. There was no way to get a field out of it, which made `action:` steps
effectively **write-only**: you could call a tool but never consume its result.

**Root cause.** The output is a JSON string end to end — the MCP dispatcher returns
`Result<String, _>` (`crates/rupu-mcp/src/dispatcher.rs:26`), `StepResult.output` is a
`String`, and it reaches minijinja verbatim as `StepOutput.output: String`
(`templates.rs:141`). minijinja is pinned with `features = ["json"]`, which registers
**only `tojson`** — there is no inverse filter in minijinja at all — and
`grep -rn "add_filter" crates/` returned **zero hits repo-wide**, so the crate registered
no filters of its own either. The only environment customization was
`env.add_function("read_file", …)`.

`tojson` goes the wrong way (it would double-encode an already-JSON string), and there is
no `split`/regex escape hatch that yields a typed value, so the only survivable pattern
was string surgery. Extracting a field was effectively impossible.

**Fix.** Registers a `fromjson` filter alongside `read_file` in `templates.rs`. There is
exactly one `Environment::new()` site in the crate, so one registration covers prompt
rendering and `when:` evaluation both. Invalid JSON fails the render naming the filter,
rather than yielding `undefined` — an undefined would render as `""` and silently ship an
empty value downstream, which is the failure mode this program exists to eliminate.

**Validation.** Five tests in `crates/rupu-orchestrator/tests/templates.rs`, 4 RED before
the change: indexing a field, **typed** round-trip (`> 5` comparison, proving the value is
a real number and not a string that merely renders the same), nested/array indexing,
`tojson` round-trip, and the invalid-JSON error.

That last test carries an explicit guard against **passing for the wrong reason** — before
the filter existed, minijinja's "unknown filter" error *also* contained the word
`fromjson`, so asserting only on that substring is satisfied by the filter being absent.
It now additionally asserts the message is **not** the unknown-filter one.

**Documented** in `docs/workflow-format.md` under a new "Template filters" section, with
the worked action-step example, the typed-comparison case, and a note that gates need no
equivalent because a gate decision is already pre-parsed as `steps.<id>.decision`. An
undocumented filter is a silent feature.

---

### I-31 — both UI hooks and their localStorage overrides are removed

**Symptom.** Two React hooks (`useAgentAuthoringUi`, `useWorkflowEditorUi`) resolved
the CP UI flags, each with an undocumented devtools-only localStorage override
(`rupu.cp.agentUi`, `rupu.cp.workflowEditorUi`) that let an operator flip the UI
behind the config's back. Nothing wrote those keys in production — they existed
purely so a developer could hand-set them in devtools.

**Fix.** With [[I-28]], [[I-29]] and [[I-30]] removing every *reader* of the flags'
values, what remained was dead plumbing. Both hook files and their tests are gone,
along with the `WorkflowEditorUi` type imports, the `workflowEditorUi` prop at every
declaring and forwarding site — including the five `StepForm` sub-components I-29
had temporarily renamed to `_workflowEditorUi` for signature stability — and the
threading in `pages/WorkflowDetail.tsx`.

The now-dead `node projection` describe in `WorkflowEditorGraph.test.tsx` was also
removed: it asserted the flag threads onto every projected node, which is no longer
something that happens. Comments in four settings cards and in `styles.css` that
described components rendering "only when `workflowEditorUi === 'next'`" were
corrected — that condition no longer exists, so the comments had become false.

**Validation.**
`grep -rn "workflowEditorUi\|WorkflowEditorUi\|useWorkflowEditorUi\|agentUi\|useAgentAuthoringUi" crates/rupu-cp/web/src`
returns **zero hits**, and neither localStorage key has a remaining reader or
writer anywhere in the tree. `npm run build` is TypeScript-clean — which is a real
check here rather than a formality, since `noUnusedParameters` and the prop-type
removals mean a half-finished removal cannot compile. `npx vitest run` green at
179 files / 1854 tests, down from 180 / 1859; the delta reconciles exactly as one
deleted test file (3 hook tests) plus the 2 dead node-projection tests.

**Note.** The two Rust config keys are intentionally *not* removed — they survive as
warn-only deprecation shims so an existing `config.toml` still parses. See [[I-28]]
for why deleting them would silently discard the operator's whole config.

---

### I-30 — the run-graph classic/next dual paths are collapsed

**Symptom.** The run-detail workflow graph rendered in two different visual
schemes depending on `[cp].workflow_editor_ui` — the *workflow-editor* flag,
reused for the run graph.

**Root cause.** Not two renderers, but two paint schemes inside one. `RunGraph`
resolved the flag, pushed it into every node's `data` as `ui`, and then each of
the six node components plus the edge memo re-derived `const next = data.ui ===
'next'` and branched. The classic arm painted flat `inkMute` edges with
`rg-edge-active`/`rg-edge-await`; the next arm painted per-kind accents from
`kindBridge` with `rg-edge-flow`.

**Fix.** The `next` scheme is now unconditional. `RunGraph` no longer calls the
hook, `ui` is gone from `NodeData` and from the data pushed to each node, the
classic edge arm is deleted, and all six node components inline the next branch.
`.rg-edge-active` and `.rg-edge-await` are removed from `styles.css` and the
neighbouring comment — which described a classic path that no longer exists — was
corrected. `components/graph/kindBridge.ts` is kept and becomes the
*unconditional* source of node and edge colour; `stepStyle.ts` is kept (it is also
used by `components/run/StepTranscriptBrowser.tsx`).

**Validation.** `npm run build` TypeScript-clean; `npx vitest run` green at
180 files / 1859 tests (down 2 from 1861 — the two deleted classic tests).
`grep -rn "rg-edge-active\|rg-edge-await" crates/rupu-cp/web/src` returns zero
hits and `grep -rn "data\.ui\b" crates/rupu-cp/web/src/components/` returns zero
hits, while `rg-edge-flow` and both kept modules remain in place.
`ContainerNodes.test.tsx` asserted *classic ≠ next* throughout; each such test was
collapsed to a next-only assertion rather than deleted, preserving coverage of the
surviving render.

**Flake caution recorded for the next person.** The web suite is load-sensitive.
A healthy run takes ~12s; under contention it stretched past 290s and produced
4 spurious failures that did not reproduce. Treat a failure as real only if it
reproduces on a fast run.

---

### I-29 — the classic workflow-editor UI is deleted

**Symptom.** Two workflow editors shipped side by side, selected by
`[cp].workflow_editor_ui` (default `"classic"`). Several prior plans explicitly
required the classic renderer stay "byte-identical", so the split was implemented
as inline early-returns and ternaries *inside shared files* rather than as separate
modules — meaning every editor change had to be authored twice, in the same file,
without breaking the other arm.

**Fix (web).** Every `next` arm is now unconditional across six components:
`WorkflowEditor.tsx` (default panel tab, collapsible source pane, palette portal,
hide-source toggle, resizable rail + separator, Blocks tab, tab order),
`WorkflowEditorGraph.tsx` (edge-memo gate, kind-tinted markers, `edge.animated`,
branch stroke, `✓ then`/`✕ else` label chips, plain-edge accent, `wfx-canvas`,
palette portal, background variant), `NodePalette.tsx` (item set now includes
BRANCH/GATE/SPLIT/JOIN, connector groups, plus deletion of the classic float dock
and the `KIND_COLOR` "classic-only fixed hex" map),
`WorkflowSettingsForm.tsx` (the classic read-only chip form is gone),
`StepForm.tsx` (`NEXT_ONLY_KINDS` no longer filters the kind picker, so `branch` /
`approval_gate` / `action` / `split` / `join` are always offerable; `size="large"`;
Approval prompt is an `ExpressionField` rather than a plain `<input>`), and
`nodes/EditableStepNode.tsx` (the classic Tailwind card is gone; the SVG-silhouette
render is the only one). `styles.css` comments that described the deleted arm were
corrected. `pages/WorkflowDetail.tsx` needed no change — it only threads the value.

**Fix (config).** `[cp].workflow_editor_ui` is **deprecated, not deleted**, for the
same `deny_unknown_fields` + `.unwrap_or_default()` reason documented under
[[I-28]]: hard-deleting it would silently discard the operator's entire config.

**Validation.** `npm run build` TypeScript-clean and `npx vitest run` green at
180 files / **1861** tests, down from 1887. The 26-test delta reconciles exactly
against the deleted `it()` blocks per file (6+3+3+4+3+7) — accounted for rather
than merely "still green". Where a test asserted *classic ≠ next* it was collapsed
to a next-only assertion instead of deleted, preserving coverage of the surviving
render; `StepForm.test.tsx`'s mixed assertion was updated, not removed.

**Three places the two arms were not cleanly separable**, resolved deliberately
rather than mechanically:
1. `NodePalette`'s render site in `WorkflowEditorGraph` — classic was an inline
   instant-add float dock, `next` is a portal-only select-then-"Add to canvas" rail
   that renders only when given a container. The instant-add dock is gone; the
   standalone-render test was rewritten to supply a container and drive the
   two-step flow.
2. Branch-arm edge styling (labels, marker colour, `animated`) — the two arms
   render genuinely different shapes with no overlap, so this collapsed to `next`'s
   shape with the default-prop tests updated to match.
3. Five `StepForm` sub-components plus `NodePalette`/`WorkflowSettingsForm` were
   left with a genuinely-unread `workflowEditorUi` param once the branching went.
   The destructured binding was renamed `_workflowEditorUi` to satisfy
   `noUnusedParameters`; **the interface and prop-passing are unchanged**, because
   removing them is [[I-31]]'s job (Task 5), not this one.

---

### I-28 — the classic agent-authoring UI is deleted

**Symptom.** Two agent-authoring UIs shipped side by side, selected by
`[cp].agent_authoring_ui` (default `"classic"`) with a `rupu.cp.agentUi`
localStorage override. The `next` card-based Agent Builder was the proven one and
the only one in actual use, but every change had to be made twice and the classic
raw-editor path kept accruing tests asserting behavior nobody wanted.

**Fix (web).** The `next` arm is now unconditional. Deleted:
`hooks/useAgentAuthoringUi.ts` and its test; `function NewAgentModal`
(`pages/Agents.tsx`, ~178 lines) with its render site and `createOpen` state; and
the classic raw-`<CodeEditor>` Cancel/Save block in `pages/AgentDetail.tsx` along
with its now-dead `draft`/`setDraft`/`save()`. `components/CodeEditor.tsx`,
`CodeHighlight.tsx` and `api.generateAgent` were each checked for other importers
and **kept** — all three are still reached from the surviving path.

**Fix (config).** `[cp].agent_authoring_ui` is **deprecated, not deleted.**
`CpConfig` is `#[serde(default, deny_unknown_fields)]`, so removing the field
outright would make any `config.toml` still setting it fail to deserialize — and
because several call paths load config with `.unwrap_or_default()`, that parse
error silently discards *every other key the operator set*. This is the exact
regression Arc 1 hit when `[retry]` was deleted, and it had to be reverted into a
warn-only shim. The same shape is used here: `Option<toml::Value>` with
`#[serde(default, skip_serializing)]`, so the key still parses, is ignored, never
round-trips back out, and emits one `tracing::warn!` naming it.

**Validation.** Observed at the consumer, not asserted structurally.
*Config:* `cp_agent_authoring_ui_still_parses_with_sibling_cp_keys_intact` is the
binding test — it sets the deprecated key **and** asserts sibling `[cp]` keys
(`gate_sweep_interval_secs = 15`, `gate_sweep_enabled = false`,
`max_workspace_bytes`) keep their **non-default** values. Without that sibling
assertion the test would pass even if the whole config had been discarded, which
is precisely how the Arc 1 regression slipped through. Plus
`cp_deprecated_ui_keys_never_round_trip_back_out` and
`cp_deprecated_ui_keys_emit_a_deprecation_warning_each` (hand-rolled
`tracing::Subscriber`). RED was observed: reverting the shim produced 6 compile
errors. *Web:* `npm run build` TypeScript-clean and `npx vitest run` green at
180 files / 1887 tests, independently re-run by the orchestrator.
`grep -rn "agentUi|useAgentAuthoringUi|rupu.cp.agentUi" crates/rupu-cp/web/src`
returns zero hits. `/agents/new` was confirmed unconditionally routed
(`App.tsx:77`), so the surviving entry point does not depend on the deleted flag.

**Note on history.** Two agents ran concurrently and one committed with
`git commit -a`, sweeping the other's staged hook deletions into `0e6730d3`;
`7f70ab8f` re-deleted them correctly. The net tree was verified clean — the two
files are gone, the working tree is empty, the suite is green — but the deletion's
authorship is split across those two commits. Cosmetic only.

---

### I-25 — a list command executes `on_reject` chains as a side effect

**Symptom.** `rupu workflow runs` — a read-only-looking listing — can post
GitHub comments and run agent steps.

**Root cause.** The listing path performs lazy approval-timeout expiry, and the
timeout-reject branch invokes the full `run_reject_cleanup` chain inline.

**Impact.** A command whose name and output imply pure observation has
side effects on external systems. Previously compounded by I-24 (now fixed):
those side effects used to run at `ask` mode regardless of how the original run
was launched; they now correctly inherit the run's own launch mode, but the
listing path still shouldn't be the one running them at all.

**Fix.** Either move timeout-driven cleanup exclusively to the `cp serve` gate
sweep (which exists and is default-on), or keep the lazy expiry but have the
listing path only *finalize state* and leave chain execution to the sweep. The
choice is a behavior decision — decided in the Arc 2 plan.

**Validation.** `crates/rupu-cli/tests/workflow_runs_no_side_effects.rs`, three tests
driving `rupu_cli::run(...)` end to end:
`listing_runs_does_not_execute_a_reject_cleanup_chain` (the chain's marker file must
not exist after a listing), `listing_runs_leaves_a_reject_timeout_gate_awaiting_approval`,
and `listing_runs_still_finalizes_a_fail_timeout_gate` (the lazy expiry that remains
is not broken). RED was observed, not claimed: against unfixed code test 1 failed with
the marker file present and test 2 failed `left: Rejected, right: AwaitingApproval`.

**The fix is "skip", not "stop cleaning up".** For `on_timeout: reject` the loop now
`continue`s *before* `expire_if_overdue`, rather than merely dropping the inline
cleanup call. This matters: `sweep_decision` (`crates/rupu-cli/src/cmd/cp.rs:337`)
only produces `ExpireThenCleanupReject` for a run still `AwaitingApproval` and falls
through to `Skip` (`:366`) for every other status. Had the listing finalized the run
to `Rejected` without running the chain, the sweep would have skipped it forever and
the cleanup would have been silently lost — a worse bug than the one being fixed.
Test 2 is the assertion that pins this. `approve` and `fail`/unset are unchanged:
`expire_if_overdue` deliberately leaves the record `AwaitingApproval` on the approve
arm, and finalizes the fail case internally with no chain to run.

**Consequence tracked as I-80.** Reject-timeout gates are now resolved only by the
`cp serve` sweep.

---

### I-26 — action steps get a `["*"]` tool allowlist while the comment claims otherwise

**Symptom.** An action step can call any MCP catalog tool, even in a workflow
whose agents are narrowed to a small `tools:` roster.

**Root cause.** `action_dispatcher_for` builds
`McpPermission::new(parse_mode_for_runtime(mode_str), vec!["*".into()])`. An
agent step's MCP allowlist is its frontmatter `tools:` (`["*"]` only when the
agent declares none). The doc comment on the builder asserts the opposite — that
an `action:` step "sees exactly the same allow/deny surface a `tools:`-using
agent step would" — which is false precisely in the case it names.

**Impact.** Defensible by author intent (the tool name is written in the
workflow), but the code comment is actively misleading and there is no per-step
narrowing knob. Note `Step.actions` *is* enforced for agent steps since
#533/#537 (`narrow_agent_tools`), so the asymmetry is now sharper.

**Fix.** At minimum correct the comment. Preferably let a step's `actions:`
narrow its own `action:` invocation the way it narrows an agent's grant.

**Scope corrected — half of this issue was based on a false premise.** The original
fix had two halves: (a) narrow an action step by its own `actions:` list, and
(b) correct the doc comment. Half (a) is **withdrawn as impossible and meaningless**:
`validate_step_actions` (`crates/rupu-orchestrator/src/workflow.rs:1389`) rejects a
step carrying both `action:` and a non-empty `actions:` at parse time with
`WorkflowParseError::ActionsOnActionStep` — *"an `action:` step must not carry a
non-empty `actions:` allowlist — its tool is already explicit"* — and a parse test at
`workflow.rs:2887` already asserts it. A workflow of the shape this issue proposed to
narrow cannot be authored at all.

**The wildcard is not exploitable, and that was verified rather than assumed.**
`opts.action_dispatcher` has exactly three production consumers (`runner.rs:4293`,
`:4700` for notify hooks, `:4923`), all funnelling into `execute_action_step`, whose
only dispatch is `dispatcher.call(tool, …)` with `tool = step.action`. Agent-step tool
calls go through the `rupu-tools` registry and `DefaultStepFactory`'s own narrowing,
never this dispatcher. Every `action:` tool is catalog-validated at parse time by
`validate_action_step`. So the only tool reachable through `["*"]` is one named
explicitly in the workflow source and already checked.

**Validation.** The doc comment on `action_dispatcher_for`
(`crates/rupu-cli/src/resume.rs:19`) no longer claims an `action:` step "sees exactly
the same allow/deny surface a `tools:`-using agent step would". It now separates the
two halves — the **mode** half genuinely does match the agent path
(`parse_mode_for_runtime` is shared, and a `readonly` run refuses Write tools here
too), while the **allowlist** half deliberately differs — and enumerates the three
invariants that make the wildcard sound, naming `ActionsOnActionStep` so the next
reader can find the enforcement. `cargo build -p rupu-cli` clean. No behavior change,
so no new test; the invariant's existing coverage is `workflow.rs:2887`.

**Follow-up filed.** Making the guarantee structural rather than invariant-dependent
— narrowing the allowlist to the single tool being invoked — is tracked as **I-79**
(P2). It needs `execute_action_step` to build or be handed a per-step dispatcher,
threading the registry through three call sites.

---

### I-23 — the autoflow author allowlist is undocumented and defaults to open

**Symptom.** Nothing in the user-facing docs describes `selector.authors` or
`selector.authors_from`. A user writing an autoflow gets no restriction by
default — any author who opens a matching issue or PR can trigger it.

**Root cause.** `AutoflowSelector.authors` (`crates/rupu-orchestrator/src/workflow.rs:412`)
and `.authors_from` (`:416`, with `AuthorScope::Collaborators | OrgMembers` at
`:429`) are implemented and enforced by `author_allowed` (`:453`), but
`docs/workflow-format.md`'s selector table documents only `states`,
`labels_all`, `labels_any`, `labels_none`, and `limit`. The default — both
fields unset — is *no* author restriction.

**Impact.** This is the control that stops an arbitrary GitHub user from
triggering an autonomous agent run by opening a PR — and autoflows commonly run
at `permission_mode: bypass`. An operator who never learns the field exists has
no reason to set it. Also undocumented on the same selector: `draft`, `base`,
`on_skip`, and `Autoflow.source`.

**Fix.** Document all of them in the selector table, with the default stated
explicitly and a recommendation to set `authors_from: collaborators` for any
autoflow that runs unattended. Consider whether the *default* should change —
that is a behavior decision, so it is called out in the Arc 2 plan rather than
assumed here.

**Validation.** Documented in `docs/workflow-format.md`: the selector table gained
`source`, `draft`, `base`, `authors`, `authors_from` and `on_skip` rows, plus a new
"Author restriction" prose subsection. Every serialized value was verified against
its `#[serde(rename_all = "snake_case")]` enum rather than assumed —
`AuthorScope` → `collaborators` / `org_members` (`workflow.rs:427`), `DraftFilter`
→ `include` / `exclude` / `only` (`:479`), `SkipAction` → `skip` /
`label_needs_human` (`:438`). The documented precedence was read off
`author_allowed` (`:464-476`) and is non-obvious: a match in `authors`
short-circuits to **allow** and overrides a failing `authors_from` scope check;
only a non-empty `authors` with no match *and* no `authors_from` denies. The prose
states plainly that with neither field set any author can trigger the autoflow, and
that autoflows commonly run at `permission_mode: bypass`.

**Default unchanged, deliberately.** Operator decision: tightening it would silently
stop existing autoflows from firing on outside contributors. Documented, not changed.

---

### I-27 — `action_protocol::validate_actions` is dead code, and three docs describe it

**Symptom.** README, `docs/agent-format.md`, and `docs/triggers.md` describe a
runtime check on emitted actions that no shipped code path performs.

**Root cause.** `crates/rupu-orchestrator/src/action_protocol.rs:18`
`validate_actions` is exported but called from nowhere in the runner — its only
callers are its own tests. `crates/rupu-agent/src/action.rs:21` likewise ships a
field-less, method-less `pub struct ActionValidator;` whose comment says "Real
impl lands in Task 11", and `ActionEnvelope` has no producer in `rupu-agent`.

**Impact.** Documentation-only: readers are told a safety check exists that does
not. Now more confusing because `actions:` *does* narrow tools via a different
mechanism (`step_factory::narrow_agent_tools`), so the docs describe the wrong
enforcement for a field that really is enforced.

**Fix.** Delete `validate_actions` and the `ActionValidator` stub, and correct
the three docs to describe the real mechanism. (The doc corrections overlap
I-51 in Arc 6; do the code deletion here and let Arc 6 own the prose.)

**Validation.** Already fixed by commit `28ec5cc3` ("chore: delete the dead legacy
action protocol"), which landed during Arc 1 and is an ancestor of this branch. It
removed all five files this issue named: `rupu-agent/src/action.rs` (the
`ActionValidator` stub), `rupu-orchestrator/src/action_protocol.rs`
(`validate_actions`), both crates' re-exports, and
`rupu-orchestrator/tests/action_allowlist.rs`. Validation is the deletion itself:
`grep -rn "validate_actions\|ActionValidator" --include="*.rs" crates` now returns
zero hits, and `cargo build --workspace` is clean. The issue was fixed without being
closed in the tracker; this closure is the bookkeeping.

**Not to be confused with live code.** `validate_action_step`
(`crates/rupu-orchestrator/src/workflow.rs:1324`) survives and is the **live**
catalog validator for `action:` steps and notify hooks. It is unrelated to the
deleted `validate_actions` and must not be removed.

**Prose corrections deferred.** README (×2), `docs/agent-format.md` and
`docs/triggers.md` still describe the deleted check. Those are owned by **I-51 in
Arc 6**, which holds the whole `actions:` documentation contradiction, so the two
do not collide.

---

### I-22 — `grep` and `ast_grep` escape the workspace

**Symptom.** An agent-supplied `path` argument reads outside the workspace:
`path: "/etc"` (absolute) or `path: "../.."` both leave the sandbox. The write
tools refuse the same input.

**Root cause.** Both tools built their search root with a bare join and no
containment check — `crates/rupu-tools/src/grep.rs:74` and
`crates/rupu-tools/src/ast_grep.rs:150`, each
`.map(|p| ctx.workspace_path.join(p))`. `Path::join` *replaces* the base when
the argument is absolute, and does not normalize `..`. The crate already had
the guard: `path_scope::is_inside` (`crates/rupu-tools/src/path_scope.rs:9`)
canonicalizes both ends and is used by `read_file.rs:60`, `write_file.rs`, and
`edit_file.rs`. The two search tools were simply never wired to it.

**Impact.** The workspace boundary is a containment guarantee the write tools
enforce and the read tools don't, so an agent could read `/etc`, `~/.ssh`, or a
sibling repo through `grep`. Bounded by being read-only and still
permission-gated, but it was a real escape from a documented boundary.

**Fix.** PR (branch `arc2/safety`). Added the same `path_scope::is_inside`
containment check `read_file`/`write_file`/`edit_file` already use, immediately
after computing `search_path` in both `crates/rupu-tools/src/grep.rs` and
`crates/rupu-tools/src/ast_grep.rs`. A path that resolves outside the
workspace root now returns `ToolOutput { error: Some("path {P} escapes
workspace"), .. }` before `rg`/`ast-grep` is ever spawned — no external command
runs against a rejected path, so nothing outside the workspace is even
attempted, let alone read. `glob` was left untouched: confirmed it takes no
user-supplied path and walks only `ctx.workspace_path` (`glob.rs:54`); the
original TODO naming it as affected was wrong.

**Validation.** `cargo test -p rupu-tools --lib` — 23 tests, 6 new (a
three-test trio per tool, added as in-file `#[cfg(test)] mod tests`):
`grep::tests::an_absolute_path_is_refused` / `a_parent_traversal_is_refused` /
`an_in_workspace_path_still_searches`, mirrored in `ast_grep::tests`. Each
absolute/traversal test asserts both the refusal string (`"escapes
workspace"`) on `ToolOutput.error` AND that the outside file's name
(`outside.txt` / `outside.rs`) never appears in `ToolOutput.stdout`, so the
refusal can't leak partial results gathered before the guard fired. The
in-workspace test proves the guard doesn't regress a legitimate search.
`ast-grep` was present on PATH in this environment (Homebrew), so its tests
exercised the real binary rather than being gated; they still self-gate via
`which::which("ast-grep")` so a box without the binary degrades to a skip
rather than a failure. `cargo build --workspace`, `cargo clippy -p rupu-tools
--lib --tests`, and `rustfmt --edition 2021 --check` on both changed files are
all clean.

---

### I-24 — `on_reject` cleanup runs at `ask` mode regardless of the run's mode

**Symptom.** A workflow deliberately launched with `--mode readonly` has its
gate rejected; the `on_reject` cleanup chain then executes with **write** tools
enabled.

**Root cause.** The run's original `--mode` is never persisted on `RunRecord`
(only `resume_mode`, set by the web-resume path). `rupu workflow reject` has no
`--mode` flag and passes `None`; `rebuild_opts_from_disk` then does
`mode.unwrap_or("ask")`, and `parse_mode_for_runtime` maps anything that isn't
`bypass`/`readonly` to `Ask`, which permits Write tools.

**Impact.** A readonly guarantee silently stops applying at exactly the moment a
human rejected the work — the cleanup chain can post comments, push branches, or
call any Write connector the agent's grant allows.

**Fix.** PR (branch `arc2/safety`). Added `RunRecord.permission_mode:
Option<String>` (`crates/rupu-orchestrator/src/runs.rs`, `#[serde(default,
skip_serializing_if = "Option::is_none")]` so every pre-existing `run.json`
still deserializes, reading back as `None`). Populated at fresh-run creation in
`run_workflow` (`crates/rupu-orchestrator/src/runner.rs`) from a new
`StepFactory::permission_mode(&self) -> Option<&str>` trait method (default
`None`, so no other `StepFactory` impl needs a change); `DefaultStepFactory`
implements it as `Some(&self.mode_str)`, and `rupu run`'s own `RunRecord` write
(`crates/rupu-cli/src/cmd/run.rs`) sets it the same way for consistency, though
that path has no `on_reject` chain of its own. `rebuild_opts_from_disk`
(`crates/rupu-cli/src/resume.rs`, shared by both the reject-cleanup and
approve-resume rebuilds) now resolves the effective mode with explicit
precedence: an explicit `--mode` on the calling command (if one exists —
`reject` has none, `approve` does) → `record.resume_mode` (the web-resume path)
→ `record.permission_mode` (new) → `"ask"`.
  A second, closely-related gap surfaced while writing the validation test
below and was fixed alongside it (same PR, same file):
  `DefaultStepFactory::build_opts_for_step` unconditionally built its agent's
  `PermissionDecider` as `Arc::new(BypassDecider)` — a decider whose own doc
  comment calls it a "Test/CI decider: always Allow regardless of mode" — so
  no workflow step's tool calls, cleanup or otherwise, ever actually honored
  `--mode readonly`/`ask` at the tool layer; only `action:` steps' separate MCP
  permission gate did. Added `rupu_agent::runner::ReadonlyDecider` (denies
  `bash`/`write_file`/`edit_file`, non-interactive so it's safe unattended) and
  had `DefaultStepFactory` select it when `mode_str == "readonly"`. Without
  this, I-24's fix would have had no observable effect: the mode would reach
  `rebuild_opts_from_disk` correctly but still hit an always-Allow decider.
  `ask`/`bypass` semantics for workflow steps are unchanged by this — a
  workflow has no per-step operator to prompt, so `ask` still permits writes
  there, exactly as documented in this write-up's "Root cause" above.
  Separately, `DefaultStepFactory::build_opts_for_step`'s step lookup
  (`crates/rupu-orchestrator/src/step_factory.rs`) only searched top-level
  `workflow.steps`, so any `on_reject:` cleanup sub-step dispatched through the
  real factory (rather than a test's fake one) panicked with `step_id from
  orchestrator must match a workflow step` — an on_reject sub-step's id lives
  nested under its gate's `approval.on_reject`, never in `workflow.steps`
  itself. This is a distinct, pre-existing gap (parked, real, and unrelated to
  the mode fallback) that blocked validating I-24 with a real agent step at
  all; the lookup now falls back to searching every gate's `on_reject` chain
  before giving up.

**Validation.** `crates/rupu-cli/tests/reject_mode_inheritance.rs`
(`reject_cleanup_inherits_a_readonly_run_mode`) drives `rupu_cli::run(...)` end
to end: launches a single-gate workflow with `--mode readonly`, whose
`on_reject` chain has one agent step that attempts `write_file`, then rejects
the parked gate via a second `rupu_cli::run(...)` call with no `--mode`. The
binding assertion is the filesystem effect, not a config value: the file the
cleanup step would have written does not exist, and the run still ends
`Rejected`. Confirmed genuinely RED pre-fix (temporarily reverting the
`rebuild_opts_from_disk` precedence back to `mode.unwrap_or("ask")` reproduces
the file being created) and GREEN with the fix restored. A second test,
`runs::tests::record_json_with_no_permission_mode_key_deserializes_as_none`
(`crates/rupu-orchestrator/src/runs.rs`), deserializes a hand-written
`run.json` JSON payload with no `permission_mode` key at all and asserts it
loads with the field as `None` — proving the back-compat contract, not just
the round-trip of a struct the code itself produced. `cargo test -p rupu-cli -p
rupu-orchestrator` is clean except the pre-existing baseline (4 `linear_runner.rs`
tests; ANSI/terminal-color-detection assertions across `output::printer` and
several other integration tests, all traced to this worktree's toolchain
mismatch — see `project_rupu_toolchain_mismatch` — and confirmed unrelated by
inspecting the diff against every failing file). `cargo build --workspace` and
`cargo test -p rupu-cp` are both clean.

---

### I-9 … I-14, I-17, I-20 — dead configuration

*(I-19 shared this write-up and was split out because it needed its own
diagnosis; it is now **fixed** — see its own entry under `## Fixed` below. Its
row is kept in the table here so no row is lost.)*

Nine keys parsed, were documented, and in several cases were editable in the CP
Settings UI, yet had **no runtime consumer**. Proof for each was a
workspace-wide grep (excluding `crates/rupu-config/` and tests) returning zero
hits.

| ID | Key | Declared | Documented as working | Reality (before) | Outcome | Validation |
|---|---|---|---|---|---|---|
| I-9 | `[providers.*].timeout_ms` | `provider_config.rs:23` | `docs/providers.md:111` ("Default: `120000`") + `ConfigEditor.tsx:220` | vendor default always used | **wired** → every provider client's HTTP builder as connect + read (inactivity) timeouts, via `ProviderTuning::http_client_builder` (`rupu-providers/src/tuning.rs`) | `tuning::tests::configured_timeout_aborts_a_stalled_request` — a 60 ms deadline against a 2 s-stalled httpmock server surfaces `is_timeout()`; plus `client_timeout_defaults_to_documented_120s` and `default_timeout_lets_a_prompt_response_through` |
| I-10 | `[providers.*].max_retries` | `provider_config.rs:25` | `docs/providers.md:112` ("Default: `5`") | real budget was a hardcoded 1 (`anthropic.rs:213`) | **wired** → `AnthropicClient::max_rate_limit_retries` (its native 429 loop) and `RetryingProvider` for every other provider. Default corrected to **1** (the code's real budget) and `docs/providers.md` fixed — raising it to the documented 5 would have slowed every cross-provider fallback by ~30 s | `anthropic::tests::max_retries_zero_issues_exactly_one_request` / `max_retries_one_issues_two_requests` count httpmock hits against a permanent 429; `tuned::tests::retry_budget_bounds_the_number_of_attempts`, `permanent_errors_are_not_retried`, `a_stream_that_already_emitted_output_is_not_re_issued` |
| I-11 | `[providers.*].max_concurrency` | `provider_config.rs:27` | `docs/providers.md:113` + `concurrency.rs:6` | `semaphore_for` was called only by SCM clients; **no LLM call ever acquired a permit** | **wired** → `ThrottledProvider` wraps every factory-built provider and holds a permit for the whole call | `tuned::tests::llm_call_holds_a_permit_from_the_configured_semaphore` observes `available_permits()` *inside* the call (2 → 1); `max_concurrency_one_serializes_two_llm_calls` proves a second call blocks while the only permit is held; `a_backoff_sleep_does_not_hold_a_concurrency_permit` samples `available_permits()` from inside the retry loop's backoff hook (must be 1, i.e. free) and `the_inverted_order_holds_the_permit_across_the_backoff` shows the same probe reading 0 under the old nesting |
| I-12 | `[providers.*].org_id` | `provider_config.rs:19` | `docs/providers/openai.md:20` | org-scoped keys unreachable | **wired** → `OpenAI-Organization` header, platform API only (the ChatGPT-subscription endpoint is account-scoped) | `openai_codex::tuning_tests::configured_org_id_lands_on_the_outgoing_request_headers` asserts the header on the real `build_headers()`; `organization_header_only_applies_to_the_platform_api` covers the pure decision |
| I-12 | `[providers.*].region` | `provider_config.rs:21` | `docs/providers/gemini.md:23,54` | non-default Vertex regions unreachable | **carried, still unused — deliberately.** No shipped Gemini client targets a regional Vertex endpoint (the three variants are AI Studio, Gemini CLI, Antigravity; none is region-scoped), so there is nothing honest to wire it to. Instead the *documentation stopped lying*: `docs/providers.md` and `docs/providers/gemini.md` now say it is accepted but has no effect, the CP Settings field carries the same help text, and the example configs no longer instruct setting it | `provider_factory::tests::provider_tuning_reads_every_configured_knob` asserts it is carried into `ProviderTuning`; the doc/UI change is the substantive fix |
| I-13 | `[retry]` (whole section) | `config.rs:113-116` | — | inert top-level section | **deleted as a live key, with a one-release deprecation shim.** `RetryConfig`, the typed `Config.retry` field and the `lib.rs` re-export are gone; `[retry]` is still *accepted* as an opaque `Option<toml::Value>` that nothing reads, and `Config::warn_deprecated_keys` (called from `validate`, i.e. every load path) warns naming the key. Superseded by per-provider `max_retries`. Rationale below | Absence greps: `grep -rn 'RetryConfig\|max_attempts\|initial_delay_ms' crates/rupu-config/src/` → 0; `grep -rn '\[retry\]' docs/ --include='*.md'` (excluding historical plan docs) → 0; not present in `ConfigEditor.tsx` or `Settings.test.tsx`. `parse::a_config_still_carrying_retry_loads_and_keeps_every_other_key` and `parse::retry_survives_layer_files_and_the_lock_aware_loader` prove the migration is non-destructive; `cargo build --workspace` + web `npm run test` clean |
| I-14 | `log_level` | `config.rs:27` | `ConfigEditor.tsx:199-207`, with a lock toggle | logging read only `RUPU_LOG` (`logging.rs:25`) | **wired** → `logging::filter_directive(cfg_level, env)`: `RUPU_LOG` > `log_level` > `warn`. Config is loaded *before* logging init at all four init sites (`lib.rs:247`, `session.rs` worker + run-turn) | `logging::tests::config_log_level_is_the_fallback_when_env_is_unset`, `env_wins_over_config`, `neither_source_yields_warn`, `blank_values_are_treated_as_unset`, `an_unparseable_directive_falls_back_to_warn` |
| I-17 | `[scm.*].timeout_ms` | `scm_config.rs:46` | `docs/scm.md:109-119` | no consumer (sibling `base_url`/`max_concurrency` *were* consumed) | **wired** → `ScmClientOptions::timeout` reaches octocrab (`set_connect_timeout`/`set_read_timeout`) plus the two ad-hoc reqwest clients in `github/client.rs`, and replaces GitLab's hardcoded 30 s | `github::client::tests::platform_config_reaches_the_client` asserts `GithubClient::timeout()` == the configured 4000 ms and 30 000 ms when unset; `client_options::tests::scm_timeout_defaults_to_30s` |
| I-19 | all of it, in rupu-app | — | — | `executor/mod.rs:210` passes `Config::default()`; self-admitted at `:109-115` | **fixed** — see its own write-up under `## Fixed` for the detail; `build_executor` now loads the layered global+project config and threads `openai_compatible`/`provider_tuning`/`default_provider`/`default_model`/`[bash]` into every workflow it starts | see I-19's write-up |
| I-20 | `resolve()` env tier | `resolve.rs:144-171` | — | both callers passed an empty map; `KeySource::Env` was unreachable | **deleted** — the `env` parameter and the `KeySource::Env` variant. Precedence is now `locked-global > project > global > default`. Six call sites (`layer_files_locked`, `rupu-cp` state + api/config) simplified; the CP UI's unreachable `env` provenance badge removed from `api.ts`'s `KeySource` union and `ConfigField.tsx`'s `SOURCE_CLASS` | Absence greps: `grep -rn 'KeySource::Env' crates/` → only the doc comment explaining the removal; `grep -rn "'env'" crates/rupu-cp/web/src` → 0. `resolve::tests::project_wins_when_unlocked_and_no_env_tier_exists` replaces the old env-precedence test; `no_policy_block_matches_layer_files` still passes |

**Impact (before).** Exactly the I-1 shape, ~9×. A user read the docs, set a
value, saw it accepted (and in several cases saw it in the web UI), and got
different behavior than documented — with no error.

**Fix.** PR (branch `arc1/config-integrity`), two commits: the deletions
(I-13, I-20) and the wiring (I-9…I-12, I-14, I-16, I-17). Three new modules
carry the consumer-side logic, each built so the decision a key drives is a
*pure function* that a test can observe without a network:

- `crates/rupu-providers/src/tuning.rs` — `ProviderTuning` plus
  `client_timeout` / `retry_budget` / `concurrency_permits` / `retry_backoff`.
- `crates/rupu-providers/src/tuned.rs` — `ThrottledProvider` and
  `RetryingProvider`, the two `LlmProvider` decorators the factory applies.
  **Retry wraps throttle** — `RetryingProvider(ThrottledProvider(client))` — so
  each attempt takes and drops its own permit and the exponential backoff sleeps
  *outside* the semaphore. The first cut of `decorate()` had the nesting
  inverted while both doc comments asserted this order; under rate limiting that
  parked all of a provider's permits in `tokio::time::sleep` for the whole
  2s/4s/8s ladder and starved every unrelated call. Fixed, with
  `a_backoff_sleep_does_not_hold_a_concurrency_permit` pinning the property and
  `the_inverted_order_holds_the_permit_across_the_backoff` kept as an executable
  counter-example so the assertion is visibly non-vacuous. Caveat: anthropic is
  not wrapped in `RetryingProvider` at all, so its *native* 429 loop still sleeps
  inside the permit — the decorators cannot reach inside a client.
- `crates/rupu-scm/src/client_options.rs` — `ScmClientOptions`,
  `CloneProtocol`, `clone_url`, `scm_timeout`, `run_clone`.

`rupu_runtime::provider_factory::provider_tuning` is the single adapter from
`rupu_config::ProviderConfig` to `ProviderTuning` — `rupu-providers` still does
not depend on `rupu-config` (hexagonal rule 1). The map is threaded through
`rupu run`, `rupu session` (turn, worker, compaction), workflow steps
(`DefaultStepFactory.provider_tuning`), and sub-agent dispatch
(`CliAgentDispatcher.provider_tuning`) so the same key resolves identically on
every path — the I-2/I-8 lesson.

**Two decisions worth flagging.**

1. *Timeout semantics.* `timeout_ms` is applied as connect + read (inactivity)
   timeouts, never as reqwest's total `timeout`. A total 120 s deadline would
   abort a legitimately long streaming generation mid-flight. The documented
   120000 ms default is exactly the Anthropic client's pre-existing
   hand-rolled `STREAM_IDLE_TIMEOUT_SECS = 120`, which is where that number
   came from, so the default is behavior-preserving. `docs/providers.md` now
   states the semantics precisely. The SCM side keeps a *total* deadline —
   those calls are short request/response round-trips with no streaming.
2. *`max_retries` default is 1, not the documented 5.* Per the brief's
   instruction to make the doc match the code when they disagree after wiring.
   Anthropic's comment gives the reason: a small budget lets `ProviderRouter`
   fail over to another vendor quickly instead of spending ~30 s of
   exponential backoff on one rate-limited provider. Anthropic is therefore
   the one provider *not* wrapped in `RetryingProvider` — it spends the budget
   in its own 429 loop, and stacking the two would square it
   (`provider_factory::provider_has_native_retry`, asserted by
   `only_anthropic_skips_the_retry_decorator`).

#### Config-key ledger — every field in `crates/rupu-config/src/**`

Arc 1's proof of completion, and the input to the Arc 6 config-reference page.
Every declared field with its non-test consumer. Verified with
`grep -rn '\bFIELD\b' --include='*.rs' crates | grep -v '^crates/rupu-config/' | grep -v test`
per key, plus targeted greps for the names that are common English words
(`kind`, `stream`, `repo`, `owner`, `project`, `enabled`, `lock`, `theme`,
`color`, `channel`) where the raw count is dominated by unrelated hits.
Three keys **do** lack a consumer — `[scm.default].owner`, `[scm.default].repo`,
`[issues.default].project`. The first pass of this ledger asserted otherwise on
the strength of an attribution that does not survive a grep; the rows are
corrected below and the gap is filed as a new I-9-class issue rather than
papered over. Every `file:line` in the table was re-verified against the tip of
`arc1/config-integrity`.

| Key | Consumer |
|---|---|
| `default_provider` | `rupu-runtime/src/provider_factory.rs:148` (`resolve_provider_name`) — I-1 |
| `default_model` | `rupu-runtime/src/provider_factory.rs:167` (`resolve_model`) — I-2 |
| `permission_mode` | `rupu-cli/src/cmd/session.rs:1261`, `cmd/run.rs` (mode resolution) |
| `log_level` | `rupu-cli/src/logging.rs` (`filter_directive`), called from `lib.rs:250` — **I-14, wired here** |
| `[bash].timeout_secs` | `rupu-cli/src/resume.rs:249`, `cmd/session.rs:6787`, `orchestrator/src/step_factory.rs` — I-18 |
| `[bash].env_allowlist` | `rupu-cli/src/resume.rs:250`, `cmd/session.rs:6786` — I-18 |
| `[retry].max_attempts` | **deleted** — I-13. Still *parses* as an inert no-op (`config.rs`'s `Config::retry`) so an existing config.toml carrying it does not lose its other keys; `Config::warn_deprecated_keys` warns; never re-serialized |
| `[retry].initial_delay_ms` | as above — **deleted**, I-13 |
| `[providers.*].base_url` | `rupu-runtime/src/provider_factory.rs:66` (`openai_compatible_params`) |
| `[providers.*].kind` | `rupu-runtime/src/provider_factory.rs:63`; validated in `config.rs:169` |
| `[providers.*].stream` | `rupu-runtime/src/provider_factory.rs:80` |
| `[providers.*].default_model` | `rupu-runtime/src/provider_factory.rs:67` + `resolve_model`'s third tier |
| `[providers.*].org_id` | `rupu-providers/src/openai_codex.rs` (`organization_header` → `build_headers`) — **I-12, wired here** |
| `[providers.*].region` | carried into `ProviderTuning` (`rupu-runtime/src/provider_factory.rs`, `provider_tuning`); **no endpoint consumes it** — documented as accepted-but-unused in `docs/providers.md` and CP Settings — I-12 |
| `[providers.*].timeout_ms` | `rupu-providers/src/tuning.rs` (`client_timeout` → `http_client_builder`) → each client's `with_tuning` — **I-9, wired here** |
| `[providers.*].max_retries` | `rupu-providers/src/tuning.rs` (`retry_budget`) → `AnthropicClient::max_rate_limit_retries` + `tuned::RetryingProvider` — **I-10, wired here** |
| `[providers.*].max_concurrency` | `rupu-providers/src/tuning.rs` (`concurrency_permits`) → `tuned::ThrottledProvider` — **I-11, wired here** |
| `[[providers.*.models]].id` | `rupu-runtime/src/provider_factory.rs:72` |
| `[[providers.*.models]].context_window` | `rupu-runtime/src/provider_factory.rs:73` |
| `[[providers.*.models]].max_output` | `rupu-runtime/src/provider_factory.rs:74` |
| `[scm.default].platform` | `rupu-scm/src/registry.rs:80` (captured at `discover`), enforced at `:281` (`default_platform`) — I-15 |
| `[scm.default].owner` | **NO CONSUMER** — the earlier "repo-ref defaulting via `ScmDefault`" claim was wrong; `grep -rn 'ScmDefault' crates --include='*.rs'` outside `rupu-config/` returns 0. Filed as a new I-9-class issue |
| `[scm.default].repo` | **NO CONSUMER** — as above |
| `[issues.default].tracker` | `rupu-scm/src/registry.rs:86` (captured at `discover`), enforced at `:312` (`default_tracker`) — I-15 |
| `[issues.default].project` | **NO CONSUMER** — `IssuesDefault::project` is read nowhere outside `rupu-config/`; the `cmd/issues.rs` attribution was wrong. Filed as a new I-9-class issue |
| `[scm.*].base_url` | `rupu-scm/src/client_options.rs` → `GithubClient::with_options` / `GitlabClient::with_options` |
| `[scm.*].timeout_ms` | `rupu-scm/src/client_options.rs` (`scm_timeout`) → octocrab + reqwest builders — **I-17, wired here** |
| `[scm.*].max_concurrency` | `rupu-scm/src/client_options.rs` → `concurrency::semaphore_for` |
| `[scm.*].clone_protocol` | `rupu-scm/src/client_options.rs` (`CloneProtocol`, `clone_url`) → both `clone_to` impls — **I-16, wired here** |
| `[ui].color` | `rupu-cli/src/cmd/ui.rs` (`UiPrefs::resolve`) |
| `[ui].theme` | `rupu-cli/src/cmd/ui.rs:661` |
| `[ui].syntax.theme` | `rupu-cli/src/cmd/ui.rs` (`UiPrefs::resolve`) |
| `[ui].palette.theme` | `rupu-cli/src/cmd/ui.rs` (`UiPrefs::resolve`) |
| `[ui].live_view` | `rupu-cli/src/cmd/ui.rs:188` (`UiPrefs::resolve`) |
| `[ui].pager` | `rupu-cli/src/cmd/ui.rs` |
| `[ui].editor` | `rupu-cli/src/cmd/agent.rs` / `cmd/workflow.rs` editor resolution |
| `[triggers].poll_sources` | `rupu-cli/src/cmd/cron.rs:598` |
| `[triggers].poll_sources[].source` / `.poll_interval` | `rupu-cli/src/cmd/cron.rs:713` |
| `[triggers].max_events_per_tick` | `rupu-cli/src/cmd/cron.rs:399` via `effective_max_events_per_tick` |
| `[autoflow].enabled` | `rupu-cli/src/cmd/autoflow.rs:9322` |
| `[autoflow].repo` | `rupu-cli/src/cmd/autoflow.rs:9200` |
| `[autoflow].checkout` | `rupu-cli/src/cmd/autoflow.rs:12027` (`AutoflowCheckout` → `AutoflowWorkspaceStrategy`) |
| `[autoflow].worktree_root` | `rupu-cli/src/cmd/autoflow.rs:12037` (`resolve_worktree_root`), consumed by `rupu-workspace/src/autoflow_worktree.rs:35` |
| `[autoflow].permission_mode` | `rupu-cli/src/cmd/autoflow.rs:1701` |
| `[autoflow].strict_templates` | `rupu-cli/src/cmd/autoflow.rs:10652` and `:11473` (`resume.rs:267` is a hardcoded `false`, not a consumer) |
| `[autoflow].max_active` | `rupu-cli/src/cmd/autoflow_runtime.rs:495` |
| `[autoflow].cleanup_after` | `rupu-cli/src/cmd/autoflow.rs:11711` (`cleanup_after_for_claim`), called at `:11672` |
| `[pricing.<provider>.<model>].input_per_mtok` | `rupu-config/src/pricing_config.rs:74` (`cost_usd`), called from `rupu-cli/src/cmd/session.rs:5810` and the run/workflow cost columns |
| `[pricing.<provider>.<model>].output_per_mtok` | as above |
| `[pricing.<provider>.<model>].cached_input_per_mtok` | as above |
| `[storage].archived_session_retention` | `rupu-cli/src/cmd/session.rs:7395` |
| `[storage].archived_transcript_retention` | `rupu-cli/src/cmd/transcript.rs:1838` |
| `[policy].lock` | `rupu-config/src/resolve.rs` (enforcement) + `rupu-cp/src/api/config.rs:325` — I-7 |
| `[cp].max_workspace_bytes` | `rupu-cp/src/config_write.rs:303` (`effective_max_workspace_bytes`) |
| `[cp].autoflow_reconcile_enabled` / `_interval_secs` | `rupu-cli/src/cmd/cp.rs:93-94` |
| `[cp].cron_tick_enabled` / `_interval_secs` | `rupu-cli/src/cmd/cp.rs:117-118` |
| `[cp].gate_sweep_enabled` / `_interval_secs` | `rupu-cli/src/cmd/cp.rs:152-153` |
| `[cp].agent_authoring_ui` | **none — deprecated no-op** (I-28, Arc 3) |
| `[cp].workflow_editor_ui` | **none — deprecated no-op** (I-29/I-30/I-31, Arc 3) |
| `[update].channel` | `rupu-cli/src/cmd/update.rs:74` |
| `[update].check` | `rupu-cli/src/lib.rs:276` |

Two keys' consumers **were not** Rust — `[cp].agent_authoring_ui` and
`[cp].workflow_editor_ui` were read by the CP web app after `/api/config`
serialized them, so a Rust-only grep reported them as inert when they were not.
**As of Arc 3 both are genuinely inert**: the classic renderers they selected
are deleted, the two React hooks that read them are gone, and the fields survive
only as `Option<toml::Value>` deprecation shims that warn and are never read
(see [[I-28]], [[I-29]]). They are kept solely so an existing `config.toml`
setting them still parses — `CpConfig` is `deny_unknown_fields`, and the
`.unwrap_or_default()` load paths would otherwise convert that parse failure
into silent loss of the operator's whole config.

**Behavioral note — why `[retry]` deprecates instead of erroring.** The first
cut simply deleted the field. Because `Config` is `deny_unknown_fields`, that
made an existing `config.toml` carrying `[retry]` fail to deserialize — and the
failure is not the clean startup error it looks like. Eight load paths swallow
it with `.unwrap_or_default()` (`rupu-cli/src/cmd/update.rs:61-62`,
`cmd/run.rs:339` and `:404`, `cmd/cp.rs:206`, `cmd/cron.rs:282`,
`cmd/workflow.rs:994`, `cmd/transcript.rs:1465`, plus `rupu-cp/src/state.rs`
which warns and falls back to defaults). On those commands the user would not
see an error; they would silently run with **every** config key reset to its
default — a harmless dead key converted into whole-config data loss, the exact
class Arc 1 exists to eliminate.

So the key stays deleted everywhere it was ever surfaced (docs, `rupu init`
templates, the CP Settings UI, `ConfigEditor.tsx`) but survives at *parse* time
for one release as `Config::retry: Option<toml::Value>` — `#[serde(default,
skip_serializing)]`, read by nothing, never written back out, and announced by a
`tracing::warn!` from `Config::warn_deprecated_keys` telling the user to delete
the section. Release notes should say "`[retry]` is deprecated and ignored;
remove it" rather than "`[retry]` is now rejected". Drop the field one release
after v0.68.

---

### I-16 — `[scm.*].clone_protocol` is inert

**Symptom.** A user on SSH-only infrastructure selects "ssh" from the
`clone_protocol` dropdown in CP Settings. Clones still go over HTTPS.

**Root cause.** `clone_protocol` (`crates/rupu-config/src/scm_config.rs:51`) had
no consumer. The clone paths hardcoded HTTPS with an embedded token —
`connectors/gitlab/repo.rs:477-479`, `connectors/github/repo.rs:442`.

**Impact.** Documented at `docs/scm.md:109-119` and given a dedicated
`https`/`ssh` dropdown at `ConfigEditor.tsx:374,463`. Clones failed or used the
wrong credentials on hosts that only permit SSH.

**Fix.** PR (branch `arc1/config-integrity`). `CloneProtocol` +
`clone_url(host, owner, repo, protocol, userinfo)` in the new
`crates/rupu-scm/src/client_options.rs`; both `clone_to` impls now ask their
client for the configured protocol and build the URL through it. `ssh` yields
`git@<host>:<owner>/<repo>.git` with the token dropped entirely.

**SSH clones shell out to the system `git`, deliberately.** Two blocking
reasons, both recorded in `ssh_clone_argv`'s doc comment: rupu's `git2` is
built without the `ssh` feature (root `Cargo.toml` enables only `https` +
vendored libgit2/openssl), so libgit2 has *no* ssh transport at all; and even
with it, libgit2 does not read `~/.ssh/config`, so host aliases,
`IdentityFile`, `ProxyJump`, and agent forwarding — precisely what SSH-only
infrastructure depends on — would silently not apply. The HTTPS path is
unchanged and still uses `git2`. A missing `git` on `PATH` surfaces as a
named `ScmError::Network`, not a silent fallback.

**Validation.** `client_options::tests::clone_url_honors_ssh_protocol` is the
binding assertion — `ssh` yields `git@github.com:o/r.git` and
`git@gitlab.com:grp/sub/r.git` (nested namespace), the default yields the
token-bearing https form both connectors used to hardcode.
`ssh_clone_url_never_leaks_the_token` guards the credential.
`clone_protocol_parses_ssh_case_insensitively` covers the parse, including the
warn-and-fall-back-to-https path for an unrecognized value.
`github::client::tests::configured_ssh_protocol_produces_an_ssh_clone_url` and
`gitlab::client::client_options_tests::configured_ssh_protocol_produces_an_ssh_clone_url`
drive the *real* config → `ScmClientOptions` → client → URL chain.
`cargo test -p rupu-scm` is fully green (50 lib + all integration tests).

**Still open, unchanged.** The clone paths use the public host constant and
still ignore `[scm.<platform>].base_url`, so self-hosted GHES / GitLab clone
URLs remain wrong. That is the separate gap already tracked in `TODO.md`; the
two host constants are now named (`GITHUB_CLONE_HOST` / `GITLAB_CLONE_HOST`)
with a comment pointing at it.

---

### I-15 — `[scm.default]` / `[issues.default]` were inert

**Symptom.** A user with both GitHub and GitLab credentials set
`[scm.default] platform = "gitlab"`. Every tool call that omitted `platform`
still went to GitHub.

**Root cause.** `Registry::default_platform` / `default_tracker`
(`crates/rupu-scm/src/registry.rs:206-230`) implemented a hardcoded
GitHub-then-GitLab preference over registered connectors and never read
`ScmDefault` / `IssuesDefault` (`crates/rupu-config/src/scm_config.rs:24-38`),
which had no consumer anywhere. The code admitted it: *"Wiring to
`[scm.default]` config lands in Task 19; this is the v0 'first registered'
fallback"* (`registry.rs:207-208`).

**Impact.** The key is written into every `rupu init` config
(`crates/rupu-cli/src/templates.rs:155`), documented as functional
(`docs/scm.md:100-107`), and *named in the error message users see when it is
missing* (`crates/rupu-mcp/src/tools/scm_repos.rs:73`: "no platform arg and no
`[scm.default]` configured"). A GitLab-primary shop silently operated against
GitHub. `default_tracker` additionally ignored Linear (and Jira) even when
registered.

**Fix.** PR (branch `arc1/config-integrity`). `Registry::discover` already took
`cfg: &Config`, so no signature change was needed anywhere — its call sites
were unaffected. At `discover` time, `Registry` now parses
`cfg.scm.default.platform` / `cfg.issues.default.tracker` into
`Platform`/`IssueTracker` and stores them on two new private fields
(`configured_default_platform`, `configured_default_tracker`); an unset or
unparsable value just leaves the field `None`. `default_platform()` /
`default_tracker()` consult the configured value first — but only return it if
a connector for it is actually registered (a stale `[scm.default]` naming a
platform with no live connector doesn't black-hole the default) — and fall
back to the previous registration-order preference otherwise.
`default_tracker()`'s fallback now walks `[Github, Gitlab, Linear, Jira]`
instead of only the first two; `Registry::discover` already registers both
Linear and Jira issue connectors (`registry.rs` Linear/Jira blocks), so both
were reachable and both are now included in the default-tracker fallback. The
stale "Wiring... lands in Task 19" comment at `registry.rs:207-208` is
rewritten to describe current behavior.

**Follow-up (same PR, review round).** The initial fix above left the
"configured-but-unavailable" fallback silent — indistinguishable in the logs
from the "nothing configured" case, i.e. the same silent-wrong-behavior shape
this issue exists to close (a user asking for GitLab and quietly getting
GitHub with no trace of why). `default_platform()` / `default_tracker()` now
log a `tracing::warn!` naming both the configured value and the fact that it
has no live connector before falling back — matching the WARN level
`Registry::discover` already uses for connector-build errors. The fallback
behavior itself is unchanged (still warn-and-fall-back, not `None`/error):
returning `None` here would make the existing "no platform arg and no
`[scm.default]` configured" error message actively misleading when a platform
*is* configured but just unavailable. The decision logic was extracted into a
pure `resolve_configured_default` helper (returns a
`DefaultResolution::{ConfiguredAndAvailable, ConfiguredButUnavailable, Unset}`
enum) precisely so the warn-triggering branch is unit-testable — the crate
has no tracing-capture harness (no `tracing-test` or equivalent dev-dep).

**Validation.** `cargo test -p rupu-scm --lib` — 42 tests, all passing (7 new
in `registry::tests`, including
`resolve_configured_default_distinguishes_unset_available_and_unavailable`,
which asserts the `ConfiguredButUnavailable` variant — the one that drives
the new WARN log — is returned when a configured value has no matching
connector, distinct from both `Unset` and `ConfiguredAndAvailable`). Binding
assertions, built via the existing `Registry::empty()` + `insert_repo_connector`
test seam plus a new `insert_issue_connector` equivalent (fake
`RepoConnector`/`IssueConnector` impls, no real credentials/network):
`default_platform_prefers_the_configured_value` registers both GitHub and
GitLab connectors, sets `configured_default_platform = Some(Gitlab)`, and
asserts `default_platform() == Some(Gitlab)`;
`default_platform_falls_back_when_config_is_unset` proves the old
registration-order behavior (GitHub) is preserved with no config set;
`default_platform_ignores_configured_value_with_no_matching_connector` proves
a stale configured value doesn't return a platform with no connector;
`default_tracker_includes_linear` registers only a Linear issue connector (no
config) and asserts `default_tracker() == Some(Linear)`;
`default_tracker_prefers_the_configured_value` and
`default_tracker_falls_back_when_config_is_unset` mirror the platform cases
for trackers. `cargo build --workspace` and `cargo clippy -p rupu-scm --lib
--tests` are both clean.

**Noted, not fixed here.** `insert_repo_connector` / `insert_issue_connector`
are documented "Test/internal" but carry no `#[cfg(test)]` /
`#[cfg(feature = "test-helpers")]` gate, unlike `empty()` one line below them.
Pre-existing, out of scope for this fix; flagged for a follow-up issue.

---

### I-18 — `[bash]` config was dropped on the workflow path

**Symptom.** `[bash].timeout_secs` and `[bash].env_allowlist` applied under
`rupu run` and `rupu session`, and were silently ignored under
`rupu workflow run`.

**Root cause.** `crates/rupu-orchestrator/src/step_factory.rs:245-246` hardcoded
`bash_env_allowlist: Vec::new(), bash_timeout_secs: 120`. `DefaultStepFactory`
carried no `Config` (`step_factory.rs:36-50`). The values were read correctly at
`cmd/session.rs:6705-6706` and `cmd/run.rs:574-575`.

**Impact.** Precisely the I-2 shape: the same agent behaved differently
depending on how it was invoked. A workflow step got a 120s bash timeout and an
empty env allowlist no matter what the user configured.

**Fix.** PR (branch `arc1/config-integrity`). `DefaultStepFactory` gained two
fields — `bash_timeout_secs: u64` and `bash_env_allowlist: Vec<String>` — used
directly at the `ToolContext` construction that previously hardcoded them.
Nothing re-reads `config.toml` inside the factory: the values are threaded in
from the callers, mirroring the I-2/I-8 approach.

- `crates/rupu-cli/src/cmd/workflow.rs:2655` (resume path) and `:4044` (fresh
  run) and `crates/rupu-cli/src/resume.rs:234` (out-of-process resume) all
  already had `cfg` in scope for `default_provider`/`default_model` and now
  additionally pass `cfg.bash.timeout_secs.unwrap_or(120)` /
  `cfg.bash.env_allowlist.clone().unwrap_or_default()` — the same fallback
  `cmd/run.rs`/`cmd/session.rs` apply.
- `crates/rupu-app/src/executor/mod.rs:100` (the GUI executor) had no real
  `Config` yet — that gap was tracked separately as I-19 (now fixed, see its
  own write-up) — so at the time of this fix it kept the same `120`/empty
  values it already produced; the TODO comment was extended to note `[bash]`
  alongside `default_provider`/`default_model` as inert there. No behavior
  change at that site *until I-19 landed*, which threads real `[bash]` values
  through the same fields this fix added.

**Validation.** `cargo test -p rupu-orchestrator --lib` — 365 tests, all
passing (1 new). `bash_config_reaches_the_step_opts`
(`crates/rupu-orchestrator/src/step_factory.rs`) is the binding assertion: a
`DefaultStepFactory` built with `bash_timeout_secs = 42` and
`bash_env_allowlist = ["FOO"]` is driven through the real
`build_opts_for_step`, and the returned `AgentRunOpts.tool_context` is asserted
to carry `bash_timeout_secs == 42` and `bash_env_allowlist` containing `"FOO"`
— not the old hardcoded `120` / empty. It fails to compile on the pre-fix
struct (no such fields), which is the RED this test drove.
`cargo build --workspace` is clean after the constructor-signature change;
`grep -rn "DefaultStepFactory {" --include="*.rs" crates` confirms every
production and test construction site was updated — no site left un-threaded.

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

### I-19 — the desktop app passes `Config::default()`; all user config is inert there

**Symptom.** Every `config.toml` value a user sets was ignored inside rupu.app.
A workflow run started from the GUI resolved a different provider, model, bash
timeout, and provider tuning than the identical run started from `rupu
workflow run`.

**Root cause.** `crates/rupu-app/src/executor/mod.rs:210` built its executor
with `rupu_config::Config::default()` rather than the layered global+project
config the CLI uses. The file admitted it at `:109-115`. Consequently
`DefaultStepFactory`'s `openai_compatible`, `provider_tuning`,
`default_provider`, `default_model`, and `[bash]` fields were all empty/None at
that site — the I-2/I-8 divergence shape at whole-config scale. A custom
`openai-compatible` provider like `oracle` failed loudly with "unknown
provider" in the GUI while working in the CLI; everything else failed
*silently* by resolving a default.

**Fix.** PR (branch `arc1/config-integrity`).

1. Extracted a pure `load_workflow_config(global: &Path, workspace_path:
   &Path) -> Config` helper (`crates/rupu-app/src/executor/mod.rs`) that calls
   `rupu_config::layer_files_locked` — global `<global>/config.toml` merged
   with the opened workspace's own `.rupu/config.toml`. Policy-bearing
   (`[scm]`, `default_provider`/`default_model`, `[bash]`, `[providers.*]`
   tuning) so it goes through the lock-aware loader, not plain `layer_files`
   (I-7). Unlike `rupu run`'s `paths::project_root_for`, there is no upward
   walk from a cwd: the desktop app already knows its project root — it's the
   workspace directory the user opened. A malformed `config.toml` is logged
   (`tracing::warn!` naming the file and the parse error) and treated as
   absent rather than blocking workspace-open outright; `build_executor`'s
   signature is infallible and all three call sites (`main.rs` ×2,
   `menu/app_menu.rs`) already treat "workspace opened" as the point of no
   return, so a broken desktop config shouldn't make the app unusable — a
   deliberately different call from I-21's, where the same kind of parse
   error must fail the command because it feeds a cost figure the user reads.
2. `build_executor` now calls this helper instead of passing
   `Config::default()` to `Registry::discover`, and additionally computes
   `rupu_runtime::provider_factory::openai_compatible_map(&cfg.providers)` /
   `provider_tuning_map(&cfg.providers)` — the identical calls `rupu run` and
   `rupu workflow run` make. `WorkflowConfig` gained the five receiving
   fields (`openai_compatible`, `provider_tuning`, `default_provider`,
   `default_model`, `bash_timeout_secs`, `bash_env_allowlist`);
   `start_workflow_with_opts` now threads them into `DefaultStepFactory`
   instead of the hardcoded `HashMap::new()` / `None` / `120` / `Vec::new()`
   the TODO comment (I-18) had documented.
3. `rupu-app`'s `Cargo.toml` gained `rupu-runtime` and `rupu-providers` as
   direct dependencies (needed to name `ProviderTuning` and call
   `provider_factory`; previously reached only transitively through
   `rupu-orchestrator`).

**Validation.** `build_executor` itself isn't directly unit-testable (it needs
a `Workspace` + ambient Tokio runtime + GPUI closure context), so per the
task's fallback instruction the extracted `load_workflow_config` helper is
tested directly, in `crates/rupu-app/src/executor/mod.rs`:

- `load_workflow_config_reads_the_layered_config` — the binding assertion. A
  global `config.toml` sets `default_provider`/`default_model`; a workspace
  `.rupu/config.toml` overrides only `default_model`. Asserts both resolve as
  `Some`, not the `None`/`None` `Config::default()` produced.
- `load_workflow_config_treats_missing_project_file_as_absent` — a workspace
  with no `.rupu/config.toml` at all still resolves the global value (no
  regression to the common "workspace has no config" case).
- `load_workflow_config_falls_back_to_defaults_on_malformed_toml` — a
  present-but-invalid global `config.toml` does not panic and resolves to
  `Config::default()` rather than propagating (matching point 1 above).

`cargo test -p rupu-app --lib` — 27 tests, all passing (3 new). `cargo build
-p rupu-app` — full binary build succeeds in this environment (Metal
toolchain present); `cargo build --workspace` and `cargo clippy -p rupu-app
--lib` are both clean.

### I-21 — a malformed `config.toml` silently yields wrong cost figures

**Symptom.** A typo in `[pricing]` made `rupu run list` / `rupu run show`
print costs computed from default rates, with no warning.

**Root cause.** `crates/rupu-cli/src/cmd/run.rs:339,404` —
`layer_files_locked(...).unwrap_or_default()` discarded a real
`LayerError::Parse` (`crates/rupu-config/src/layer.rs:23-28`) and fed
`cfg.pricing` into `query_run_detail`. The fallback was deliberate per the
comment at `run.rs:335`, but it was reasoned about for UI preferences, not for
numbers the user reads and trusts.

**Fix.** PR (branch `arc1/config-integrity`). Chose to **fail the command**
rather than warn-and-continue: `list()` and `show()` already return
`anyhow::Result<()>`, the dispatcher already renders an `Err` via
`crate::output::diag::fail(e)`, and `run_inner`'s main launch path
(`cmd/run.rs:473`) already propagates the identical `layer_files_locked` error
with `?` — failing here is the path of least surprise, not a new failure mode
for the binary. Both sites now `.map_err(...)` the `LayerError` into an
`anyhow::Error` whose message names the config path and the underlying parse
error (so it's legible even though `diag::fail` prints via `Display`, not the
source chain), and additionally emit a `tracing::warn!` naming the same
file/error before returning it. A **missing** `config.toml` is unaffected —
`layer_files_locked` already treats that as an empty layer with no `Err`, so
the common fresh-install case still resolves `PricingConfig::default()`
without complaint. The identical pattern at `cmd/workflow.rs:993` and
`cmd/cron.rs:281` affects only UI preferences and was deliberately left alone,
per the task brief.

**Validation.** `cargo test -p rupu-cli --lib cmd::run::` — 14 tests, all
passing (2 new):

- `malformed_config_surfaces_on_the_pricing_path` — the binding assertion.
  With `RUPU_HOME` pointed at a temp dir whose `config.toml` is invalid TOML,
  both `list(10, None, None)` and `show("run_missing", None)` return `Err`
  rather than a default-priced report.
- `missing_config_file_still_falls_back_without_erroring` — the regression
  guard: no `config.toml` at all under `RUPU_HOME` still returns `Ok` from
  `list()`, proving only a *present-but-broken* file fails the command.

Both tests hold `crate::test_support::ENV_LOCK` for the `RUPU_HOME`
mutation, mirroring the existing pattern in `cmd/cron.rs`/`cmd/webhook.rs`.
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
