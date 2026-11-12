# Verified transcript window following

Private reader component for `workflow.observation.live.v1`; it is called only
through the workflow member authority route. It introduces no raw-path HTTP
entry point, legacy SSE cursor, model tool or execution grant.

## Authority and actions

The first call obtains an exact VerifiedMember from the owning workflow capture.
Subsequent calls carry an opaque server cursor. MAC verification precedes decoding
and current owner reauthorization; the cursor never bypasses exact workflow,
workspace, child run, path, recorded root or bounded transcript-prologue checks.
The cursor belongs to one AppState's random key/instance. It remains usable after
the observation cache expires because it retains the signed recorded membership;
current workflow owner and transcript prologue are revalidated on every read.
It conveys recorded read authority, never child running status or send authority.

Public parent route: GET `/api/runs/:id/observation/transcript` with exact ws_id
and optional host=local. Initial request supplies the exact captured member
selector (snapshot, section, optional parent/step_id, position, record_id) and
latest action. Continuations supply cursor/action/limit only, apart from ws_id
and host; selectors and cursor are mutually exclusive. No client path exists.
The parent route supplies workflow ID/workspace ID and an internal Query:
`{action: latest|poll|earlier, cursor: string|null, limit: 1..200}` (default limit100).
Only latest accepts an initial verified member without a cursor. No client path
or byte-offset argument is accepted. Cursor maximum32KiB; no legacy page cursor
or Last-Event-ID is accepted. Native byte offsets are display/evidence only.

- latest: read a bounded recent window, initially or after ordinary source change.
- poll: no transcript-body parsing when the FullStamp is unchanged. A changed
  stamp returns state changed as normal control flow; native may request latest
  while following, keeping the visible window until the new window verifies.
- earlier: read a bounded fresh window ending at the signed complete-record
  boundary before the retained window. This works while a writer appends.

## Exact reply

Reply fields:
`{schema,workflow_run_id,workspace_id,child_run_id,host_id,path,instance_id,
file_identity,snapshot,previous_snapshot,state,relationship,observed_at,prologue,
window,records,poll_cursor,earlier_cursor,limits}`.

schema is `transcript.window_follow.v1`; host_id is local. IDs/path/instance are
strings. snapshot and file_identity are opaque server digests. previous_snapshot
is nullable, otherwise echoes the input cursor's snapshot. state is window,
unchanged or changed. relationship is initial, same_snapshot, fresh_latest or
verified_anchor_fresh_window or retained_snapshot. prologue is
`{run_id,workspace_id,agent,provider,model,started_at}`, all required strings;
agent/provider/model may be empty and are capped4096 UTF-8 bytes. started_at is
RFC3339. It never contains system prompt. run_id/workspace_id match the reply. window is nullable for changed/unchanged; records is empty
in those states. poll_cursor is always an opaque string. earlier_cursor is
nullable. Limits are exact integers `{window_bytes:524288,anchor_bytes:524288,
prologue_bytes:262145,records:200,reply_bytes:1048576}`.

State/action matrix:

| Request | state | relationship | snapshot semantics |
| --- | --- | --- | --- |
| latest initial | window | initial | newly verified full stamp |
| latest cursor | window | fresh_latest | newly verified full stamp, possibly same |
| poll unchanged | unchanged | same_snapshot | retained input cursor snapshot |
| poll changed | changed | retained_snapshot | retained input cursor snapshot |
| earlier unchanged stamp | window | same_snapshot | current stamp equals input stamp |
| earlier changed stamp / same anchor | window | verified_anchor_fresh_window | new current stamp |

Poll changed does not issue a new observed window; snapshot and poll_cursor stay
bound to the retained input observation. It reports that current metadata differs,
with the same verified file identity. A latest request creates the new observation.
Replacement or observed truncation is refused even on latest-with-cursor; obtaining
new initial member authority is explicit. previous_snapshot is null only initially.

instance_id is a26-character ULID. file_identity and snapshot are opaque43-character
base64url HMAC digests. Row identity is `sha256:` plus64 lowercase hex; content_sha256
is64 lowercase hex. Cursors are opaque ASCII<=32768 bytes. They are process-instance
bound and carry no time expiry; every use still requires normal HTTP authorization
and fresh exact owner/prologue checks, and process restart invalidates them.

A window contains decimal STRING byte quantities:
`{source_bytes,observed_start_byte,observed_end_byte,display_start_byte,
display_end_byte,omitted_before_bytes,omitted_after_bytes,partial_tail_bytes,
requested_before_byte,complete_source,boundary_available}`.
requested_before_byte is nullable and echoes the server-signed Earlier target.
complete_source and boundary_available are booleans. The actual observed interval
may include a probe byte and bytes outside the displayed row limit. Omitted byte
counts are not omitted event counts; no absolute line ordinal is fabricated.
A cut leading record is never decoded as a complete event. An unterminated live
tail is omitted with explicit partial_tail_bytes (the observed unterminated suffix, not an invented full-record
size). omitted_after_bytes includes the undisplayed suffix, including a partial
tail when it follows displayed records. Empty display bounds collapse to one
observed boundary; they do not assert an event exists. If no complete boundary
is available, boundary_available is false and earlier_cursor is null.
complete_source means display outer bounds equal0..source_bytes; individual rows
may still be explicitly malformed or oversized. Missing earlier context can
split assistant/tool activity; native must present a partial window honestly.

Each record is `{identity,byte_offset,byte_end,status,content_sha256,event}`.
Byte offsets/end are decimal strings; end is exclusive and includes the newline.
status is event, malformed, oversized or scope_mismatch. A later RunStart naming
another child run or workspace has scope_mismatch and null event. Well-formed
unknown event tags retain the existing rupu_transcript::Event forward-compatible
semantics; they never establish ownership. event is nullable. content_sha256 hashes the exact
observed line bytes. identity binds file identity, byte range and this content
hash; cross-capture reuse is allowed only for exact matching identity/content.
No inference about unobserved prefix continuity follows from a matching row.

## File changes and Earlier

FullStamp reuses the existing recorded transcript page stamp: length, mtime,
ctime, creation time and platform file device/inode. snapshot binds that stamp
and exact authority. Metadata is not a whole-file content CAS.

Poll does not label a length increase as proven append. It reports changed, and
native treats this as ordinary bounded-refresh control flow, not an error banner,
reset or jump. It must preserve selected reader, drafts, immutable earlier views
and scroll position. It must not merge arrays merely by byte offset.

Earlier's signed cursor contains a before_byte boundary and the exact SHA256 of
a previously observed interval of at most512KiB. Current owner/prologue and file
identity must still match; current length must not be smaller than the previous
observed file length. The reader rehashes only that bounded anchor, then reads a
new bounded window ending at before_byte. A changed FullStamp is accepted only
when that anchor is identical. The new result explicitly reports
verified_anchor_fresh_window and a current snapshot. This proves the anchor bytes
survived; unseen earlier bytes are a fresh observation and may have been rewritten.
It never promises an unchanged global prefix, atomic multi-window history, or
detection of an unobserved truncate-and-regrow that leaves all checked evidence
identical. Replacement, observed truncation or changed anchor returns409. No
unbounded file copy, prefix scan, absolute line count or persistent index is used.

The FullStamp must remain stable throughout each new read, including the current
prologue and final name/handle checks. A race returns409 without publishing mixed
bytes. Native retains its last verified window and retries bounded work only
while active following or on an explicit Earlier action.

## Errors

Errors retain the parent's `{error:string}` envelope. Stable code strings from
this component:400 transcript_window_cursor_invalid, transcript_window_limit_invalid,
transcript_window_cursor_required, transcript_window_member_required,
transcript_window_mixed_authority, transcript_window_no_earlier_boundary;
409 transcript_window_owner_mismatch, transcript_window_file_replaced,
transcript_window_file_truncated, transcript_window_anchor_changed,
transcript_window_changed_during_read;408 transcript_window_cancelled;
503 transcript_window_work_limit;413 transcript_window_cursor_limit,
transcript_window_io_limit or transcript_window_reply_limit. Owning member proof
errors retain their own explicit400/404/409/413 classifications. A409 during a
race is bounded refresh/retry control flow while following, not permission to
reset selection or convert a cursor. Replaced/truncated/changed-anchor errors
require explicit new authority/reload before adopting changed history.

## Budgets and cancellation

Per request: at most256KiB+1 prologue bytes, at most512KiB prior anchor rehash,
at most512KiB new window input,200 rows,256KiB retained line payload,1MiB serialized
reply. Bytes for the proof read, old anchor and new window are accounted separately
in the caller's total budget. The current member helper owns its additional
bounded workflow/registration reads. No full-file scan occurs, even for large
files. First open seeks near the end before reading the body.

Cancellation is cooperative through CancellationToken before IO, during bounded
chunk reads and between rows. It cannot cancel a blocked kernel regular-file
read. The parent route must cancel on request-future drop and hold its admission
permit until the bounded blocking worker returns. No detached indefinite stream,
background retry, filesystem mutation or provider call is created here.

Source-only fixtures and review are separate from coordinated compilation,
execution, GUI and live workflow acceptance.
