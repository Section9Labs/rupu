# Exact-run recorded event scan v1

Candidate source contract. Build/test execution and runtime acceptance are pending. Capability: `events.scan.v1`. No recent-run catalog or globally chronological search is provided.

## Request

`POST /api/events/scan`, JSON body at most 16 KiB:

```json
{
  "host": "local",
  "selection": {"kind": "runs", "run_ids": ["run_example"], "workspace_id": "ws_example"},
  "q": "group:error",
  "limit": 100,
  "cursor": null
}
```

- Omitted host means local. Explicit other hosts return 422 `events_scope_unsupported`; no fallback.
- Exact run IDs only, 1–20 entries; identifiers are ASCII alphanumeric/underscore/hyphen, at most 128 bytes. Duplicate IDs are canonicalized to one exact owner, sorted lexically for request binding.
- Only the configured active global run store is searched. No archive, project-local run store, remote host, finding ledger or discovered recent catalog is searched.
- Optional workspace ID is an exact additional owner constraint, validated against a bounded registered-workspace record. It is never inferred from a path basename.
- `q` defaults to empty, maximum 8 KiB UTF-8; existing `events` grammar applies. Relative since/until evaluation time is captured once and signed into continuation. `project:` keeps its existing basename-text semantics inside the exact optional workspace constraint.
- `limit` defaults to 100; valid range 1–200, rejected outside range. Cursor defaults to null and is bounded to 4 KiB.
- Unknown request/selection fields and duplicate typed JSON keys are rejected. `kind:recent` is unsupported.
- The existing control-plane auth middleware reauthorizes every call. Current CP authority is one configured token/local authority, not independent per-user principals. Cursors are not bearer grants.

## Response

```json
{
  "version": 1,
  "host_id": "local",
  "scope": {"workspace_id":"ws_example", "run_scope":"active", "run_ids":["run_example"]},
  "query": {"q":"group:error", "grammar":"events", "version":1, "evaluation_time_ms":1794211200000},
  "snapshot": "opaque-process-local-id",
  "order": "run_started_desc_record_offset_desc",
  "limit": 100,
  "records": [{
    "id":"opaque-record-id", "run_id":"run_example", "workspace_id":"ws_example",
    "byte_offset":8192, "timestamp_ms":1794211200000, "timestamp_source":"snapshot_file_mtime",
    "event":{"type":"step_failed", "run_id":"run_example", "step_id":"scan", "error":"example"}
  }],
  "next_cursor":"opaque-authenticated-continuation",
  "has_more_scan":true,
  "coverage": {
    "selection":"explicit_runs", "selected_runs":1, "catalog":null,
    "window_exhausted":false, "archive_searched":false, "remote_hosts_searched":false,
    "unavailable_runs":[],
    "page": {
      "event_bytes_read":4096, "metadata_bytes_read":512,
      "file_observations":8, "root_checks":14,
      "physical_lines_completed":12, "records_evaluated":10, "matches_returned":1,
      "stop_reason":"record_limit",
      "skipped":{"blank":1,"malformed":1,"unknown_event":0,"oversized":0,"owner_mismatch":0},
      "pending_oversized_record":false, "unterminated_tail_bytes":0
    },
    "limitations":["active_global_run_store_only","no_subrun_codename_derivation","no_findings_ledger","not_global_event_time_order","append_invalidates_snapshot"]
  }
}
```

The response echoes canonical run set, exact workspace constraint, host, q and limit. Native must bind the response and continuation to the initiating client connection identity and selection generation, not just endpoint URL. `byte_offset` is an unsigned exact byte position, not a legacy event line number. Decode it without floating-point precision loss. `timestamp_source` is `event`, `snapshot_file_mtime` or `run_started_at`; only `event` claims an event-owned timestamp.

Traversal orders runs by exact recorded start timestamp (including submillisecond precision), then run ID descending; within a file records are traversed by decreasing byte offset. This is not global event-time ordering. A record ID is stable within a snapshot. Query matching uses bounded recorded run metadata and event fields, with no workflow parsing, transcript reads or legacy subrun codename enrichment.

`has_more_scan` means selected scan work remains, not that another match exists. Empty records plus continuation are valid. Continue explicitly even when one page has no matches. `next_cursor:null`, `has_more_scan:false`, and `window_exhausted:true` together mark exhaustion of traversable selected bytes; unavailable owners, skipped records, missing files and excluded tails still qualify coverage. Do not label this complete history.

Page counters describe actual work for that request. File observations/root checks count logical snapshot operations; anchored path walking may perform multiple syscalls per operation. Metadata bytes include bounded reads only on initial capture; continuations validate stamps without rereading content. Skipped oversized/tail record outcomes are counted when the record finishes; `unterminated_tail_bytes` is the full size of fragments completed on that page, potentially spanning earlier requests. It is not bytes read on that page. Do not double-count retry counters. `unavailable_runs` is immutable selection coverage repeated on every page.

Stop reasons are `record_limit`, `response_budget`, `byte_budget`, `line_budget`, or `window_exhausted`. No wall-clock stop is used, so cursor replay against unchanged files is deterministic. Disk IO latency is not guaranteed.

## Bounds

- Two concurrent blocking scan jobs; excess calls return 429 without queuing unbounded work.
- Actual event reads: at most 8 MiB per request, including repeated delimiter lookahead. Completed physical lines: at most 20,000.
- Ordinary JSONL records: at most 256 KiB. Larger records and unterminated tails advance in discard mode; cursors contain no payload bytes. Blank/malformed/unknown records cannot produce an unbounded loop.
- Authoritative metadata: at most 32 KiB per record and 1 MiB aggregate. Oversized/unreadable metadata is qualified as unavailable, never partially parsed as an owner.
- Encoded response: at most 1 MiB including envelope. A matching row deferred by response size is not consumed; its exact pre-record boundary is continued next time. Actual work already done remains charged.
- Manifest: at most 32 KiB serialized, with bounded strings/vectors, no raw RunRecord or event payload. Cache: at most 128 manifests globally and eight per configured control-plane/store owner, ten-minute fixed TTL and LRU eviction. Encoded manifests total at most 4 MiB; retained structs have bounded string/vector overhead. Two in-flight jobs are separately bounded.
- Selected paths: at most 20 run metadata files and 20 event files plus optional workspace registration. Configured/canonical anchors are bounded to 4 KiB and 64 path components. No RunStore::list/load, subtree traversal, remote probing or legacy event enrichment occurs on the scan path.

## Changed and unavailable records

A single exact run missing from the active store returns 404 `events_missing_run`. In a multi-run request missing, unreadable, malformed or oversized owner records appear in `coverage.unavailable_runs` while other exact owners remain searchable. Positive run-ID or workspace-ID mismatch rejects the whole request. A run with no event file is an unavailable event source, distinct from a missing run.

Every page validates all selected owner/event paths, optional project registration and root identities before and after scanning, including files not yet reached and prior unavailable observations. Append, truncate, replacement, same-length rewrite, archive move or changed ownership rejects the page with 409 `events_snapshot_changed`. Inaccessible sources remain qualified unavailable while their observed failure class is unchanged; this does not claim their hidden contents were stable.

This is unchanged-file validation under the existing same-user recorded-file ownership model, not an atomic filesystem transaction. Appends invalidate v1 snapshots. Native should preserve q, owner and prior visible results as stale and offer explicit Refresh; it must not merge a fresh snapshot automatically.

## Errors and continuation

Query syntax errors preserve the existing 400 `{error,code,token,start,end}` envelope with Unicode-scalar offsets. Other errors use `{error,code}` with the HTTP status. Known codes include `events_invalid_selection`, `events_scope_unsupported`, `events_invalid_cursor`, `events_cursor_scope_mismatch`, `events_snapshot_expired`, `events_snapshot_changed`, `events_owner_mismatch`, `events_missing_run`, `events_missing_workspace`, `events_workspace_unavailable`, `events_scan_busy`, and `events_scan_cancelled`. Filesystem/configuration errors may use `events_scan_failed`.

Continuation repeats the same canonical run set, optional workspace, q and limit plus the exact cursor. HMAC binds scope, process-local snapshot ID, frozen query time and reverse traversal state. Changes to scope fail before manifest/file access. Eviction/TTL returns 410; process restart invalidates the process-key signature (400 invalid cursor). Neither condition silently restarts. The existing `/api/events` route, array response and old cursor semantics remain unchanged.

Native entry points are exact run context only. Use “Recorded events · 1 run” and “Continue Search.” A connection/project recent-events command requires a separately approved bounded catalog and capability and is not part of v1.
