# Live usage ledger — accurate, near-real-time tokens and cost

**Date:** 2026-09-29
**Status:** approved in chat (matt: "fix all of it and make it live in the CP";
Anthropic prompt caching **on by default**, per-provider/per-agent opt-out)
**Plans:** Plan 1 — ledger + correct accounting (Rust); Plan 2 — live CP + transport
(web + host mirrors); Plan 3 — Anthropic prompt caching + cache-token accounting.

## 1. Problem

Token and cost numbers are wrong and stale, across every surface. Measured against
`~/.rupu/runs` on 2026-09-29 (CP method = what the CP shows; actual = every transcript
the run's own `events.jsonl` + `sub/` dirs point at):

| Run | Status | CP shows | Actual | Missing |
|---|---|---:|---:|---:|
| `run_01M1NC9AN0V749FACZHQZQCPNT` | failed | 71,773 | 118,163,026 | ~100% |
| `run_01KWD9YK7NZZ7NDRQN4G5N9YJF` | failed | 247,236 | 17,642,620 | 99% |
| `run_01M1J749ZX7157Y56D1VRZH7BK` | cancelled | 0 | 4,617,813 | 100% |
| `run_01KWDJWTSQ76NNHE1C31ZQWEVY` | completed | 1,118,716,123 | 1,142,818,709 | 24.1M (2%) |

Root causes (all verified by reading code):

1. **Completion-gated source.** `rupu-cp/src/usage.rs::run_transcript_paths` builds a
   run's transcript list only from `step_results.jsonl`, appended when a step
   **completes**. In-flight steps count 0; cancelled/failed runs lose their in-flight
   steps forever; every aggregate (`/api/usage*`, outliers, agents/projects/workflows
   rollups, runs lists, `rupu run show/list`) inherits this.
2. **Retried fan-out units dropped.** An `ItemResultRecord` keeps only the final
   attempt's transcript; in `run_01KWDJWT…` 21 units started 22 extra times and those
   22 transcripts are exactly the missing 24.1M tokens.
3. **Invisible work.** `dispatch_agent` children have no slot in `StepResultRecord`;
   `parallel` sub-steps emit no `UnitStarted` and get no sink; the compaction
   summariser's LLM calls (`rupu-agent/src/runner.rs::compact_messages`) write no
   `Usage` event at all.
4. **No liveness.** Web `RunDetail` fetches header usage (`getRunGraph`) and the
   per-turn chart (`getRunUsageTimeline`) once on mount. `SessionDetail` header +
   timeline are fetched once; session totals come from `session.json`, which is only
   updated when a turn ends. The Usage page never refreshes.
5. **Remote runs.** SSH/tunnel/bucket runs compute usage from local mirrors; step
   transcripts are only pulled when terminal or opened, so a live remote run shows 0.
   SSH autoflow rows are hard-coded 0; SSH agent rows carry total only.
6. **Scope gaps.** Standalone `rupu run` agent runs and session turns are excluded from
   every aggregate Usage endpoint (they only appear in the agent-runs list).
7. **Broken live readers.**
   - `rupu-cp/src/transcript_tail.rs` re-reads the whole file every 250 ms per
     viewer, advances past a partial trailing line (the event is lost), and drops a
     whole chunk when the byte slice splits a UTF-8 char.
   - `FileTailRunSource` has a drain/poll race that can replay the backlog twice.
   - The transcript footer shows the *first* `usage` event, not a running sum
     (`web/src/components/transcript/transcriptView.ts:586-594`).
8. **CLI live view is a sampler.**
   - It accumulates tokens only from the one focused transcript
     (`rupu-cli/src/output/live_run.rs:1609-1651`), so concurrent fan-out units are
     missed.
   - `TranscriptTailer::new` restarts at byte 0 on refocus, so it re-counts.
   - `DispatchCompleted` totals are added on top of the child transcript it already
     tailed, so dispatch is double-counted.
   - `rupu run <agent>` and `--plain` show totals only at the end, with no cost.
9. **Honesty.** `UnitCompleted.tokens_in/out` are hard-coded `0` even for local units,
   whose `RunResult` carries real totals. Session `total_tokens_cached` is taken from
   a last-snapshot file (`live-usage.json`), not summed.
10. **Anthropic cache accounting (latent).**
    - `cache_creation_input_tokens` is never read.
    - Anthropic's `input_tokens` *excludes* cache reads and writes, but
      `ModelPricing::cost_usd` assumes `cached ⊆ input`.
    - The pricing comment claims writes are "folded into input_tokens" — false.
    - It is latent only because rupu never sends `cache_control`: every Anthropic
      transcript on disk has `cached_tokens: 0`.

## 2. Goals / non-goals

**Goals**
- Every token an agent spends inside a run is counted exactly once. That covers
  in-flight turns, retries, dispatch children (any depth), parallel sub-steps, panel
  reviewers and fixers, compaction calls, and cancelled/failed runs.
- The CP shows a running run's tokens and cost within ~2 s of each LLM turn. This
  applies to local runs and to runs on SSH/tunnel hosts; bucket hosts are bounded by
  their 15 s poll.
- One fold function is the only definition of "a run's usage". The CP, the CLI live
  view, `rupu run show/list` and `rupu usage` all use it.
- Aggregates (Usage page, outliers, rollups) include standalone agent runs and
  session turns, each transcript counted once.
- Historical runs get the corrected transcript set (fallback path) with no migration.
- Anthropic prompt caching on by default; cache reads/writes accounted and priced
  correctly.

**Non-goals**
- Re-attributing usage by *turn time* in the Usage page window (it keeps run-start
  semantics).
- Back-filling ledger files for runs that ran before this change (the fallback path
  covers them).
- A run's already-running process: an in-flight run started by an old binary writes
  no ledger; the CP's fallback path counts what is locally readable. For remote
  runs, that is only transcripts already mirrored.
- The dashboard (it shows no token fields).
- The deprecated macOS app. It must keep decoding: new fields are additive and
  `make macos-fixtures` is regenerated.

## 3. The usage ledger (Plan 1)

### 3.1 File and row

`<runs>/<run_id>/usage.jsonl` — append-only, one JSON object per LLM call made
anywhere inside the workflow run (`RunStore::usage_ledger_path(run_id)`).

```json
{"v":1,"id":"01K…ULID","at":"2026-09-29T20:14:03.120Z","kind":"turn",
 "step_id":"assess","unit_index":3,"unit_key":"app-gw",
 "agent_run_id":"run_01K…","parent_agent_run_id":null,
 "transcript":"/…/transcripts/run_01K….jsonl","agent":"reviewer",
 "provider":"anthropic","model":"claude-opus-5-5",
 "input_tokens":41234,"output_tokens":1056,"cached_tokens":0}
```

- `id` — ULID minted at emission. It is the dedup key: a consumer that sees the same
  line twice (SSH pump replay, bucket re-append after CP restart, tailer races)
  counts it once.
- `kind` — `turn` | `compaction`.
- `model` / `provider` — the same values the transcript `Usage` event records
  (requested model; `served_model` is not needed for pricing).
- `output_tokens` — billable output (`output + reasoning`), identical to the
  transcript.
- `step_id` / `unit_index` / `unit_key` — set by the orchestrator for every
  step/unit it launches.
- Dispatch children have `step_id: null` and `parent_agent_run_id` set; the fold
  attributes them to their root ancestor's step.
- Plan 3 adds `cache_write_tokens` (serde default `0`).

### 3.2 Writer

`rupu_orchestrator::usage_ledger::UsageLedger` — a cloneable
`Arc<Mutex<File>>`, opened `O_APPEND | O_CREAT`.
- `append(&LedgerRow)` serialises the row plus `\n` into one buffer and issues
  **one** `write_all` under the lock, then flushes.
- The file is never truncated.
- A write error logs `warn!` and is swallowed: usage accounting must never fail a
  run.
- Several processes may append to one ledger (a resume worker, a `dispatch_agent`
  in a CLI process). Each row is one `write(2)` on an `O_APPEND` file, well under
  4 KiB, so rows never interleave on local filesystems.

### 3.3 Emission

- **Agent loop hook.** Add
  `AgentRunOpts.on_usage: Option<OnUsageCallback>`, where
  `OnUsageCallback = Arc<dyn Fn(&UsageTurn) + Send + Sync>` and
  `UsageTurn { kind, provider, model, input_tokens, output_tokens, cached_tokens }`.
  - It is invoked immediately after every transcript `Event::Usage` write:
    `runner.rs` ~1316, *before* any early exit.
  - `compact_messages` gains a `Usage` transcript event and an `on_usage(kind:
    compaction)` call for the summariser request.
- **Workflow steps.** `dispatch_one` (`rupu-orchestrator/src/runner.rs` ~7449) is
  the single choke point for every local agent launch in a workflow: linear, the
  for_each unit, parallel sub-step, panel reviewer, panel fixer and reject cleanup.
  - It gains a `LedgerTag { step_id, unit_index, unit_key }` argument.
  - After the `StepFactory` returns, it sets `agent_opts.on_usage` when
    `OrchestratorRunOpts.usage_ledger` is `Some`.
  - This mirrors how `pause` is set, and leaves the ~46 `StepFactory` impls
    untouched.
- **Dispatch children.** `CliAgentDispatcher` gains
  `usage_ledger: Option<UsageLedger>`, set where it is built for a workflow run
  (`cmd/workflow.rs` ~3214 / ~4759, `resume.rs` ~297).
  - Child rows carry `agent_run_id = sub_run_id` and `parent_agent_run_id = the
    dispatching agent's run id`.
  - Standalone `rupu run` has no ledger. Its children are counted by the fallback
    path (§4.2).
- **Placed/distributed units on remote hosts** do not go through `dispatch_one` and
  write no coordinator rows. They are counted by the fallback path from their
  mirrored transcript (`agent_mirror_path`), which the SSH pump fills live.
- **Where the ledger comes from:** no new `OrchestratorRunOpts` field. The
  runner derives it from the `run_store` it already has plus the workflow run
  id (`UsageLedger::for_run`), whenever both are present — including on resume.

### 3.4 Honest unit totals

`UnitCompleted.tokens_in/out` carry real values:
- **Local units:** from `dispatch_one`'s `RunResult` (callers stop discarding it).
- **Remote units:** a one-shot fold of the mirrored transcript after
  `await_run_mirror`.
- The doc comment on the variant is updated.

## 4. The fold — one definition of a run's usage (Plan 1)

### 4.1 Inputs

- **L** — ledger rows from `usage.jsonl`, deduped by `id`.
- **K** — the run's *known transcripts*, keyed by `transcript_key(path)`. Ledger
  rows are keyed by the **same function applied to their `transcript` field**, so
  the two sides can never disagree on identity. K is the union of:
  - `step_results.jsonl` transcript paths (steps and items);
  - `events.jsonl` transcript paths (`StepWorking`, `UnitStarted`,
    `DispatchStarted`);
  - `RunStore::sub_run_ids_recursive(run_id)` transcripts.

  The key is the transcript file stem, or the parent dir name for the nested
  `…/<sub_id>/transcript.jsonl` layout. This is the same rule as
  `findings.rs::sub_run_ids_from_events`, which is refactored onto the new shared
  `RunStore::known_transcripts`.

### 4.2 Rule

A run's usage is:

- the **sum of ledger rows**; plus
- for each known transcript whose key has **no** ledger row, a **fold of that
  transcript file**.

  The file is resolved with the existing mirror rules: local path, then the worker
  mirror cache, then the agent mirror path. The fold uses the same per-file
  semantics as today's `aggregate_rows_and_metrics`: anchored on `RunStart`, a
  `Usage` event's own provider/model wins, `RunComplete` sets the duration.

This one rule covers every case:
- new local runs (all ledger);
- legacy runs (all fallback, now with the corrected set that includes retries,
  cancels and dispatch children);
- remote placed units (fallback on the live mirror);
- a transcript whose ledger write failed.

If a known transcript is not readable anywhere, it contributes nothing and sets
`partial = true`.

### 4.3 Output

```rust
pub struct RunUsage {
    pub rows: Vec<UsageRow>,                       // by (provider, model, agent)
    pub by_step: BTreeMap<String, Vec<UsageRow>>,  // "" = unattributed
    pub turns: u64,
    pub duration_ms: Option<u64>,
    pub points: Vec<TurnPoint>,  // ledger arrival order, then fallback order
    pub partial: bool,
}
```

Pricing is applied by consumers (`rupu-cp::usage::summarize`, the CLI's
`rupu_config::pricing`), never stored.

### 4.4 Placement

- `rupu-transcript` gains `JsonlCursor`: an offset-based incremental line reader.
  - It seeks to the offset and reads to EOF.
  - It consumes only up to the last `\n`, holding back a partial trailing line.
  - It decodes UTF-8 only on whole lines.
  - It reports `Reset` when the file shrank.

  The CP tailers and the CLI all use it.
- `rupu-orchestrator::usage_ledger` holds `LedgerRow` and `UsageLedger`, and
  `RunStore::known_transcripts` provides the known set.
- The fold plus its process-wide incremental cache live in
  `rupu-cp::usage_index`, behind `rupu_cp::usage::{run_usage,
  transcripts_usage}`. The CLI already depends on `rupu-cp`
  (`cmd/session.rs` uses `rupu_cp::usage`), so one implementation serves both.
  The existing `summarize_run` / `run_metrics` / `run_transcript_paths` keep
  their signatures and are re-implemented on top of it.

## 5. Control plane (Plans 1 + 2)

### 5.1 `UsageIndex` (Plan 1)

`AppState.usage_index: Arc<UsageIndex>` keeps, per `(store root, run_id)`, the
cursors for `usage.jsonl`, `events.jsonl` and each fallback transcript, plus the
fold state (`usage_index`). Each request reads only bytes appended since the last one; a
shrunk file resets that file's contribution.

`UsageIndex` replaces `summarize_run` / `run_metrics` / `run_transcript_paths`
consumers:
- graph header;
- `RunListRow::with_usage`;
- `/api/usage` and `/api/usage/runs`;
- `/api/usage/timeline`;
- outliers;
- agents, projects and workflows rollups;
- `run_streams` autoflow rows;
- the run usage-timeline endpoint (series from `RunUsage.points`, which fixes the
  mirror-cache gap noted in the audit).

Heavy aggregate endpoints do their IO in `spawn_blocking`.

`run_transcript_paths` survives only as the K-resolution helper.

### 5.2 Aggregate scope (Plan 1)

Aggregates add two sources, with each transcript file counted **once**, attributed
workflow run > session > standalone:
- standalone runs: `<global>/transcripts/*.jsonl`, non-recursive (archive
  excluded);
- session turns: `session.json` `runs[].transcript_path`.

Transcripts already claimed by any run's K (for example placed-unit mirrors) are
excluded.

`UsageRunRow` gains `kind: "workflow" | "agent" | "session"`; standalone rows have
`workflow_name: null`. Session usage (`api/sessions.rs::session_usage` and the
timeline) folds the session's run transcripts instead of `session.json` totals.

### 5.3 Live endpoint (Plan 2)

`GET /api/runs/:id/usage[?host=][&since=N&epoch=E]` returns:

```json
{"summary": UsageSummary, "steps": {"<step_id>": UsageSummary}, "turns": 1234,
 "partial": false, "epoch": "…", "points_from": N, "points": [TurnPoint…]}
```

- `points` are appended in arrival order.
- When `epoch` mismatches (CP restart, file reset, or a fallback transcript
  appearing out of order), the server answers from `0` with the new epoch.
- It routes by `?host=` / `resolve_run_location` exactly like
  `get_run_usage_timeline`: mirrored transports build from the local mirror,
  and HTTP hosts proxy.
- `UsageSummary` gains `partial: bool` (serde default `false`).

### 5.4 Readers (Plan 2)

`transcript_tail.rs` and `executor/file_tail.rs` switch to `JsonlCursor`:
- offset reads, not full-file re-reads;
- no partial-line loss;
- no UTF-8 chunk drops;
- a single cursor owner, removing the drain/poll double replay.

## 6. Transport for remote hosts (Plan 2)

- **SSH:** the tail pump adds `usage.jsonl` to its `tail -n +1 -F` list and routing
  (`host/ssh.rs` ~1364 / ~1437).
- **Tunnel:** `rupu node` forwards `usage.jsonl` lines like `events.jsonl`, and the
  CP appends them to the mirror.
- **Bucket:** `usage.jsonl` joins the batch upload. It is ≤15 s behind by design.
- **HTTP:** the live endpoint is proxied. An older remote CP returns 404, and the web
  falls back to the one-shot graph usage.
- Ledger dedup-by-`id` makes any re-append harmless.

## 7. Web (Plan 2)

- **`RunDetail`:** while the run is non-terminal, it polls `/api/runs/:id/usage`
  every 2 s, and once more on reaching a terminal state.
  - The header (in/out/cached/total/cost, with `≥` when `partial`) and the "Token
    usage by turn" chart come from it.
  - Graph nodes that show tokens use `steps`.
  - Polling stops when the tab is hidden (`document.visibilityState`).
- **Runs lists:** unchanged cadence (Active tab 5 s). The server numbers are now
  live and correct.
- **`SessionDetail`:** refreshes session usage and the timeline on the existing
  runs-poll cadence while a turn is active.
- **Transcript footer:** a running sum of all `usage` events, deduped against the
  SSE backlog (the `TranscriptPanel` snapshot/stream overlap).
- **Usage page:** refetches every 30 s when the selected window ends at "now".
- The ledger never enters `events.jsonl`, so Live Events, the RunDetail feed, the
  firehose and the dashboard are unaffected.

## 8. CLI (Plan 1)

- **`output/live_run.rs`:**
  - Tokens and cost come from `rupu_cp::usage::run_usage` (ledger + fallback), read each tick
    via `JsonlCursor`. That covers all units and dispatch children, with no
    refocus re-count.
  - The transcript-`Usage` accumulation and the `DispatchCompleted` token add are
    removed.
  - The focused-transcript tail stays for the activity feed only.
- **`rupu run <agent>`:** a live `⇡in ⇣out · $cost` status, updated from the
  transcript `Usage` events it already tails.
- **`--plain` workflow printer:** per-step `tokens · $cost` at step end from the
  ledger fold, plus a run total with cost.
- **`rupu run show/list`, `rupu usage`:** `rupu_cp::usage::run_usage`.
- **Sessions:** `total_tokens_cached` is summed from the turn's `RunResult` (which
  gains `total_tokens_cached`), not the live snapshot.

## 9. Anthropic prompt caching (Plan 3)

1. **Normalise `Usage` semantics across providers.** `input_tokens` = the whole
   prompt. `cached_tokens` = cache reads ⊆ input. A new `cache_write_tokens` =
   cache writes ⊆ input.
   - `anthropic.rs` reads `cache_creation_input_tokens` and folds reads and writes
     into `input_tokens`. It does this in `message_start`, in `message_delta` (when
     present) and in non-streaming `decode_response`.
   - The broker types pass the field through.
   - The transcript `Event::Usage`, `UsageRow`, the ledger row, `UsageSummary` and
     `TurnPoint` gain `cache_write_tokens` (serde default `0`, skipped when `0`
     where the shape allows).
   - Historical Anthropic transcripts all have `cached_tokens: 0`, so the semantic
     change needs no migration.
2. **Pricing.** `ModelPricing.cache_write_per_mtok: Option<f64>`.
   - `cost_usd(input, output, cached, cache_write)` =
     `(input − cached − cache_write)·in + cached·read + cache_write·write + output·out`.
     `write` defaults to the input rate when unset.
   - All built-in Anthropic entries get `write = 1.25 × input` (5-minute TTL).
   - Add `claude-opus-5-5` ($4 / $20, read $0.20, write $5.00) and
     `claude-sonnet-5-5` ($2 / $10, read $0.20, write $2.50).
   - Fix the misleading comment.
3. **Enable caching.** When enabled, the request builder places explicit
   `cache_control: {"type":"ephemeral"}`:
   - (a) on the last `system` block — this caches tools + system;
   - (b) on the last content block of the final message — the rolling
     conversation breakpoint. String content is converted to block form, and
     thinking blocks and restored raw reasoning blocks are never marked.

   That is two breakpoints, under the limit of 4. It is placed after the tool-name
   sanitiser and after `restore_reasoning_blocks`. Explicit markers work on API key,
   OAuth, and Anthropic-compatible gateways alike.
4. **Config.** `ProviderConfig.prompt_cache: Option<bool>` (`None` ⇒ on) and agent
   frontmatter `anthropicPromptCache: false`. Both are threaded through
   `rupu-runtime/src/provider_factory.rs` next to `with_oauth_system_prefix`, into
   `AnthropicClient::with_prompt_cache(bool)`.
5. **Verification.**
   - Request-shape tests: markers present or absent, count ≤ 4, raw reasoning
     untouched.
   - Usage parse tests for both streaming and non-streaming, including the
     normalisation, and pricing tests.
   - An `#[ignore]` live test (run only with matt's approval — it spends money)
     asserting that a second identical request reports
     `cache_read_input_tokens > 0`.

## 10. Testing

- **Unit:**
  - `JsonlCursor`: partial line, UTF-8 split, shrink reset.
  - Ledger writer: concurrent appends produce whole lines.
  - `usage_index` fold: dedup, fallback exclusion by key, dispatch attribution through
    ancestry, compaction kind, partial.
  - `UsageIndex`: incremental append equals a one-shot fold.
- **Orchestrator integration (MockProvider):**
  - A linear step plus a 3-unit for_each plus a panel plus a parallel step: ledger
    rows equal LLM calls, and are correctly attributed.
  - A retried unit's first attempt is counted.
  - A cancelled run's in-flight step is counted.
  - A `dispatch_agent` child is attributed to its step.
- **Regression against real data:**
  - A read-only check over `~/.rupu/runs` that `rupu_cp::usage::run_usage` equals the sum over
    every transcript the run references (the §1 measurement).
  - It is run once and the result recorded in the PR; it is not committed as a
    fixture (public repo).
- **CP:**
  - The live endpoint `since`/`epoch` contract.
  - Aggregates include standalone and session transcripts exactly once.
  - The SSH pump routes `usage.jsonl`.
- **Web (vitest):**
  - `RunDetail` polls while live and stops when terminal or hidden.
  - The transcript footer sums.
  - `SessionDetail` refreshes.
- **CLI:** the live view sums all units with no dispatch double count; the plain
  printer prints cost.
- **GUI rule:** CP web changes are checked in a browser against a live run before
  merge (matt runs it; Claude pre-verifies with the in-app browser against a local
  `rupu cp serve` and a simulated ledger).

## 11. Compatibility

- Older CP + newer runs: `usage.jsonl` is ignored, and the fallback still gives
  corrected-set totals.
- Newer CP + older runs: all fallback.
- `UsageSummary.partial`, `UsageRunRow.kind`, `cache_write_tokens`: additive; macOS
  fixtures regenerated (`make macos-fixtures`).
- No change to `events.jsonl` or to the `Event` enum.
