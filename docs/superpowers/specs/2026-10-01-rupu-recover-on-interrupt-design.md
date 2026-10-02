# rupu — Recover on interrupt: continue interrupted agent work from its transcript

**Date:** 2026-10-01 · **Status:** design approved in conversation; spec for review

## Context

When a workflow's runner stops before it finishes — a pause, a closed terminal, a
killed process, a crash, an unreachable host — `rupu workflow resume` restarts every
agent attempt that was in flight from its original prompt. On long fan-outs this is
the dominant cost: each in-flight unit may have run for many turns, and all of it is
paid for again.

PR #705 made *finished* fan-out units durable (each unit checkpoints the moment it
finishes). This spec covers the remaining waste: work that was *in progress* when the
runner stopped.

What already exists and this design builds on:

- **Durable transcripts.** Every agent attempt writes its transcript event by event, so
  it survives any interruption. A clean end writes `RunComplete { status }`: `ok` when
  the run finished, `aborted` for a cooperative pause or a shutdown signal
  (`"terminating (SIGTERM)"`), `error` for a failure. A process that died writes no
  `RunComplete` at all.
- **Replay.** `rupu_agent::replay::reconstruct_transcript(path)` rebuilds the exact
  provider conversation (v2 transcripts include reasoning blocks with their `raw`
  payloads), drops a trailing turn that has no `TurnEnd`, and resolves seed chains up to
  1024 links deep.
- **Seed by reference.** `AgentRunOpts::seed_source` makes a new run's transcript start
  with a `Seed` event that references the earlier transcript by path + sha256 instead of
  copying it. `rupu session` already chains its turns this way.
- **Graceful pause of linear steps.** Today a manual pause that lands mid-step persists
  the agent's in-memory messages to `paused_seeds.json`; resume re-seeds the step from
  them. A tool-boundary pause writes `TurnEnd` before stopping, and a mid-stream pause
  drops the partial turn — so replaying the transcript yields exactly the same
  conversation as the in-memory seed.
- The transcript-fidelity spec (`2026-08-31-rupu-transcript-fidelity-design.md` §4)
  already names "converging resume onto `reconstruct_messages`" as a follow-up.

## Decisions (approved)

1. **Scope:** all local agent work — linear steps, `for_each` units, `parallel`
   sub-steps — and remote `distribute:` units. **`panel` steps are excluded from v1**:
   their round state (iteration, reviewer findings) lives only in memory, so a panel
   interrupted mid-round restarts (flagged). Persisting panel round state is a
   follow-up.
2. **Default on.** Every resume continues interrupted attempts.
   `rupu workflow resume --restart-interrupted` (and the matching CP resume option)
   restores today's restart-from-scratch behaviour.
3. **When an attempt can't be continued, restart just that unit and say so.** A
   `restarted` outcome carries its reason and is shown in the CLI and CP. The rest of
   the resume carries on. Nothing degrades silently.
4. **Architecture:** a reusable primitive (continue an agent run from its transcript,
   also exposed as `rupu run --continue`) + discovery from a new per-run attempts
   ledger, with a fallback to `events.jsonl` for runs that predate the ledger — so runs
   interrupted, paused or cancelled before this ships can be continued too.

## Goals

- After any interruption, no agent work is redone except the single turn that was in
  flight when the runner stopped.
- One recovery path for pause, kill, crash and host loss.
- Works on existing runs (via the `events.jsonl` fallback).
- Every resumed attempt reports what happened to it: continued, recovered, or
  restarted (with why).

## Non-goals

- Continuing a `panel` step mid-round (restarted, flagged — follow-up).
- Resuming `workspace: sync` workflows (still refused, unchanged).
- Continuing attempts that *failed* (`RunComplete { status: error }`, or a failed unit
  checkpoint) — those restart as today.
- Continuing a remote unit on a different host than the one holding its transcript.
- Resuming a partially streamed provider response (impossible by design).
- Exactly-once tool side effects: the dropped in-flight turn may have partially applied
  a change. The continuation note tells the agent to check.

## Design

### 1. The continuation primitive (`rupu-agent`, new `continuation` module)

```rust
pub enum Continuation {
    /// The attempt finished; only its record was lost. No model call needed.
    Finished { output: String },
    /// Rebuild and continue.
    Resume { messages: Vec<Message>, seed_source: PathBuf },
    /// The attempt ended in failure — not an interruption. `seeded_from` is the
    /// transcript it was seeded from when it was itself a continuation.
    Failed { error: Option<String>, seeded_from: Option<PathBuf> },
}

pub fn prepare_continuation(transcript: &Path) -> Result<Continuation, ContinuationError>;
```

Classification by how the transcript ends:

| Transcript ends with | Outcome |
|---|---|
| `RunComplete { status: ok }` | `Finished` — output = final assistant text |
| `RunComplete { status: error }` | `Failed` |
| `RunComplete { status: aborted }` (pause or shutdown signal), or no `RunComplete` (process died) — and the rebuilt conversation ends in a **user** message | `Resume` |
| the same, but the rebuilt conversation ends in an **assistant answer with no tool call** (the final turn completed; only `RunComplete` was lost) | `Finished` — output read the same way as for `ok`, so it never depends on whether the last flush landed |
| the same, but the rebuilt conversation ends in an assistant message **with a tool call and no result** | `Err(DanglingToolCall)` |
| no `RunStart`, or nothing rebuilds (empty conversation) | `Err(NotAnAgentRun)` |
| missing, unreadable, malformed, seed-chain/hash error | `Err(Read)` / `Err(Replay)` → caller restarts, flagged, with the error as the reason |

Replay now applies the role-alternation merge below. A session transcript chain
written *before* this change recorded its `Seed` hashes over the *unmerged* history
(two consecutive user messages), so one that crosses a compaction may now fail replay
with `SeedHashMismatch` — loud, not silent, and nothing replays sessions in
production today.

`Resume.messages` is `reconstruct_transcript(transcript)`. Because the trailing
incomplete turn is dropped, the conversation always ends on a **user** message (the
original prompt, or the tool results of the last complete turn).

**Running a continuation.** The caller builds the agent's opts exactly as for a fresh
attempt, then sets `initial_messages = messages`, `seed_source = Some(transcript)`, and
`user_message = CONTINUATION_NOTE`:

> This session was interrupted and has been resumed. Work from the step you were in
> the middle of was lost and may be partially applied — check the current state, then
> continue the task.

If the rebuilt conversation *already ends with that note* (the attempt being continued
was itself a continuation that died before finishing its first turn), the new run is
seed-only — empty `user_message` — so the note is never stacked twice.

**Role-alternation rule (runtime + replay, lockstep).** Today `run_agent` appends a
non-empty `user_message` as a new user message. When `initial_messages` already ends in
a user message, that would put two user messages in a row, which some providers
reject. New rule: a non-empty `user_message` that follows a trailing user message is
**merged into it as an extra text block**. The transcript still records it as its own
`UserMessage` event after the `Seed`; `reconstruct_messages` applies the same merge, so
replay keeps reproducing the runtime conversation exactly. The seed itself stays
byte-identical to the referenced transcript's replay, so its sha256 verifies.

The new attempt gets a fresh agent run id; its transcript links to the previous one
through the `Seed` reference, so a unit interrupted several times forms a chain that
replay follows.

**`rupu run <agent> --continue <agent_run_id>`** exposes the primitive for
standalone agent runs (and is what remote hosts execute, §5): it resolves
`<transcripts>/<id>.jsonl`, refuses when the transcript's `RunStart` names a
different agent, builds the run from that agent's *current* definition, and runs
`prepare_continuation`. `Finished` prints the recovered output without calling the
model; `Failed` exits non-zero with the recorded error. `--continue` takes no
prompt or target, and refuses any `--run-id` that already has a transcript (the
runner truncates its transcript at start, so reusing an id would wipe that run —
the one being continued, or an ancestor in its chain).

A continued run's coverage manifest records `continued_from` (the seed source
transcript); its `user_prompt` is only the note, so `rupu coverage rerun` refuses
it and points at the run it continued.

### 2. The attempts ledger (`rupu-orchestrator`)

New per-run file `<run_dir>/attempts.jsonl`. The orchestrator appends one line right
before it dispatches any agent attempt — a linear step, a `for_each` unit (local or
placed) or a `parallel` sub-step:

```json
{"v":1,"step_id":"review","unit":{"index":41},"agent_run_id":"run_01…",
 "transcript_path":"/…/transcripts/run_01….jsonl","host":null,
 "continued_from":null,"started_at":"2026-10-01T20:53:44Z"}
```

`unit` is `{}` for a linear step, `{"index": n}` for a `for_each` unit,
`{"sub_id": "…"}` for a `parallel` sub-step. `host` is the placement host for a remote
unit. `continued_from` is the previous attempt's `agent_run_id` when this attempt is a
continuation. Appends are serialized (same pattern as `append_unit_checkpoint`).

### 3. Discovery on resume (`rupu_orchestrator::recovery`)

All logic lives in the orchestrator (the CLI stays thin); `resume_run` calls it and puts
the result on `ResumeState`.

1. Load attempts: from `attempts.jsonl`; if absent, derive them from `events.jsonl` —
   `unit_started` (step, index, transcript path, host), refined by `agent_started`
   (agent run id, transcript path, unit index), `step_working` for linear steps, and the
   sub-step `unit_started` events of `parallel` steps.
2. For every step with no recorded step result (the same "done" set resume uses
   today), take the **latest** attempt per unit key. A fallback-host retry of a remote
   unit is its own attempt, so the retry is the latest one.
3. That attempt is **settled** if a unit checkpoint carries its `agent_run_id`
   (`UnitCheckpoint.run_id`) — success or failure — and is left alone. Otherwise it is
   **interrupted**. Linear steps and `parallel` sub-steps have no per-unit checkpoint,
   so their latest attempt is always examined; its transcript decides
   (`Finished` → recovered, not re-run).
4. Each interrupted attempt gets a plan:
   - local: `prepare_continuation(transcript)` → `Continue` / `Recovered` / `Restart`
     (`Failed` and errors → `Restart` with the reason);
   - remote: §5.

```rust
pub enum AttemptPlan {
    Continue { transcript: PathBuf, from_agent_run_id: String, host: Option<String> },
    Recovered { output: String, agent_run_id: String, transcript: PathBuf },
    Restart { reason: String },
}
```

With `--restart-interrupted`, discovery is skipped entirely (today's behaviour). From
the CP, the resume request records the choice on the run's resume marker (next to
`resume_mode`), and the `cp serve` resume worker passes `--restart-interrupted` to the
`rupu workflow resume` subprocess it spawns.

### 4. Resume integration

- `ResumeState` gains `attempt_plans: BTreeMap<UnitKey, AttemptPlan>`.
- **Recovered:** written as the unit's checkpoint (a `for_each` unit) or folded into the
  step result (`parallel` sub-step, linear step) with no dispatch.
- **Continue:** dispatched through the normal path (same semaphore, placement, events,
  usage hook, codename); the agent opts get the continuation overrides from §1. The
  ledger line records `continued_from`.
- **Restart:** dispatched as today.
- Linear steps use the same mechanism; the existing `resume_seed` hook is replaced by
  the plan. **Graceful pause converges onto it:** pause stops writing
  `paused_seeds.json`; resume still reads that file when present, for runs paused by
  older binaries.
- `panel` steps never get a `Continue` plan; their interrupted members produce
  `Restart { reason: "panel steps can't be continued yet" }`.

### 5. Remote (`distribute:`) units

- A remote unit continues **on the host that ran it** (ledger `host`, or
  `unit_started.host` in the fallback), because that host holds the transcript.
- `UnitDispatch` gains `continue_from: Option<String>` — the previous attempt's agent
  run id on that host. Connectors launch `rupu run <agent> --run-id <new> --continue <old> …`.
  The coordinator still mints the new run id up front, so the mirror path is known
  before dispatch.
- **Contract gap — assigned to PR 3.** The coordinator reads a placed unit's
  `final_output` from the *new* run's record
  (`fleet_unit_dispatcher.rs`, the `final_output` read on the terminal `get_run`).
  PR 1's `rupu run --continue` does not produce one on `Finished`: it prints the
  recovered answer to stdout, returns, and writes no run record for `<new>`. So a
  remote unit that had in fact finished would surface as a missing run. PR 3 must
  close this: on `Finished` under `--run-id <new>`, the host has to produce a
  completed run record for `<new>` carrying the recovered output (or the coordinator
  must accept the recovered output another way, e.g. from the `--continue` process's
  stdout). No code change in PR 1.
- Hosts advertise a new **`agent.continue`** capability exactly like
  `agent.findings_profile` (HTTP `/api/host/info` `features`, tunnel
  `Hello.capabilities`, bucket `nodes/<worker>.json` markers). A connector refuses
  `continue_from` for a peer that didn't advertise it; the coordinator turns the refusal
  into `Restart { reason: "host <h> doesn't support agent.continue" }`.
- Host unreachable at dispatch → `Restart` with the reason, through normal placement
  (including the existing fallback-host retry).
- `Finished` detection for remote units happens on the host: `rupu run --continue` on a
  finished transcript returns the recovered output without a model call.

### 6. Events, CLI and CP

- New executor event, one per interrupted attempt, emitted when resume plans it:

  ```rust
  Event::AttemptResumed {
      run_id, step_id,
      unit_index: Option<usize>, sub_id: Option<String>,
      mode: AttemptResumeMode,          // Continued | Recovered | Restarted
      from_agent_run_id: Option<String>,
      reason: Option<String>,           // set for Restarted
  }
  ```

  It carries no state transition on its own: a continued or restarted attempt still
  emits `unit_started`, and a recovered one emits `unit_completed`.
- **CLI:** resume prints one summary line per affected step before dispatching, e.g.
  `review: 412 done · 18 continued · 2 recovered · 1 restarted (host b lacks agent.continue)`.
  The live view marks continued units.
- **CP run graph:** a small marker on continued / restarted unit squares; the tooltip
  shows the attempt count and, for a restart, the reason.
- **CP transcript view:** a continued attempt's header reads "Continued from attempt N"
  and links to the previous transcript by agent run id (remote ones through the existing
  mirror route), so a unit reads as one conversation across attempts.

### 7. Accounting and naming

- Usage-ledger tags are unchanged, so a continuation's tokens accrue to the same unit.
  Its first request re-sends the whole rebuilt conversation, usually without a warm
  prompt cache — one full context read, versus re-running every turn on a restart.
  Existing overflow handling compacts and retries if the conversation no longer fits.
- A unit keeps its codename across attempts. The final checkpoint records the last
  attempt's agent run id and transcript; `attempts.jsonl` keeps the history.

### 8. Edge cases

- **Original runner still alive:** the existing duplicate-execution guard refuses the
  resume until it exits; two runners never work on the same unit.
- **Interrupted repeatedly:** each continuation seeds by reference from the previous
  transcript; replay follows the chain (cap 1024).
- **Agent provider/model changed between attempts:** the continuation uses the step's
  current agent config; reasoning blocks from another provider aren't echoed (the
  existing provider-tag rule).
- **v1 transcripts:** continue without reasoning blocks.
- **Last outcome was a failure:** restart as today.
- **`for_each` list length changed since the run started:** the existing "re-run all
  units" rule applies; those units restart, flagged.
- **Fallback with old event shapes:** an attempt whose transcript path can't be
  determined restarts, flagged ("no transcript recorded for this attempt").

## Testing

- **`prepare_continuation`:** transcripts ending after the prompt only, after tool
  results, finished (`ok`), aborted by pause, aborted by shutdown signal, no
  `RunComplete`, `error`, missing file, hash mismatch, and a multi-link chain.
- **Role-alternation rule:** round-trip test — a seeded run whose seed ends in a user
  message plus a non-empty `user_message`; assert the provider saw the merged message
  and `reconstruct_messages` of the new transcript equals the runtime `messages`.
- **Discovery:** `attempts.jsonl` and the `events.jsonl` fallback produce the same plans
  for the same run; the latest attempt per unit wins; a checkpoint with the attempt's
  run id settles it.
- **End to end (mock providers):**
  - a fan-out with one unit blocked is aborted, then resumed — the provider receives the
    rebuilt conversation plus the note, the new transcript's `Seed` references the old
    one, finished units are not re-run, and the unit's checkpoint lands;
  - the same for a linear step and a `parallel` sub-step;
  - a finished-but-unrecorded attempt is recovered with zero provider calls;
  - graceful pause → resume continues from the transcript with no
    `paused_seeds.json` written; a legacy `paused_seeds.json` is still honoured.
- **Remote:** a fake dispatcher receives `continue_from`; a peer without
  `agent.continue` yields `AttemptResumed { mode: Restarted }` with the reason.
- **Opt-out:** `--restart-interrupted` reproduces today's behaviour exactly.
- **Web:** graph marker and the "Continued from attempt N" link render.

## Rollout

One spec, three PRs:

1. **Primitive** — `continuation` module, role-alternation rule in runtime + replay,
   `rupu run --continue`. Useful on its own for standalone runs.
2. **Local recovery** — attempts ledger, `recovery` discovery with the `events.jsonl`
   fallback, `ResumeState` plans, linear / `for_each` / `parallel` wiring, pause
   convergence, `--restart-interrupted`, `AttemptResumed` + CLI summary.
3. **Remote + display** — `UnitDispatch.continue_from`, `agent.continue` capability
   across connectors, CP graph marker and transcript chain link.
