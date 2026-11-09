# Finite retained workflow process logs

Feature `workflow.process_logs.pages.v1` (Unix CP only). This read-only operator HTTP surface does not add an operator MCP tool or a model tool/grant. Existing `/api/runs/:id/log` remains untouched: it is workflow **events.jsonl SSE**, including existing proxy behavior, not stdout/stderr.

`GET /api/runs/:id/process-log?ws_id=<exact>&host=local&source=retry|session_launch`

Source names map only to `retry.log` (workflow resume process output) and `session-launch.log` (session-launched workflow process output). No supplied path, arbitrary file, Agentiflow detach log, agent transcript, or background service log is accepted. A normal workflow launch may have no retained process log. Missing means unavailable, not successful/quiet/complete. Run ownership uses the existing exact local active/archive or exact registered project-local owner; no federated resolution and no project scan. Explicit nonlocal or malformed host metadata refuses. Local identifies the artifact store: process execution host is unknown, never inferred from query `host=local`.

First GET captures at most the final256KiB bytes of the chosen regular file using anchored no-follow/nonblocking descriptors. Before/after descriptor metadata, anchored pathname identity, freshly opened owner-directory identity and recorded owner fields are checked. Writes during capture return409; no retry is hidden. Checks are observations, not an atomic filesystem snapshot. A suffix can start mid-line or mid-codepoint; `head_truncated` and `invalid_utf8` disclose that. Display text is UTF-8 replacement decoding; no binary/original-byte fidelity claim.

Response `{header, offset, end, text, next_cursor}`. Header fields:
- `schema`, exact `run_id`, `workspace_id`, `host_id:"local"`, `source`, opaque `snapshot`, `observed_at`;
- `store_kind: global|global_archive|project_local`, `archived`, `provenance:"locally_retained_workflow_artifact"`, `process_host:"unknown"`, `history_complete:false`;
- `state:available|missing`, decimal strings `source_bytes`, `captured_start`, `captured_end`, `head_truncated`;
- `text_encoding:"utf8_lossy"`, `invalid_utf8`, integer `text_bytes`, `text_sha256:"sha256:<hex>"`.

Offsets/end are byte offsets **within captured display UTF-8**, not original-file offsets. At most16KiB text perpage, scalar-boundary-safe. `captured_start/end` separately describe original source range. Missing yields emptytext but state missing. Source EOF atcapture is not lifecycle completion. Header remains immutable acrosspages.

Continue GET adds exact `snapshot` and `cursor`, keeping owner/source. Start uses snapshot withoutcursor. MAC binds cursor to exact run/project/source/snapshot/offset. No filesystem IO during replay. Cache holds at most4 captures, each at most768KiB projected UTF-8 (worst-case replacement), TTL10minutes; eviction/expiry/foreignowner yields409. This bounds retained text, not total process RSS. Pages share4-reader admission, captures share1 workflow capture permit, content read in8KiB cancellation/deadline checked chunks. Header owner records each<=256KiB; exact registration bounded256KiB whenprojectlocal. Capture publication checks cancellation under the cache lock. Deadlines are cooperative checks around filesystem calls, not hard wall-clock guarantees. Dropped requests may finish bounded synchronous IO but cannot publish after cancellation observed.

400 invalid selector or malformed cursor; 404 missing exact owner; 409 observed source/owner change, foreign signed cursor, stale/foreign snapshot; 413 owner bound; 429 admission busy. No automatic retry, no poll/SSE follow. Explicit Refresh captures newdata; prior text may remain labeled retained when Refreshfails.

Native: workflow Details → Process Logs → Retry Output / Session Launch Output → Read Latest → Earlier/More or Refresh. No top-level tab. Context must include exact workflow run, positive project and connection; no selected-session inference. Historical and normal workflow Details share the component. Feature absence/unavailable stays explicit; no fallback to raw SSE. Copy only displayed page text.

Source fixtures are prepared, not executed. Warm validation and live native acceptance are separate gates. No log content or real IDs are committed.
