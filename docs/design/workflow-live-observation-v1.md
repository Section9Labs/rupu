# Live workflow observation follow-up (design)

This is additive to frozen workflow.observation.pages.v1. New feature:
`workflow.observation.live.v1`. No legacy graph/SSE behavior changes, model grants,
remote calls, running-status inference or service lifecycle changes.

## Cheap change detection

GET `/api/runs/:id/observation/frontier?ws_id=...&host=local[&known_frontier=...]`.
Reuse the exact active/archive/registered-project owner proof. Read bounded owner
and registration records; stat only the six fixed source/log files. Response:
`{schema,run_id,workspace_id,host_id,store_kind,frontier,changed,observed_at,run}`.
`run` is the bounded recorded RunRecord; status is recorded, not inferred process
liveness. `frontier` is an opaque per-AppState salted digest of exact owner,
store kind and per-file existence/device/inode/size/mtime/ctime witnesses.
`changed` compares against optional known_frontier (true when absent). There is
no timestamp/array-count/SSE cursor and no whole-log parse on an unchanged poll.
Metadata observations are not an atomic filesystem transaction or content CAS.
New files, ordinary appends, rewrites, replacement, status changes and resume
writes alter the stamp. Namespace files and symlinks retain no-follow guards.

## Latest bounded window

Capture adds `window=prefix|latest` (default prefix retains existing behavior).
Header/pages echo window. Capture cache reuse must include window. `latest`
reads the last bounded window of each log, including one boundary probe byte,
skips a cut leading JSONL record, keeps exact absolute byte offsets for complete
records, and reports skipped leading bytes/head truncation. A partial trailing
record remains explicit. Unknown omitted totals remain null; source_complete
is true only if the entire underlying file was included and no partial tail.
This reaches new activity beyond an old prefix without unbounded rescanning.
Source and owner remain full bounded reads. Step filtering uses the same finite
immutable index and preserves duplicate attempts.

Each capture returns `frontier` built from the metadata witnesses of the actual
first read of each file. A later append cannot be falsely acknowledged by taking
a newer metadata stamp after reading old bytes. A subsequent frontier poll then
observes the change. Rechecks of identity do not overwrite the initial witness.
An unchanged stamp does not extend snapshot expiry. Explicit recapture handles
expiry/eviction; no hidden retry or fabricated event cursor.

## Exact child read-follow proof

The native selected reader must not rely on an arbitrary raw event path or
absence of terminal data as running authority. A bounded member-proof read will
select one exact cached available record using snapshot, section, optional parent
and step filter, position, and expected record_id. Supported records are results,
items, checkpoints, v1 attempts, and exact-owner AgentStarted events. Other event
shapes are not guessed into child run identities.

The owning workflow is revalidated; selected child run ID/path/step comes only
from that captured member. A no-follow bounded transcript prologue read must
match exact child run ID and workspace, and its path must remain under the
recorded transcript root. The response contains only qualified identity/prologue
metadata, never system prompt or raw unrelated bytes. Remote child records are
explicitly unavailable in this local proof surface. No PID, missing RunComplete,
parent status or event timing is converted into child running status.

Read-follow is a separate policy from lifecycle status: a verified transcript
may be followed while new bytes appear, with unknown status shown honestly.
Existing canonical reader/prologue safeguards remain required. No send, launch,
resume or other execution authority is added.

## Native follow policy

Only an actively followed workflow polls (suggested 2–3 seconds, bounded backoff).
No parser/capture for unchanged frontiers. Changed frontier schedules one debounced
latest capture, without replacing selected transcript, draft, or pinned historical
page. When follow is off or a prior page is being browsed, show new activity rather
than jumping. Pause/navigation cancels follow work. Retain visible finite data on
errors and expose stale/partial/expired state. Do not automatically drain pages,
reroute historical readers, infer membership or manufacture SSE continuation.

## Bounds and validation

Reuse capture byte/row/cache budgets. Frontier/member reads have separate bounded
concurrency, input caps and cooperative deadlines. Metadata/syscall/parser limits
remain qualified; no hard kernel cancellation/RSS guarantee. Tests must cover
unchanged no-log-read detection, new files/status/resume changes, latest activity
past prefix limit, leading/trailing partial rows, absolute offset identity,
window/cursor isolation, cached expiry, exact member/prologue mismatch and read-only
paths. Source review precedes coordinated build/test execution; no live services.

## Frozen frontier/latest wire fields

Frontier request denies unknown fields: `host?:String`, `ws_id:String`,
`known_frontier?:String`. known_frontier when present is exactly `sha256:` followed
by64 lowercase hexadecimal digits. GET route is
`/api/runs/:id/observation/frontier`. Only local host is supported.

Success requires these fields:

- schema: literal `workflow.observation.live.v1`.
- run_id, workspace_id, host_id(local), store_kind(global|global_archive|project_local).
- frontier:String (same format as known_frontier).
- changed:Bool, true without known_frontier or when different.
- observed_at: RFC3339 UTC.
- comparison: literal `metadata_read_time`.
- run: exact bounded recorded RunRecord object, with run.id/workspace_id equal
  to the outer owner. Its lifecycle is recorded, not a child liveness claim.
- input_bytes:Int (0..1048576; two bounded256KiB run reads and, only when needed,
  two bounded256KiB registration reads; no log body reads).

Maximum response1MiB; two concurrent frontier jobs per AppState, immediate429
instead of a queued worker; five-second cooperative deadline, no kernel syscall
cancellation claim. Owner/path/artifact failures use existing400/404/409/413/501;
503 deadline. All IO runs off the async runtime in spawn_blocking. The opaque
frontier is bound to the AppState instance, exact run/workspace/store and first
metadata observations; it is not a bearer token or a content integrity claim.

Existing capture query adds `window` enum prefix|latest, default prefix. Invalid
window is400. Existing success schema remains workflow.observation.pages.v1;
Header adds required `window` and `frontier`. Page adds the same fields, copied
from its immutable capture. New live clients require the new host feature before
using latest; existing unflagged prefix requests retain their prior semantics.
A latest page must match the exact owner/snapshot/source_generation/window/frontier
of its header, as well as section/parent/step_id selectors. Cache reuse includes
window; tokens and HMACs remain capture-bound. Capture/page budgets and TTL remain
unchanged from v3. No global or source freshness is inferred from cached reuse.

Present log section evidence additionally has:

- start_offset:String, decimal absolute byte offset of the first retained byte.
- head_truncated:Bool, true when bytes before this window were not captured.
- leading_skipped_bytes:Int, boundary probe plus cut leading record bytes skipped;
  <=read_bytes. Those bytes are never parsed into an invented event.
- tail_truncated:Bool, whether bytes after the retained window were not read.
- read_bytes:Int includes the boundary probe and skipped bytes (actual content IO).
- prefix_truncated remains true only for an incomplete prefix-mode capture.

Existing malformed_rows/partial_rows/oversized_rows remain separate. Missing
sections retain the old missing evidence shape; they do not acquire fake offset
or zero-history proofs. complete is false when head/tail truncation or a partial
final record exists. source_total is null in these cases. Exact step filter and
unassigned_rows semantics are unchanged; filtered totals cannot become known
because the selected latest window happens to contain zero matching rows.

The member proof has no standalone public native endpoint. It is an internal
sealed authority input to the separate bounded transcript bridge's initial call.
That route accepts captured row selectors rather than a caller-supplied file path.

## Public bounded child transcript route

GET `/api/runs/:id/observation/transcript`. Common query fields are exact `ws_id`,
optional local `host`, required `action` (latest|poll|earlier), optional `limit`
(1..200, default100). Initial requests have action latest and required snapshot,
section, position, record_id, plus optional parent and step_id selectors. position
is the exact captured filtered page offset plus row ordinal, never a stream line
number. Continuations instead provide only the server-issued cursor (<=32KiB)
and common fields; mixing initial selectors with cursor is400. Unknown fields,
including a caller-supplied path, are rejected. No standalone member-proof route.

Initial proof selects an available exact captured record. Supported kinds:
results, items, checkpoints, v1 attempts, and only AgentStarted events with the
exact workflow run_id and explicit child agent_run_id/path/step. Other events,
including UnitStarted and DispatchStarted, are not guessed into child identities.
Items and AgentStarted additionally require matching captured local attempt or
checkpoint attribution: child run, exact path, step and unit/sub-step identity.
A parent or matching attribution explicitly recording a remote host is refused.
Unavailable or absent attribution remains unknown, including when local mirrored
bytes exist. This bounded captured check does not claim unseen histories contain
no conflicts and does not infer running status. A five-second cooperative scan
budget and HTTP cancellation are checked between captured rows; no live history scan is performed. Cancellation also brackets fresh owner/prologue proof.

Fresh owner and prologue checks constrain the file to both its recorded transcript
root and either the configured global root or the exact currently registered
workspace root. External project registration is bounded and rechecked; a path
and matching prologue alone never widen filesystem roots. The required first
RunStart must name exact child run/workspace; host fields must be absent/local.
Only safe metadata is returned in prologue: required run_id,workspace_id,agent,
provider,model,started_at; strings agent/provider/model may be empty but at most
4096 bytes. No system prompt or unrelated raw bytes are returned by proof.

At most four transcript workers are admitted, immediate429 otherwise. The HTTP
future owns a cancellation drop guard and a five-second response timeout; the
blocking worker retains its permit until cooperative work actually exits. A
blocked regular-file kernel read cannot be forcibly cancelled. Fresh member proof
reads at most1.25MiB content (owner/registration rechecks and256KiB prologue), plus
the bridge's separate at-most512KiB anchor and512KiB window. The typed reply and
server cursor contract are specified in transcript-window-follow-v1.md. No model
routes/grants, send authority, SSE cursor conversion or file mutation are added.
