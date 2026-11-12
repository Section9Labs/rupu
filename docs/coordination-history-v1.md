# Recorded coordination history v1

`coordination.history.v1` is advertised by `GET /api/host/info`. It exposes the
selected owner's persisted coordination sources without launching work, changing
board state, or inserting operator actions into an agent transcript.

```
GET /api/{sessions|runs|agentiflows}/:id/coordination/history
    ?source=messages|claims&host=local&limit=100&cursor=...
```

`source` is required. `host` must be absent or `local`; explicit remote hosts
return 501. `limit` defaults to 100 and must be 1–200. Unknown query fields are
rejected. Cursors belong to one endpoint, owner and source. This HTTP capability
does not advertise an SSH helper or MCP tool. Normal CP authorization applies.

The response shape is:

```json
{
  "version": 1,
  "owner": {"route": "sessions", "kind": "session", "id": "ses_example", "host": "local"},
  "source": "messages",
  "ordering": "source_append_order",
  "snapshot": "opaque-observation-identity",
  "consistency": "fenced_source_observation",
  "fence_bytes": 512,
  "records": [{"record_id": "opaque-observation-identity:1", "position": 1, "status": "event", "event": {"t": "seen", "id": "msg_example", "by": "reader", "turn": 3, "at": "2026-11-11T00:00:00Z"}}],
  "next_cursor": null,
  "has_more": false,
  "coverage": {"source_present": true, "invalid_records": 0, "unsupported_records": 0, "incomplete_tail_bytes": 0}
}
```

Owner kinds are `session`, `workflow`, `standalone`, and `agentiflow`. Session
and run archives remain readable. Owner metadata and source containment are
checked on every page. Symlinked owner/store aliases and special files are
refused; callers must select the canonical owner rather than a continuation's
message-space alias. Missing standalone run metadata is allowed for the existing
pending-run directory shape. Other owner metadata must be readable and identify
the requested owner. Reads do not depend on owner liveness or write authority.

## Source records and attribution

`messages` reads the owner's `log.jsonl`. An `event` is a typed canonical `msg`
or `seen` line, preserving message IDs, timestamps, addresses, kind, board
subtype, references, delivery and recorded operator provenance. The type is not
rewritten into a transcript event. `seen` has `id`, `by`, `turn`, and `at`; it
does not contain a run ID. An operator has no sending run to invent.

`claims` reads `board/claims.jsonl`. Events contain `ev` (`claimed`, `released`,
or `expired`), `key`, `owner`, `at`, and `lease_expires_at` on acquisitions.
Lease tokens and unknown raw journal extensions are never returned. These are
recorded lease transitions, not job completion events. A record ID identifies
one source occurrence within the observation; it is not a grant token.

Participant addresses are not historical run identities. The current roster
cannot safely bind historical entries to runs because addresses may be reused.
Canonical transcript `message` events provide exact run-local placement;
qualified owner/message IDs can correlate those events to this history.

Source ordering is independent. The endpoint provides no merged order across
the message log, claim journal, or agent transcripts. Timestamps are evidence,
not an inferred causal relation. A UI may show an owner-history section beside
a conversation, but must identify its source and cannot label it an agent's
sent/received event or a response to a nearby message.

## Observation and paging boundaries

The first page captures an EOF byte fence. The opaque `snapshot` remains stable
through that paging chain; it identifies a **fenced source observation**, not a
proof that the entire file is immutable. Later appends are excluded from the
chain. Refresh without a cursor to observe them.

Signed cursors bind the owner selection and identity, source, opened-file
identity, fence, physical position, byte offset, and hashes of bounded head,
fence and continuation-boundary ranges. Each request hashes the bytes actually
read, including bounded buffer lookahead and discarded oversized lines, then
reopens through the anchored regular-file walker and verifies that same range
before returning it. Truncation, replacement, changed owner identity, or a
changed checked range requires reset. Rewrites entirely outside those ranges
are not claimed detected. This is a bounded read observation, not a filesystem
transaction or a global snapshot.

Per request: at most 200 records, 256 KiB retained per line, 8 MiB source scan,
and 1 MiB response. Verification rereads at most that scanned range plus thirteen
4 KiB anchor reads, including capture and repeated boundary checks; owner
metadata is bounded to 1 MiB per inspection. There is no
owner population scan, retained server cache, provider call or background task.

`position` counts physical lines, including blank ones. Blank lines produce no
record. Malformed/oversized/unknown-type lines produce diagnostic records with
no `event`; status is `malformed`, `oversized`, or `unsupported`. Coverage
diagnostic counts are **page-local**, not totals for the unread source.
An unterminated final line is excluded and its byte count is reported when the
reader reaches it. A zero tail count on a page with `has_more` does not establish
that the unread tail is complete. A line exceeding the scan budget returns 413
when no complete physical line advances the cursor. A blank-only scan window
returns an empty page with an advancing cursor, so later records remain reachable.

Record IDs are opaque nonempty strings, stable only within the same observation. Refresh may produce new
record IDs; canonical message IDs remain the correlation key. Cursors are
authenticated with the existing process-local recorded-page signer and a
separate history domain. Restarting the CP invalidates its cursors. The legacy
transcript pager keeps its original encoding and strict file-change behavior.

Errors use the ordinary `{ "error": "..." }` envelope: 400 for invalid query,
cursor or another selection's cursor; 404 for an unavailable owner; 409 with
`coordination_snapshot_changed` for a changed observation; 413 with
`coordination_history_scan_limit` for the scan bound; 501 for remote scope.

## Evidence limits and compatibility

This is recorded source history, **not a complete operator audit**. Denied
claims and many rejected operations were never persisted. Claim journaling is
best effort in the existing writer. A missing source does not prove that no
actions occurred. Logging a message precedes mailbox/state application, so a
logged row alone does not prove successful delivery. Existing partial-write
`logged_message_id` errors and all mutation behavior remain unchanged.

New transcript message events optionally carry `post_kind`. Agent-origin sent
posts copy it from the exact bus receipt. Received posts copy it from the
mailbox. An operator-authored addressed post retains `kind: post` and its board
subtype while preserving `InjectionKind::Operator`, instruction framing and
priority. Ordinary operator messages remain `kind: operator`. Old records omit
the optional field and retain their original meaning; no default is inferred.

Operator claim/release/retraction still do not generate run transcript events.
Unaddressed posts do not generate recipient events. A directive can appear in
a recipient transcript only when that run's collector actually observes it.

## Read-only local acceptance

The opt-in test `optional_real_owner_history_exports_production_dtos` accepts
`RUPU_COORDINATION_HISTORY_MANIFEST`, a path to a private JSON manifest. Without
that variable it returns immediately. Each `selections` entry specifies
`global_dir`, `route`, `id`, `source`, `source_sha256`, `expected_message_ids`,
`expected_claim_count`, and `export_file`. Keep the manifest, source identities,
and exported content outside Git. The test checks each source's SHA-256 before
and after calling the production reader, validates exact owner/message identity
and claim field filtering, and writes the production DTO to the explicit export
path. Sources are bounded to 8 MiB and must fit one 200-record page for this
acceptance mode. This validates persisted-source projection without launching a
service or sending messages; it does not establish live HTTP/native integration.
