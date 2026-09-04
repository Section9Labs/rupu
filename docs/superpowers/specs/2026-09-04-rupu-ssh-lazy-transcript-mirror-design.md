# SSH lazy transcript mirror — design

**Date:** 2026-09-04
**Status:** approved in brainstorm, awaiting spec review
**Scope:** SSH hosts only. Designed so tunnel (`rupu node`) and bucket hosts can
follow without touching anything above the connector.

## 1. Problem

Transcripts for work that runs on an SSH host do not show in rupu.app or the
web CP. The failure is not one line; it is five distinct holes, and the fix
has been deferred several times (see the memory `project_remote_transcript_gap`
and PR #646, which closed one of them).

State on `main` at ef858ee3:

| # | Scenario | What happens today |
|---|----------|--------------------|
| 1 | Whole workflow launched on an SSH host (`launch_run`) | The tail pump (`crates/rupu-cp/src/host/ssh.rs::spawn_tail_pump`) mirrors `events`/`step_results`/`unit_checkpoints` but never the per-step transcripts under the remote working dir's `.rupu/transcripts/`. Mirrored step rows carry remote paths; `SshHostConnector::get_transcript` reads that path on the **local** disk (`connector.rs::read_transcript_file`), finds nothing, returns `{events: []}`. Every step is empty, forever. |
| 2 | Local workflow with a `host:` placed step or `distribute:` fan-out unit | `fleet_unit_dispatcher::dispatch_unit` discards the remote run id; `UnitOutcome` has no transcript field. The runner records `transcript_dir/<step_run_id>.jsonl` — a coordinator-local path that is never written. PR #646 mirrors the remote agent transcript to `<global>/transcripts/<remote_run_id>.jsonl`, but only the *child* run knows that path. |
| 3 | Standalone agent run launched on an SSH host | The mirror fills live, but the `"agent"` step row that points at it is synthesized only in `NodeMirror::finish`. While running, the graph has no transcript path to open. |
| 4 | Any remote transcript, live | `GET /api/transcript/stream` ignores `?host=` (validates the path locally only). `RunDetailStore` (macOS) hard-disables the tail factory and the run event stream for remote runs. Even a mirrored file that is growing live only gets REST snapshots. |
| 5 | Usage / cost for SSH workflow runs | `usage::run_transcript_paths` reads step transcripts from local disk; remote paths resolve to nothing, so token and cost columns are blank. |

Additionally, the pump `cat`s the remote `run.json` on every poll tick
(`pump_finalize_if_terminal`) but only mirrors it once the run is terminal, so
a running SSH workflow's `active_step_id` / `active_step_transcript_path` never
reach the local record.

## 2. Goals and non-goals

**Goals**

- Selecting any step, unit, or agent run that executed on an SSH host shows its
  transcript, live while it runs (as close to real time as one `tail -F` over
  ssh allows) and complete after it finishes.
- Lazy: no ssh session and no coordinator disk is spent on a transcript nobody
  opens while it runs. One ssh child per distinct open file, shared across
  viewers, killed when the last viewer leaves.
- Durable: after a run is terminal, every one of its transcripts lives on the
  coordinator, readable with the host offline or removed, and counted by the
  usage rollup.
- Honest: a transcript the coordinator could not fully collect is labelled
  partial, never silently truncated.
- No visibility or telemetry gap for SSH remains after this arc.

**Non-goals (explicitly deferred)**

- Tunnel (`rupu node`) and bucket hosts. They keep today's behaviour. §10 says
  exactly what they implement later.
- Sessions over ssh (still `Invalid` on the connector).
- Back-filling runs mirrored before this change. They get the on-demand pull
  on first open like any other cache miss.
- Per-node host threading in the frontends' graph models. Not needed for SSH
  once §5 lands; revisit only if a host type needs it.

## 3. Cache layout and path contract

### 3.1 Layout

Every transcript that originates on an SSH host and is not already mirrored by
the pump lands in a lazy cache under the CP global dir:

```
<global>/mirror/<host_id>/transcripts/<key>.jsonl
<global>/mirror/<host_id>/transcripts/<key>.jsonl.complete   # sidecar, §6
```

`<host_id>` is the registry id (`host_<ULID>`, already filesystem-safe).
`<key>` is derived from the remote path:

- `…/.rupu/transcripts/<name>.jsonl` → `<name>` (a ULID-bearing step / unit /
  standalone-run id; unique by construction).
- `…/.rupu/runs/<parent>/sub_runs/<sub>/transcript.jsonl` →
  `<parent>__sub_runs__<sub>` (sub-run transcripts are all literally named
  `transcript.jsonl`, so the key carries the disambiguating components).
- Anything else is not a transcript this design serves; the allowlist in §3.3
  rejects it before mapping is attempted.

The `mirror/` directory is new. It sits outside `<global>/transcripts/` on
purpose: `api/run_streams.rs::collect_standalone_runs` scans that directory
for `*.meta.json` and must not see cache files. `api/transcript.rs::allowed_roots`
adds `<global>/mirror` so the local read and stream paths accept cache files.

The PR #646 agent-run mirror (`<global>/transcripts/<run_id>.jsonl`, written by
`NodeMirror::append(Transcript)`) is unchanged and is **not** moved into the
cache. It is the "already mirrored" case in §4.1.

### 3.2 Recorded paths stay remote

Mirrored `step_results.jsonl`, `unit_checkpoints.jsonl`, `events.jsonl`, and
`run.json` keep the remote `transcript_path` the executing host wrote. That is
the truth about where the file was written, it is what the allowlist matches
against, and rewriting it would break the cache-miss case (a path we have not
pulled yet has no local counterpart to rewrite to).

The SSH connector owns the remote→cache mapping (`SshHostConnector::cache_path_for`).
Nothing outside the connector computes cache paths.

### 3.3 A remote read is scoped to a run

`GET /api/transcript` and `GET /api/transcript/stream` gain an optional
`run=<run_id>` query parameter alongside the existing `path` and `host`.

Resolution order in the handler, when `host` names a non-local host:

1. If `path` validates against the local allowed roots and exists on local
   disk (the #646 agent-run mirror), serve it locally. No `run` needed.
2. Otherwise map `path` through the connector's `local_transcript_path`
   (§10). If the mapped cache file has a `.complete` sidecar, serve it
   locally. No `run` needed: the bytes were collected under a run's own claim
   at pull time (§6), and serving them again re-validates nothing.
3. Otherwise `run` is required (400 without it). The connector loads the
   mirrored run record (`RunStore::load`), and requires:
   - `record.worker_id == host_id` (the run belongs to this host), else 400;
   - `path ∈ recorded_transcript_paths(run)` (§3.4), else 400;
   - syntactic validity: absolute, extension `.jsonl`, no `..` component,
     no NUL. The path is single-quoted via the existing `shell_escape` when it
     is interpolated into any remote command.

So the connector never `cat`s or `tail`s a remote path the run did not itself
claim. This is stricter than the HTTP connector (which forwards the path and
lets the remote CP validate against its own roots), and deliberately so: for
SSH the coordinator is the only validator.

### 3.4 `recorded_transcript_paths(run)`

One function in `rupu-cp` (`host/transcript_paths.rs`, new) that enumerates
every transcript path a mirrored run's own artifacts claim, deduplicated:

- `step_results.jsonl` rows → `transcript_path`
- `unit_checkpoints.jsonl` rows → `transcript_path`
- `events.jsonl` → `transcript_path` on `StepWorking` and `UnitStarted`
  (the only way a *running* step's path is known before its result row exists)
- `run.json` → `active_step_transcript_path`
- sub-run transcripts referenced by the run's `sub_runs` layout, when present

The allowlist (§3.3) and the terminal pull (§6) both call this function, so
they cannot disagree about what a run owns.

## 4. Read path: `GET /api/transcript`

### 4.1 Cache miss

When resolution reaches step 3 of §3.3 (no local file, no complete cache),
the connector performs a one-shot `cat '<remote path>'` over ssh, writes the
bytes to `<cache>.tmp`, renames into place, and — only if the mirrored run is
terminal — writes the `.complete` sidecar. The handler then reads the cache
file with the existing local reader (`read_events_counting_unparsed`), so
`unparsed` reporting is identical to a local transcript.

If the run is not terminal, no sidecar is written: the file is a snapshot and
will be re-pulled or re-tailed on the next open.

### 4.2 Partial

A terminal run whose cache file has no `.complete` sidecar (the terminal pull
failed for that file, §6.2) is retried once on demand. If the host is still
unreachable, the handler serves whatever is on disk and adds `"partial": true`
to the response body. Clients render it (§8).

`partial` is absent (not `false`) for every other response, so existing
decoders are unaffected.

### 4.3 Errors

| Condition | Response |
|-----------|----------|
| Unknown `host` | 404 |
| `host` is remote, path is not local, `run` missing | 400 |
| Run not owned by host, or path not in `recorded_transcript_paths` | 400 |
| Host unreachable on a cache miss with nothing on disk | 502, message names the host |
| Host unreachable with a partial file on disk | 200 + `partial: true` |

## 5. Live path: `GET /api/transcript/stream`

The handler resolves the cache path exactly as §4 does (steps 1–3 of §3.3),
opens the existing `crate::transcript_tail::TranscriptTail` on the **cache
file**, and — before returning the SSE response — calls
`HostConnector::ensure_transcript_feed(run_id, remote_path) -> FeedGuard`.
The guard is held for the lifetime of the SSE stream (dropped when the client
disconnects).

The macOS and web clients therefore consume remote transcripts through the
same SSE contract as local ones; there is no second streaming protocol.

### 5.1 `LazyTailRegistry` (SSH connector)

Keyed by cache path. Behaviour:

- **First subscriber:** truncate the cache file (create empty), spawn
  `tail -n +1 -F '<remote path>'` via `RemoteExec::spawn_lines`, and append
  each received line to the cache file. `tail -n +1` replays from byte zero,
  which is why truncation is both safe and what makes a re-subscribe
  idempotent — the same pattern the pump uses for the agent transcript
  (`NodeMirror::reset_transcript`).
- **Additional subscribers:** share the running child; the refcount
  increments. Two viewers of one step (web + Mac) cost one ssh session.
- **Last guard dropped:** the child is killed (`kill_on_drop` on the
  `SshLineStream`, as today), the registry entry is removed, the partial
  cache file stays on disk.
- **Refusal:** if a `.complete` sidecar exists, no child is spawned and the
  guard is a no-op — a finished transcript never re-opens ssh. `TranscriptTail`
  simply drains the complete file.
- **Child exit while subscribed** (ssh dropped, remote `tail` died): the
  registry marks the entry dead and stops appending. The `TranscriptTail`
  keeps polling the file (which stops growing), so the SSE stream stays open
  but quiet; when the client reconnects (existing behaviour on both clients),
  the fresh subscribe truncates and replays. No duplicate lines are possible
  because every replay starts from an empty file.

The registry lives on `SshHostConnector` next to `pumps`, and is bounded by
construction: at most one child per distinct file, never for complete files.

### 5.2 `run.json` on every tick

`pump_finalize_if_terminal` gains one line: when the probe returns `Alive`, the
non-terminal `run.json` is mirrored (`NodeMirror::append(RunJson)`) before
returning. `RunStore` already tolerates a running record being overwritten
with a running record. Effect: `GET /api/runs/:id?host=<ssh>` for a running
workflow carries `active_step_id` and `active_step_transcript_path`, which
both frontends already use as the initial-focus fallback.

## 6. Terminal pull

### 6.1 Happy path

In `pump_finalize_if_terminal`, after mirroring the terminal `run.json` and
before `mirror.finish`, the pump:

1. Computes `recorded_transcript_paths(run)` (§3.4) from the now-complete
   mirrored artifacts.
2. Drops any path that already has a `.complete` sidecar or is the #646
   agent-run mirror (which `pump_catch_up_transcript` already replaces).
3. Issues **one** ssh invocation that, for each remaining path, prints the
   pump's own header format and then the file:
   ```sh
   for p in '<p1>' '<p2>' …; do printf '==> %s <==\n' "$p"; cat "$p"; done
   ```
   The output is parsed with the existing `parse_tail_marker`, so a header
   switches the destination file. One round trip regardless of step count —
   the connection-burst rule from `feedback_ssh_connection_burst` applies here
   as much as to probes.
4. Each file is written to `<cache>.tmp`, renamed into place, then its
   `.complete` sidecar is written. A file that is absent on the remote (`cat`
   prints to stderr, which is discarded) produces an empty cache file and a
   sidecar: "this step legitimately has no transcript" is a complete answer.

`finish` runs afterwards exactly as today.

### 6.2 Failure

If the invocation fails or the stream ends early, files that did not receive
a header keep whatever the lazy tail left (possibly nothing) and get no
sidecar. The run still finishes — a transcript collection failure must never
leave a run `Running`, the same rule #646 followed. §4.2 handles the retry and
the `partial` flag on read.

### 6.3 Usage rollup

`usage::run_transcript_paths` (and `summarize_run`) gain host awareness: when
the run record's `worker_id` names a registered host whose connector serves
runs from the local mirror, each recorded path is mapped through
`HostConnector::local_transcript_path(remote_path)` (default: identity) to its
cache path before reading. After the terminal pull, SSH workflow runs get real
token and cost numbers; before it, they get what has been pulled so far, which
is the honest answer for a running run.

## 7. Placed steps and fan-out units in a local workflow

The coordinator mints the remote unit's run id before dispatch, so the
mirrored transcript path is deterministic before anything launches.

- `rupu_orchestrator::runner::UnitDispatch` gains `unit_run_id: String`
  (`run_<ULID>`, minted by the runner per unit / per placed step).
- `rupu_cp::agent_launcher::AgentLaunchRequest` gains `run_id: Option<String>`.
  `SshHostConnector::launch_agent` uses it when present instead of minting
  (`agent_argv` already passes `--run-id`); `NodeMirror::create_run` is called
  with the same id. Local and HTTP connectors ignore it this arc (their
  transcripts are not the problem).
- `UnitDispatcher` gains
  `fn unit_transcript_path(&self, host: &str, unit_run_id: &str) -> PathBuf`.
  The fleet dispatcher returns `NodeMirror::transcript_mirror_path(unit_run_id)`
  — `<global>/transcripts/<unit_run_id>.jsonl`, the #646 location, which is a
  local file under the existing allowed roots.
- The runner records that path as the step's / unit's `transcript_path` (in
  `dispatch_placed_step`'s caller and the `distribute:` branch of the fan-out
  loop) **instead of** `opts.transcript_dir/<id>.jsonl`, and emits it on
  `StepWorking` / `UnitStarted` as it already does for local steps. The parent
  run's graph points at a file the pump is filling from the remote agent's
  first line; §5's lazy tail is not involved.
- `ItemResult.run_id` / `UnitCheckpoint.run_id` are set to `unit_run_id` for
  agent units (today `String::new()`), so the child run in Activity is
  linkable from the parent.
- `StepResult` / `StepResultRecord` gain `host: Option<String>`
  (`#[serde(default)]`, mirroring `UnitCheckpoint.host`). Set for placed
  steps, `None` otherwise.

`UnitOutcome` is unchanged: the path is known before the outcome exists, so
it does not need to travel back.

## 8. Standalone agent run on an SSH host

When `spawn_tail_pump` sees the transcript's first `==>` header (the point
where it calls `reset_transcript` today), it also calls a new
`NodeMirror::note_transcript_started(run_id, host_id)`, which sets the local
record's `active_step_id = "agent"` and
`active_step_transcript_path = transcript_mirror_path(run_id)` and saves it.

Both frontends already resolve a running step's transcript through the active
step fallback (`RunDetailStore.resolveTranscriptPath`; the web's
`runGraphModel`), so the transcript opens live with no client change.
`NodeMirror::finish` clears the active step and synthesizes the `"agent"`
step-result row exactly as it does now.

## 9. Clients

### 9.1 macOS (`RupuStore` / `RupuAPI` / `RupuRunDetail`)

- `RunDetailStore`: remove the `isRemote` gating in three places.
  `activate()` starts the run event stream for remote runs, passing `host`
  (`/api/events/stream?run=&host=` already proxies `stream_run_events`, which
  for SSH tails the local mirror). The convenience init builds the transcript
  tail factory for remote runs too, passing `host` and `runID`. `focusStep`'s
  `willTail` becomes "path known and step running and factory present".
- `BackendController.makeTranscriptStream(path:host:runID:)` and
  `makeRunEventStream(runID:host:)` append the new query items.
- `CPClient.transcript(path:host:run:)` sends `run`.
- `APITranscriptPage` decodes optional `partial: Bool`; `RunDetailStore`
  exposes `transcriptPartial`; `TranscriptFeed` renders it as a notice row
  beside the existing unparsed-lines notice ("Transcript incomplete — the host
  could not be reached to collect the rest").
- `make macos-fixtures` regenerates for the new field. Swift Testing covers
  the store wiring with fake factories (remote run starts both streams; a
  running remote step tails; a finished one fetches once).

### 9.2 Web (`crates/rupu-cp/web`)

- `TranscriptPanel` gains `runId?: string`, forwarded on both the fetch and the
  SSE URL via `api.getTranscript` / `api.subscribeTranscript`.
- `RunDetail`, `StepTranscriptBrowser`, `RunTranscript` pass the run id they
  already hold.
- `TranscriptResponse` gains `partial?: boolean`; `transcriptView` shows a
  badge beside the unparsed badge.
- `make cp-web` before release, per `project_rupu_release_embeds_web`.

No graph-model or node-model changes on either client.

## 10. Connector trait changes (host-type-neutral)

Added to `HostConnector` with defaults, so Local/HTTP/Tunnel/Bucket compile
unchanged:

```rust
/// Map a transcript path recorded by this host to the coordinator-local file
/// that serves it. Identity for hosts whose recorded paths are already local.
fn local_transcript_path(&self, remote_path: &str) -> PathBuf;

/// Ensure the remote file is being fed into its local counterpart while the
/// returned guard lives. Default: Unsupported.
async fn ensure_transcript_feed(&self, run_id: &str, remote_path: &str)
    -> Result<FeedGuard, HostConnectorError>;

/// One-shot pull of `remote_path` into its local counterpart; marks it
/// complete when `terminal`. Default: Unsupported.
async fn pull_transcript(&self, run_id: &str, remote_path: &str, terminal: bool)
    -> Result<(), HostConnectorError>;
```

`get_transcript(path)` keeps its signature for HTTP; the handler routes SSH
through the three methods above. Everything in §3–§6 above the connector —
cache layout, `recorded_transcript_paths`, `.complete` sidecars, the `run=`
parameter, `partial`, both endpoints, the usage mapping — is host-type-neutral.

**What tunnel implements later:** `local_transcript_path` (same cache layout,
keyed by its node id), `ensure_transcript_feed` (ask the node client over the
existing websocket to stream the file as an artifact into the cache), and
`pull_transcript` (ask it to send the file once). The node client learns to
tail an arbitrary transcript path on request, mirroring what `ArtifactFile::Transcript`
already does for the agent transcript. No changes above the connector.

## 11. Testing

Unit, at the seams:

- `host/transcript_paths.rs`: `recorded_transcript_paths` against real mirrored
  artifact rows (step results, checkpoints, `StepWorking`/`UnitStarted` events,
  active step, sub-runs); dedup; empty run.
- Cache key mapping: step/unit/standalone/sub-run shapes; rejection of
  non-transcript paths, relative paths, `..`, non-`.jsonl`.
- Allowlist: path not in the run → 400; run owned by another host → 400;
  local-mirror short-circuit needs no `run`.
- `LazyTailRegistry` with the existing `FakeExec`: truncate on first
  subscribe; one child for two subscribers; kill on last drop; refusal on
  `.complete`; child death leaves the file and the entry dead; re-subscribe
  replays from empty.
- Terminal pull: multi-file `==>` stream parsing into separate cache files;
  absent remote file → empty file + sidecar; stream ending mid-file → no
  sidecar for that file, run still finishes.
- `pump_finalize_if_terminal`: `Alive` now mirrors `run.json`.
- Runner: minted `unit_run_id` flows into `StepWorking`/`UnitStarted`,
  checkpoints, and `ItemResult.run_id`; `StepResultRecord.host` round-trips
  and defaults; existing mock dispatcher tests keep passing with the new trait
  method defaulted.
- `SshHostConnector::launch_agent` honours a supplied `run_id` (argv and
  mirror record both carry it).
- `api/transcript.rs`: `partial` present only when set; `run` required only
  on the remote cache-miss branch.
- Usage: an SSH-hosted run's rollup reads cache files after a pull.

Live check before merge (per the GUI validation rule and
`feedback_computer_use_available`): on matt's mini host, one workflow launched
on the host, one local workflow with a placed step, one standalone agent run.
For each: transcript visible while running in rupu.app and the web, complete
after finish, host offline afterwards still readable, cost column populated.
Batch the ssh probes; do not diagnose by hammering the host.

## 12. Rollout

Single PR arc, branch `claude/ssh-workflow-transcripts-2c3041`. Server and
clients ship together: the new query parameters are optional and the new
response field is optional, so an old client against a new server, or the
reverse, degrades to today's behaviour rather than breaking. `VersionGate.minimum`
does not move. `rupu cp serve` is a daemon: restart it to pick up the change
(`project_rupu_session_worker_daemon`).
