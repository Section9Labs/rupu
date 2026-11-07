# Recorded workflow history and session attachment

This adds a recorded-run path beside workflow launch. It never executes a workflow, creates agent ancestry, edits a transcript, or manufactures a source revision.

## Read project history

`GET /api/projects/:id/workflow-history?host=local&offset=0&limit=50&q=search`

- Project identity is the registered workspace ID in the URL. Unknown project: 404. Remote host: 501. Unknown fields: 400.
- `q` is a plain case-insensitive substring over run ID, workflow name, and recorded codename. It is not the CP query language.
- Default limit 50, accepted 1…100. Active and archived executions are scoped to the project before pagination. Newest first, stable ID tie-breaker. Duplicate observations during archive races are collapsed.
- Response: `{workspace_id, host_id:"local", runs:[{run_id,workflow_name,codename,status,started_at,finished_at,archived}], next_offset}`. Codename, finished_at, and next_offset can be null.

## Attach a recorded run

`POST /api/sessions/:id/workflows?host=local` with strict `{run_id}`.

- Requires writable CP and active (not archived) session.
- Run must exist in this CP's active or archived workflow storage. Reject traversal and symlink escape.
- Canonical run ID and project ID must match. The target session path must match the current registered project path. The historical run path may have moved or disappeared; attachment grants no execution there. No cross-project or cross-host fallback.
- Stores a link under the session, using the original run ID. The original run metadata, graph and transcripts remain byte-for-byte unchanged. No launcher is invoked.
- Reattaching is idempotent and preserves an existing launched association's provenance.
- Response `{session_id,host_id:"local",association}`. Attachment association extends the existing shape with `origin:"attached"`, `attached_at`, and `source:null`; `launched_at` remains the original run start. Historical scope/revision remain unknown, and the UI must not substitute today's source.
- Original launch associations expose `origin:"launched"` and their unchanged `source` object. Existing stored records default to this origin and decode without migration.
- Existing session workflow listing follows the original run state across archiving. An attached run removed later becomes `status:"unavailable"` with a reason, never remains falsely completed.

## Archived readers

The graph and explicitly selected local event stream now resolve active or archived canonical storage. Archive access is read-only; it does not restore/copy the run. Transcript reads and SSE keep the original recorded path, resolving a missing path under the original run directory to its archived counterpart only through the exact local run ID and existing allowed-root validation. Normal global/project transcript files are read in place. Missing historical project-local transcript files are not manufactured or recovered from unrelated paths.

External operator MCP clients have `sessions.workflows`, `sessions.attach_workflow`, and `projects.workflow_history` through the same handlers/schemas. These operator tools are not automatically granted to the built-in session agent. Its next real turn receives bounded reference context containing only canonical run identity, workflow name, attachment origin, recorded status, and active/archive storage. Raw inputs, event payloads, and source paths are excluded. References do not create transcript turns or rerun work.

Capability integration (parent owns the concurrently edited host_info.rs): advertise `workflow.session_history` for this server build; advertise `workflow.session_attach` when `s.launcher.is_some()`.

## Idle session creation

The CLI's legacy no-prompt start still substitutes `go` and queues the first turn. Opening recorded work instead uses the explicit `idle:true` API flag or `--idle` CLI flag, advertised by `session.idle_harness`.

The flag travels through CP `SessionStartRequest`, runtime `argv::SessionStart`, CLI `StartArgs`, and the project start body. CLI creation saves the real Idle record and its provider/model settings, prints `session: <id>`, and returns before `launch_turn`. Idle creation rejects prompts, targets, and clone destinations. CLI skill grants may be configured for later turns; no skill is executed by creating the idle record. Native may create idle and then attach the validated run; if attachment fails, preserve the actual empty session and show its error rather than deleting potentially user-owned work or retrying creation.

Preparation currently discovers SCM registry metadata before constructing the session, but does not issue a model request. It creates a record with `status:idle`, zero turns, empty runs/history. A separate future refactor can defer unnecessary SCM discovery when no target is requested; do not bypass canonical configuration/project resolution.

## Verification

`CARGO_TARGET_DIR=/private/tmp/rupu-historical-attachment-target cargo test -p rupu-cp --lib session_workflow --offline` passed 23 tests, 0 failures (2.62s test execution). This covers scoped history and pagination, active/archived attachment, idempotent reopen, known launch provenance, moved historical paths, byte-preserved run/session records, deleted-run unavailable state, remote/read-only/unknown-field/identity/symlink refusal, original archived graph, run SSE, and transcript snapshot/SSE through the original path without restoring the run.

The archived event test intentionally omits a terminal event while retaining a terminal canonical run record, demonstrating that an old truncated event log closes rather than waiting forever. The shared SSE path preserves normal active-run tailing behavior. `cargo test -p rupu-cp --lib sse::tests --offline` passed all 10 existing parser/firehose regression tests (1.86s execution). The macOS linker emitted its large-debug-binary `__eh_frame` warning; there were no Rust source warnings in that final build. The root integration pass still owns operator-MCP route/name fixture validation and installed-app acceptance. This is protocol/storage validation with disposable unit fixtures, not real-provider or installed-app acceptance. No provider requests, CP restart, install, push, or live record mutation were performed by this workstream.
