# Bounded recorded run-family transcript search

Capability: `run.transcript_search.v1` (direct local owner only).

`GET /api/runs/:id/transcripts/search?project=<workspace-id>&q=<literal>&host=local&limit=20&cursor=<opaque>`; MCP `runs.transcript_search` uses the same schema and handler. `project` and `q` are mandatory. Literal case-sensitive matching searches string values of valid typed transcript events, including tool arguments/results and reasoning; JSON field names are not search text. Query is 1–256 UTF-8 bytes. `limit` is 1–50. Scope is the canonical root's recorded run family, including root where it has a transcript. No subtree inference or Agentiflow lead-root fallback.

Response schema version 1:
```
{schema_version, owner_host_id:"local", root_run_id, project_id, query,
 snapshot, hits:[{run_id, authorization_run_id, transcript_path,
 provenance:[{parent_run_id,run_id,source}], position, excerpt,
 first_run_start:<raw typed event>,
 page_cursor:{snapshot,token,position}}],
 next_cursor, has_more, coverage:{members_total, members_completed,
 bytes_scanned, records_scanned, malformed_records, oversized_records,
 unavailable_transcripts, unverified_transcripts, remote_members,
 membership_malformed_rows, membership_incomplete_tail_bytes,
 history_complete:"unknown", consistency:"captured_observation"}}
```

Hit `position` is one-based nonblank physical record position. `page_cursor.position` is the one-based position of the first returned record, equal to the hit position; token goes directly to existing `/api/transcript/page` with exact path, local host and `authorization_run_id`. Its snapshot uses that endpoint's existing file-stamp binding. `first_run_start` is the actual first nonblank record, validated against exact member run and requested project; no filename-derived proof. Provenance retains exact recorded ledger, workflow-step or legacy directory edges; it is not a claim that a native navigation stack has already loaded those edges. Original root remains the authorization run.

Each request loads only the exact project registry record (64 KiB cap), never enumerates the workspace catalog. Allowed paths are the connected global root and that exact registered project; registration changes invalidate continuation. Root ownership records are revalidated under the existing 16 MiB-per-record metadata quota, separate from the 8 MiB transcript scan budget.

Capture proves canonical active/archived root ownership through the existing bounded owner resolver, pins project ownership, and enumerates recorded children and workflow step results under source quotas. It records metadata stamps and locators, never whole transcript contents. Membership is finite and frozen for ten minutes; at most 10,000 members, 16 MiB membership, 16 snapshots / 64 MiB retained cache. Existing capture limits additionally apply (16 MiB/source, 64 MiB aggregate metadata input, 500,000 source rows, 100,000 directory entries). Legacy children only supply recorded directory edges and are verified by their own first RunStart before producing matches. Missing files, unproven members, malformed source rows, remote members, and incomplete ledger tails remain explicit coverage limits. Unknown historical completeness is never upgraded to complete history.

Each call scans at most 8 MiB of transcript bytes and 10,000 physical records, returns at most 1 MiB including proof/cursors, and retains at most 256 KiB per line. A page may contain zero hits with a continuation; the client must offer Continue Search. Oversized complete records are counted and skipped. A single line exceeding the scan budget returns 413, not a fabricated complete scan. Calls observe cancellation between membership entries, file opens and lines. One process-wide search permit uses try-acquire (429) before entering the blocking pool; cache locks cover only lookup/insert. Source ownership and captured file identity are rechecked at use; file edits/append/replacement invalidate that file's search continuation and locator with 409. A captured observation is not a filesystem transaction.

Errors: 400 invalid scope/query/cursor, 404 absent canonical root, 409 changed owner/project/file or expired/evicted capture, 413 bounded source/line/response limit, 429 search busy, 501 unsupported remote or root evidence. Root project mismatch fails closed before returning any member data. Missing/invalid member project proof is explicit unverified coverage, not a cross-project hit. There is no legacy full-transcript fallback.
