# Board work queue v1

Status: implemented backend contract for review and native integration; not deployed.

The Board needs a durable work item that connects a request, its pickup, the
execution that reports progress, and its result. Existing posts, directives,
messages and expiring claims remain separate records. None is migrated or
interpreted as a task by this change.

## Scope and capability

`board.work_queue.v1` describes the local queue read/schema contract.
`board.work_queue.mutations.v1` additionally advertises an installed mutation
service. Each owner response reports its actual allowed actions. Clients must
check both the server capability and those owner-specific actions.

V1 records work and supports an agent explicitly picking it up. Posting,
claiming, cancelling and requeuing do not launch, interrupt or stop an agent.
Every mutation receipt says `execution_effect: "none"`. Automatic dispatch,
operator assignment to a worker, scheduling and lease expiry are outside v1.
An agent must explicitly call the queue tools to report work; run-ledger
`Started`, roster presence, lease release and process exit never imply a queue
transition.

The same store serves agent tools and the authenticated operator API. Queue
tools are coordination bookkeeping (`Effect::Record`), deliberately available
in readonly permission mode when granted; they confer no workspace write,
external action or spawn permission. Observation-only/archived contexts cannot
mutate. Execution remains subject to the normal tool permission policy.

## Durable identity and lifecycle

An item has an immutable ID, creation sequence, title, instructions, creator,
posting time, current state, item revision and an ordered attempt history.
The queue has a persistent ID and a monotonically increasing revision. Reads
of a missing store return `availability: "not_initialized"`, revision zero,
and no queue ID without creating files. This says nothing about legacy work.

States are exactly `queued`, `claimed`, `working`, `blocked`, `completed`,
`failed`, and `cancelled`. A successful claim mints a new `work_attempt_id`.
The claimant's exact execution identity is immutable. Start records the
executor separately; v1 requires the executor to be the claimant execution.

| Command | Allowed current state | Actor and effect |
| --- | --- | --- |
| post | no item | Operator or granted agent; creates queued item |
| claim | queued | Agent self-claim; creates claimed attempt |
| start | claimed | Same claimant execution; records working executor |
| block | working | Same executor; blocked with nonempty reason |
| resume | blocked | Same executor; returns to working, same attempt |
| complete | working or blocked | Same executor; completed with result summary |
| fail | claimed, working or blocked | Same claimant/executor; failed with reason/result |
| cancel | any nonterminal | Operator; cancelled, without stopping execution |
| requeue | claimed, working, blocked or terminal | Operator; closes any open attempt and returns the same item to queued |

Requeue preserves earlier outcomes, source references and attempts. A later
claim creates a different attempt; a stale executor cannot finish the new
attempt. Completing an item records the executor's reported outcome, not an
independent verification. A failed process leaves unresolved work until an
explicit transition; no timeout silently rewrites history.

Agent tools expose post/list/get/timeline, claim/start/block/resume/complete/fail.
Operator API/MCP actions expose post/cancel/requeue. The operator cannot submit
an agent identity, choose an executor, claim on behalf of a run, or forge a
worker result. Agent release/reassignment can be added later with an explicit
authority policy; v1 agents ask the operator to requeue abandoned work.

## Execution and source references

The runtime constructs trusted execution context from the assembled launch,
not tool arguments. It includes run ID, agent name, participant address,
canonical queue owner, exact recording identity, and available origin fields
such as workflow step/unit or Agentiflow lead round. A lead's reused run ID
does not identify its round. Each invocation contributes runtime turn/call
indices and the recorded tool-call ID, so repeated model IDs are distinguishable.

Each assembled execution also has a fresh runtime-minted `execution_id`,
recorded with its Bus join. New mutation authority checks that exact ID in the
current participant slot, in addition to run/recording identity. This fences
an earlier lead round even though the flow reuses its run ID. Historical
receipt replay does not require the execution still to be current.

The queue validates that the joined Bus member matches this execution and
that the current participant slot has not been replaced. The message-space
lock precedes the queue lock, serializing this check with join/leave. A new
launch at the same address, or a later lead round, cannot inherit ownership.
Owner and execution context are bound at assembly; a guessed path or a
continuation symlink alone cannot establish authority. Supported launch placements
and explicit unavailable cases are listed in
[the runtime binding contract](board-work-queue-runtime-v1.md).

The runtime records a `work_queue_execution` Notice immediately after RunStart.
Its message is the serialized trusted WorkActor. Source resolution checks that
immediate marker, the run/agent identity and recorded owner membership; it never
searches for a later matching marker. Available targets include `execution_id`.
Clients must retain and revalidate it against the loaded raw recording before
opening the conversation, and keep this internal marker out of conversation text.
Missing or mismatched markers make the source unavailable without discarding the
recorded Queue result.

Mutation lock order is retention lifecycle lease, message-space Bus lock, then
SQLite transaction. The lease remains held through commit so supported archive
operations cannot move the owner between validation and publication. Current
roster authorization uses bounded strict parsing; malformed later joins/leaves
cannot expose an older participant as current. Exact execution IDs also fence
stale leave events from an earlier round.

Each event is itself a durable source reference. An agent result links to the
exact execution/recording and call that reported it. Operator posts have no
invented sending run or transcript. Message notification is not part of the
queue transaction; v1 does not fabricate a Board post or message ID. Result
text is stored in the immutable queue event, independently of transcript
availability. Arbitrary model-provided file paths, URLs and message IDs are
not accepted as canonical evidence.

Read responses retain recorded source references if their underlying recording
is missing. Conversation resolution must validate owner/run/recording identity
and return availability separately. No fallback to a different run, latest
round or current session turn is permitted. A successful queue receipt does
not prove that the transcript writer subsequently recorded the tool output.

## Atomic mutations and retries

The queue uses an indexed SQLite database under the owner's Board. One
`BEGIN IMMEDIATE` transaction commits the item update, attempt update, timeline
event and idempotency receipt together. Queries and mutations do not load or
rewrite the entire queue. The initial storage mode is rollback journal with
`synchronous=EXTRA`, bounded busy timeout, and platform full-sync where supported.
Safe opens reject links/special files and unexpected sidecars. No queue
correctness depends on best-effort legacy journals or mailbox delivery. This
is a locally owned store, not a hostile-filesystem sandbox.

First initialization builds, commits, closes and syncs a private database in the
same directory before publishing it with a no-replace rename and directory sync.
An interrupted initialization cannot expose an incomplete final schema. Existing
malformed final databases are refused and never silently initialized again.

Commands carry `request_id`, `expected_queue_id`, and `expected_revision`.
The expected queue ID fences replacement/reinitialization of a database inside
the same owner; null is allowed only for a post into an uninitialized queue.
Post compares the queue
revision; all other commands compare the target item revision. IDs are bounded
opaque strings. The idempotency scope is the exact queue owner and actor
execution; the request digest includes the canonical operation and expected
revision. Invocation-specific transcript positions are recorded with the first
commit and do not make a transport retry a different operation.

Order under lock:

1. Validate exact owner and caller identity and command shape.
2. Look for the actor/request receipt. Identical request returns the immutable
   receipt; changed payload conflicts. Replay precedes current-revision and
   current-roster checks, so a lost successful reply remains recoverable.
3. For a new mutation, revalidate active owner and current execution authority,
   expected revision, attempt ownership, legal transition and capacity.
4. Commit all records atomically with checked revision increments.

Replies contain `receipt`, `replayed`, `current_item`, and `queue_revision`.
The receipt includes applied queue/item revisions and event/attempt IDs. On a
replay those historical revisions remain unchanged; `current_item` describes
the state actually read now. This and item GET are bounded projections of the
current item/attempt/result; historical attempts and results are paginated
through the timeline. A retry must never repaint a historical receipt
as the current item state.

After an ambiguous durability error, return an explicit reconciliation error
and retain the request ID. The caller retries the identical command with the
same ID. No receipt/history garbage collection occurs in v1.

Bounds apply to individual records and responses, not a fixed lifetime item
count. Title is at most 256 UTF-8 bytes, instructions 16 KiB and transition/result
text 8 KiB. Queue/item/timeline responses include machine-readable capacity:
used bytes, configured storage limit, recorded item/event counts and admission
status. The initial database size ceiling is 2 GiB, explicitly exposed; new
admission preserves room for final transitions on unresolved items. Actual
filesystem exhaustion can still fail a write and must remain an explicit error.
Receipts and historical attempts are never silently purged. Indexed pagination
and stable wire identities support thousands of items and a later storage
implementation without a client schema change.

Cancel and requeue replies expose `prior_attempt_may_still_run` when applicable,
retain the earlier attempt's conversation reference and never assert worker
termination. A new queued item may therefore coexist with an earlier executor
that has not stopped; the client must show that distinction.

## HTTP contract

Prefix: `/api/{sessions|runs|agentiflows}/:owner_id/board/work`.
Host must be absent or `local`. Nonlocal host requests return 501 without an
HTTP/SSH/helper fallback. Authentication is the existing CP policy; an owner
token is an optimistic precondition, not a credential.

| Method | Path suffix | Operation |
| --- | --- | --- |
| GET | empty | Current queue page |
| POST | empty | Operator post |
| GET | `/:work_id` | Current item and allowed actions |
| GET | `/:work_id/timeline` | Item's immutable event page |
| POST | `/:work_id/transitions` | Operator cancel or requeue |

Writes require `owner_token`, `request_id`, `expected_revision` and a typed
operation. Body actor/executor fields and unknown fields are rejected. Reads
include `{route, kind, id, host, token, archived, writable, allowed_actions}`.
Active idle sessions can accept posts. Archived owners are readable and never
writable; old active-owner tokens cannot write a moved or replaced directory.
Completed workflow/standalone/Agentiflow owners are read-only. Runtime agents
commit their own result while their exact launch remains authorized.

A standalone CLI run creates its terminal run record after execution. An identical
operator request committed before that record existed remains recoverable through
a narrowly validated historical token: the same directory identity, a complete
terminal standalone record and the recorded root participant must agree. This path
can only recover an existing receipt; it cannot initialize a Queue or admit new
work. Changed retry payloads still conflict.

Owner records are bounded at 16 MiB and read through anchored regular-file handles.
Replacement during a read is refused; legitimate atomic session saves between
reads preserve the immutable owner identity. An oversized record is explicit
unavailability or a capacity error, never an empty Queue.

Queue filters initially support a state and claimant run ID. Queue order is
descending creation sequence; timeline order is ascending event sequence.
The default limit is 50, maximum 200, and serialized responses are capped at
1 MiB. Every page includes normalized filters, ordering, source revision,
`next_cursor`, `has_more`, and counts with
`scope: "matching_filters_at_source_revision"`, returned and matching total.
Item summaries omit full history/results; item and timeline reads retrieve
the details. Counts never mean only the loaded page. The byte budget may reduce
the number of whole records returned below the requested limit; the cursor
follows the last complete emitted record. Text is not truncated. A single
record that cannot fit with its envelope produces an explicit 413.

Opaque HMAC-authenticated cursors bind endpoint, owner instance, queue ID,
source revision, exact filters and position. Queue changes invalidate queue
cursors with 409. Timeline cursors may bind item revision, avoiding invalidation
from unrelated item changes. Restart invalidation and changed snapshots are
explicit refresh outcomes. A request cannot reuse a cursor for another owner,
filter, item or endpoint.

Errors distinguish invalid input (400), authentication/actor denial (401/403),
missing owner/item (404), owner/revision/attempt/idempotency/transition conflicts
(409), bounds (413), unsupported host/service (501), and persistence failures
requiring reconciliation (5xx). Error codes and retry/reconcile guidance are
machine readable: `{error: {code, message, retry: "none" | "refresh" |
"same_request", request_id?}}`. An uncertain commit uses `same_request`;
revision/owner/cursor conflicts use `refresh`. Nothing reports a successful
result before the queue commit.

Metadata, counts, capacity and returned rows are read in one database read
transaction. A legitimate hot rollback journal may require controlled writable
recovery; archived read-only access reports that need explicitly instead of
deleting the journal, recreating storage or returning an empty queue.

Source resolution is separate from stored provenance:
`{recorded, availability: "available" | "unavailable" | "unsupported",
reason?, transcript?}`. Available transcript targets are emitted only after
validating the exact owner/run/recording relationship. Missing transcript
bytes do not change the committed queue result or silently clear its locator.
Trusted configured-root aliases and this exact owner's active/archive move are
normalized consistently; unrelated prefixes and descendant symlinks are refused.
The bounded source resolver proves at most 16 captured launch roots per response;
sources that exceed its proof budget remain explicitly unsupported.

## Validation and native handoff

Storage tests must cover competing claims, stale revisions/executions, separate
rounds with the same run ID, replay after state advancement/restart, changed
retry payload, requeue and attempt history, capacity reserves, malformed/link
stores and transaction rollback and uncertain commit outcomes. Tools must prove authority comes from runtime,
not model input; current grants and readonly semantics require regression tests.
HTTP tests must cover authentication, all owner families, archive reads/write
refusal, path/host/route mismatch, cursor binding, count scope, bounds and exact
source references. Runtime tests must prove the recording/round and invocation
indices actually reach tool calls.

These tests use isolated stores through the production APIs. They do not claim
real-provider execution, native UI acceptance, live deployment or a release.
Native integration begins only after this version/schema is agreed with the
design owner, and after the backend PR's exact source is available.

## Storage references

The transaction boundary follows [SQLite atomic commit](https://www.sqlite.org/atomiccommit.html)
and [rusqlite transactions](https://docs.rs/rusqlite/0.42.0/rusqlite/struct.Transaction.html).
Durability settings follow [SQLite synchronous documentation](https://www.sqlite.org/pragma.html#pragma_synchronous).
The implementation must query effective settings rather than assume unsupported
PRAGMAs were accepted.
