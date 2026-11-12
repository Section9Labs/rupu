# Exact standalone transcript deletion

`transcript.delete.exact.v1` is a positive, local-only host capability advertised
only when the owning CLI adapter implements this contract. Older services lack
the new strict mutation route and return 404. Native clients must never fall back
to the legacy ID-only DELETE route, including after capability discovery, context
expiry, conflict, timeout or connection changes.

## Review and request

GET `/api/transcripts/:id/delete-context?host=local&ws_id=<registered-id>&path=<exact-path>`
prepares a bounded source review. The full canonical `run_<ULID>` ID, exact
registered workspace ID and absolute canonical source spelling are required.
The source is one exact active/archive transcript location in the registered
project or configured global root. Startup CWD, prefixes, codenames, precedence
search and arbitrary paths are not used.

Ready response fields are all required:

- `schema`: `transcript.delete.exact.v1`; `id`, `workspace_id`, `host_id:local`,
  and `path` exactly echo the selected source.
- `state`: ready|unavailable; `reason`: null when ready, otherwise a bounded
  human-readable explanation (not a machine enum).
- `source_kind`: global_active|global_archive|project_active|project_archive,
  nullable when unavailable; `archived`: Boolean, nullable when unavailable.
- `revision`: `sha256:` plus 64 lowercase hex when ready, null when unavailable.
- `observed_at`: RFC3339 UTC.
- `effects`: exactly two ready effects, selected transcript and metadata;
  empty when unavailable. Each has `kind` (transcript|metadata), exact absolute
  `path`, and `bytes` as a decimal string. No global same-ID directory is an effect.
- `coordination`: `advisory_retention_lease_metadata_revision`.
- `limits`: metadata_bytes262144, prologue_bytes262144, effect_entries32,
  context_bytes65536.

The native confirmation presents the exact project/run/source and the two file
removals. It retains the ready context and sends its exact revision only following
an explicit reviewed delete intent. Display names, status badges or credential
presence never substitute for source identity or capability.

DELETE `/api/transcripts/:id/exact-source` requires `ws_id`, `expected_path` and
`expected_revision`, with optional `host=local`. All fields must match the reviewed
context. Query decoding rejects unknown fields; `ignore_liveness` is not accepted.
The reply is `{ok:true,id,workspace_id,host_id:"local",path,revision}` after the
owning command confirms completion. The existing DELETE route and CLI default
behavior remain backward compatible; the new native flow uses only `/exact-source`.
The current legacy route also refuses partial expected-source fields rather than
silently treating them as an unconditional request.

## Eligibility and effects

The new path fails closed unless bounded version 1 standalone metadata names the
exact full run, has no session ownership, records the supported local_checkout
backend, matches the exact registered workspace path, and has a valid recorded
PID whose direct liveness check returns ESRCH (no such process), followed by the
existing liveness guard. Successful probes, permission failures and all other
unknown probe errors refuse deletion. Missing, malformed, future or
ambiguous metadata and unknown/live PID make the action unavailable. Relative
metadata workspace paths are refused. Registered workspace paths must open
without following symlinks; registrations using symlink aliases are unavailable
under this initial exact-delete contract. A bounded
first RunStart must prove the exact run/workspace and local ownership. Existing
session-owned and live-PID guards still run in the owning CLI.

Only the selected `.jsonl` and its sibling `.meta.json` are removed. Same-ID managed
run directories remain untouched. Any same-ID global stream-only directory makes
this initial contract unavailable: its run-only stream prologue cannot prove
workspace ownership, so recursive cleanup or cross-project deletion is refused
before mutation. No session, workflow run, child directory or unrelated same-ID
transcript is deleted. Restoration is not offered by this contract.

## Coordination, revisions and failures

The owning CLI acquires the existing nonblocking retention lifecycle lease before
source capture and holds it through deletion. The configured global root is
resolved once, pinned before lease acquisition and rechecked immediately after
acquiring the lease against that same physical path. The original registered
workspace directory and its canonical root are both pinned and rechecked. Descriptor-relative no-follow opens
pin the exact files and directories; hardlinks, symlinks, unexpected source shapes
and identity changes are refused. Registration, metadata and transcript prologue
are bounded. The revision includes exact scope, pinned identities, sizes/timestamps,
bounded content hashes and retained run-directory disposition. It is not a hash
of the full transcript and not atomic filesystem CAS against arbitrary writers.

The command rechecks ownership, liveness and all source/effect preconditions before
the first unlink. It unlinks the transcript, rechecks metadata/liveness, then
unlinks metadata and syncs the parent directory. The pair is not atomic. A failure
after the first unlink can be partial; a final sync failure leaves durability
unconfirmed. Normal cooperative operations are coordinated by the advisory lease;
old binaries and arbitrary filesystem writers can ignore it. A matching metadata
revision does not promise immunity to such writers.

Conflict or unavailable state 409 preserves the precondition failure; missing
source 404 and unsupported adapter 501 remain distinct. Invalid selectors 400, query
errors, command failures 500 and timeouts are explicit. Failure/timeout is not
permission to retry deletion automatically. Refresh the exact catalog/context and
show the observed remaining state before any new reviewed action.

The adapter uses direct argv without a shell, two admitted child commands, bounded
stdout 64 KiB/stderr 16 KiB and a ten-second response timeout. An owned async task
keeps an authorized command alive when its HTTP waiter is cancelled. On its
execution timeout or output failure, the task requests child termination and
retains admission until the child exits and is reaped; a blocked kernel operation
can occupy that admission beyond the response deadline. Process death or timeout
can still leave an unconfirmed result. No durable exactly-once claim is
made. The read context and mutation routes add no model tool grant.

Source fixtures cover exact-source resolution, duplicate IDs, metadata/liveness,
prologue/registry/version changes, links, bounds/lease, guarded HTTP dispatch,
legacy compatibility, output bounds and cancellation. Execution is coordinated
separately; authored fixtures are not runtime acceptance.
