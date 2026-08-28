import Foundation
import Observation

/// The mutating verbs this store's write path supports. Every verb below
/// is a fire-and-forget POST from the caller's perspective — the server's
/// 200 means "recorded", not "done" — so each one gets a `PendingActions`
/// entry that either self-confirms off an observed status transition
/// (`resolve(runID:observedStatus:)` — approve/reject/cancel/pause/resume)
/// or waits for an explicit `confirm(_:)` from the call site once its own
/// effect is visible another way (archive/restore/send/launch: a 2xx
/// response body, or a navigation that already implies success).
public enum ActionVerb: String, Sendable, Hashable {
    case approve, reject, cancel, pause, resume, archive, restore, send, launch
    /// Phase 5A, Task 6 addition: `FleetStore.removeHost(id:)`. Entity-scoped
    /// to a host id, not a run id — `resolve(runID:observedStatus:)` below
    /// never touches it (there is no `ActivityStatus` for a host), same
    /// "never resolved by status" bucket as `archive`/`restore`/`send`/
    /// `launch`. Its own confirmation path is bespoke: `FleetStore.
    /// applyHosts(_:)` confirms it directly once a fresh `/api/hosts` batch
    /// no longer contains the removed id — see that method's doc comment.
    case remove
    /// Phase 5A, Task 7 addition: `LibraryStore.setAutoflowEnabled(...)`.
    /// Entity-scoped to a definition, not a run id — same "never resolved by
    /// status" bucket as `archive`/`restore`/`send`/`launch`/`remove`.
    /// **Always built via `ActionKey.autoflow(name:scopeKind:scopeID:verb:)`**
    /// (review fix, round 1) — never a plain `ActionKey(name, .setEnabled)` —
    /// see that method's doc comment for why a bare name collides across two
    /// repos defining the same autoflow name.
    ///
    /// **Confirmed immediately, not on a later refetch** (unlike `.remove`'s
    /// "row disappearing IS the confirmation" contract): `POST /api/
    /// autoflows/:name/enable|disable`'s response body IS the on-disk file's
    /// actual new state (`AutoflowSetEnabledResponse.enabled`), not a
    /// recorded-but-not-yet-applied marker — see `CPClient.
    /// setAutoflowEnabled`'s doc comment. `LibraryStore.setAutoflowEnabled`
    /// calls `confirm(_:)` directly off that response, same as `archive`/
    /// `restore`/`send`/`launch` already do for their own response-visible
    /// effects.
    case setEnabled
    /// Phase 6B, Task 3 addition: `ClaimsStore.release(issueRef:)`. Entity-
    /// scoped to an issue ref (`ActionKey(issueRef, .release)`, no composite
    /// needed — see `ClaimsStore`'s doc comment on why `issue_ref` is already
    /// globally unique). Same "never resolved by status" bucket as every
    /// other verb below — a claim has no `ActivityStatus` at all.
    /// `POST /api/autoflows/claims/release` is idempotent (`released: false`
    /// for an already-untracked issue is still a confirmed 200, not a
    /// failure), so this confirms on ANY successful response, not just
    /// `released: true` — see `ClaimsStore.release(issueRef:)`'s doc comment.
    case release
    /// Phase 6B, Task 3 addition: `ClaimsStore.requeue(issueRef:)`. Same
    /// entity-scoping as `.release` above (`ActionKey(issueRef, .requeue)`).
    /// `POST /api/autoflows/claims/requeue` only enqueues a manual wake — it
    /// has no "done" signal to wait for at all (the wake itself is consumed
    /// asynchronously by whichever autoflow worker picks it up next), so this
    /// confirms as soon as the request itself succeeds, same as `.release`.
    case requeue
    /// Workflow Builder (macOS design plan) Task 9 addition:
    /// `RupuBuilder.BuilderStore.save()`. Entity-scoped to the workflow name
    /// being written (`ActionKey(graph.meta.name, .save)`) — same "never
    /// resolved by status" bucket as every other verb above except approve/
    /// reject/cancel/pause/resume: a workflow definition has no
    /// `ActivityStatus` at all. `PUT /api/workflows/:name` is synchronous
    /// server-side (200 = written to disk, same contract as
    /// `putConfigGlobal`/`putConfigProject`/`putConfigPolicy` — see
    /// `CPClient.writeWorkflow`'s doc comment), so `BuilderStore.save()`
    /// confirms directly off the response, same as `.setEnabled` does for
    /// its own immediate write.
    case save
}

/// Identifies one in-flight mutation: which entity (a run ID today; any
/// string-identified entity in general) and which verb. Two verbs against
/// the same entity — e.g. a gate's `approve` and a separate `cancel` a user
/// fires in quick succession — track fully independently, each keyed by its
/// own `ActionKey`.
public struct ActionKey: Hashable, Sendable {
    public let entityID: String
    public let verb: ActionVerb

    public init(_ entityID: String, _ verb: ActionVerb) {
        self.entityID = entityID
        self.verb = verb
    }

    /// **Composite-entity convention for gate verbs** (Phase 3, Task 5 fix
    /// round 1): `approve`/`reject` target one specific parked gate, not
    /// "the run" — a run can have more than one gate awaiting at once
    /// (`RunRecord.awaiting` is a set), and `RunDetailScreen`'s banner
    /// renders one control pair per gate, all at once. A plain
    /// `ActionKey(runID, .approve)` collided across gates: approving gate A
    /// spinnered/disabled gate B's controls too (same key, same state), and
    /// then silently un-disabled them the moment gate A's own confirmation
    /// landed — B's own mutation was never actually tracked.
    ///
    /// The fix is this composite `entityID`: `"\(runID):\(stepID)"`, built
    /// only via this helper so every call site (`RunDetailStore`/
    /// `ActivityStore`'s `approve`/`reject`, `RunDetailScreen`'s banner,
    /// `ActivityTable`'s compact row) agrees on the exact same string.
    /// **Every other verb stays run-scoped** (plain `ActionKey(runID, verb)`)
    /// — cancel/pause/resume/archive/restore all act on the run as a whole,
    /// with no per-gate ambiguity to disambiguate.
    ///
    /// `resolve(runID:observedStatus:)` below treats any key whose
    /// `entityID` carries `runID` as this `"\(runID):"` prefix as belonging
    /// to that run too, alongside an exact `entityID == runID` match — so
    /// one observed run-level status transition still resolves every gate's
    /// pending `approve`/`reject` for that run in one pass (a run leaving
    /// `.awaiting` means every gate that was blocking it got settled one way
    /// or another, whichever gate a given key names).
    public static func gate(runID: String, stepID: String, verb: ActionVerb) -> ActionKey {
        ActionKey("\(runID):\(stepID)", verb)
    }

    /// **Composite-entity convention for `.setEnabled`** (Phase 5A, Task 7
    /// review fix, round 1) — same class of bug `gate(runID:stepID:verb:)`
    /// above already fixed once for approve/reject: a plain
    /// `ActionKey(name, .setEnabled)` collides whenever two different repos
    /// define an autoflow with the same `name` (a legitimate, unremarkable
    /// case — `LibraryStore.applyAutoflowEnabled`'s own data patch already
    /// matches `name` + `scopeKind` + `scopeID` together for exactly this
    /// reason, per that method's doc comment). Without this, toggling repo
    /// A's row also showed repo B's same-named row as pending/disabled, and
    /// a failure on A's POST painted its error message onto B's row too —
    /// two independent mutations sharing one tracked state.
    ///
    /// Built only via this helper so every call site
    /// (`LibraryStore.setAutoflowEnabled`, `LibraryScreen`'s row toggle,
    /// `WorkflowDetailScreen`'s detail-page toggle) agrees on the exact same
    /// composite string. `scopeKind`/`scopeID` are folded in as-is (`nil`
    /// rendered as the literal `"nil"` token, distinct from any real scope
    /// id) — two rows can only produce the same key if they genuinely agree
    /// on `name`, `scopeKind`, AND `scopeID`, i.e. are the same row.
    public static func autoflow(name: String, scopeKind: String?, scopeID: String?, verb: ActionVerb) -> ActionKey {
        ActionKey("\(name):\(scopeKind ?? "nil"):\(scopeID ?? "nil")", verb)
    }
}

/// Lifecycle of one `ActionKey`. `.idle` is the default for a key that was
/// never `begin()`-ed (or was `clear()`-ed since). `.pending(since:)` records
/// when the mutating call was fired — the injected `now()` clock, not
/// `Date()` directly, so tests can drive staleness deterministically.
public enum ActionState: Equatable, Sendable {
    case idle, pending(since: Date), confirmed, failed(String)
}

/// Store-level primitive for the phase's pending-state mutation contract: a
/// mutation POST's 200 means *recorded*, and a marker verb (approve/resume/
/// ...) only proves *done* once the observed status transition it implies
/// actually arrives over the live feed. Call sites `begin(_:)` a key right
/// after firing the request, `fail(_:_:)` it if the POST itself errors, and
/// either let the next `resolve(runID:observedStatus:)` call (driven by
/// whatever live/refetched status the store already tracks) confirm it, or
/// `confirm(_:)` it directly for verbs `resolve` never touches.
///
/// **Confirmation table** (`resolve(runID:observedStatus:)`): for every
/// key belonging to `runID` that is currently `.pending`,
/// - `approve` confirms once `observedStatus` is neither `.awaiting` (the
///   run hasn't left the gate yet), `.pending` (still queued, never having
///   reached the gate at all), nor `.unknown` (an unrecognized status
///   proves nothing — see below);
/// - `resume` confirms once `observedStatus` is neither `.paused` (the run
///   hasn't left the pause), `.pending`, nor `.unknown`;
/// - `reject` confirms on `.rejected` or `.cancelled` — both are terminal
///   outcomes a reject can produce depending on the workflow's `on_reject`
///   routing;
/// - `cancel` confirms only on `.cancelled`;
/// - `pause` confirms only on `.paused`;
/// - `archive`/`restore`/`send`/`launch` are never confirmed by `resolve` —
///   their effects are response-visible or navigation-visible, not a run
///   status transition, so only an explicit `confirm(_:)` moves them.
///
/// **`.unknown` is never confirming for approve/resume.** Those two verbs
/// are defined by *exclusion* (confirm once the status has moved off one
/// specific blocking value), and `.unknown(_)` is neither `.awaiting` nor
/// `.paused` nor `.pending` — so a naive two-exclusion test would treat an
/// unrecognized/transient status string as proof the run left the gate or
/// the pause. It proves nothing of the kind: `.unknown` covers both a
/// genuinely novel server status *and* a transient feed glitch (a dropped
/// field, a race with the row not existing yet), and confirming a marker
/// verb on it would silently defeat the exact guarantee `PendingActions`
/// exists to give the approve/reject/pause/resume gate flow. `reject`/
/// `cancel`/`pause` are exact-match against one or two known values, so
/// `.unknown` was never at risk there — this exclusion only needed adding
/// to the two exclusion-shaped verbs.
///
/// `resolve` only ever touches keys that are currently `.pending` — a key
/// that's `.idle`, `.confirmed`, or `.failed` is left exactly as it is,
/// regardless of what status arrives.
@MainActor
@Observable
public final class PendingActions {
    private var states: [ActionKey: ActionState] = [:]
    private let now: () -> Date

    public init(now: @escaping () -> Date = Date.init) {
        self.now = now
    }

    /// `.idle` for any key that was never `begin()`-ed, or was `clear()`-ed
    /// since — there is no separate "unknown key" case, `.idle` covers both.
    public func state(_ key: ActionKey) -> ActionState {
        states[key] ?? .idle
    }

    /// Marks `key` as freshly fired, timestamped with the injected clock.
    /// Overwrites whatever state `key` was already in — a caller retrying a
    /// failed mutation calls `begin(_:)` again rather than needing a
    /// separate "retry" API.
    public func begin(_ key: ActionKey) {
        states[key] = .pending(since: now())
    }

    /// The mutating request itself errored (network failure, non-2xx) —
    /// distinct from the mutation succeeding but its effect never being
    /// observed (that shows up as `isStale(_:)`, not `.failed`).
    public func fail(_ key: ActionKey, _ message: String) {
        states[key] = .failed(message)
    }

    /// Explicit confirmation for verbs `resolve(runID:observedStatus:)`
    /// never touches (archive/restore/send/launch), or for a call site that
    /// already has stronger evidence than the next status poll would give.
    public func confirm(_ key: ActionKey) {
        states[key] = .confirmed
    }

    /// Resolves every currently-`.pending` key belonging to `runID` against
    /// one observed status, per the confirmation table on this type's doc
    /// comment. A key that isn't `.pending` — `.idle`, already `.confirmed`,
    /// or `.failed` — is never touched, and a verb this table doesn't cover
    /// for the given status is left `.pending` untouched too (still waiting
    /// for a later, more decisive status).
    ///
    /// "Belonging to `runID`" matches two shapes of `entityID`: an exact
    /// `runID` (every run-scoped verb — cancel/pause/resume/archive/
    /// restore), and a composite `"\(runID):<stepID>"` (a gate-scoped
    /// approve/reject built via `ActionKey.gate(runID:stepID:verb:)` — see
    /// that method's doc comment). One observed run-level status therefore
    /// resolves every gate's pending key for this run in the same pass, not
    /// just one arbitrarily-chosen gate.
    public func resolve(runID: String, observedStatus: ActivityStatus) {
        let gatePrefix = "\(runID):"
        let matchingKeys = states.keys.filter { $0.entityID == runID || $0.entityID.hasPrefix(gatePrefix) }
        for key in matchingKeys {
            guard case .pending = states[key] else { continue }
            if Self.confirms(key.verb, observedStatus) {
                states[key] = .confirmed
            }
        }
    }

    private static func confirms(_ verb: ActionVerb, _ status: ActivityStatus) -> Bool {
        if case .unknown = status, verb == .approve || verb == .resume {
            // An unrecognized status proves nothing — see this type's
            // doc-comment note on why `.unknown` must not satisfy either
            // exclusion-shaped verb's confirmation.
            return false
        }
        switch verb {
        case .approve:
            return status != .awaiting && status != .pending
        case .resume:
            return status != .paused && status != .pending
        case .reject:
            return status == .rejected || status == .cancelled
        case .cancel:
            return status == .cancelled
        case .pause:
            return status == .paused
        case .archive, .restore, .send, .launch, .remove, .setEnabled, .release, .requeue, .save:
            return false
        }
    }

    /// `true` only for a key currently `.pending` whose `since` predates
    /// `now() - timeout`; every other state (`.idle`/`.confirmed`/`.failed`)
    /// is never stale. Uses the same injected clock as `begin(_:)` so tests
    /// can assert the boundary deterministically rather than racing a real
    /// clock.
    public func isStale(_ key: ActionKey, timeout: TimeInterval = 30) -> Bool {
        guard case .pending(let since) = state(key) else { return false }
        return now().timeIntervalSince(since) > timeout
    }

    /// Drops `key` back to `.idle` — used once a caller has fully handled a
    /// terminal state (`.confirmed`/`.failed`), e.g. after dismissing an
    /// error toast, so a later `begin(_:)` starts clean.
    public func clear(_ key: ActionKey) {
        states.removeValue(forKey: key)
    }
}
