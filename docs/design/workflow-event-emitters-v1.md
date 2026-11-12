# Workflow event emitter classification v1

This optional, read-only observation projection distinguishes syntax failures from
recorded events emitted by a workflow's worker agents. It does not establish
execution authorization, transcript membership, lifecycle truth, or permission to
send messages. Existing member/prologue and descendant-edge readers remain the
only transcript-opening paths.

## Opt-in and compatibility

Host capability: `workflow.observation.event_emitters.v1` (local Unix CP only).

`GET /api/runs/:id/observation?ws_id=<exact project>&include_event_emitters=true`
opts a capture into this projection; `window=prefix|latest` and the existing local
host/owner requirements are unchanged. The Boolean defaults to false. Absent or
false preserves the old header, page rows, evidence counters, and classification.
Capture cache reuse includes the mode. Each immutable snapshot and its existing
MAC-bound cursor retains the mode; page queries accept no mode override.

Opted-in headers **and every page** contain `event_emitters: true`. Legacy headers
and pages omit the field. A new client requesting enrichment must require the
true header echo and exact matching page echo, never silently downgrade. Failure
to discover capabilities is not proof that the capability is absent.

Old native source (`WorkflowObservation.swift`) admits only available, malformed,
partial, and oversized row states and requires every available event run_id to
match the workflow. It would reject both new unavailable rows and nested emitter
rows. Opt-in prevents either shape from reaching a legacy capture. This is a
source compatibility finding, not native execution evidence.

## Rows

The existing `workflow.observation.pages.v1` envelope, record ID, byte offset,
chronology, original event payload, and original emitter run_id remain intact.

Canonical events emitted by the workflow owner have the exact legacy available
row shape: no `emitter`, `reason`, or `event_type` fields.

Only canonical `dispatch_started` and `dispatch_completed` can receive nested
emitter qualification. Their emitter run_id must exactly match an available,
already captured typed result, result item, attempt, or unit checkpoint reference.
No new filesystem read, global lookup, history scan, or event-self-reference is
used to establish this association. Row state remains `available`; its `value`
remains the recorded event. The row adds:

```json
{
  "emitter": {
    "scope": "captured_workflow_agent",
    "run_id": "run_worker",
    "reference_record_ids": ["attempt:0", "result:0/items:0"],
    "references_truncated": false
  }
}
```

Reference IDs identify available records in the same immutable capture, including
off-page records. At most 32 distinct IDs are included, in deterministic captured
section/row order; additional references set `references_truncated: true`.
Only syntactically valid ASCII run IDs (`[A-Za-z0-9_-]`, 1–128 bytes) are indexed.
Reference records need not be visible in the current native window. A reference
is display provenance, not a navigation handle or proof of a local file path.

Canonical events with an unqualified emitter, canonical non-dispatch worker
events, and unknown future event variants become:

```json
{
  "record_id": "event:123",
  "byte_offset": "123",
  "state": "unavailable",
  "reason": "event_emitter_unverified",
  "value": null,
  "event_type": "dispatch_started",
  "emitter": {
    "scope": "unverified",
    "run_id": "run_worker",
    "reference_record_ids": [],
    "references_truncated": false
  }
}
```

`event_type` is a nonempty string of at most 128 UTF-8 bytes, or null; it is a
bounded recorded diagnostic, never an admission decision. Unverified `run_id`
is null unless it meets the identifier rule above. No other raw payload is echoed.
A latest capture can omit the relevant older reference; that means unverified,
not a nonexistent relationship and not malformed syntax.

Bad JSON or failed canonical typed decoding remains `malformed`. Partial and
oversized source records retain their existing placeholder shapes. These rows
have no `emitter`, `reason`, or `event_type`. The existing final row-byte limit can
also replace any available event with an oversized placeholder, discarding its
payload and provenance rather than clipping an identity.

## Evidence and resource limits

Only enriched `sections.events` evidence adds required integer counters
`nested_emitter_rows` and `unverified_emitter_rows`, including zero when the file
is missing. `malformed_rows` then counts JSON/typed decoder failures only.
Counters count classifications before final row-size projection; a classified
nested event may subsequently become an oversized placeholder. Existing source
completeness/total semantics are unchanged. Unverified rows also contribute to
header `unavailable_rows`.

The emitter index scans only finite captured rows and checks the existing capture
deadline per row. It contains at most the existing 100,000 captured-row bound and
32 references per run. Its accounted string/entry bytes share the 32 MiB capture
preparation payload budget; event rows remain at most 64 KiB and pages 1 MiB.
The index is discarded after capture preparation; it is not another persistent
cache. These are payload/accounting and cooperative deadline bounds, not a hard
allocator-RSS or kernel I/O deadline guarantee.

## Validation scope

Source-only fixtures cover an authentic-shaped nested dispatch pair; unchanged
root rows; legacy preservation; future/foreign emitters; a latest window with no
captured reference; malformed JSON/typed events; refusal of event-self-proof;
reference cap; unavailable-reference exclusion; bounded diagnostics; and cache
mode isolation. Rustfmt and diff checks are source checks. No build, fixture
execution, service, API, provider, or native GUI acceptance is claimed here.
