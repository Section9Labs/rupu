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

@MainActor @Test func pendingActionIsStaleIsFalseForNonPendingStates() {
    let actions = PendingActions(now: { Date() })
    let idleKey = ActionKey("run-1", .approve)
    let confirmedKey = ActionKey("run-1", .cancel)

    actions.begin(confirmedKey)
    actions.confirm(confirmedKey)

    #expect(actions.isStale(idleKey) == false)
    #expect(actions.isStale(confirmedKey) == false)
}
