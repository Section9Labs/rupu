import Testing
import Foundation
@testable import RupuStore
import RupuAPI

/// `NotificationPosting` recorder — never touches `UNUserNotificationCenter`
/// (no bundle context under `swift test`; see `RunNotifier`'s doc comment).
/// An `actor`, not an `@unchecked Sendable` class with plain mutable
/// `var`s: its methods are called from inside `RunNotifier`'s own
/// `Task { [weak self] in ... }` closures, concurrently with this file's
/// `@MainActor` test code reading the recorded state back out — actor
/// isolation is what actually makes that safe, rather than merely silencing
/// the compiler's Sendable check.
private actor RecordingNotificationPoster: NotificationPosting {
    private(set) var authorizationRequests = 0
    private(set) var posted: [NotificationContent] = []
    private var authorizationGrant = true
    private var status: NotificationAuthorizationStatus = .notDetermined

    func requestAuthorization() async -> Bool {
        authorizationRequests += 1
        return authorizationGrant
    }

    func post(_ content: NotificationContent) async {
        posted.append(content)
    }

    func currentAuthorizationStatus() async -> NotificationAuthorizationStatus {
        status
    }

    func setAuthorizationGrant(_ granted: Bool) {
        authorizationGrant = granted
    }

    func setStatus(_ newStatus: NotificationAuthorizationStatus) {
        status = newStatus
    }
}

/// Repo condition-poll idiom (see `ActivityStoreTests`/`DashboardStoreTests`/
/// etc.) — `pollUntil`/`expectEventually`, never a fixed sleep-then-assert.
/// This copy's `condition` is `async`, unlike the other copies in this test
/// suite, because it needs to read `RecordingNotificationPoster`'s
/// actor-isolated state across the actor boundary.
@MainActor
private func pollUntil(
    timeout: Duration = .seconds(2),
    interval: Duration = .milliseconds(10),
    _ condition: () async -> Bool
) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while true {
        if await condition() { return true }
        if ContinuousClock.now >= deadline { return false }
        try? await Task.sleep(for: interval)
    }
}

@MainActor
private func makeNotifier(poster: RecordingNotificationPoster = RecordingNotificationPoster()) -> RunNotifier {
    let defaults = UserDefaults(suiteName: "test-\(UUID())")!
    return RunNotifier(defaults: defaults, poster: poster)
}

private let gateEvent = CPEvent.stepAwaitingApproval(runID: "run-1", stepID: "step-1", reason: "needs a human")
private let stepFailedEvent = CPEvent.stepFailed(runID: "run-1", stepID: "step-1", error: "boom")
private let runFailedEvent = CPEvent.runFailed(runID: "run-1", error: "boom", finishedAt: "2026-08-25T00:00:00Z")
private let runCompletedEvent = CPEvent.runCompleted(runID: "run-1", status: "success", finishedAt: "2026-08-25T00:00:00Z")

/// pref-off → nil, for every one of the three notified kinds — a fresh
/// `RunNotifier` starts with all three prefs OFF (never opt the user into
/// system notifications by default), so `decision` must suppress everything
/// until the matching pref is turned on.
@MainActor @Test func decisionSuppressesWhenMatchingPrefIsOff() {
    let notifier = makeNotifier()

    #expect(notifier.decision(for: gateEvent, now: Date()) == nil)
    #expect(notifier.decision(for: stepFailedEvent, now: Date()) == nil)
    #expect(notifier.decision(for: runFailedEvent, now: Date()) == nil)
    #expect(notifier.decision(for: runCompletedEvent, now: Date()) == nil)
}

/// A gate event with `notifyGates` on produces content carrying the run's
/// ID — the field `AppDelegate`'s tap handler reads back out of
/// `userInfo["runID"]` to navigate.
@MainActor @Test func gateEventWithPrefOnProducesContentWithRunID() {
    let notifier = makeNotifier()
    notifier.notifyGates = true

    let content = notifier.decision(for: gateEvent, now: Date())
    #expect(content?.runID == "run-1")
}

/// Replay-on-reconnect guard: the same gate firing twice inside the 30s
/// dedup window must suppress the second one.
@MainActor @Test func sameRunAndKindWithinWindowSuppressesSecondCall() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    let t0 = Date()

    #expect(notifier.decision(for: gateEvent, now: t0) != nil)
    #expect(notifier.decision(for: gateEvent, now: t0.addingTimeInterval(10)) == nil)
}

/// Past the 30s window, the same gate posts again — this is what makes the
/// dedup a replay guard rather than a permanent per-run mute.
@MainActor @Test func sameRunAndKindAfter31SecondsPostsAgain() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    let t0 = Date()

    #expect(notifier.decision(for: gateEvent, now: t0) != nil)
    #expect(notifier.decision(for: gateEvent, now: t0.addingTimeInterval(31)) != nil)
}

/// Review-round fix (finding: dedup key was too coarse): TWO DIFFERENT
/// gates parked on the SAME run within the window must both post — they're
/// different, independently-actionable asks, not a replay of the same one.
/// The dedup key includes `stepID` for the step-scoped kinds specifically
/// so this doesn't collapse.
@MainActor @Test func twoDifferentGatesOnSameRunWithinWindowBothPost() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    let t0 = Date()

    let gateA = CPEvent.stepAwaitingApproval(runID: "run-1", stepID: "step-A", reason: "needs a human")
    let gateB = CPEvent.stepAwaitingApproval(runID: "run-1", stepID: "step-B", reason: "needs a human")

    #expect(notifier.decision(for: gateA, now: t0) != nil)
    #expect(notifier.decision(for: gateB, now: t0.addingTimeInterval(5)) != nil)
}

/// Companion to the above: the SAME gate (same run, same step) firing twice
/// within the window is still a replay, and still suppressed.
@MainActor @Test func sameGateTwiceWithinWindowSuppressesSecond() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    let t0 = Date()

    let gate = CPEvent.stepAwaitingApproval(runID: "run-1", stepID: "step-A", reason: "needs a human")

    #expect(notifier.decision(for: gate, now: t0) != nil)
    #expect(notifier.decision(for: gate, now: t0.addingTimeInterval(5)) == nil)
}

/// `.runCompleted` maps to `notifyCompletions`, not `notifyFailures` or
/// `notifyGates` — on with only the OTHER two prefs must still suppress it.
@MainActor @Test func runCompletedMapsToCompletionsPrefOnly() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    notifier.notifyFailures = true
    #expect(notifier.decision(for: runCompletedEvent, now: Date()) == nil)

    notifier.notifyCompletions = true
    #expect(notifier.decision(for: runCompletedEvent, now: Date()) != nil)
}

/// Both `.stepFailed` and `.runFailed` map to the SAME `notifyFailures`
/// pref — a run that fails after one of its steps already failed is still
/// two distinct notifications (different `kindClass`, so the dedup guard
/// above doesn't collapse them), but both are gated by the one pref.
@MainActor @Test func stepFailedAndRunFailedBothMapToFailuresPref() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    notifier.notifyCompletions = true
    #expect(notifier.decision(for: stepFailedEvent, now: Date()) == nil)
    #expect(notifier.decision(for: runFailedEvent, now: Date()) == nil)

    notifier.notifyFailures = true
    #expect(notifier.decision(for: stepFailedEvent, now: Date()) != nil)
    #expect(notifier.decision(for: runFailedEvent, now: Date()) != nil)
}

/// Every other `CPEvent` case — including `.unknown` — maps to `nil`
/// regardless of which prefs are on, per the brief's "all other cases →
/// nil".
@MainActor @Test func unmappedEventKindsAlwaysSuppress() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    notifier.notifyFailures = true
    notifier.notifyCompletions = true

    #expect(notifier.decision(for: .runStarted(runID: "run-1", workflowPath: "wf.yaml", startedAt: "t"), now: Date()) == nil)
    #expect(notifier.decision(for: .stepStarted(runID: "run-1", stepID: "step-1", kind: "agent", agent: nil, host: nil), now: Date()) == nil)
    #expect(notifier.decision(for: .unknown(type: "something_new", runID: "run-1"), now: Date()) == nil)
}

/// Review-round fix (finding: `pruneStale` was untested): the seen-map
/// prunes ON INSERT, not on a timer — inserting a second, still-fresh key
/// after the first has aged past the 30s window must leave the map holding
/// ONLY the fresh one. Uses `@testable` access to `recentlyNotified`/
/// `DedupKey` (both non-`private` specifically for this).
@MainActor @Test func pruneOnInsertRemovesOnlyExpiredEntries() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    let t0 = Date()

    let eventA = CPEvent.stepAwaitingApproval(runID: "run-A", stepID: "step-A", reason: "r")
    let eventB = CPEvent.stepAwaitingApproval(runID: "run-B", stepID: "step-B", reason: "r")

    _ = notifier.decision(for: eventA, now: t0)
    _ = notifier.decision(for: eventB, now: t0.addingTimeInterval(31))

    #expect(notifier.recentlyNotified.count == 1)
    #expect(notifier.recentlyNotified.keys.first?.runID == "run-B")
}

/// A pref's OFF→ON flip is the only trigger for `ensureAuthorization()` —
/// flipping it on then off then on again requests authorization each time
/// it goes on (there's no state kept across the boundary), but flipping it
/// to the SAME value never triggers a spurious extra request.
@MainActor @Test func prefOffToOnTransitionRequestsAuthorizationOnceNotOnRedundantSet() async {
    let poster = RecordingNotificationPoster()
    let notifier = makeNotifier(poster: poster)

    notifier.notifyGates = true
    #expect(await pollUntil { await poster.authorizationRequests == 1 })

    notifier.notifyGates = true // already true — no transition, no new request
    // Asserting an ABSENCE can't be a positive poll — deliberately short
    // (not the usual wide margin) since we WANT this to time out.
    _ = await pollUntil(timeout: .milliseconds(150)) { await poster.authorizationRequests > 1 }
    #expect(await poster.authorizationRequests == 1)
}

/// A denied authorization sets `authorizationDenied` — the signal
/// `NotificationsTab`'s banner reads.
@MainActor @Test func deniedAuthorizationSetsAuthorizationDenied() async {
    let poster = RecordingNotificationPoster()
    await poster.setAuthorizationGrant(false)
    let notifier = makeNotifier(poster: poster)

    #expect(notifier.authorizationDenied == false)
    notifier.notifyFailures = true
    #expect(await pollUntil { notifier.authorizationDenied == true })
}

/// Review-round fix (finding: `init` must have zero UN side effects, not
/// just "only re-validates"): restoring an already-on persisted pref in
/// `init` must NEVER call `ensureAuthorization()` — `isInitializing` is the
/// explicit guard for this, since `@Observable` rewrites stored properties
/// into computed ones and Swift's usual "no observer calls during a type's
/// own init" carve-out doesn't apply once a property is macro-rewritten
/// (see `RunNotifier.init`'s doc comment).
@MainActor @Test func persistedPrefAlreadyOnAtInitNeverRequestsAuthorization() async {
    let poster = RecordingNotificationPoster()
    let defaults = UserDefaults(suiteName: "test-\(UUID())")!
    defaults.set(true, forKey: "notify.gates")

    let notifier = RunNotifier(defaults: defaults, poster: poster)
    #expect(notifier.notifyGates == true)

    _ = await pollUntil(timeout: .milliseconds(150)) { await poster.authorizationRequests > 0 }
    #expect(await poster.authorizationRequests == 0)
}

/// `syncAuthorizationStatus()` is the non-prompting sync path (`activate()`
/// once, `NotificationsTab`'s `.task` every time the tab appears) — it must
/// track BOTH directions: a denial sets the banner on, and a later grant
/// (e.g. the user re-enabled it from System Settings) clears it again,
/// without ever calling `requestAuthorization()`.
@MainActor @Test func syncAuthorizationStatusTracksBothDirections() async {
    let poster = RecordingNotificationPoster()
    let notifier = makeNotifier(poster: poster)

    await poster.setStatus(.denied)
    await notifier.syncAuthorizationStatus()
    #expect(notifier.authorizationDenied == true)

    await poster.setStatus(.authorized)
    await notifier.syncAuthorizationStatus()
    #expect(notifier.authorizationDenied == false)

    #expect(await poster.authorizationRequests == 0)
}

/// `.notDetermined` (never asked yet) must never be treated as `.denied` —
/// otherwise the banner would show before the user has ever been prompted.
@MainActor @Test func syncAuthorizationStatusTreatsNotDeterminedAsNotDenied() async {
    let poster = RecordingNotificationPoster()
    let notifier = makeNotifier(poster: poster)

    await poster.setStatus(.notDetermined)
    await notifier.syncAuthorizationStatus()
    #expect(notifier.authorizationDenied == false)
}

// MARK: - Final-review fix (I1): rebinding requires deactivate + activate

/// Thread-safe invocation counter for a `streamFactory` closure — the
/// closure is `@escaping () -> EventStreamClient?` (not `@Sendable`), but
/// it's called from inside `RunNotifier`'s own `Task`, so its captured
/// state still has to be safe to read back from the test body.
private final class FactoryCallCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    func increment() { lock.withLock { v += 1 } }
    var value: Int { lock.withLock { v } }
}

/// **The mechanism behind the I1 backend-rebinding bug.** `activate` is
/// idempotent by design (`guard task == nil`), and the running loop only
/// re-enters `streamFactory()` when the current stream's `events()` ends —
/// which `JSONEventStream` never lets happen, since it reconnects
/// internally, forever, to its construction-time URL and token. So handing
/// `activate` a NEW factory after a backend identity change does nothing at
/// all: the old factory's binding stays live. Only `deactivate()` first
/// re-enters the factory, which is exactly what `RupuApp.
/// activateHealthDependents()` now does on a `backend.clientIdentity()`
/// change.
///
/// Both factories return `nil` (no stream), which drives the loop's
/// backoff path — enough to observe WHICH factory the loop is calling
/// without standing up a real SSE endpoint.
@MainActor @Test func activateWithANewFactoryIsIgnoredUntilDeactivateAndThenRebinds() async {
    let notifier = makeNotifier()
    let first = FactoryCallCounter()
    let second = FactoryCallCounter()

    notifier.activate(streamFactory: { first.increment(); return nil })
    #expect(await pollUntil { first.value >= 1 }, "the first factory must be entered immediately on activate")

    // A bare re-`activate` with a different factory: the guard makes it a
    // no-op, so the SECOND factory is never reached. Bounded negative wait
    // — the loop's own retry backoff starts at 1s, so 400ms is comfortably
    // inside the window where a rebind, if it happened at all, would have
    // had to come from this call rather than a later backoff tick.
    notifier.activate(streamFactory: { second.increment(); return nil })
    try? await Task.sleep(for: .milliseconds(400))
    #expect(second.value == 0, "activate() while already running must not rebind — this is the I1 bug's mechanism")

    // Deactivate first, and the new factory takes over.
    notifier.deactivate()
    notifier.activate(streamFactory: { second.increment(); return nil })
    #expect(await pollUntil { second.value >= 1 }, "deactivate() + activate() must rebind to the new factory")

    notifier.deactivate()
}
