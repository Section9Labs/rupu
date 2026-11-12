# Bounded descendant catalogs and transcript windows

Capability `workflow.observation.descendants.v1` adds direct-child navigation to
a verified workflow member's bounded transcript window. The original workflow
owner remains fixed through child and grandchild navigation. Existing children
APIs and direct-member transcript requests retain their contracts.

## Requests

GET `/api/runs/:workflow/observation/transcript/children` takes required `ws_id`
and `parent_cursor`, optional `host=local`, `limit`1..50 (default25), and optional
opaque catalog `cursor` (at most2048 bytes). Without catalog cursor, it captures
only this verified parent's direct recorded children. With catalog cursor, it
returns another page of that same immutable capture and does not rescan children.

`parent_cursor` is any retained server-issued `transcript.window_follow.v1`
window cursor, at most32768 bytes. It need not be the newest cursor: Earlier and
manual browsing remain usable during append. Authentication, exact owner,
ancestry/prologue and selected parent file identity are checked every time;
observed truncation or replacement still refuses reuse. A workflow frontier,
legacy SSE ID or arbitrary path cannot substitute for a parent cursor.

GET `/api/runs/:workflow/observation/transcript/descendant` takes required `ws_id`,
`parent_cursor`, catalog `snapshot` and exact `record_id`; optional local `host`
and `limit`1..200 (default100). It opens the selected available agent/subagent
with the existing bounded `transcript.window_follow.v1` reply. The request accepts
no path, child run ID, byte offset, status or caller-supplied ancestry. Subsequent
latest/poll/earlier calls use the existing `/observation/transcript` route with
this new reply's cursor. Unknown fields and invalid limits return400.

## Catalog response

Required fields:

- `schema`: `workflow.transcript_children.v1`.
- `workflow_run_id`, `workspace_id`, `host_id` (local).
- `parent_run_id`, `parent_path`, `parent_file_identity`: exact verified selected
  parent, independent of the original workflow's run ID.
- `instance_id` and `snapshot`: process-owned26-character ULIDs.
- `depth`:0..16, number of descendant hops after the direct workflow member.
- `observed_at`: RFC3339 UTC capture time.
- `records`: at most requested limit, each row described below.
- `next_cursor`: nullable opaque ASCII catalog token, at most2048 bytes.
- `captured_total`: exact number of retained rows in this immutable catalog.
- `source_complete`: whether the selected direct sources were fully observed
  within the documented limits, not a claim about unrecorded/global history.
- `evidence`: `ledger_state`, `legacy_state`, nonnegative `malformed_rows`, and
  `omitted_total` (null; no invented underlying count).
- `limits`: `ledger_bytes:4194304`, `source_bytes:8388608`, `ledger_rows:20000`,
  `legacy_entries:256`, `page_records:50`, `reply_bytes:1048576`, `depth:16`,
  `snapshot_ttl_seconds:600`.

A row has required `record_id` (`child:` plus64 lowercase hex), `source`
(ledger|legacy), `child_run_id` (valid identifier at most128 bytes), nullable
`handle` (at most256 bytes), `kind` (agent|subagent|workflow), display `name`
(at most4096 UTF-8 bytes), nullable `recorded_status`, `transcript_available`
Boolean, and nullable `reason`. Only an explicit recorded Finished contributes
`recorded_status`; absence does not imply running. Legacy rows remain unknown.
No prompt/input/output, credential or full transcript is included in the catalog.

Workflow rows have `transcript_available:false` and
`reason:workflow_navigation_required`. They navigate to the exact child workflow
using its existing bounded workflow observation contract and independently
verified workspace owner; they do not open as agent transcripts. Missing recorded
transcript identity is explicit `identity_or_transcript_unavailable`.

Ledger states are absent, captured, partial_tail, oversized, row_limit, or
parent_store_absent. Legacy states are absent, captured, entry_limit, or
parent_store_absent. Malformed/oversized lines remain counted qualifications;
unknown omitted totals stay null. A missing parent store is unknown, never a
complete empty tree. Refresh is an explicit new capture; continuation preserves
its captured rows even if the parent appends or launches more children.

## Ownership, limits and continuation

The signed selected parent authorizes only its exact candidate run directory:
configured global active/archive, exact currently registered project's active/
archive stores, and an inline `sub/<child>/transcript.jsonl` directory only when
that path is established by a sealed prior edge. Multiple candidate directories
are ambiguous and refused. Direct source resolution never walks the workflow
hierarchy or guesses an arbitrary transcript's parent directory. Every source
uses anchored no-follow files/directories.

Ledger input is at most4MiB and20000 lines, with each retained line at most64KiB.
An oversized ledger is explicitly unavailable rather than folded from a prefix.
Legacy inspection visits at most257 directory entries (the extra is a limit
sentinel), retains at most256, and reads only bounded64KiB first-record proofs.
Capture source IO is at most8MiB, separate from ancestor proof. No child transcript
completion scan or full-tree scan occurs. Each retained snapshot accounts at most2MiB of serialized rows plus sealed edges;
cache accounting is at most16MiB/32snapshots with600-second TTL and lazy expiry/
eviction. This payload accounting excludes Authority/struct/String allocation,
allocator overhead and temporary parse buffers; it is not a process RSS bound.
Replies are at most1MiB. Catalog cursors bind exact snapshot and parent authority;
cache expiry requires explicit refresh, not fallback to the unbounded API.

Selected edges are recorded byte ranges plus hash/file identity or exact legacy
subdirectory membership. Opening and continuation revalidate original workflow
ownership, every ancestor prologue and append-stable ancestor file epoch, exact
edge content and child prologue. Chains reject cycles and exceed16 hops with an
explicit error. Opaque window tokens retain sealed ancestry after catalog cache
eviction, remain process-instance bound and at most32KiB, and confer read authority
only. Root/member proof and all edge checks are bounded; registry lookup is exact and pinned. Scoped traversal reads registration once
before and once after the chain, never once per hop; original root-member proof
has its own separately bounded owner/registration reads.
Metadata and content proofs describe observed reads, not an atomic global snapshot.

Conservative whole-request content IO ceilings are20MiB for initial catalogs,
12MiB for descendant selection plus first window, and6MiB for subsequent windows.
These include bounded root/ancestry/registration proofs as well as source/window
reads, and exclude metadata/open/directory operations. Catalog continuation does
not rescan child sources. These finite IO limits are not process-memory limits.

Both routes share the existing four admitted transcript workers, immediate429
when occupied, HTTP cancellation-on-drop and five-second response timeout. Workers
keep their permits until actual work exits and check cancellation/deadlines across
proof, rows and reply preparation; blocking kernel IO cannot be forcibly cancelled.
A source Stamp race is409 `transcript_window_changed_during_read`, allowing normal
bounded retry without replacing a reader. Other ancestry, replacement, expiry,
source/prologue or scope failures remain explicit409/404/413/503 errors. No silent
replay, automatic rerun, model tool, send rights or execution-host grant is added.

Fixtures cover direct and recursive navigation, actual inline sub/sub sources,
ancestry replacement, immutable pages, cache eviction, registry replacement,
foreign/remote/symlink/malformed/oversized sources, cancellation and HTTP selectors.
They are authored source fixtures; execution is owned by the coordinated validator.
