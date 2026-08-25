import Testing
import Foundation
@testable import RupuStore
import RupuAPI

/// `NotificationPosting` recorder — never touches `UNUserNotificationCenter`
/// (no bundle context under `swift test`; see `RunNotifier`'s doc comment).
/// These tests exercise the pure `decision(for:now:)` seam only, so
/// `post`/`requestAuthorization` are never actually invoked, but a fresh
/// `RunNotifier` still needs a `NotificationPosting` to construct.
private final class RecordingNotificationPoster: NotificationPosting, @unchecked Sendable {
    private(set) var authorizationRequests = 0
    private(set) var posted: [NotificationContent] = []
    var grantAuthorization = true

    func requestAuthorization() async -> Bool {
        authorizationRequests += 1
        return grantAuthorization
    }

    func post(_ content: NotificationContent) async {
        posted.append(content)
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

/// Replay-on-reconnect guard: the same `(runID, kind)` firing twice inside
/// the 30s dedup window must suppress the second one.
@MainActor @Test func sameRunAndKindWithinWindowSuppressesSecondCall() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    let t0 = Date()

    #expect(notifier.decision(for: gateEvent, now: t0) != nil)
    #expect(notifier.decision(for: gateEvent, now: t0.addingTimeInterval(10)) == nil)
}

/// Past the 30s window, the same `(runID, kind)` posts again — this is what
/// makes the dedup a replay guard rather than a permanent per-run mute.
@MainActor @Test func sameRunAndKindAfter31SecondsPostsAgain() {
    let notifier = makeNotifier()
    notifier.notifyGates = true
    let t0 = Date()

    #expect(notifier.decision(for: gateEvent, now: t0) != nil)
    #expect(notifier.decision(for: gateEvent, now: t0.addingTimeInterval(31)) != nil)
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

/// A pref's OFF→ON flip is the only trigger for `ensureAuthorization()` —
/// flipping it on then off then on again requests authorization each time
/// it goes on (there's no state kept across the boundary), but flipping it
/// to the SAME value never triggers a spurious extra request.
@MainActor @Test func prefOffToOnTransitionRequestsAuthorizationOnceNotOnRedundantSet() async throws {
    let poster = RecordingNotificationPoster()
    let notifier = makeNotifier(poster: poster)

    notifier.notifyGates = true
    // `ensureAuthorization` spawns a detached `Task`; give it a beat to run
    // before asserting on the recorder.
    try await Task.sleep(for: .milliseconds(50))
    #expect(poster.authorizationRequests == 1)

    notifier.notifyGates = true // already true — no transition, no new request
    try await Task.sleep(for: .milliseconds(50))
    #expect(poster.authorizationRequests == 1)
}

/// A denied authorization sets `authorizationDenied` — the signal
/// `NotificationsTab`'s banner reads.
@MainActor @Test func deniedAuthorizationSetsAuthorizationDenied() async throws {
    let poster = RecordingNotificationPoster()
    poster.grantAuthorization = false
    let notifier = makeNotifier(poster: poster)

    #expect(notifier.authorizationDenied == false)
    notifier.notifyFailures = true
    try await Task.sleep(for: .milliseconds(50))
    #expect(notifier.authorizationDenied == true)
}

/// Restoring an already-on persisted pref in `init` DOES re-check
/// authorization — `@Observable` rewrites stored properties into computed
/// ones, so the usual "no observer calls during a type's own init"
/// Swift carve-out doesn't apply here (see `RunNotifier.init`'s doc
/// comment); this is also the desired behavior, not just an accepted
/// side effect — it re-validates against a user who may have revoked
/// permission in System Settings since the last launch, every time the app
/// starts with a pref already on, not only the first time it's ever
/// switched on from the Settings tab.
@MainActor @Test func persistedPrefAlreadyOnAtInitRequestsAuthorization() async throws {
    let poster = RecordingNotificationPoster()
    let defaults = UserDefaults(suiteName: "test-\(UUID())")!
    defaults.set(true, forKey: "notify.gates")

    let notifier = RunNotifier(defaults: defaults, poster: poster)
    #expect(notifier.notifyGates == true)

    try await Task.sleep(for: .milliseconds(50))
    #expect(poster.authorizationRequests == 1)
}
