# Remote findings transport and artifact pull — design

- **Status:** approved in brainstorming, 2026-09-30.
- **Parent spec:** `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md` (§Artifacts, §CP API). This spec builds the rule that spec states for placed units: "artifacts are recorded as `external` with `host` set … pulled into the coordinator's store on first view … verified against the recorded sha256".
- **Stacked on:** Section9Labs/rupu#675 (`findings_profile` reaches remote units), which is stacked on `claude/finding-reports-contract`.

## Problem

A remote unit (a `host:` step, or a `distribute:` fan-out unit) runs `rupu run` on another machine. That run writes its coverage (runs, file touches, concern assertions, findings) into `<workspace>/.rupu/coverage/<target_id>/` on the host, and copies finding artifacts into the host's `<RUPU_HOME>/findings/artifacts/`. Today:

- **Findings reach the coordinator only by accident.** They arrive as a side effect of `workspace: sync`, which only local, SSH and HTTP support, and only on success: a failed unit's delta is discarded.
- **They arrive under the wrong key.** `target_id` hashes the host's scratch workspace path, so each synced unit lands in a new target.
- **Tunnel and bucket units' coverage never arrives.**
- **Artifact blobs never reach the coordinator.** Findings carry `stored: copied` with `host: None`, pointing at a store the coordinator doesn't have.
- **Nothing serves artifacts.** There is no `GET /api/findings/:id/artifacts/:sha256`.

So an artifact pull built on its own would be reachable only for synced, successful SSH/HTTP units, and dead code for tunnel and bucket.

## Decisions (from brainstorming)

1. **Build the full chain, as two plans.**
   - **Plan A:** coverage transport, plus artifacts recorded as `external` + `host`.
   - **Plan B:** the pull, plus the serving endpoint.
   - Web UI rendering of artifacts stays in the parent spec's Plan 2.
2. **Carry the unit's whole coverage record, not only findings.** That's every ledger line attributed to the unit's run, so `.rupu/coverage/` can leave the sync delta without regressing the coverage harness.
3. **Transport the stream on the existing mirror channels.** The host copies each coverage write into a run-scoped stream. SSH, tunnel and bucket already ship `runs/<run_id>/*.jsonl` to the coordinator, and HTTP fetches the stream at unit end. The alternative, a connector pull at unit end, needed new request/response plumbing on tunnel and bucket, and lost coverage when a host dropped offline right after a run.
4. **Bucket hosts upload referenced blobs at run end.** The worker may never poll again, so on-demand requests aren't used; the coordinator's pull is a plain bucket download.

## Plan A — the coverage stream

### A1. Host side: a run-scoped stream

- **File:** `$RUPU_HOME/runs/<run_id>/coverage.jsonl`. It's written by every `rupu run` agent run, the command every connector launches, whether a coordinator launched it or someone ran it by hand. Keeping the rule unconditional for `rupu run` means the local connector reads it the same way the remote transports do.
  - In-process workflow steps and sessions don't write one; they already write into the coordinator's workspace ledger.
  - `rupu run` asks for the stream through a new `AgentRunOpts.coverage_stream: Option<PathBuf>`. `DefaultStepFactory` and sessions leave it `None`.
- **Line format:**

  | `ledger` | Meaning | Other fields |
  |---|---|---|
  | `begin` | First line, written at run start even if the run records no coverage. It proves the host streams. | `v: 1`, `run_id` |
  | `runs` | A `RunManifest` | `scope_name`, `record` |
  | `files` | A file-touch event | `scope_name`, `record` |
  | `concerns` | A concern assertion | `scope_name`, `record` |
  | `findings` | A `FindingRecord` | `scope_name`, `record` |
  | `catalog` | The catalog snapshot the agent runner writes at run start | `scope_name`, `record` |

  Lines carry `scope_name`, never `target_id`: `target_id` hashes the host's workspace path, so the coordinator recomputes it for its own workspace.
- **One write path.** The four ledger write sites (`ledger/manifest.rs`, `ledger/writer.rs`, `tools/coverage_mark.rs`, `tools/report_finding.rs`), plus the catalog snapshot write in `rupu-agent`'s runner, go through one `ledger::append` helper in `rupu-coverage`.
  - The helper writes the ledger line and then, when `CoveragePaths.run_stream: Option<RunStream { path, scope_name }>` is set, the stream line. Each is written with a single `write` and flushed.
  - The async `files.jsonl` writer keeps a second append handle for the stream.
  - `rupu-agent`'s runner builds `run_stream` from `AgentRunOpts.coverage_stream` and the `scope_name` it already computes.
  - The begin line is written when `rupu run` starts its run (`rupu-cli`), before the agent loop.
- **The host's own workspace ledger doesn't change.**

### A2. Getting the stream to the coordinator

- **New required `HostConnector` method:** `unit_coverage(run_id) -> Result<Vec<u8>, HostConnectorError>`. It has no default implementation, so every connector and test double must answer it.

  | Connector | How it gets the stream |
  |---|---|
  | local | Reads `<global>/runs/<run_id>/coverage.jsonl` directly (the same RUPU_HOME). |
  | SSH | `coverage.jsonl` joins the tail-pump file list and the final batched pull (with `.complete` semantics). After `await_run_mirror(run_id)`, reads the mirror copy. |
  | tunnel | The node tails `coverage.jsonl` with the run's other files and sends `Frame::Artifact { file: ArtifactFile::Coverage, … }`. Frames on one socket arrive before `RunFinished`, so reading the mirror after the unit is terminal is ordered. |
  | bucket | The worker uploads `coverage.<seq>.jsonl` result objects before `finished`. The CP poller classifies them into the mirror. |
  | HTTP | Runs aren't mirrored, so it GETs `/api/runs/:id/coverage` from the remote CP (new; serves the remote's local file). Gated on the `run.coverage_stream` feature in the remote's `/api/host/info` `features`. |

- **`NodeMirror::append`** gains `ArtifactFile::Coverage`, appended to `<global>/runs/<run_id>/coverage.jsonl` in the coordinator's mirror.

### A3. Older peers

Missing coverage must never look like "the unit recorded nothing".

- **Tunnel.** A new node must not send an unknown `ArtifactFile` variant to an older CP, whose frame parse would fail.
  - `Frame::Welcome` gains `capabilities: Vec<String>` (`#[serde(default)]`).
  - The CP advertises `mirror.coverage`, and the node sends coverage frames only if it saw that.
  - An older node never sends them, so the stream has no begin line (see below).
- **Bucket.** An older CP's poller skips result keys it doesn't classify (`classify_key` → `None`), so a new worker's `coverage.*.jsonl` objects are harmless there.
- **HTTP.** An older remote CP answers any unknown `/api/...` GET with the SPA `index.html` and a 200, so a 404 can't be the signal. The connector checks the `run.coverage_stream` feature first, the same way `agent.findings_profile` is checked in #675.
  - Going forward, the CP's static fallback returns a JSON 404 for any unmatched `/api/*` path, so future endpoints fail cleanly on this version and later.
- **SSH.** An older host never writes the file.
- **In every case,** a stream that is absent, or has no `begin` line, means the unit's coverage was not collected.

### A4. Collecting and merging on the coordinator

- **The dispatcher collects on every outcome after launch:** success, agent failure, poll error after the run was observed, poll timeout, and cancel.
  - `UnitDispatcher::dispatch_unit` returns `Result<UnitOutcome, UnitFailure>`. `UnitFailure { error: RunError, coverage: UnitCoverage }` implements `From<RunError>`, with `coverage: NotLaunched`.
  - Failures that trigger the fan-out's fallback-host retry keep their semantics, and still carry what the failed attempt recorded.
  - `UnitOutcome` gains `coverage: UnitCoverage`.
  - `enum UnitCoverage { Stream(Vec<u8>), Unavailable(String), NotLaunched }`. `Unavailable` covers a connector error, a refused feature, or a missing begin line.
  - A launch that never happened carries `NotLaunched`.
- **The merge is a pure function in `rupu-coverage`:** `ledger::ingest::ingest_unit_stream(coordinator_workspace, source: IngestSource { host: Option<String> }, stream) -> Result<IngestReport, IngestError>`.
  - **Begin line:** required. Without it, the result is `IngestReport { begin_seen: false, .. }` and nothing is written.
  - **Re-keying:** each line goes to `target_id(coordinator_workspace, scope_name)`, through `ledger::append` with no `run_stream`.
  - **Duplicates:** findings are deduplicated by `id`; `runs` / `files` / `concerns` lines by exact equality of the serialized `record`. Existing keys are loaded once per target. Re-ingesting the same stream appends nothing, and a local host whose run already wrote into the coordinator's workspace produces no duplicates.
  - **Artifacts:** when `source.host` is `Some` (any non-local host), every finding's `report.artifacts[]` becomes `stored: external`, `host: Some(<registry host id>)`, keeping `path`, `sha256`, `size` and `kind`. A local host keeps `copied`, since it shares the store.
  - **Catalog:** a `catalog` line overwrites the target's `catalog.yaml`, the same "latest run wins" rule the agent runner applies.
  - **Malformed lines** are skipped and counted.
- **The runner calls it for every remote unit, whatever the outcome.**
  - This covers the placed-step path and both fan-out paths, including the fallback-host retry's attempt.
  - Merged coverage appears in `GET /api/findings` when the unit ends.
  - **Warnings:** `Unavailable`, `begin_seen: false`, or an `IngestError` produce a new `Event::StepWarning { run_id, step_id, index: Option<usize>, message }`. The message names the host, the reason, and the counts.
  - **A warning never fails the unit.**
- **The sync delta no longer carries coverage.** `.rupu/coverage/` is excluded when the delta is collected and ignored when it's applied, so synced units stop producing scratch-path targets. Coverage arrives only through the stream.

## Plan B — pulling and serving artifacts

### B1. `HostConnector::pull_finding_artifact(sha256, dest)`

This is a required method with no default. It streams the host-store blob `<aa>/<sha256>` into `dest`, a temp file the caller created inside the coordinator's store directory. The caller verifies the result.

| Connector | How it delivers the blob |
|---|---|
| local | Copies from the shared store (normally never reached, because local artifacts stay `copied`), or returns `NotFound`. |
| SSH | A hidden `rupu __findings artifact <sha256>` on the host streams the blob from its store to stdout, and exits nonzero with a message if it's absent. A new `RemoteExec::run_to_file(cmd, dest, max_bytes)` streams stdout straight to disk and aborts once it passes `max_bytes` (the recorded size). That's one SSH invocation per pull, never a burst. |
| HTTP | Streams `GET /api/findings/artifacts/:sha256` (new on the remote CP: the blob by hash from its own store, bearer-authenticated). Gated on the `findings.artifact_blob` feature. |
| tunnel | Sends `Frame::ArtifactPull { req, sha256 }`. The node answers with `Frame::ArtifactChunk { req, seq, data_b64 }` frames (1 MiB decoded each) and then `Frame::ArtifactPullDone { req, error: Option<String> }`. `NodeConn` routes chunks to per-request channels. Requires the node to be online and to have advertised the `findings.artifact_pull` capability in `Hello`. A per-request idle timeout applies. |
| bucket | Streams `artifacts/<sha256>` from the bucket. At run end, before `finished`, the worker uploads every blob its run's findings reference (from its own coverage stream: `stored: copied` artifacts) to `artifacts/<sha256>`, skipping ones that already exist. |

### B2. `GET /api/findings/:id/artifacts/:sha256` on the coordinator

1. **Check the request.** `sha256` must be exactly 64 lowercase hex characters (400 otherwise), before it touches any path. The finding is located by `id` across the coordinator's workspaces with the same walk `GET /api/findings` uses. It's a 404 if the finding is unknown, or if `sha256` is not in its `report.artifacts`. This endpoint never serves arbitrary store blobs by hash.
2. **Store hit.** If `<global>/findings/artifacts/<aa>/<sha256>` exists, serve it.
3. **External artifact with a `host`.** Pull it:
   - **One pull per sha:** a process-wide map of in-flight pulls, so concurrent requests await the same one.
   - **Verify:** `pull_finding_artifact` writes to a temp file in the store, then both size and sha256 are checked.
   - **Store:** rename into the content address. An identical blob already there is fine, since it's content-addressed.
   - **Then serve it.** On failure the temp file is removed.
4. **External artifact with no `host`** (a local over-cap file). Serve `<workspace>/<path>` while it exists and still hashes to `sha256`, streaming the hash.
5. **Anything else:** 404 `{"unavailable": "<reason>"}`. Reasons include host unreachable, node offline, not in the host's store, remote too old for the blob endpoint, hash or size mismatch, and workspace file gone or changed.

- **Response headers:** artifacts of a text kind are served `text/plain; charset=utf-8` with `X-Content-Type-Options: nosniff`, never as HTML. Everything else gets `Content-Disposition: attachment; filename="<basename of path>"`.
- **Bodies stream from disk.**

## Error handling summary

| Situation | Result |
|---|---|
| Host can't stream (older version) | A step warning; the unit's outcome is unchanged. |
| Stream has malformed lines | Skipped and counted in the warning. |
| Ingest I/O error | A step warning; the unit's outcome is unchanged. |
| Artifact pull fails | 404 with `unavailable` and a reason; nothing stored. |
| Pulled bytes don't match | Discarded; 404 `unavailable: hash mismatch`. |

## Testing

**Plan A**

- **`rupu-coverage`:**
  - `ledger::append` writes the ledger line and, when configured, the stream line.
  - `ingest_unit_stream`:
    - re-keys by scope;
    - deduplicates findings by id and the other ledgers by record equality;
    - rewrites artifacts for a remote host and leaves them for a local one;
    - overwrites the catalog;
    - skips and counts malformed lines;
    - refuses a stream without a begin line;
    - is a no-op on re-ingest.
- **`rupu-agent`:** a `MockProvider` run with `run_stream` writes a stream line for each ledger it touches. `rupu run` writes the begin line even when no coverage is recorded.
- **`rupu-cp`:**
  - `NodeMirror` appends `Coverage` lines.
  - The SSH tail list and final pull include `coverage.jsonl`.
  - Tunnel: `Welcome` capabilities, and the node sends coverage frames only when advertised.
  - Bucket: `classify_key("coverage.0001.jsonl")`.
  - HTTP: `/api/runs/:id/coverage` serves the file; an unmatched `/api/*` path is a JSON 404.
  - `unit_coverage` on all five connectors, including each connector's older-peer path.
- **`rupu-cli`:**
  - The dispatcher attaches coverage on success, agent failure, post-observation poll error and timeout; a failed launch carries `NotLaunched`.
  - The node executor and the bucket worker ship the stream.
- **`rupu-orchestrator`:**
  - The runner merges on every remote-unit outcome and emits `StepWarning` on `Unavailable`.
  - The sync delta excludes `.rupu/coverage/`.
  - **End to end:** a `host:` workflow through a fake connector whose unit's stream has a finding with an artifact. The finding lands under the coordinator's target with `stored: external`, `host` set. A second fan-out through a failing unit still merges its coverage.

**Plan B**

- `pull_finding_artifact` per connector:
  - SSH: a fake `run_to_file`, plus the size abort.
  - HTTP: against a real remote CP router, plus a feature-gated refusal.
  - Tunnel: frames against a fake node, plus offline and missing-capability.
  - Bucket: in memory.
  - Local.
- The worker uploads referenced blobs at run end and skips existing ones.
- `rupu __findings artifact`: streams a present blob; exits nonzero on an absent one or a malformed sha.
- The endpoint:
  - a store hit;
  - pull, verify and store;
  - a hash mismatch stores nothing and returns 404 `unavailable`;
  - a sha that isn't on the finding is 404;
  - a malformed sha is 400;
  - concurrent requests share one pull;
  - headers per kind;
  - a local external workspace file is served, and 404 once it has changed.
- **End to end:** Plan A's end-to-end test continues with `GET` on the artifact: the bytes come back, and the blob is now in the coordinator store.

## Plan breakdown

- **Plan A:** A1–A4. After it, remote coverage and findings reach the coordinator on every transport, with artifacts recorded as `external` + `host`.
- **Plan B:** B1–B2. After it, those artifacts are viewable through the CP API.

## Out of scope

- Web UI rendering of `report.artifacts`. The parent spec's Plan 2 consumes B2.
- Merging coverage live, mid-unit. It merges when the unit ends.
- Artifact garbage collection.
- macOS.
- Detecting an older bucket worker that polls the same bucket as an upgraded one. That gap is inherent to a handshake-free dead drop and is documented in #675.

## Changes to the parent spec

§Artifacts's placed-unit rule is implemented as written. §CP API's `GET /api/findings/:id/artifacts/:sha256` is delivered by Plan B, ahead of the parent's Plan 2, which renders it. It adds one reason for the `unavailable` body: the local external workspace file has changed.

## As built (Plan A)

Where Plan A differs from the text above (the body is left as designed):

- **The stream path rides on `ToolContext.coverage_stream`**, not a new `AgentRunOpts` field. `AgentRunOpts` has about 58 struct literals and no `Default`, and `ToolContext` already carries run-scoped coverage state. `rupu run` sets it; `DefaultStepFactory` and sessions leave it `None`.
- **The write helper is `ledger::stream::append_record`**, not `ledger::append`. The catalog snapshot goes through `stream_catalog`. The async file-touch writer writes and flushes its ledger line and then mirrors it with `stream_json`, rather than keeping a second append handle for the stream. A stream write that fails is logged and never returned, so it cannot fail the run.
- **The workspace-sync delta still carries `.rupu/coverage/`; the coordinator strips it per unit.** Collection runs on the host, which cannot know whether the coordinator ingests streams: an older coordinator never does, and the delta is then its only copy of the unit's coverage. So collection is unchanged, and the runner — which merges each unit's stream before any delta applies — drops `.rupu/coverage/` from a unit's delta only when that unit's complete stream (`UnitCoverage::Stream`, not `Partial`) arrived with its begin line and merged every line, none malformed (an unknown line kind from a newer host counts) (`UnitDispatcher::strip_delta_coverage`, a required method, which the fleet dispatcher implements with `rupu_workspace::Delta::without_coverage`: the path lists, the tar entries, and the git file patches, re-printed from the parsed patch). With no stream (an older host, or a stream that failed), a `Partial` one, or one with malformed lines, the delta's coverage applies as before Plan A, under a scratch-path target: a possibly short stream must not displace a copy that may hold the findings it lacks, so a duplicate is preferred to a loss. The strip matches only `.rupu/coverage/` at the workspace root, deletions included; a strip that fails applies the delta whole.
- **A coverage read says whether it is complete.** `HostConnector::unit_coverage` returns `CoverageRead { bytes, complete }`. Local, tunnel, bucket and HTTP reads are complete once the run is terminal: the run's own file, frames that precede `RunFinished`, a finished marker written after the uploads, the remote's own file. The SSH tail pump's terminal pull fetches `coverage.jsonl` in the same ssh round trip as `usage.jsonl`, before the transcript catch-up, and marks the replaced mirror copy with a `coverage.jsonl.complete` sidecar only when the host's `run.json` was terminal. An SSH read without the sidecar is not complete.
- **`UnitCoverage` has a fourth variant, `Partial { bytes, reason }`,** for a stream the coordinator cannot confirm is whole: a read after a poll error on an observed run, a read at the wall timeout, or a terminal read the transport could not confirm complete. The runner merges it like `Stream` and emits a `StepWarning` that names the host and the reason, and says findings recorded after the stream was collected may be missing. A warning mentions the unit's workspace-sync delta (its own copy kept, or the only copy) only when the unit returned one: a failed unit, including every poll-error or timeout snapshot, returns none. The `Unavailable` reason no longer repeats the host. A missing begin line is reported as an older host *or* a stream that failed to start or was lost in transport.
- **The bucket poller orders result objects by kind and numeric sequence.** Their keys are `<kind>.<seq:04>.jsonl`, which sort wrong past 9,999 chunks; the key format stays, because older workers write it.
- **A standalone run's stream-only run dir follows its transcript.** A `rupu run` that never wrote `run.json` (a failed pre-flight, a killed run) leaves `runs/<run_id>/` holding only its stream. `rupu transcript archive` moves that dir to `runs-archive/<run_id>`; `transcript delete`, `transcript prune` and `rupu cleanup` remove it. A run dir with `run.json` is left to `RunStore`.
- **`dispatch_agent` children on a host stream into the unit's file.** `CliAgentDispatcher` carries the stream path and hands it to each child's `ToolContext`.
- **The stream has a seventh line kind, `assets`**: the engagement asset ledger (#716), merged after this spec was written. `ingest_unit_stream` de-duplicates asset lines by how many times each line occurs, since the asset store folds last-line-wins and a state may legitimately recur.
- **`Event::StepWarning` is shown** on the run graph and event feed in the CP run view, in the Situation Room, in the CLI live view, and in the completion summary and `rupu workflow show-run`.
- **`Event::StepWarning` is not in the macOS fixtures.** The app is deprecated and decodes unknown event tags as `.unknown`.

### Known limits (Plan A)

- **A coordinator-side abort drops the unit's coverage without a warning.** A whole-run cancel, a `wait: any` loser, or the coordinator process exiting drops the unit's dispatch before it collects the stream.
- **A workflow's remote units and its in-process steps record under different scopes.** The host's `rupu run` uses the agent's name as `scope_name`, and an in-process step uses the workflow's, so one workflow's findings can land in two targets.
- **`mark_external` also rewrites artifacts that were already `external`.** A host records a workspace file over the copy cap as `external` with no `host`; the merge sets `host` on it like any other artifact. Plan B therefore cannot tell a blob in the host's store from a file in the host's workspace by the record alone.
- **A standalone remote agent run launched from the CP outside a workflow is mirrored but never merged.** Only the workflow runner ingests a unit's stream.
