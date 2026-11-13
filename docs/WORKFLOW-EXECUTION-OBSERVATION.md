# Workflow execution observation v1

Status: implemented and fixture-validated. Runtime deployment and native live
acceptance remain separate; source capability does not imply an installed upgrade.

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
| `revision` | Canonical positive UInt64 decimal string increasing within one generation, or null |
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
join, loop, gate decision and rejection cleanup bodies use explicit activation
scopes. Notification hooks remain inside their parent gate activation: their
legacy synthetic IDs are not unique authored workflow step identities. Parked
human approval and semaphore-queued work are absent. Recursive loop schedulers
share the owner and carry their actual iteration coordinates. Tokio task boundaries explicitly propagate context.

A normal body return removes only that activation. Abnormal return, dropped
future, panic, or abort invalidates the whole generation rather than claiming
that potentially detached child work stopped. Existing cancellation and
remote-dispatch behavior is not strengthened by observation. A manual pause
request does not remove running bodies; acknowledged drain and body exit do.

Cancellation revocation and admission use the same strict mutation protocol.
An explicit terminal-to-running retry clears its cancellation barrier only
while holding that protocol and after proving the prior owner has released
its lifetime lock. Ordinary status changes do not silently clear revocation.
An invalidated generation cannot admit another observation member. A previous
generation's late completion cannot alter a resumed generation. Ownership
teardown revokes current authority; retained data becomes historical evidence.

## Cross-process qualification

Local cooperating Unix schedulers hold a dedicated fail-closed per-run
ownership lock and a unique per-generation lifetime lock. A generation-specific
lock prevents a new owner from making an old saved snapshot look live during
replacement initialization. Mutation and revocation serialize separately.
Producers wait at most two seconds for the mutation lock and never proceed
unlocked; readers use an immediate probe. Ordinary polling contention therefore does not immediately
revoke a producer. A stored producer on a supported platform refuses to start
if its ownership cannot be established, including a conflicting live owner.

Owned lock guards explicitly unlock on normal destruction, even if another
descriptor still references the same open-file description. Failed lock
acquisitions never unlock an incumbent. Abrupt termination cannot run that
destructor: CLOEXEC closes descriptors across exec, but a surviving duplicate
or pre-exec fork can retain the lock until its descriptor closes. A lock
witness is therefore a cooperating lifetime observation, not a process probe.

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
Freshness is based on successful observation receipt, not the age of
`observed_at`: a long-running, unchanged body can retain an old scheduler
publication time while a fresh read witnesses current ownership.

The execution snapshot is limited to 256 members and 256 KiB, with bounded
identifiers and ancestry. Overflow never yields a truncated complete set.
`execution_input_bytes` separately accounts for execution sidecar reads; the
existing frontier `input_bytes` budget remains unchanged. Final header,
frontier, capture, and cache limits include the additive envelope.

Clients compare revisions only within the same generation and exact owner.
A new generation replaces the old live set. Capability absence, malformed or
missing envelope, unsupported validity, incomplete set, stale owner, revision
regression, or contradictory same-revision membership prevents live emphasis.
At an equal revision, witness loss may legitimately downgrade validity without
changing the member array; reject contradictory membership, not that downgrade.
Recorded evidence remains inspectable.

## Validation boundary

Fixture checks use dedicated temporary directories and the production
scheduler, run storage, and observation routes. They cover admitted versus
queued work, concurrent membership and exact removal, iteration coordinates,
pause and gate behavior, abnormal teardown, generation replacement,
cancellation revocation, filesystem constraints, and capture/frontier
coherence. Fixture evidence does not establish real-provider animation,
remote cancellation, deployment, or installed-app acceptance.

Validation on the implemented code:

- `cargo test --offline -p rupu-orchestrator --lib`: 608 tests passed, including
  25 ownership/storage fixtures and 14 scheduler integration fixtures.
- `cargo test --offline --no-default-features -p rupu-cp --lib
  api::workflow_observation::tests`: 50 tests passed, including eight execution
  observation fixtures through the production routes.
- `cargo check --offline --no-default-features -p rupu-cli --all-targets`: passed.
- Scoped Rust formatting and whitespace checks passed.

These checks used Homebrew Rust/Cargo 1.99 on macOS. The repository-pinned 1.95
toolchain was unavailable and is not certified by this validation. PDF, rebuilt
CP web assets, production packaging, and installed/native live behavior were
outside these checks. The pause fixture synchronizes with actual provider-body
entry; its bounded watchdogs do not assume a fixed dispatch latency.
