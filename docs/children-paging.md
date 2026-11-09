# Recorded child catalog pages

`GET /api/runs/:id/children?limit=50` captures the recorded local child catalog once and returns a bounded page. The owner advertises `run.children.pages.v1` in `/api/host/info.features` and supported root types in `/api/host/info.children_paging_root_types`: `recorded_run`, `standalone_agent`, and `session_turn`. The existing operator MCP tool `runs.children` exposes the same typed query and JSON response.

The default request, without `limit`, `cursor` or `parent_run_id`, preserves the legacy `{children,truncated?}` response and its 2,000-directory cap. Explicit remote `host` requests return 501 rather than reading a coincidentally matching local run. Agentiflow children have a separate existing route; this capability does not add pagination to that route.

## Scope and continuation

- `id` identifies an independently owned local root. Accepted evidence is a canonical active/archive `RunRecord`; version-1 `<global>/transcripts/<id>.meta.json` (or its archive) with an exact standalone run ID; or a canonical active/archive session record claiming that exact turn and its matching local transcript locator. Session metadata must agree with the canonical session's membership. Queued and legacy session turns can be resolved by a bounded scan of the canonical session collections before metadata exists.
- A proven session or standalone root may have no `runs/<id>` directory yet; its captured child catalog is empty without creating anything. A child directory or ledger alone never establishes ownership. A standalone/session proof does not add filesystem roots or authorize reading a recorded foreign transcript path. Project-local transcript roots outside the global owner need a separate resolver contract and return explicit unsupported-root status.
- `host` must be absent or `local`; connect directly to the remote owner when needed.
- `limit` defaults to 50 and clamps to 1–200. A conservative 4 MiB response-row budget can return fewer rows. Follow `next_cursor`, not the number returned.
- `parent_run_id` is an optional exact direct-parent filter. It must be the root or a parent reached through the recorded local tree. It never enlarges the root's membership. Preserve its exact value, including absence, for every page.
- `cursor` is opaque and MAC-protected. It binds the captured owner, root and exact parent filter. Page size may change. A cursor cannot be used with another root, owner or filter.

The walk follows the same sources and latest-handle folding as the legacy catalog: the root ledger, recorded completed step/result-item roots, each discovered child's ledger, and legacy `sub/` directories. It preserves recorded `parent_run_id`, depth, handle, kind, status and transcript locator. Remote step roots explicitly recorded in step results are not read from local paths. Missing history, live steps not represented in these sources and remote descendants are not invented.

```json
{
  "schema_version": 1,
  "root_type": "recorded_run",
  "owner_host_id": "local",
  "root_run_id": "run_example",
  "snapshot": "children-0000000000000001",
  "children": [],
  "next_cursor": null,
  "snapshot_matching_total": 0,
  "evidence": {
    "consistency": "captured_observation",
    "history_complete": "unknown",
    "source_scan": "complete_within_recorded_local_sources",
    "malformed_rows": 0,
    "incomplete_tail_bytes": 0,
    "run_directories_scanned": 1,
    "directory_entries_scanned": 0,
    "bytes_read": 512,
    "source_rows": 0,
    "missing_legacy_transcripts": 0,
    "remote_step_roots_not_walked": 0
  }
}
```

`snapshot_matching_total` counts materialized matching DTOs in this observation. It is not a claim about all executions or all historical descendants. `source_scan` says the bounded walk reached the end of its recorded local sources; malformed rows and incomplete final JSONL lines remain explicit evidence limits. `history_complete` is always `unknown`.

Capture reads files sequentially, so it is an observation rather than an atomic filesystem transaction. Once captured, pages use fixed DTOs and perform no ledger or transcript rescans. Later source appends or rewrites do not change this snapshot. Starting without a cursor refreshes the observation. Removing or moving authoritative ownership evidence invalidates its snapshot. Workflow-root directory replacement and standalone metadata replacement also invalidate it. Ordinary atomic session-record updates for subsequent turns preserve the fixed snapshot.

## Resource bounds and errors

One capture runs at a time per process. Captures read at most 64 MiB total, 16 MiB per file, 256 KiB per complete JSONL row, 500,000 nonblank source rows, 100,000 run directories, 100,000 legacy directory entries and 100,000 pending parents. Sources are regular files; symlinked membership paths are rejected. Legacy transcript reads are subject to the same budgets.

The cache retains at most 64 snapshots and a conservative 64 MiB of DTO storage, with at most 16 MiB per capture. Snapshots expire after ten minutes; insertion evicts the oldest captures deterministically when either global cap is reached. Monotonic process-local snapshot identifiers never collide with an earlier capture, including evicted ones. Cursor signatures use a fresh process-local secret.

- **400:** malformed identifier/cursor, changed owner/root/filter or unsafe source path.
- **404:** no recorded root exists, or the requested parent is not a member of the captured local tree. Never use this as a signal to silently downgrade to legacy.
- **409:** expired/evicted snapshot, root replacement/removal, invalid canonical root identity, or source mutation detected during capture. Restart a stale snapshot without its cursor; do not combine old and new pages.
- **413:** a source, transient walk or retained DTO budget was exceeded. No partial catalog is returned as success.
- **429:** another capture is in progress. Existing snapshots can still be paged.
- **501:** a nonlocal host was requested, or an existing source lacks supported root ownership evidence. Only the latter returns a structured `{code:"children_root_unsupported",supported_root_types:[...],error:"..."}` response. A client may explicitly select the legacy catalog once for that exact owner/root after this code; a plain 501, 404, or other failure is not a fallback signal.

A process restart loses snapshots and rotates the cursor-signing key. A previous process's token is rejected; restart without a cursor. No read starts agents, retries work, changes claims or writes source artifacts.
