# Bounded workflow observations

Feature: `workflow.observation.pages.v1` (Unix HTTP operator surface). Existing
`GET /api/runs/:id/graph` is unchanged. This protocol does not add model tools or
change tool grants. Initial scope: a recorded workflow in the connected CP's
active or archived global run store, or the exact requested registered project's
`.rupu/runs` store. No all-project search, remote probe,
mirror fetch, unpersisted run synthesis, bare-agent synthesis or current workflow
fallback. A missing local owner returns 404. `host_id: local` names the connected
artifact owner; it does not assert that every recorded child executed locally.
Recorded child host fields remain intact.

## Capture

`GET /api/runs/:id/observation?ws_id=<exact recorded workspace>&host=local&refresh=true`

`host` may be omitted. `refresh` defaults false; a capture for the exact run and
workspace may be reused for five seconds. Unknown query fields are rejected.
The pinned directory's bounded typed `run.json` must match both identifiers and
contain a nonempty workflow name other than `agent:*`. The owner is mandatory:
it is never replaced by an unavailable-row placeholder. Re-read owner identity
must retain run ID, workspace ID/path, workflow name, start time and transcript
directory. No workspace, session or execution-host relationship is guessed from
names, the selected native project, path similarity or event timing.

After global/archive owning-record misses, only `workspaces/<exact ws_id>.toml` is
read (256KiB maximum, typed Workspace, exact record.id and absolute registered
path). The registered path is walked without symlinks to `.rupu/runs/<id>`; the
run record must still prove the exact requested workspace. The same pinned
registry directory is reread before publication and registration id/path must
match. Both reads count toward the total IO/generation budget. No current-working
directory, inferred repo match or unrelated workspace scan is used. Historical
run.workspace_path is echoed as recorded, never rewritten to the current locator.

Success JSON (all fields required unless explicitly nullable):

- `schema`: literal feature string above.
- `run_id`, `workspace_id`, `host_id`: exact requested/verified owner; host local.
- `snapshot`: opaque string <=64 bytes. This is private to this AppState cache.
- `source_generation`: `sha256:` plus lowercase hex of length-delimited observed
  artifact bytes/names, including owner reads. It is not a filesystem revision.
- `observed_at`, `expires_at`: RFC3339 UTC. Lifetime is ten minutes.
- `freshness`: literal `finite_read_time_capture`.
- `archived`: boolean; reading never restores an archive.
- `store_kind`: global, global_archive or project_local.
- `run`: exact recorded JSON object, validated as RunRecord, <=256 KiB input.
- `workflow_source`: `{state: available|missing|malformed, sha256: string|null}`.
  SHA is plain lowercase hex over exact run-start YAML. No source expansion.
- `sections`: object with `steps`, `edges`, `results`, `checkpoints`, `attempts`,
  `events`. Each value has `state`, `captured_rows`, `complete`, `source_total`.
  The results section additionally has `unassigned_rows`: captured result rows
  whose exact step membership cannot enter the bounded index (unavailable row or
  empty/over1024-byte step_id). Zero does not imply complete underlying history.
  `source_total` is nullable; it counts physical nonblank records, including
  unavailable placeholders, only when the full source was read without a partial
  tail. It never means total valid runs or total execution history.
- Present log section evidence also has `read_bytes`, `file_bytes_at_read`,
  `prefix_truncated`, `malformed_rows`, `partial_rows`, `oversized_rows`.
  Missing log sections have state `missing`, complete false, source_total null
  and zero captured/malformed/partial/oversized rows. A missing log is not proof
  that there were no runs. Empty existing files are complete with zero records.
- `unavailable_rows`: records replaced for serialized row-size limits (separate
  from malformed/partial/oversized input-line evidence).
- `captured_rows`: total rows across sections, including all children.
- `input_bytes`: actual bounded bytes read, including both owner reads.
- `limits`: page_bytes, row_bytes, file_bytes, input_bytes, capture_bytes,
  capture_rows (integers).
- `membership`: `recorded_evidence_only; transcript_prologue_verification_required`.
- `session_linkage`: `not_inferred`.

`complete` means the entire bounded artifact source was read, not that all its
rows were valid, that the workflow is finished, or that all history exists.
Source-size truncation and unknown omitted counts remain visible even when a
client reaches the final captured page. Capture reads files at separate times;
it is not an atomic coordinator snapshot. Files changing size/mtime/ctime during
a read cause 409. Later live appends do not mutate an already captured page.

## Pages

`GET /api/runs/:id/observation/page?ws_id=...&host=local&snapshot=...&section=...&limit=50[&parent=...][&step_id=...][&cursor=...]`

Allowed sections: steps, edges, results, checkpoints, attempts, events, items,
findings. `items` and `findings` require exact `parent=result:<decimal byte offset>`;
all other sections reject parent. `limit` is 1..100 (default50). Cursor is opaque,
max128 bytes. HMAC binds run, workspace, random snapshot/key, section, parent and
position and optional exact step_id. Reusing it for another parent, step filter
or capture fails. Only results accepts optional `step_id` (1..1024 UTF-8 bytes,
exact equality without normalization); other sections reject it. A captured
step-to-result index selects matching rows without reparsing/scanning live files
or draining chronological pages. Duplicate attempts retain their original order
and record IDs. Child pages use parent only, without step_id. Other off-page
membership is explicitly not loaded; clients must not automatically drain every
page or fall back to the whole graph.

Page response:
`{schema,run_id,workspace_id,host_id,snapshot,source_generation,observed_at,expires_at,section,parent,step_id,offset,captured_total,source_complete,source_total,unassigned_rows,records,next_cursor}`.
`parent`, `step_id` and `next_cursor` are nullable strings. `offset` and `captured_total` are
bounded integers counting this captured section. `source_complete/source_total`
match header evidence, or true/exact child count for the selected fully parsed
parent result. These do not claim the individual payload rows are available.
For a step-filtered results page, captured_total counts known matching captured
records and source_total is that matching count only when the entire underlying
source was read and unassigned_rows is zero; otherwise source_total is null.
source_complete still describes the underlying artifact read, so it can be true
with unknown filtered total. Page unassigned_rows equals the header result count
for results (filtered or not), and is zero for other sections. An empty matching
page with unassigned_rows>0 does not prove that this step had no runs.
Reaching next_cursor null means end of this capture only.

Each row has `record_id`, `state`, `value`. State is available, malformed, partial
or oversized. Unavailable value is null and grants no membership/navigation.
Steps/edges have index and IDs `step:<index>` / `edge:<index>`, carrying the existing
StepNodeDto/EdgeDto from the recorded source. Logs have `byte_offset` as a decimal
STRING (never a floating-point JSON number). IDs are result/checkpoint/attempt/
event plus colon and that offset. Order is source byte order; duplicate step IDs,
retries, unmatched and malformed rows are not deduplicated or silently omitted.
Offsets are meaningful only under this exact owner and immutable snapshot.

Available result rows contain `value`: the recorded StepResultRecord object with
`items` and `findings` removed, plus envelope `omitted_fields:["items","findings"]`,
`payload_state:"included"`, `items_count`, `findings_count`, `children_complete:true`.
If that summary exceeds64KiB, the present payload fields `output`, `rendered_prompt`
and `run_outcome` are also removed and their exact names appended to omitted_fields;
payload_state becomes `omitted_oversized`. This preserves navigable child membership
when a fanout aggregate output is large. Other metadata/identity fields are never
clipped or removed; if the reduced summary still exceeds64KiB, it becomes an
unavailable parent placeholder and child pages return404. There is no hidden
payload fetch or claim that the omitted aggregate can be reconstructed from child
transcripts. Clients show the omission explicitly. All legacy omitted fields
remain omitted. Do not feed this summary to a decoder that treats omitted
findings as an empty full result or otherwise claims complete result details.
Children are always separate pages, including when only one item exists.

Child rows have `parent_record_id`, `index` (array position), and IDs such as
`result:123/items:0`. `value` is the exact ItemResultRecord/FindingRecord object.
Array position is not a replacement for ItemResultRecord.index: both are kept.
No child page is valid without the exact parent captured result and independent
transcript proof. Empty child collections are available with zero records. If the parent summary
itself exceeds the row bound it becomes unavailable and both child sections are
absent (404), even if the underlying children could individually fit.

Checkpoint and attempt values preserve their recorded raw fields; attempts must
use supported ledger version1. Byte offset is not the legacy attempts route's
ledger-line or attempt ordinal. Event values must parse as a known Event with
exact owning workflow run_id; foreign/unknown events become malformed evidence,
never member proof. Event-only panel agents remain visible as raw event evidence;
this protocol does not perform a lossy step/index deduplication or fabricate
checkpoints. Client must respect incomplete event/checkpoint evidence. Existing
transcript reader/project/prologue checks remain necessary before opening a run.

## Resource and lifecycle bounds

One capture worker per AppState; additional simultaneous capture reads return429
without queueing. At most four concurrent page preparations are admitted (429
when busy); this is not a global HTTP response-body allocation quota. All file IO runs in spawn_blocking. One pinned, descriptor-opened
run directory and no-follow regular-file reads prevent FIFO and symlink traversal.
64MiB total actual input IO,16MiB per log,8MiB maximum parsed JSONL row,1MiB source,
256KiB owner,4096 declared steps,65536 projected edges,100000 total captured rows,
32MiB total serialized capture plus step-index key/index bytes (including header),
64KiB serialized row,1MiB response.
An oversized output row becomes an identity-only unavailable placeholder; its
fields are not silently clipped. An incomplete newline tail is an explicit row.
Logs may capture only a finite prefix; unknown omitted record totals remain null.

The source and individual parsed rows are input-bounded; JSON/YAML parsing and
DAG construction still allocate transient objects. These are not advertised as a
strict process RSS bound. A five-second cooperative work deadline is checked
between file/row work; it cannot cancel a blocked regular-file kernel read or
interrupt an individual parser. No claim of paging alone bounding capture.

At most four cached captures /64MiB serialized cache payload plus index bytes;
FIFO eviction.
Vec/map/index overhead is additionally bounded by100000 records per capture.
TTL cleanup is lazy on the next capture/page request; cache is released with the
AppState. Eviction/expiration/restart produces409 and requires explicit recapture.
No disk cache, background services, durable state writes or automatic restarts.

Errors use existing `{error:string}`:400 invalid selectors/nonworkflow;404 owner
or exact parent unavailable;409 unsafe/racing artifacts or stale/wrong cursor;
413 source/header/aggregate row/byte/DAG limits;429 capture busy;503 cooperative
work deadline. Existing local-host guard returns its established unsupported
host response. Clients retain already displayed finite pages on read failure;
feature-supported failures never trigger legacy wholegraph fallback.

## Validation scope

Authored fixtures cover owned/mismatched/remote/archived reads, giant fanout split,
duplicate result identity, foreign events/unsupported attempts, malformed/partial
rows, prefix budgets, row placeholders, exact parent/cursor binding, immutable
pages after appends, cache lifecycle and read-only anchored input. Fixtures are
source-only until the coordinated warm owner records execution evidence.
