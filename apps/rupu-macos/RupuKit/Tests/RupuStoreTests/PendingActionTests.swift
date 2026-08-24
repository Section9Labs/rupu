import Testing
import Foundation
@testable import RupuStore

/// Thread-safe mutable clock, mirroring `PagedSnapshotTests.FlagBox`: a
/// plain captured `var` can't cross into the `@escaping` `now` closure
/// under Swift 6 strict concurrency once it's read from a non-isolated
/// context, so tests that need to advance time do it through this instead.
private final class ClockBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v: Date
    init(_ v: Date) { self.v = v }
    var value: Date {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

@MainActor @Test func pendingActionBeginSetsPendingSinceNow() {
    let fixedNow = Date(timeIntervalSince1970: 1_000)
    let actions = PendingActions(now: { fixedNow })
    let key = ActionKey("run-1", .approve)

    #expect(actions.state(key) == .idle)
    actions.begin(key)
    #expect(actions.state(key) == .pending(since: fixedNow))
}

@MainActor @Test func pendingActionFailSetsFailedWithMessage() {
    let actions = PendingActions(now: { Date() })
    let key = ActionKey("run-1", .approve)

    actions.begin(key)
    actions.fail(key, "network error")
    #expect(actions.state(key) == .failed("network error"))
}

@MainActor @Test func pendingActionConfirmSetsConfirmed() {
    let actions = PendingActions(now: { Date() })
    let key = ActionKey("run-1", .archive)

    actions.begin(key)
    actions.confirm(key)
    #expect(actions.state(key) == .confirmed)
}

@MainActor @Test func pendingActionClearResetsToIdle() {
    let actions = PendingActions(now: { Date() })
    let key = ActionKey("run-1", .approve)

    actions.begin(key)
    actions.clear(key)
    #expect(actions.state(key) == .idle)
}

@MainActor @Test func pendingActionTwoKeysSameEntityDifferentVerbsAreIndependent() {
    let actions = PendingActions(now: { Date() })
    let approveKey = ActionKey("run-1", .approve)
    let rejectKey = ActionKey("run-1", .reject)

    actions.begin(approveKey)
    #expect(actions.state(approveKey) != .idle)
    #expect(actions.state(rejectKey) == .idle)

    actions.confirm(approveKey)
    #expect(actions.state(approveKey) == .confirmed)
    #expect(actions.state(rejectKey) == .idle)
}

// MARK: - resolve(runID:observedStatus:) confirmation table

/// One row of the per-verb confirmation table `resolve` encodes. `verb` is
/// begun as pending on `"run-1"`, `resolve` is called with `observedStatus`,
/// and the resulting state is asserted to be `.confirmed` when
/// `expectedConfirmed` is `true`, or still `.pending` (untouched) otherwise.
private struct ResolveCase {
    let verb: ActionVerb
    let observedStatus: ActivityStatus
    let expectedConfirmed: Bool
    let label: String
}

private let resolveCases: [ResolveCase] = [
    // approve: confirmed once the run has left `.awaiting` and isn't still
    // `.pending` (queued, never having reached the gate at all).
    ResolveCase(verb: .approve, observedStatus: .running, expectedConfirmed: true, label: "approve+running"),
    ResolveCase(verb: .approve, observedStatus: .completed, expectedConfirmed: true, label: "approve+completed"),
    ResolveCase(verb: .approve, observedStatus: .awaiting, expectedConfirmed: false, label: "approve+awaiting"),
    ResolveCase(verb: .approve, observedStatus: .pending, expectedConfirmed: false, label: "approve+pending"),
    // regression: an unrecognized status must not fake-confirm approve —
    // see `pendingActionResolveUnknownStatusNeverConfirmsAnyVerb` for the
    // full per-verb sweep.
    ResolveCase(verb: .approve, observedStatus: .unknown("—"), expectedConfirmed: false, label: "approve+unknown"),

    // reject: confirmed on either terminal outcome a reject can produce.
    ResolveCase(verb: .reject, observedStatus: .rejected, expectedConfirmed: true, label: "reject+rejected"),
    ResolveCase(verb: .reject, observedStatus: .cancelled, expectedConfirmed: true, label: "reject+cancelled"),
    ResolveCase(verb: .reject, observedStatus: .running, expectedConfirmed: false, label: "reject+running"),

    // cancel: confirmed only on `.cancelled`.
    ResolveCase(verb: .cancel, observedStatus: .cancelled, expectedConfirmed: true, label: "cancel+cancelled"),
    ResolveCase(verb: .cancel, observedStatus: .failed, expectedConfirmed: false, label: "cancel+failed"),

    // pause: confirmed only on `.paused`.
    ResolveCase(verb: .pause, observedStatus: .paused, expectedConfirmed: true, label: "pause+paused"),
    ResolveCase(verb: .pause, observedStatus: .running, expectedConfirmed: false, label: "pause+running"),

    // resume: confirmed once the run has left `.paused` and isn't still
    // `.pending`.
    ResolveCase(verb: .resume, observedStatus: .running, expectedConfirmed: true, label: "resume+running"),
    ResolveCase(verb: .resume, observedStatus: .paused, expectedConfirmed: false, label: "resume+paused"),
    ResolveCase(verb: .resume, observedStatus: .pending, expectedConfirmed: false, label: "resume+pending"),
    ResolveCase(verb: .resume, observedStatus: .unknown("—"), expectedConfirmed: false, label: "resume+unknown"),

    // archive/restore/send/launch: never resolved by status — status-based
    // resolution must leave these pending no matter what arrives; only an
    // explicit `confirm(_:)` call (response- or navigation-driven) moves
    // them.
    ResolveCase(verb: .archive, observedStatus: .completed, expectedConfirmed: false, label: "archive+completed"),
    ResolveCase(verb: .restore, observedStatus: .running, expectedConfirmed: false, label: "restore+running"),
    ResolveCase(verb: .send, observedStatus: .completed, expectedConfirmed: false, label: "send+completed"),
    ResolveCase(verb: .launch, observedStatus: .running, expectedConfirmed: false, label: "launch+running"),
]

@MainActor @Test func pendingActionResolveConfirmationTable() {
    for c in resolveCases {
        let actions = PendingActions(now: { Date() })
        let key = ActionKey("run-1", c.verb)
        actions.begin(key)

        actions.resolve(runID: "run-1", observedStatus: c.observedStatus)

        if c.expectedConfirmed {
            #expect(actions.state(key) == .confirmed, "\(c.label) should confirm")
        } else {
            guard case .pending = actions.state(key) else {
                Issue.record("\(c.label) should remain pending, got \(actions.state(key))")
                continue
            }
        }
    }
}

/// Regression for the coordinator's review finding: `.unknown` must never
/// satisfy approve/resume's exclusion-shaped confirmation (it's neither
/// `.awaiting`/`.paused` nor `.pending`, so a naive two-exclusion test
/// would wrongly treat an unrecognized/transient status as proof the run
/// left the gate or the pause). reject/cancel/pause are exact-match against
/// known values, so `.unknown` was never a risk there — asserted here too,
/// to pin that it stays that way.
@MainActor @Test func pendingActionResolveUnknownStatusNeverConfirmsAnyVerb() {
    let unknown = ActivityStatus.unknown("—")
    let verbsThatResolveByStatus: [ActionVerb] = [.approve, .reject, .cancel, .pause, .resume]

    for verb in verbsThatResolveByStatus {
        let actions = PendingActions(now: { Date() })
        let key = ActionKey("run-1", verb)
        actions.begin(key)

        actions.resolve(runID: "run-1", observedStatus: unknown)

        guard case .pending = actions.state(key) else {
            Issue.record("\(verb) against .unknown should remain pending, got \(actions.state(key))")
            continue
        }
    }
}

@MainActor @Test func pendingActionResolveNeverConfirmsAVerbThatIsNotCurrentlyPending() {
    let actions = PendingActions(now: { Date() })
    let idleKey = ActionKey("run-1", .approve)
    let failedKey = ActionKey("run-1", .cancel)
    let confirmedKey = ActionKey("run-1", .pause)

    // idleKey never had begin() called — stays .idle.
    actions.begin(failedKey)
    actions.fail(failedKey, "boom")
    actions.begin(confirmedKey)
    actions.confirm(confirmedKey)

    actions.resolve(runID: "run-1", observedStatus: .cancelled) // would confirm .cancel if pending
    actions.resolve(runID: "run-1", observedStatus: .paused) // would confirm .pause if pending

    #expect(actions.state(idleKey) == .idle)
    #expect(actions.state(failedKey) == .failed("boom"))
    #expect(actions.state(confirmedKey) == .confirmed)
}

@MainActor @Test func pendingActionResolveOnlyAffectsMatchingEntityID() {
    let actions = PendingActions(now: { Date() })
    let runOneApprove = ActionKey("run-1", .approve)
    let runTwoApprove = ActionKey("run-2", .approve)

    actions.begin(runOneApprove)
    actions.begin(runTwoApprove)

    actions.resolve(runID: "run-1", observedStatus: .running)

    #expect(actions.state(runOneApprove) == .confirmed)
    guard case .pending = actions.state(runTwoApprove) else {
        Issue.record("run-2's key must be untouched by a resolve for run-1, got \(actions.state(runTwoApprove))")
        return
    }
}

// MARK: - staleness

@MainActor @Test func pendingActionIsStaleUsesInjectedClock() {
    let start = Date(timeIntervalSince1970: 10_000)
    let clock = ClockBox(start)
    let actions = PendingActions(now: { clock.value })
    let key = ActionKey("run-1", .approve)

    actions.begin(key)

    clock.value = start.addingTimeInterval(29)
    #expect(actions.isStale(key, timeout: 30) == false)

    clock.value = start.addingTimeInterval(31)
    #expect(actions.isStale(key, timeout: 30) == true)
}

// MARK: - ActionKey.gate composite convention (Phase 3, Task 5 fix round 1)

/// Two gates on the same run must track independently — the bug the
/// review's Critical finding flagged: a plain `ActionKey(runID, .approve)`
/// gave every gate on a run the SAME key, so approving one gate
/// spinnered/disabled another gate's controls too.
@MainActor @Test func pendingActionGateKeyIsScopedByStepIDNotJustRunID() {
    let actions = PendingActions(now: { Date() })
    let gateA = ActionKey.gate(runID: "run-1", stepID: "gate-a", verb: .approve)
    let gateB = ActionKey.gate(runID: "run-1", stepID: "gate-b", verb: .approve)

    actions.begin(gateA)
    #expect(actions.state(gateA) != .idle)
    #expect(actions.state(gateB) == .idle)

    actions.confirm(gateA)
    #expect(actions.state(gateA) == .confirmed)
    #expect(actions.state(gateB) == .idle)
}

/// `resolve(runID:observedStatus:)` must sweep every composite gate key
/// belonging to `runID` — not just an exact `entityID == runID` match —
/// since a gate-scoped key's `entityID` is `"\(runID):\(stepID)"`, never
/// the bare `runID`. One observed run-level transition therefore resolves
/// every gate's pending key for that run in the same pass.
@MainActor @Test func pendingActionResolveSweepsEveryCompositeGateKeyForTheRun() {
    let actions = PendingActions(now: { Date() })
    let gateA = ActionKey.gate(runID: "run-1", stepID: "gate-a", verb: .approve)
    let gateB = ActionKey.gate(runID: "run-1", stepID: "gate-b", verb: .reject)
    let plainRunKey = ActionKey("run-1", .cancel)

    actions.begin(gateA)
    actions.begin(gateB)
    actions.begin(plainRunKey)

    // gateB is `.reject` — only `.rejected`/`.cancelled` confirm it, so
    // `.running` resolves gateA and the plain key but leaves gateB pending;
    // this also proves the sweep doesn't blindly confirm every matched key,
    // only the ones whose own verb-specific rule says yes.
    actions.resolve(runID: "run-1", observedStatus: .running)

    #expect(actions.state(gateA) == .confirmed)
    guard case .pending = actions.state(plainRunKey) else {
        Issue.record("cancel should remain pending against .running, got \(actions.state(plainRunKey))")
        return
    }
    guard case .pending = actions.state(gateB) else {
        Issue.record("gateB's reject should remain pending against .running, got \(actions.state(gateB))")
        return
    }
}

/// Guards the prefix match itself against a false-positive on a numeric
/// run-id collision: `"run-1"` must never be treated as a prefix-match for
/// a DIFFERENT run's composite key `"run-12:gate-a"` — the match requires
/// the full `"run-1:"` (with the trailing colon), which `"run-12:..."`
/// does not start with.
@MainActor @Test func pendingActionResolveDoesNotFalsePositiveOnANumericRunIDPrefixCollision() {
    let actions = PendingActions(now: { Date() })
    let otherRunsGate = ActionKey.gate(runID: "run-12", stepID: "gate-a", verb: .approve)

    actions.begin(otherRunsGate)
    actions.resolve(runID: "run-1", observedStatus: .running)

    guard case .pending = actions.state(otherRunsGate) else {
        Issue.record("run-12's gate key must be untouched by a resolve for run-1, got \(actions.state(otherRunsGate))")
        return
    }
}

@MainActor @Test func pendingActionIsStaleIsFalseForNonPendingStates() {
    let actions = PendingActions(now: { Date() })
    let idleKey = ActionKey("run-1", .approve)
    let confirmedKey = ActionKey("run-1", .cancel)

    actions.begin(confirmedKey)
    actions.confirm(confirmedKey)

    #expect(actions.isStale(idleKey) == false)
    #expect(actions.isStale(confirmedKey) == false)
}
