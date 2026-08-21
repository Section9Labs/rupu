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
///   run hasn't left the gate yet) nor `.pending` (still queued, never
///   having reached the gate at all);
/// - `resume` confirms once `observedStatus` is neither `.paused` (the run
///   hasn't left the pause) nor `.pending`;
/// - `reject` confirms on `.rejected` or `.cancelled` — both are terminal
///   outcomes a reject can produce depending on the workflow's `on_reject`
///   routing;
/// - `cancel` confirms only on `.cancelled`;
/// - `pause` confirms only on `.paused`;
/// - `archive`/`restore`/`send`/`launch` are never confirmed by `resolve` —
///   their effects are response-visible or navigation-visible, not a run
///   status transition, so only an explicit `confirm(_:)` moves them.
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
    public func resolve(runID: String, observedStatus: ActivityStatus) {
        let matchingKeys = states.keys.filter { $0.entityID == runID }
        for key in matchingKeys {
            guard case .pending = states[key] else { continue }
            if Self.confirms(key.verb, observedStatus) {
                states[key] = .confirmed
            }
        }
    }

    private static func confirms(_ verb: ActionVerb, _ status: ActivityStatus) -> Bool {
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
        case .archive, .restore, .send, .launch:
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
