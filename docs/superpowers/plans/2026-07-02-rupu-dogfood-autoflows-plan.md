# Dogfood Autoflows + PR-entity Retro-fit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `PullRequest` autoflow entity + author-allowlist + a `cp serve` cron-tick loop to rupu, then ship four dogfood autoflows (PR code-review, issue triage, nightly maintainability/security, nightly health) in the rupu repo.

**Architecture:** Phase 1 extends the existing autoflow engine (`AutoflowEntity`, selector, claim), the SCM connector (author check — PR list/diff/comment already exist), the polled-event tick, and `cp serve`. Phase 2 is four `.rupu/workflows/*.yaml` autoflows using existing agents/workflows. All effects are comment/label/draft/issue — never auto-merge or push to protected branches.

**Tech Stack:** Rust 2021 (MSRV 1.88), tokio, thiserror (libs) / anyhow (cli), serde/serde_yaml; rupu agents + `code-review-panel` workflow; GitHub/GitLab SCM connectors.

## Global Constraints

- All autoflow effects are **comment / label / draft PR / issue only** — no auto-merge, no push to protected branches, no destructive ops.
- **Author-allowlist gate runs before any agent** — a non-collaborator PR/issue is skipped (optionally labeled `needs-human`); external/untrusted content never drives a tool-running agent.
- **PR claim key = `(repo, pr_number, head_sha)`** — re-review on a new push; never re-process an unchanged PR.
- Issue-opening autoflows **dedup** (rolling issue per category by stable title/label) — no nightly spam.
- Nightly health uses the **pinned** toolchain (`rust-toolchain.toml`); it runs `cargo build`/`test`/`clippy` but **NOT `fmt --check`** (rupu `main` is known fmt-dirty under pinned rustfmt → would be permanently red).
- Backward compatible: additive `AutoflowEntity`/`AutoflowClaimKey` variants + additive selector fields (serde defaults) ⇒ existing issue autoflows + `Manual`/`Cron` triggers behave exactly as today.
- `#![deny(clippy::all)]`; no `unsafe`; `thiserror` (libs) / `anyhow` (cli); workspace deps only; hexagonal (rupu-orchestrator/rupu-agent know only the autoflow model + SCM ports, never rupu-cp).
- **Per-file rustfmt only**: never bare `rustfmt` on `lib.rs`/any `mod`-declaring root file (it reformats the whole crate tree). `--skip-children` does NOT exist in rustfmt 1.9.0 — for a mod-root file, hand-format instead. Never `cargo fmt`. `git status --short` before each commit; `git restore` stray drift by name.
- Clippy `--no-deps`, scoped to changed crates. Pre-existing 1.95-toolchain lints (rupu-cli ssh.rs, `items_after_test_module`, etc.) are unrelated.

## Grounded current shapes (verified)

- `crates/rupu-orchestrator/src/workflow.rs`: `AutoflowEntity { Issue }` (~263); `AutoflowSelector { states, labels_all/any/none, limit }` (~270); `AutoflowIssueState { Open, Closed }`; `AutoflowClaimKey { Issue }` (~301); `Autoflow { enabled, entity, source, priority, selector, wake_on, reconcile_every, claim, workspace, outcome }` (~238); `TriggerKind { Manual, Cron, Event }`.
- `crates/rupu-scm/src/connectors/mod.rs`: `RepoConnector` has `list_prs(&RepoRef, PrFilter) -> Vec<Pr>`, `get_pr(&PrRef) -> Pr`, `diff_pr(&PrRef) -> Diff`, `comment_pr(&PrRef, &str) -> Comment`, `create_pr`. `IssueConnector` has `list`/`comment_issue`/etc. **No `is_collaborator` yet.** GitHub impls under `connectors/github/`, GitLab under `connectors/gitlab/`.
- `crates/rupu-cli/src/cmd/cron.rs`: `tick(dry_run, skip_events, only_events)` (~286) + `tick_polled_events(global, dry_run)` (~377) — issue polling to mirror for PRs.
- `crates/rupu-orchestrator/src/templates.rs`: event context uses `event.pull_request.{number,title,merged}` + `event.repository.full_name` (~485).
- `crates/rupu-cli/src/cmd/cp.rs`: `cp serve` `tokio::spawn`s `run_resume_worker` (~285) + `run_bucket_poller` (~171) at ~58/64 — the pattern for a new `run_cron_tick_loop`.
- `.rupu/agents/`: code-reviewer, maintainability-reviewer, performance-reviewer, security-reviewer, issue-understander, review-diff, summarize-diff, … `.rupu/workflows/`: `code-review-panel.yaml`, `review-changed-files.yaml`, `issue-to-spec-and-plan.yaml`, … (read `code-review-panel.yaml` for the panel step DSL + how a step posts a comment / uses `actions:`).

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `crates/rupu-orchestrator/src/workflow.rs` | `AutoflowEntity::PullRequest` + PR selector fields + `AutoflowClaimKey` PR/head-sha + author-allowlist selector fields + validation | 1, 3 |
| `crates/rupu-orchestrator/src/templates.rs` | PR event context vars (`base/head/head_sha/author/url/diff`) | 4 |
| `crates/rupu-scm/src/connectors/mod.rs` + `github/` (+ gitlab where cheap) | `is_collaborator`/`list_collaborators` for the allowlist | 2 |
| `crates/rupu-cli/src/cmd/cron.rs` | PR polling in `tick_polled_events` + author-allowlist filter + head-sha claim | 5 |
| `crates/rupu-cli/src/cmd/cp.rs` | `run_cron_tick_loop` background worker | 6 |
| `.rupu/workflows/nightly-maintainability-security.yaml` | cron autoflow (independent of Phase 1) | 7 |
| `.rupu/workflows/nightly-health.yaml` | cron autoflow (independent of Phase 1) | 8 |
| `.rupu/workflows/issue-triage.yaml` | issue autoflow (needs allowlist, T3) | 9 |
| `.rupu/workflows/pr-code-review.yaml` | PR autoflow (needs PR entity, T1–T5) | 10 |

**Parallelization:** Tasks **7 and 8** (cron-only YAMLs) depend on nothing in Phase 1 and touch disjoint files — build them **in parallel** with Phase 1 (worktree-isolated subagent). Tasks 1→2→3→4→5→6 are coupled Rust — sequential. Task 9 needs T3 (allowlist); Task 10 needs T1–T5.

---

## Task 1: `AutoflowEntity::PullRequest` + PR selector + claim key

**Files:** Modify `crates/rupu-orchestrator/src/workflow.rs`; Test: same file.

**Interfaces — Produces:** `AutoflowEntity::PullRequest`; `AutoflowSelector` gains `draft: Option<DraftFilter>` (`Include|Exclude|Only`) + `base: Option<String>` (used only for PR entity); `AutoflowClaimKey::PrHeadSha`. Validation: PR-only selector fields on an `Issue` entity → clear error, and vice-versa.

- [ ] **Step 1: Failing tests**
```rust
#[test]
fn pull_request_entity_and_selector_parse() {
    let y = "name: x\nautoflow:\n  enabled: true\n  entity: pull_request\n  selector:\n    states: [open]\n    draft: exclude\n    base: main\n  claim:\n    key: pr_head_sha\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
    let wf = Workflow::parse(y).unwrap();
    let af = wf.autoflow.unwrap();
    assert_eq!(af.entity, AutoflowEntity::PullRequest);
    assert_eq!(af.selector.base.as_deref(), Some("main"));
    assert_eq!(af.claim.unwrap().key, AutoflowClaimKey::PrHeadSha);
}
#[test]
fn draft_filter_on_issue_entity_is_rejected() {
    let y = "name: x\nautoflow:\n  enabled: true\n  entity: issue\n  selector:\n    draft: exclude\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
    assert!(Workflow::parse(y).is_err());
}
```
- [ ] **Step 2:** `cargo test -p rupu-orchestrator -- pull_request_entity draft_filter_on_issue` → FAIL.
- [ ] **Step 3:** Add `PullRequest` to `AutoflowEntity`; add `draft: Option<DraftFilter>` + `base: Option<String>` to `AutoflowSelector` (serde default None) + a `DraftFilter { Include, Exclude, Only }` enum (snake_case); add `PrHeadSha` to `AutoflowClaimKey`. In `Workflow::parse`'s autoflow validation, reject PR-only selector fields (`draft`, `base`) when `entity == Issue`, and reject issue-only combos on a PR entity if any; give clear `WorkflowError` variants.
- [ ] **Step 4:** tests pass; full `cargo test -p rupu-orchestrator` green (additive — existing issue autoflows unaffected).
- [ ] **Step 5:** `rustfmt --edition 2021 crates/rupu-orchestrator/src/workflow.rs`; clippy `-p rupu-orchestrator --lib --no-deps`; commit `feat(autoflow): PullRequest entity + PR selector + head-sha claim key (T1)`.

## Task 2: SCM `is_collaborator` for the author allowlist

**Files:** Modify `crates/rupu-scm/src/connectors/mod.rs` (trait) + `connectors/github/` (impl; gitlab returns a clear `Unsupported`/best-effort); Test: github connector test module (mock transport, mirror existing connector tests).

**Interfaces — Produces:** `RepoConnector::is_collaborator(&self, r: &RepoRef, login: &str) -> Result<bool, ScmError>` (default impl may return `Err(ScmError::Unsupported)` so only GitHub must implement it now).

- [ ] **Step 1: Failing test** — a GitHub connector test with a mock HTTP transport: `is_collaborator(repo, "octocat")` hits `GET /repos/{owner}/{repo}/collaborators/{login}` → 204 ⇒ true, 404 ⇒ false. Mirror how existing github connector tests mock transport.
- [ ] **Step 2:** run → FAIL (method missing).
- [ ] **Step 3:** Add the trait method (default `Err(ScmError::Unsupported("is_collaborator".into()))` if `ScmError` has such a variant, else the nearest); implement on the GitHub `RepoConnector` (204→true / 404→false; map other statuses to `ScmError`). Add a thin GitLab impl only if the members endpoint is trivially available, else inherit the default.
- [ ] **Step 4:** test passes; `cargo test -p rupu-scm` green.
- [ ] **Step 5:** per-file rustfmt the changed non-root files; clippy `-p rupu-scm --no-deps`; commit `feat(scm): is_collaborator for autoflow author allowlist (T2)`.

## Task 3: Author-allowlist selector fields + gate (orchestrator)

**Files:** Modify `crates/rupu-orchestrator/src/workflow.rs` (selector fields + a pure eligibility helper); Test: same file.

**Interfaces — Consumes:** T1 selector. **Produces:** `AutoflowSelector` gains `authors: Vec<String>` (explicit logins) + `authors_from: Option<AuthorScope>` (`AuthorScope { Collaborators, OrgMembers }`); a pure `fn author_allowed(selector, author_login, is_collaborator: bool) -> bool` the tick uses (so the network check stays in the tick/SCM layer, the decision stays pure + testable). `on_skip: Option<SkipAction>` (`SkipAction { Skip, LabelNeedsHuman }`, default `Skip`).

- [ ] **Step 1: Failing tests** for `author_allowed`: explicit-list match; `authors_from: collaborators` + `is_collaborator=true` ⇒ allowed; `is_collaborator=false` + not in list ⇒ denied; empty allowlist + no `authors_from` ⇒ **denied by default** (safe: an allowlist autoflow must specify who) OR allowed — pick DENIED-when-authors_from-set, ALLOWED-when-neither-specified (backward compat: existing issue autoflows without author fields keep matching everyone). Encode that explicitly + test both.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Add the fields (serde default: empty/None ⇒ no author restriction, preserving existing behavior); add `author_allowed`. Validation: `authors_from` requires the SCM to support the check (documented; the tick surfaces a clear error if unsupported).
- [ ] **Step 4:** tests pass; suite green (existing autoflows without author fields unchanged).
- [ ] **Step 5:** rustfmt the file; clippy; commit `feat(autoflow): author-allowlist selector + eligibility gate (T3)`.

## Task 4: PR event context vars (templates)

**Files:** Modify `crates/rupu-orchestrator/src/templates.rs`; Test: same file.

**Interfaces — Produces:** when the tick fires a PR autoflow it builds an `event` context with `event.pull_request.{number,title,base,head,head_sha,author,url}` + `event.pull_request.diff` (or a bounded `event.pull_request.diff` string) + `event.repository.full_name`. This task ensures the template layer renders those paths (extend the sample/context shape + any typed context builder).

- [ ] **Step 1: Failing test** — render `"{{ event.pull_request.number }} {{ event.pull_request.head_sha }} {{ event.pull_request.author }}"` against a context built from a `Pr` + diff → the expected string; and `{{ event.pull_request.base }}`.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Add a helper `pr_event_context(pr: &Pr, diff: &Diff, repo_full_name: &str) -> serde_json::Value` (or extend the existing event-context builder) producing the documented shape. Keep diff bounded (truncate very large diffs with a note) so template/context size stays sane.
- [ ] **Step 4:** test passes; suite green.
- [ ] **Step 5:** rustfmt; clippy; commit `feat(autoflow): PR event context vars for templates (T4)`.

## Task 5: PR polling in `tick_polled_events`

**Files:** Modify `crates/rupu-cli/src/cmd/cron.rs`; Test: same file (mirror the issue-polling test with a fake connector + fake claim store).

**Interfaces — Consumes:** T1 entity/selector/claim, T2 `is_collaborator`, T3 `author_allowed`, T4 `pr_event_context`. **Produces:** `tick_polled_events` handles `AutoflowEntity::PullRequest`: `list_prs(repo, PrFilter from selector)` → filter by selector (state/draft/base/labels) → `author_allowed` (calling `is_collaborator` when `authors_from` is set; on-skip: skip or label) → for each PR whose `(pr_number, head_sha)` is unclaimed, `claim` it and run the workflow with the PR event context (fetch `diff_pr` for the diff). `--dry-run` lists matches without claiming/running.

- [ ] **Step 1: Failing test** — fake `RepoConnector` returning 2 open PRs (one draft, one by a non-collaborator) + a fake claim store; an autoflow selector `{states:[open], draft: exclude, authors_from: collaborators}`. Assert: the draft is filtered out, the non-collaborator is skipped, the eligible PR is claimed by `(number, head_sha)` and dispatched; a second tick with the SAME head_sha does NOT re-dispatch; a tick after the head_sha CHANGES re-dispatches.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Implement the PR branch in `tick_polled_events` (read the existing issue branch and mirror it: entity switch → list → filter → allowlist → claim → dispatch with context). Reuse the existing claim store; the claim id encodes head_sha for PRs.
- [ ] **Step 4:** tests pass; `cargo test -p rupu-cli --lib -- cron` green (ignore pre-existing unrelated cli failures).
- [ ] **Step 5:** rustfmt the file; clippy `-p rupu-cli --no-deps` (no new issues in changed file); commit `feat(autoflow): poll pull-request autoflows in cron tick (T5)`.

## Task 6: `cp serve` cron-tick loop

**Files:** Modify `crates/rupu-cli/src/cmd/cp.rs`; Test: same file (a unit test of the loop body with an injected tick fn + a fake clock/interval, asserting it invokes the tick).

**Interfaces — Produces:** `run_cron_tick_loop(global, interval)` `tokio::spawn`ed at `cp serve` start (beside `run_resume_worker`/`run_bucket_poller`); on each interval it runs the same tick entrypoint `rupu cron tick` uses. Gated by a config/flag (`[cp] cron_tick` enable + `tick_interval`, default enabled with ~60 s cron-due checks; PR/issue polling honored per-autoflow `reconcile_every`). Extract the tick core into a callable fn if it currently only exists as a CLI command body.

- [ ] **Step 1: Failing test** — inject a counting tick fn into the loop body; drive 3 intervals with a controllable ticker; assert the tick fn ran 3×; assert a disabled flag runs it 0×.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Factor the tick core into a reusable async fn (shared by the CLI `cron tick` command and the loop); add `run_cron_tick_loop`; spawn it in `cp serve` behind the enable flag; wire the interval from config with a sane default.
- [ ] **Step 4:** test passes; `cargo build -p rupu-cli`; `cargo test -p rupu-cli --lib -- cp` green.
- [ ] **Step 5:** rustfmt the changed non-root files (cp.rs is not a mod-root); clippy; commit `feat(cp): cron-tick loop worker in cp serve (T6)`.

## Task 7 (PARALLEL): `nightly-maintainability-security.yaml`

**Files:** Create `.rupu/workflows/nightly-maintainability-security.yaml`; Test: a parse/validate assertion (via `rupu workflow` load or an orchestrator parse test fixture) + `rupu cron tick --dry-run` recognizes it.

**Independent of Phase 1** (cron trigger, no PR entity/allowlist). Read `code-review-panel.yaml` + `review-changed-files.yaml` for the step DSL and how a step posts to SCM via `actions:`/agent tools.

- [ ] **Step 1:** Author the YAML: `name`, `autoflow: {enabled: true}`, `trigger: {cron: "0 7 * * *"}`, steps → `maintainability-reviewer` + `security-reviewer` over the crates (a `for_each` over crate dirs or a whole-repo review prompt) → a final step that **opens/updates one rolling issue per category** (title like `Nightly maintainability findings` / `Nightly security findings`; dedup: comment on the existing open issue if present, else create). No auto-fix.
- [ ] **Step 2:** Validate: it parses (`Workflow::parse` fixture test OR `rupu workflow validate`/load) and `rupu cron tick --dry-run --only-events`/cron listing shows it as due-eligible.
- [ ] **Step 3:** commit `feat(autoflow): nightly maintainability+security sweep (T7)`.

## Task 8 (PARALLEL): `nightly-health.yaml`

**Files:** Create `.rupu/workflows/nightly-health.yaml`; Test: parse/validate + dry-run recognizes it.

**Independent of Phase 1.**
- [ ] **Step 1:** Author the YAML: `trigger: {cron: "0 6 * * *"}`, an agent step (bash tool) that runs, **with the pinned toolchain**, `cargo build --workspace`, `cargo test --workspace`, and `cargo clippy --workspace --no-deps` (NOT `fmt --check`), captures pass/fail + failing output, then a step that **opens/updates a rolling `ci-health` issue** on failure (dedup by title/label) and closes/comments it on green. Prompt the agent to use the repo's pinned toolchain (do not override with a system toolchain).
- [ ] **Step 2:** parse/validate + dry-run recognition.
- [ ] **Step 3:** commit `feat(autoflow): nightly build/test/clippy health check (T8)`.

## Task 9: `issue-triage.yaml` (needs T3 allowlist)

**Files:** Create `.rupu/workflows/issue-triage.yaml`; Test: parse/validate + dry-run.

- [ ] **Step 1:** Author: `autoflow: {enabled: true, entity: issue, selector: {states:[open], labels_none:[triaged], authors_from: collaborators}, claim: {key: issue}}`, `trigger: {event: ...}` (or the polled-event trigger shape the issue autoflows use — mirror an existing issue autoflow if one exists, else the schema from workflow.rs). Steps → `issue-understander` over `event.issue` → post a triage comment (via `comment_issue` action/tool) + suggested labels + add the `triaged` label. Read-mostly.
- [ ] **Step 2:** parse/validate; `rupu cron tick --dry-run` recognizes it; author-allowlist present.
- [ ] **Step 3:** commit `feat(autoflow): issue triage (T9)`.

## Task 10: `pr-code-review.yaml` (needs T1–T5)

**Files:** Create `.rupu/workflows/pr-code-review.yaml`; Test: parse/validate + dry-run; ideally an integration check via T5's tick test path with this workflow.

- [ ] **Step 1:** Author: `autoflow: {enabled: true, entity: pull_request, selector: {states:[open], draft: exclude, authors_from: collaborators}, claim: {key: pr_head_sha}}`, trigger polled-event. Steps → `code-review-panel` (reuse/compose it: security + maintainability + performance reviewers with the gate) over `event.pull_request.diff` → a final step posting a **summary comment + inline findings** on the PR via `comment_pr` (no approve/merge). Reference `code-review-panel.yaml` for the panel step DSL.
- [ ] **Step 2:** parse/validate; confirm the PR entity/selector/claim are accepted by T1's parser; `rupu cron tick --dry-run` (with a seeded fake PR, if the harness allows) shows it would fire.
- [ ] **Step 3:** commit `feat(autoflow): PR code-review panel (T10)`.

---

## Self-Review

**Spec coverage:** PR entity → T1; author allowlist → T2 (SCM) + T3 (selector/gate); PR context → T4; PR polling → T5; cp-serve tick loop → T6; the four autoflows → T7/T8/T9/T10. All spec sections mapped. ✅

**Placeholder scan:** Tasks carry concrete test code (T1) + exact interfaces, enum variants, endpoints, cron expressions, selector fields, and file paths; YAML tasks (T7–T10) specify the exact autoflow/trigger/selector/steps + dedup behavior and point at the real reference files (`code-review-panel.yaml`) an author must read. The two genuinely deferred decisions (tick-loop default on/off; diff-as-var vs fetch-step) are called out for the implementer with a recommended default. No "TBD"/vacuous asserts.

**Type consistency:** `AutoflowEntity::PullRequest` (T1) used by T5/T10; `AutoflowClaimKey::PrHeadSha` (T1) used by T5/T10; `AutoflowSelector.{draft,base,authors,authors_from}` (T1/T3) used by T5/T9/T10; `author_allowed` (T3) + `is_collaborator` (T2) used by T5; `pr_event_context` (T4) used by T5; `run_cron_tick_loop` (T6). Names align across tasks.

---

## Execution Handoff

Build via subagent-driven-development. **Parallelism:** dispatch T7 + T8 (cron-only YAMLs, disjoint files, no Phase-1 dependency) in a worktree-isolated stream **concurrently** with the sequential Phase-1 Rust (T1→T6); then T9 (after T3) and T10 (after T1–T5). Final whole-branch review, then one PR to `main` (no self-merge — matt reviews; the autoflows are validated by a dry-run + a real `cp serve` tick on the rupu repo before enabling).
