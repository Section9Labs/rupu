# Remote netflow transport — design

- **Status:** approved in brainstorming, 2026-10-14.
- **Sibling spec:** `docs/superpowers/specs/2026-09-30-rupu-remote-findings-transport-design.md` (Plans A/B). That spec brought a remote unit's coverage stream and artifacts back to the coordinator. This one does the same for its netflow ledgers, on the same channels and under the same completeness rules.
- **Netflow background:** `docs/superpowers/specs/2026-08-04-rupu-netflow-per-run-ledger-design.md` (one ledger file per run id), `docs/superpowers/specs/2026-10-06-rupu-netflow-progressive-loading-design.md` (`NetflowIndex`).

## Problem

Every run writes its netflow ledger to `<netflow dir>/<run id>.jsonl` (`rupu_runtime::netflow::for_run`). The directory is `<project>/.rupu/netflow/` when that exists, otherwise `<RUPU_HOME>/netflow/`. A run on a remote host writes its ledger on that host. Today the CP sees it in only one case.

- **A wholly placed run** (`RunRecord.worker_id` names a remote host: SSH, tunnel or bucket mirrors it into the coordinator's `<global>/runs/<id>/`).
  - At run scope, `worker_host_flows` fetches its flows live through `HostConnector::run_netflow`. That returns processed rows, so it loses the `Dropped` and `Capture` lines.
  - At project and global scope there is no fetch. `RunMetaIndex::remote_workers` turns the run into an `incomplete` entry.
- **A placed step or unit** of a local workflow run (`host:`, `distribute:`, including the fallback-host retry). Its ledger is not read at any scope, and nothing reports that it is missing. The unit runs under the coordinator-minted `UnitDispatch.run_id`, which the item record keeps, so `run_and_unit_ids` already returns that id. But `resolve_ledger_paths` finds no file under it on the coordinator. Only the disclosure text (`ScopeDisclosure.tsx`, including the agentiflow note from #852) mentions the gap.
- **Sub-agents and process children of a remote run** write their own ledgers under their own ids (`sub_…`, `run_…`). The coordinator cannot discover those ids, because `run_and_unit_ids` walks `sub/` directories that exist only on the host.
- **On the host, a missing file doesn't mean "no flows".** The ledger writer opens its file lazily, so a run that made no network calls leaves no file. A missing file is equally consistent with an older host, or with a ledger that failed to open.

## Decisions (from brainstorming)

1. **Pull at unit end, never at read time.** Fetching at read time would put SSH calls behind page-polled project and global endpoints, which the CP's load-time rules forbid. It would also need every host online forever, and it cannot work for bucket. After a pull, every scope reads a local copy.
2. **The host tees its netflow lines into a run-scoped stream** (the coverage-stream pattern). Each transport adds one file to a mirror channel it already has, and a `begin` line separates "no flows" from "not collected". This happens only for runs a coordinator launched (see N1).
3. **A copy that is incomplete or missing is recorded, and the Network views read that record.** It becomes an `IncompleteSource` at every scope, and the unit also raises a `StepWarning`. A short copy never looks complete.
4. **Wholly placed runs are included.** Their ledgers land the same way at their end. After that, the run-scope live fetch is used only while the run is still in progress, and the project/global gap note stays only for a run that has not landed completely.

## N1. Host side: the netflow stream

- **File.** `$RUPU_HOME/runs/<root run id>/netflow.jsonl`, on the host.
- **Which runs write it.** A *root*: a `rupu run` or `rupu workflow run` started with `--run-id` and no `LaunchLink`. Every coordinator launch goes through W6's `RunArgv`, which always renders `--run-id`, so every placed unit and wholly placed run qualifies. A user's own `rupu run` from a terminal does not tee. `cp serve`'s local launches also pass `--run-id`, so they tee too. That costs some disk on the coordinator and is otherwise harmless.
- **Who else writes into a root's stream.** Every run beneath the root tees into the same file:
  - workflow steps and their units (`Origin::WorkflowStep`);
  - sub-agents (`Origin::SubAgent`);
  - process children with a `LaunchLink`. The child's stream is `<dir of LaunchLink usage root>/netflow.jsonl`, the same run directory its rollup usage rows go to, and it tees only when that file already has a `begin` line. So a child of a root that doesn't tee (an agentiflow, a terminal `rupu run`) doesn't tee either.

  The run assembler works out the stream path once per run, as `NetflowStream { path, ledger_id }`, next to its `UsageRoot`. It hands that to `rupu_runtime::netflow::for_run`, which passes it to the ledger writer.
- **Line format.**

  | `kind` | Meaning | Other fields |
  |---|---|---|
  | `begin` | First line, written by the root when it starts, even if the run makes no network calls. It proves the host streams. | `v: 1`, `run_id` |
  | `line` | One ledger line, exactly as written to the ledger | `ledger` (the ledger file's run id), `line` (the raw `LedgerLine` JSON) |

  The `ledger` tag is what carries a sub-agent's or child's ledger back under its own id.
- **One write path.** `NetflowWriterHandle`'s writer task writes and flushes the ledger line, then appends the stream line.
  - The stream append is one `write` under an `fs2` lock on `netflow.jsonl.lock`, because several writer tasks and processes share one stream.
  - A failed stream write is logged and never returned. Capture must never fail a run.
  - The final `Dropped` line goes through the same path.
- **The host's own ledger doesn't change.** The host's views read it exactly as before.
- **Run-directory housekeeping.** A stream-only run directory follows its transcript through `rupu transcript archive|delete|prune` and `rupu cleanup`, as Plan A's coverage-only directories do. `rupu netflow prune` leaves streams alone, because they belong to the run directory.

## N2. Getting the stream to the coordinator

Every transport reuses Plan A's mechanics and completeness rules for `coverage.jsonl`.

| Connector | How the stream arrives | When a copy is complete |
|---|---|---|
| local | Nothing to fetch: the unit's ledgers are already in the coordinator's own netflow directories (same `RUPU_HOME`). | Always. Nothing is landed and nothing is recorded. |
| SSH | `netflow.jsonl` joins the terminal ledger pull: `for f in usage.jsonl coverage.jsonl netflow.jsonl`, in the same ssh invocation, with the same `==> end <status> <==` trailers. It isn't tailed live, because nothing reads it before the run ends. | The host's `run.json` was terminal and the file's `cat` exited 0. The mirror copy then gets a `netflow.jsonl.complete` sidecar. |
| tunnel | The node tails `netflow.jsonl` with the run's other files and sends `Frame::Artifact { file: ArtifactFile::Netflow, … }`, but only to a CP whose `Welcome.capabilities` lists `mirror.netflow` (new; `cp_capabilities`). `NodeMirror::append` writes it to `<global>/runs/<run_id>/netflow.jsonl`. | Frames on one socket arrive before `RunFinished`. |
| bucket | The worker uploads `netflow.<seq:04>.jsonl` result objects inside the terminal hold, after the final coverage drain and before `finished`. `classify_key` maps `netflow*` to `ArtifactFile::Netflow`, ordered by numeric sequence like coverage. | The `finished` marker is written after the uploads. |
| HTTP | Not mirrored. The dispatcher GETs `/api/runs/:id/netflow/stream` (new; serves the remote's own `runs/<id>/netflow.jsonl`, behind the CP bearer token), but only from a peer whose remembered `/api/host/info` `features` lists `run.netflow_stream` (new, in `host_features()`). | The run is terminal on the peer. |

### Older peers

As in Plan A §A3, missing netflow must never look like "the unit made no calls".

- **SSH host without the tee:** the file is absent, so `cat` fails and the section doesn't arrive.
- **Tunnel:** a node that didn't see `mirror.netflow` never sends the frames. An older node never sends them.
- **Bucket:** an older CP's poller skips the unclassified keys. An older worker never uploads any.
- **HTTP:** the feature is checked first. An older peer would answer the unknown GET with its SPA `index.html`.
- **Every case** ends in a stream that is absent or has no `begin` line, which is recorded as `unavailable` (N3).

## N3. Landing on the coordinator

### The landing function

`rupu_netflow::land::land_stream(global_netflow_dir, stream: &[u8]) -> Result<LandReport, LandError>` is pure apart from its writes.

- **No `begin` line:** returns `LandReport { begin_seen: false, .. }` and writes nothing.
- **Splitting:** groups `line` entries by `ledger` and writes each group to `<global>/netflow/<ledger>.jsonl`.
  - Each file goes through a temp file named `.<ledger>.landing-<pid>-<n>.tmp` (not `*.jsonl`, so no ledger reader picks it up half-written), then a rename. A fresh copy replaces whatever an earlier landing wrote.
  - Ledger ids are validated as file-name safe (`run_*` / `sub_*` / ULID charset). Any other id is skipped and counted as malformed, so a stream can never write outside the directory.
  - The directory is created through `ensure_netflow_dir`, which makes it self-ignoring.
- **Malformed lines** are skipped and counted.
- **Returns** the ledger ids written, plus the counts.

`NetflowIndex` already re-reads a ledger whose inode changed (`netflow_index.rs`). A test pins that a landed replacement is re-read.

### The landing record

`<global>/runs/<stream run id>/netflow.landing.json`, a single JSON object written atomically (temp file, then rename) *after* the ledger files:

```json
{"v":1,"host":"host_ssh","status":"complete|partial|unavailable","reason":null,"ledgers":["run_…","sub_…"],"at":"…"}
```

- **Never downgrade.** An existing `complete` record is never replaced by a `partial` or `unavailable` one. Any other record is replaced by the newer one, so a later complete pull, such as the next terminal pull after a host-side resume, wins. A crash between the ledger files and the record leaves files with no record, which reads as not collected. The next landing repairs it.
- **One lander per stream, chosen by transport.**
  - **SSH:** the tail pump's terminal pull lands the mirror copy as soon as it has written it. `complete` follows the `.complete` rule; otherwise the status is `partial`, with a reason. A section that didn't arrive is `unavailable`.
  - **Tunnel and bucket:** `NodeMirror::finish` (both transports end there) lands `<global>/runs/<id>/netflow.jsonl`. It's `complete` when the stream has a `begin` line, otherwise `unavailable`.
  - **HTTP:** the fleet dispatcher, after the unit's outcome, through the new required `HostConnector::land_run_netflow(run_id) -> NetflowLanding`. HTTP fetches and lands. A terminal run is `complete`; a read after a poll error or at the wall timeout is `partial`. SSH, tunnel and bucket answer `ByMirror` (after `await_run_mirror`), and local answers `NotNeeded`.

  These landers cover placed units and wholly placed runs alike, because both are mirrored runs on those transports.

### The unit roster

The runner appends `{"unit_run_id":"…","host":"…","step_id":"…","index":n}` to `<workflow run dir>/netflow.units.jsonl` (fs2-locked append) when it dispatches each placed unit attempt, including the fallback-host retry, before the launch.

- **Retries:** a failed first attempt's id would otherwise be lost, because the item record keeps only the final attempt's id.
- **Pending units:** a unit whose coordinator died before the end has a roster line but no landing record, and reads as "not collected".
- **Local-host units** get no roster line.

### Warnings

After each remote unit's outcome, the runner reads the unit's landing record and emits `Event::StepWarning { run_id, step_id, index, message }` when:

- the record is missing (named host, "netflow not collected"),
- it is `partial` (with its reason), or
- it is `unavailable` (older host, or a stream that failed or was lost).

A warning never fails the unit.

## N4. Read side

All three CP netflow scopes, plus the agentiflow scope (which walks the same ids for the workflows it launched), use one extended id walk.

- **`run_and_unit_ids` becomes `NetflowIds { ids, gaps }`** built by `netflow_ids(store, global_dir, run_id)`. It's `pub` so `rupu netflow prune` keeps protecting every ledger a run owns.
  1. Collect today's ids: the run, every step and item id, and every sub-run.
  2. Add every `unit_run_id` in `<run dir>/netflow.units.jsonl`.
  3. For each collected id, read `<global>/runs/<id>/netflow.landing.json`, if any.
     - `complete`: add its `ledgers`.
     - `partial`: add its `ledgers` and push an `IncompleteSource { host_id, reason }`.
     - `unavailable`: push a gap only.
     - No record, for a roster unit or a wholly placed run: push "not collected yet".
  4. Dedupe the ids.
- **Run scope.**
  - The gaps join `incomplete`.
  - A wholly placed run whose own record is `complete` is read entirely from local copies, and `worker_host_flows` is skipped.
  - Otherwise, for a run in progress or one with no complete landing, today's live fetch stays. It still marks a failed fetch incomplete.
- **Project and global scope.**
  - `RunMetaIndex::insert_run` uses `netflow_ids`, so landed ledgers are attributed to their run and workflow. The ledgers live in `<global>/netflow/`, which global scope's directory union and project scope's global-fallback id set already read.
  - The gaps are collected into the index.
  - `remote_workers` / `scope_gap_from_workers` keep only wholly placed runs without a complete landing.
  - The `RunMetaCache` memo key must change when a landing record or roster changes. The spec requires that a new landing invalidates it; the plan picks the stamp, the record's mtime folded into the run's.
- **Agentiflow scope.** `agentiflow_netflow_scope` builds its ids with the same walk (today `run_and_unit_ids` over the flow's runs), so its landed ledgers are read and its gaps join its response's `incomplete`.
- **The run-scope merge is unchanged.** Landed ledgers are ordinary ledgers, so on a `FlowId` collision the finalized ledger record still wins over a mirrored transcript's snapshot.

## N5. Disclosure text

`ScopeDisclosure.tsx` stops saying remote-placed steps are missing.

- **`AGENTIFLOW_SCOPE_NOTE`:** the sentence about a workflow step on a remote host is replaced with "a step that runs on a remote host appears once it finishes; one whose flows could not be collected is listed as incomplete".
- **Run and project/global text** gets the same sentence wherever it describes remote placement.
- **`SUBPROCESS_CAPTURE_LIMITS`:** the clause "a remote host's bash flows stay on that host" becomes "a remote host's bash flows arrive when its unit finishes".
- **The `incomplete` list** already renders each `IncompleteSource`; its reasons say "netflow not collected from host X: …".

## Error handling summary

| Situation | Result |
|---|---|
| Host can't stream (older version, missing capability) | Record `unavailable`; a `StepWarning` for a unit; `incomplete` at every scope. |
| Copy not confirmed whole | Record `partial` with its ledgers landed; a warning; `incomplete`. |
| Coordinator stopped before collecting | A roster line with no record: "not collected yet" in `incomplete`. |
| Malformed stream line or unsafe ledger id | Skipped and counted in the record's reason and the warning. |
| Landing I/O error | Logged; the record isn't written, so it reads as not collected; a warning. |
| Stream write fails on the host | Logged on the host; the run continues; the copy may be short, which shows as `partial` or unavailable. |

## Testing

- **`rupu-netflow`:**
  - The writer tees every ledger line, including the final `Dropped`, into a configured stream with its `ledger` tag. Two writers sharing one stream interleave whole lines.
  - `land_stream`:
    - splits by ledger;
    - refuses a stream with no `begin` line;
    - skips unsafe ids and malformed lines;
    - re-landing replaces;
    - leaves no `*.jsonl` temp file behind.
  - The landing record's never-downgrade rule.
- **`rupu-runtime`:** the assembler's `NetflowStream` per origin (root with `--run-id`, workflow step, sub-agent, `LaunchLink` child with and without a begun root stream, terminal `rupu run`), as a row in the `origin_matrix` snapshot. The root writes `begin` even when it records no flows.
- **`rupu-cp`:**
  - SSH: the terminal pull includes `netflow.jsonl`, writes the `.complete` sidecar under the coverage rules, and lands complete, partial and missing sections.
  - Tunnel: `mirror.netflow` is advertised, the node sends frames only when advertised, and `NodeMirror::finish` lands.
  - Bucket: `classify_key("netflow.0001.jsonl")`; the worker uploads inside the terminal hold.
  - HTTP: `/api/runs/:id/netflow/stream` serves the file, and `land_run_netflow` refuses a peer without `run.netflow_stream`.
  - `land_run_netflow` on all five connectors.
  - `NetflowIndex` re-reads a replaced ledger.
  - `netflow_ids`: roster, landing record, gaps.
  - Run, project and global responses: landed flows are attributed, gaps are listed, and a wholly placed run with a complete landing makes no live fetch.
- **`rupu-orchestrator`:** a roster line per dispatched attempt (retry included), and a `StepWarning` for each missing, partial or unavailable record.
- **End to end (`rupu-cli`):** a `host:` workflow through a fake connector whose stream carries the unit's ledger plus a sub-agent ledger. Both show in the run's netflow response, and the project and global responses attribute them to the run. A second unit with a `partial` copy is listed incomplete.
- **Web:** `ScopeDisclosure` text tests updated.
- **Real data:** a local workflow with a `host:` step on an SSH host. Every probe is batched into one ssh invocation.

## Out of scope

- **Merging netflow live, mid-unit.** It lands when the unit ends; a running unit's flows show only through the existing transcript merge.
- **HTTP wholly placed runs.** They are the peer's runs, read with `?host=`, and never enter the coordinator's store.
- **A coordinator-side abort's own gap.** A unit that was never collected reads as "not collected yet" and is not retried.
- **Pruning landed ledgers on the coordinator.** `rupu netflow prune` treats them like any other ledger.
- **macOS app.**

## As built

Where the build differs from the text above (the body is left as designed):

- **The ledger writer opens its file eagerly.** The Problem section says a run with no network calls leaves no ledger file; the writer actually creates an empty one at start. The `begin` line is still what separates "no flows" from "not collected", because an older host writes no stream at all.
- **Landing lives in `NodeMirror`, triggered by the run turning terminal.** The mirror lands its copy on the terminal `run.json` (before the status update, so a dispatcher polling `get_run` never sees a terminal run without a landing record) and in `finish`. That covers SSH, tunnel and bucket with one hook. A bucket poller that re-mirrors `run.json` every tick costs one record read once the landing is `complete`. An SSH copy pulled before terminal carries a `netflow.jsonl.partial` marker holding its reason. Tunnel and bucket copies are complete by transport order, so they carry no marker.
- **Landed lines are byte-exact.** The stream's `line` is carried as raw JSON (`serde_json` `raw_value`), so a landed ledger holds exactly the host's bytes, including a line kind the coordinator doesn't know.
- **`rupu run` finishes its netflow before its record turns terminal.** Origin/main's #863 added `RunNetflow::finish`, which flushes the writer and backfills the ledger from the transcript. `rupu run` now runs it before writing the terminal `run.json`, so a coordinator never pulls a stream still being written. The backfill tees its lines into the stream too.
- **The id walk is built on #860's `run_tree`.** `netflow_ids` and `netflow_tree` add the roster and the landing records on top of the launched-children walk. Only the run itself and roster units are checked for a landing record, never every step id, to keep file opens off the project/global read path.
- **The roster is the workflow run's own.** A workflow launched by a run (W7) keeps its placed units in its own `netflow.units.jsonl`; the launching run's walk reaches that workflow's units through `run_tree`, but not its roster. A missing landing for such a unit is therefore not listed as "not collected yet" at the launching run's scope (it is at the launched workflow's own scope).
- **HTTP landings name the registry host id.** `HttpHostConnector::with_host_id`, set by the registry, is what the landing record's `host` carries.
- **Placed-unit warnings carry the host.** The dispatcher now prefixes every warning `launch_and_wait` returns with `host <id>: `, not only the skill-shipping ones.
