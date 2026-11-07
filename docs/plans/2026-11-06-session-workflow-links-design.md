# Session workflow links

The approved native workflow launcher must retain an operator-launched workflow in the actual launching session. It must pin the selected saved source revision, survive reload, and expose real workflow agents while they run. The existing workflow launcher preserves the revision but has no session association. Session slash commands are read-only for workflows; model dispatch is turn-bound, subject to agent/workflow allowlists, and has no saved-revision request.

## Options and decision

1. Use model dispatch: reuses turn ancestry, but adds a model call, cannot guarantee the selected saved revision, and rejects gated/placed workflows. Rejected for an explicit operator launch.
2. Append a fabricated session turn or parent run: would misrepresent execution ownership and interfere with turn/continuation semantics. Rejected.
3. Add an explicit durable operator-workflow association: selected. Preserve ordinary workflow execution, save the session/project/source/run identities before spawning, and return/list that record through the CP. The association is separate from the agent-turn transcript.

## Contract

`POST /api/workflows/:name/run` accepts optional `session_id` only with `expected_revision`, explicit source scope and registered `workspace_id`. The active session must match that project and its canonical path. Remote/proxy launches are refused. `workflow.session_link` advertises support. A successful receipt includes `session_id` and `association` with session_id, run_id, workspace_id, host_id, workflow_name, source {scope_kind, scope_id, revision}, launched_at and status. `GET /api/sessions/:id/workflows` returns {session_id, host_id, workflows:[association]}; archived sessions remain readable.

The CP reserves a server-generated run ID and saves a private, synced record before a launcher capable of honoring that exact ID can spawn. Spawn failure retains a `launch_failed` record. Successful receipt follows persistence; no transcript or parent ancestry is invented. Before run metadata appears, status is `starting`; clients retry reads, never ambiguous launch mutations. Workflow events already carry `agent_started.agent_run_id` and `transcript_path`; graph and transcript APIs remain the live data sources.

## Dependencies and sequence

Implement durable association and exact-ID launcher port, then API guards and discovery, then advertise the feature. Native integration proceeds independently from the agreed response shape. Backend is isolated in a clone; original worktrees and the real CP remain untouched.

## Validation and merge gates

Run focused backend tests with disposable fixtures: identity and project/path mismatch; archived/missing/remote/unguarded/stale-revision rejection before spawn; durability before adapter invocation; exact run ID forwarded; reload after definition deletion; launch failure retained; no turn or parent mutation. Compile CP/CLI and run existing scoped revision tests. Actual provider execution and CP replacement are outside this task.

## Approval

Parent approved the design and explicitly authorized implementation without another approval request. Native UI is a separate workstream. No new provider calls or live mutations are authorized.

## Existing live data contract

- `/api/sessions/:id/commands` accepts `/workflow show`, `list`, `show-run`, and `runs`; it deliberately rejects `run`. `/api/sessions/:id/orchestration` enables an agent's lead tools for the next turn; it does not launch operator work.
- `/api/runs/:id/graph` reconstructs the DAG from the run's own workflow snapshot, not the current catalog definition. Its durable step results, unit checkpoints, and event-derived identities restore the finished graph on reload.
- `/api/events/stream?run=<workflow-id>&host=local` tails the workflow log, supports `Last-Event-ID`, and terminates with `end`. `step_started` creates a running row; `step_working` may provide the path before a result exists. `agent_started` provides `run_id` (workflow), `step_id`, optional `unit_index`, `agent_run_id`, `transcript_path`, `agent`, optional `provider`, `model`, and `codename`. `unit_started` provides unit index/key and transcript path for fan-out. Keep workflow and agent run identities separate.
- `/api/transcript?path=<returned-path>&host=local&run=<workflow-id>` reads the real transcript; `/api/transcript/stream` follows it with its own numbered `Last-Event-ID` cursor and `end`. Snapshot array length is not a stream cursor.
- `/api/runs/:id/messages` and `/messages/stream` read the workflow's own message space. Session messages remain under `/api/sessions/:id/messages`; a client may display both in its conversation while retaining the true owning run identity.

The completed qualification run's existing on-disk records were inspected read-only: its two steps each emitted `step_working` with a transcript path and `agent_started` with the exact agent run ID before completion. It was a standalone workflow in the same project, absent from the session's turn records, with no parent run. No new execution was performed during this audit; live identifiers are intentionally omitted from this document.

## Reviewed startup and archive handling

A session archive may race launch preflight. Creating the association directory uses `create_dir` for the child only; it never recreates a vanished session parent. Before successful spawn, publication and directory sync must succeed. A completed association moves with the whole session directory and remains discoverable in the archive.

An OS spawn is only acceptance, not evidence that the workflow initialized. The exact-ID launcher retains the child, logs stderr to a private file, and observes its actual exit. If the child exits without active or archived run metadata, the CP records a startup result under the reserved run directory; session reads expose `launch_failed` with up to 4096 bytes of stderr. Keeping this result outside the moving session directory prevents archive races from losing the outcome. Failure to observe the process yields `unknown`. The private association record also retains the CP monitor PID and process start stamp; if that monitor disappears without an outcome or run record, reads report `unknown`, never a failure guessed from elapsed time. Actual run metadata takes precedence over startup evidence.

These safeguards are tested with disposable records and a shell executable that prints its argv and exits with status 7. They do not call a provider or launch a real workflow. Clients must refresh session workflow associations while a graph is not yet available, and must not retry a launch after an ambiguous response.

## Validation evidence

On the isolated `codex/session-workflow-links` checkout based on `b430bff183e07c0efa30893ac0b0c8887d0777f5`, with default dev/test profiles and the shared scratch Cargo target:

- `cargo test -p rupu-cp --no-default-features session_workflow_link --lib`: 11 passed, including the archive race and startup observer regressions.
- `cargo test -p rupu-cp --no-default-features scoped_revision --lib`: 6 passed.
- `cargo test -p rupu-cli --no-default-features cp_launcher::tests --lib`: 6 passed, including the real disposable child-process fixture. The debug test executable produced a nonfatal linker warning about its large DWARF unwind table.

No production state, provider credentials, or live session/workflow records were modified. Native presentation and real-provider acceptance are validated in the separate owning workstream.

Final source review closed with no actionable findings after the archive race, early-process-exit handling, and tracking-directory publication ordering were corrected. The final storage-failure test covers both the session link directory and global run-tracking root: a known failure in either path prevents a spawn, and failure of the tracking root publishes no association. All 11 session-link tests and all six revision tests were rerun successfully after that ordering correction; the six CLI adapter tests cover unchanged adapter code.

`cargo build -p rupu-cli --no-default-features --bin rupu` succeeded on the final source. The unoptimized debug binary emitted the same nonfatal large-DWARF-unwind linker warning as the test executable. Binary SHA-256: `f2ba244efc5058c22db42a6ee6398f6c156e0b49d5a93229f11c5cbe429ad1e2`. The binary is a local acceptance artifact; no CP was replaced by this workstream.
