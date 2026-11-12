# Workflow execution observation v1

Status: implementation and fixture validation in progress. No deployment is
implied by the capability's presence in source.

The workflow observation API already captures recorded execution evidence.
The additive `workflow.observation.executing_steps.v1` capability identifies
which scheduler step bodies have entered and have not left at an observed
revision. A `StepStarted` event may precede semaphore admission, and the legacy
`active_step_id` is singular. Neither is authority for this set.

## Meaning and wire contract

Observation headers and frontier responses gain a top-level `executing_steps`
envelope. The existing six observation sections, immutable page protocol, and
MCP route exclusions remain unchanged.

The envelope has these fields:

| Field | Meaning |
| --- | --- |
| `schema` | `workflow.observation.executing_steps.v1` |
| `scope` | `scheduler_step_bodies` |
| `run_id`, `workspace_id`, `host_id` | Exact validated owner; supported host is `local` |
| `owner_started_at` | Recorded owner identity, in addition to anchored directory validation |
| `execution_generation` | Opaque scheduler ownership generation, or null when unavailable |
| `revision` | Canonical decimal string increasing within one generation, or null |
| `observed_at` | Scheduler snapshot time, never a substituted HTTP response time |
| `validity` | `current`, `stale`, or `unavailable` |
| `complete` | Whether the captured member array is complete for this scope |
| `reason` | Bounded qualification reason, or null |
| `members` | Bounded atomic array of activations |

Each member contains an opaque `activation_id`, exact `step_id`, `entered_at`,
and ordered `iterations` of `{loop_id, iteration}`. Workflow loop iterations
are zero-based. Coordinates come from the active scheduler scope, not names,
completed loop progress, transcript offsets, or panel display indexes. This
version does not fabricate per-agent, provider, unit, or attempt identities.

Live graph emphasis requires both `validity == current` and `complete == true`.
Current means that the reader witnessed the exact owning generation at its
read point. It is not a lease that guarantees future execution. A body may
be preparing work, waiting for machine admission, awaiting a tool/provider,
or collecting child results. It does not prove token generation or remote
provider liveness. Concurrent aggregate children are not separate step-body
members in this version.

## Producer lifecycle

Each `run_workflow` ownership invocation, including resume, establishes a new
generation after the recorded owner is available. Gate re-entry within that
invocation preserves the generation. Standalone rejection cleanup establishes
its own generation; recorded terminal status alone is not proof of no work.

Ordinary DAG activation begins after its semaphore permit is acquired. The
legacy sequential driver uses the same body boundary. Inline branch, split,
join, loop, gate decision, notification and rejection cleanup bodies use
explicit activation scopes. Parked human approval and semaphore-queued work
are absent. Recursive loop schedulers share the owner and carry their actual
iteration coordinates. Tokio task boundaries explicitly propagate context.

A normal body return removes only that activation. Abnormal return, dropped
future, panic, or abort invalidates the whole generation rather than claiming
that potentially detached child work stopped. Existing cancellation and
remote-dispatch behavior is not strengthened by observation. A manual pause
request does not remove running bodies; acknowledged drain and body exit do.

Cancellation revocation and admission use the same strict mutation protocol.
An invalidated generation cannot admit another observation member. A previous
generation's late completion cannot alter a resumed generation. Ownership
teardown revokes current authority; retained data becomes historical evidence.

## Cross-process qualification

Local cooperating Unix schedulers hold a dedicated fail-closed per-run
ownership lock and a unique per-generation lifetime lock. A generation-specific
lock prevents a new owner from making an old saved snapshot look live during
replacement initialization. Mutation and revocation serialize separately.

The reader opens bounded regular files relative to the already validated run
directory, validates file identity, and probes that exact generation's lock.
PID existence and the legacy run JSON lock are not liveness proofs. Failure to
persist a complete state, malformed identity, lock errors, or limits revoke
authority. Missing legacy data is unavailable; archived or dead ownership is
stale. Mirrored and unsupported placements cannot be promoted to current.

This is a cooperating-writer protocol. It does not provide protection against
arbitrary filesystem writers or old producers that do not participate. It
does not grant transcript access or change the existing owner authorization.

## Coherence and bounds

The reader freezes one execution envelope. Capture header membership, source
generation, and frontier contribution reuse that value. Execution-only changes
and validity loss change the frontier without requiring a transcript append.
Query-time timestamps do not change stable comparison identity.

A cached capture remains point-in-time evidence, including its original
execution observation. The API must not splice a fresh execution set into an
older capture. Live clients request `refresh=true` and use fresh frontier
observations; failed polls or transport loss downgrade live emphasis.

The execution snapshot is limited to 256 members and 256 KiB, with bounded
identifiers and ancestry. Overflow never yields a truncated complete set.
`execution_input_bytes` separately accounts for execution sidecar reads; the
existing frontier `input_bytes` budget remains unchanged. Final header,
frontier, capture, and cache limits include the additive envelope.

Clients compare revisions only within the same generation and exact owner.
A new generation replaces the old live set. Capability absence, malformed or
missing envelope, unsupported validity, incomplete set, stale owner, revision
regression, or contradictory same-revision membership prevents live emphasis.
Recorded evidence remains inspectable.

## Validation boundary

Required fixture checks use dedicated temporary directories and the production
scheduler, run storage, and observation routes. They cover admitted versus
queued work, concurrent membership and exact removal, iteration coordinates,
pause and gate behavior, abnormal teardown, generation replacement,
cancellation revocation, filesystem constraints, and capture/frontier
coherence. Fixture evidence does not establish real-provider animation,
remote cancellation, deployment, or installed-app acceptance. Executed checks
and their limits will be recorded before publication.
