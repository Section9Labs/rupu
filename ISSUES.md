# rupu — known issues

Bugs and correctness defects in shipped code. Distinct from [`TODO.md`](TODO.md),
which tracks *deferred features*; this file tracks *things that are wrong*.

Each entry should name **symptom**, **root cause** (with `file:line`), **impact**,
and **fix**. Move an entry to `## Fixed` with the PR number when it lands.

---

## Open

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
