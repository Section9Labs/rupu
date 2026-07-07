# rupu — dogfood autoflows + PR-entity retro-fit

Status: approved (design), pending implementation plan
Date: 2026-07-02

## Context

rupu supports **autoflows** — workflows (`.rupu/workflows/*.yaml`) with an
`autoflow:` block (`enabled`, `entity`, `selector`, `claim`, …) plus a
`trigger:` (`TriggerKind::{Manual,Cron,Event}`). Firing is real: `rupu cron
tick` (crates/rupu-cli/src/cmd/cron.rs) fires due **cron** workflows and polls
**event** autoflows (`tick_polled_events`), which list matching entities via the
SCM connector, `claim` each (process-once), and run the workflow per match.

**Current limitation:** `AutoflowEntity` has a single variant — `Issue`. The
selector is issue-specific (`states` open/closed, `labels_all/any/none`,
`limit`). There is **no `PullRequest` entity**, so the keystone CI/CD autoflow —
review a PR on open/update — cannot be expressed as YAML today.

`rupu cron tick` is currently a **manual command**; nothing runs it on a
schedule automatically. `cp serve` spawns background workers
(`run_resume_worker`, `run_bucket_poller`) via `tokio::spawn`.

Existing agents (`.rupu/agents/`) and workflows (`code-review-panel`,
`issue-to-spec-and-plan`, `review-changed-files`, …) provide the building
blocks. This initiative dogfoods rupu by maintaining rupu with rupu's own
autoflow engine, and stress-tests a real new capability (the PR entity).

## Goals

- Add a **`PullRequest` autoflow entity** so PR-triggered autoflows are
  expressible as YAML (the retro-fit that unblocks the keystone).
- Add an **author-allowlist** safety gate to autoflow selection so an
  autoflow never drives an autonomous agent on untrusted (non-collaborator)
  content.
- Ship a starter set of **dogfood autoflows** for the rupu repo: PR code
  review, issue triage, nightly maintainability/security sweep, nightly
  build/test/clippy health.
- Make autoflows actually fire on a schedule (wire a **cron-tick loop into
  `cp serve`**).
- Everything is **comment / label / draft / issue only** — no auto-merge, no
  push to protected branches, no destructive automation.

## Non-goals

- Real-time webhook delivery (polling on the cron tick is the model).
- Auto-merge / auto-apply of any change (drafts + comments only).
- GitLab PR-entity parity beyond what is cheap (GitHub first; GitLab
  best-effort where the connector already supports it).
- A `fmt --check` health gate (rupu `main` is known fmt-dirty under the pinned
  rustfmt; such a gate would be permanently red — deliberately excluded).

## Spine decisions (approved)

1. **Scope:** the PR-entity + author-allowlist + tick-loop retro-fit **plus**
   all four autoflows.
2. **Safety = author allowlist** (org members / repo collaborators / explicit
   list). Non-allowlisted PRs/issues are **skipped** (optionally labeled
   `needs-human`). External/untrusted content never drives an agent.
3. **PR review = the full `code-review-panel`** (security + maintainability +
   performance reviewers, with the existing gate), posting a summary + inline
   comments — no approve/merge.
4. **Re-review on every push:** the PR claim key is `(repo, pr_number,
   head_sha)`, so a new push re-reviews while an unchanged PR is never
   re-processed.
5. **Issue-opening autoflows dedup** (a rolling issue per category, keyed by a
   stable title/label) so nightly runs never spam.
6. **Scheduler = a cron-tick loop inside `cp serve`** (beside the existing
   workers), so autoflows fire without an external scheduler.

## Architecture

### Part 1 — Retro-fit (rupu code)

#### 1a. `PullRequest` autoflow entity (`rupu-orchestrator`)
- Add `AutoflowEntity::PullRequest` (`crates/rupu-orchestrator/src/workflow.rs`).
- A PR selector (mirroring the issue selector): `states` (open),
  `draft` (`include`/`exclude`/`only`), `base` (branch name), `labels_any/all
  /none`, `limit`. Parse + validate in `Workflow::parse` with clear errors.
- PR **claim key** `(repo, pr_number, head_sha)` — extend `AutoflowClaimKey`
  so the claim record encodes the head SHA; a new head SHA is a fresh claim
  (re-review), an unchanged SHA is already-claimed (skip).
- PR **context vars** for step templates: reuse the existing `pull_request`
  template shape (`templates.rs` already models `pull_request.{number,title,
  merged,…}`) — extend with `{base, head, head_sha, author, url}` and make the
  **diff** available to steps (a `pull_request.diff` var or a step input).

#### 1b. Author-allowlist gate (`rupu-orchestrator` + `rupu-scm`)
- Selector gains `authors: [<login>…]` and/or `authors_from:
  {collaborators|org_members}` — an entity is eligible only if its author is in
  the allowlist. Non-matching entities are skipped; an optional
  `on_skip: label <name>` marks them `needs-human`.
- The collaborator/member check goes through the SCM connector
  (`rupu-scm`): add `is_collaborator(repo, login)` / `list_collaborators`
  (GitHub; GitLab where cheap). Results may be cached per tick.

#### 1c. PR polling in the tick (`rupu-cli` `cmd/cron.rs`)
- `tick_polled_events` learns to poll PRs when an autoflow's entity is
  `PullRequest`: SCM connector `list_pull_requests(selector)` → filter by
  selector + author allowlist → for each unclaimed `(pr, head_sha)`, claim and
  run the workflow with PR context (number/title/base/head/head_sha/author/url
  + diff).
- SCM connector (`rupu-scm`): ensure `list_pull_requests(selector)`,
  `get_pull_request_diff(number)`, and `comment_on_pull_request(number, body)`
  / inline-comment support exist (GitHub first).

#### 1d. Cron-tick loop in `cp serve` (`rupu-cli` `cmd/cp.rs`)
- Add `run_cron_tick_loop` (beside `run_resume_worker`/`run_bucket_poller`),
  `tokio::spawn`ed at `cp serve` start: on an interval (configurable; default
  ~60 s for cron-due checks, PR/issue polling ~10 min) invoke the same tick
  logic `rupu cron tick` uses. A `[cp] tick_interval` (or reuse the config
  surface from CP Settings) toggles/tunes it; disabled by default is
  acceptable if opt-in is safer — **decide in the plan** (recommend
  enabled-with-a-flag).

### Part 2 — The autoflows (`.rupu/workflows/*.yaml`)

1. **pr-code-review.yaml** — `autoflow: {enabled, entity: pull_request,
   selector: {states:[open], draft: exclude, authors_from: collaborators},
   claim: {key: pr_head_sha}}`; steps → `code-review-panel` over
   `pull_request.diff` → post a summary comment + inline findings on the PR
   (no approve/merge).
2. **issue-triage.yaml** — `entity: issue`, selector `{states:[open],
   labels_none:[triaged], authors_from: collaborators}` → `issue-understander`
   → post triage comment + suggested labels + add `triaged`.
3. **nightly-maintainability-security.yaml** — `trigger: {cron: "0 7 * * *"}`
   → `maintainability-reviewer` + `security-reviewer` over the crates
   (changed-since-last or a rotating subset) → open/update **one rolling issue
   per category** (dedup by stable title/label).
4. **nightly-health.yaml** — `trigger: {cron: "0 6 * * *"}` → an agent (bash
   tool) runs the **pinned-toolchain** `cargo build` + `cargo test` + `cargo
   clippy` (NOT `fmt --check`) → on failure open/update a rolling `ci-health`
   issue with the failing output; on green, close/comment it.

### Operational

`cp serve` runs the tick loop on the `local_checkout` worker (repo + toolchain
present). Cron autoflows fire nightly; PR/issue polls ~every 10 min. `claim`
prevents double-processing across ticks; the head-SHA claim gives re-review on
push. Autoflows are visible in the CP Build → Autoflows view.

## Errors & security

- **Author allowlist enforced before any agent runs** — a non-collaborator PR/
  issue never drives a tool-running agent; it is skipped (optionally labeled).
- All effects are comment/label/draft/issue — no auto-merge, no push to
  protected branches, no `rm`/destructive ops. Agents run in the run's
  configured mode (read-only/ask) appropriate to the autoflow.
- Health check uses the **pinned** toolchain (`rust-toolchain.toml`) so results
  are meaningful, not 1.95-drift noise; excludes the known-dirty fmt check.
- Issue-opening autoflows **dedup** (rolling issue per category) — no nightly
  spam.
- `claim` TTL prevents a stuck run from blocking an entity forever.
- No new secrets; SCM auth via the existing connector credentials.
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI
  `anyhow`; workspace deps only; hexagonal (orchestrator/agent know only the
  autoflow model + SCM ports, never `rupu-cp`).

## Testing

- **Retro-fit:** `AutoflowEntity::PullRequest` + PR-selector parse/validate
  (states/draft/base/labels/author); PR claim by `(pr, head_sha)` — re-review
  on new SHA, skip unchanged; author-allowlist gate skips a non-collaborator;
  `tick_polled_events` selects matching PRs and fires with rendered PR context;
  SCM connector `list_pull_requests`/diff/comment (mock transport).
- **Cron-tick loop:** the `cp serve` loop invokes the tick on interval (a
  short-interval unit test with a fake clock/tick, or an injected tick fn).
- **Autoflows:** each YAML parses + validates (autoflow.enabled + trigger +
  selector); `rupu cron tick --dry-run` lists what would fire for a seeded
  repo state; dedup logic doesn't re-open an existing rolling issue.

## Decomposition (two plans)

- **Plan 1 — retro-fit (rupu code):** PR entity + PR selector + head-SHA claim
  + author allowlist + SCM PR methods + PR polling in the tick + cron-tick loop
  in `cp serve`. Sequential (coupled Rust); the deliverable is "PR/allowlist
  autoflows can fire on a schedule."
- **Plan 2 — the four autoflow YAMLs.** The three issue/cron autoflows depend
  only on existing capability and can be built in **parallel** with Plan 1;
  `pr-code-review.yaml` depends on Plan 1's PR entity.

## Open questions (resolve in the plan)

- **Tick-loop default on/off** in `cp serve` (recommend on, gated by a config
  flag, so dogfooding is live but disable-able).
- **Diff delivery to the panel** — a `pull_request.diff` template var vs a
  step that fetches the diff via the SCM tool. Prefer the template var if the
  diff size is bounded; else a fetch step.
- **`authors_from: collaborators` caching** granularity (per tick vs TTL).
