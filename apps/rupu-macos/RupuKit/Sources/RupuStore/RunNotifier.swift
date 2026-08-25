import Foundation
import Observation
import RupuAPI
import UserNotifications

/// One notification's content, ready to hand to a `NotificationPosting`
/// implementation. `runID` rides along so `AppDelegate`'s tap handler
/// (`UNUserNotificationCenterDelegate.userNotificationCenter(_:didReceive:)`)
/// can route straight back to the right run without re-parsing anything out
/// of the system notification's own `userInfo`.
public struct NotificationContent: Equatable, Sendable {
    public var title: String
    public var body: String
    public var runID: String

    public init(title: String, body: String, runID: String) {
        self.title = title
        self.body = body
        self.runID = runID
    }
}

/// Seam between `RunNotifier` and the real OS notification center.
/// `UNUserNotificationCenter` throws (no bundle context) under `swift test`
/// — every unit test run is in exactly that situation — so `RunNotifier`
/// never talks to it directly, only through this protocol. Tests inject a
/// recorder; only `RupuApp` (the App target, always inside a real bundle)
/// ever constructs the prod implementation below.
public protocol NotificationPosting: Sendable {
    /// Requests `.alert`/`.sound` authorization (a no-op re-ask, never a
    /// second system prompt, once the user has already decided) and reports
    /// whether it's currently granted.
    func requestAuthorization() async -> Bool
    /// Posts one local notification immediately (no trigger).
    func post(_ content: NotificationContent) async
}

/// Prod `NotificationPosting` — the only thing in this arc allowed to touch
/// `UNUserNotificationCenter`. Stateless: every call reads
/// `UNUserNotificationCenter.current()` fresh, so there's nothing here for a
/// test to accidentally share.
public struct UNCenterNotificationPoster: NotificationPosting {
    public init() {}

    public func requestAuthorization() async -> Bool {
        (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])) ?? false
    }

    public func post(_ content: NotificationContent) async {
        let payload = UNMutableNotificationContent()
        payload.title = content.title
        payload.body = content.body
        payload.sound = .default
        payload.userInfo = ["runID": content.runID]
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: payload, trigger: nil)
        try? await UNUserNotificationCenter.current().add(request)
    }
}

/// Firehose-driven local notifications for gate/failure/completion events
/// (Phase 6A, Task 7). Three independent per-kind prefs, each defaulting OFF
/// — a fresh install must never surprise the user with system notifications
/// before they've opted in — a 30s replay-on-reconnect dedup guard, and an
/// `authorizationDenied` flag the Settings tab surfaces with a System
/// Settings deep link.
///
/// **`decision(for:now:)` is the pure seam** — pref gating, dedup, and
/// content building all happen there with no I/O; it's what the tests drive
/// exhaustively. `activate(streamFactory:)` is deliberately thin glue around
/// it: pull events off an independent firehose connection (the same
/// `BackendController.makeFirehoseStream(onConnectionChange:)` seam
/// `OverviewScreen`/`ActivityScreen` already use for their own independent
/// streams — see that method's doc comment), call `decision`, hand anything
/// non-nil to `poster.post`.
@MainActor
@Observable
public final class RunNotifier {
    private enum Keys {
        static let gates = "notify.gates"
        static let failures = "notify.failures"
        static let completions = "notify.completions"
    }

    /// Post a system notification when a run parks on a gate
    /// (`CPEvent.stepAwaitingApproval`). Backed by `UserDefaults` key
    /// `"notify.gates"`; defaults OFF. Flipping this OFF→ON asks for
    /// notification authorization if it hasn't been decided yet.
    public var notifyGates: Bool = false {
        didSet {
            defaults.set(notifyGates, forKey: Keys.gates)
            if notifyGates, !oldValue { ensureAuthorization() }
        }
    }

    /// Post on `.stepFailed`/`.runFailed`. Backed by `"notify.failures"`;
    /// defaults OFF.
    public var notifyFailures: Bool = false {
        didSet {
            defaults.set(notifyFailures, forKey: Keys.failures)
            if notifyFailures, !oldValue { ensureAuthorization() }
        }
    }

    /// Post on `.runCompleted` (any terminal status — success or not; the
    /// failure-shaped terminal events are `.runFailed`, covered by
    /// `notifyFailures` instead). Backed by `"notify.completions"`; defaults
    /// OFF.
    public var notifyCompletions: Bool = false {
        didSet {
            defaults.set(notifyCompletions, forKey: Keys.completions)
            if notifyCompletions, !oldValue { ensureAuthorization() }
        }
    }

    /// `true` once a `requestAuthorization()` call has come back denied —
    /// the Settings tab shows a banner with a System Settings deep link
    /// while this is `true`. There's no OS API to be told "un-denied"
    /// short of asking again, so this only clears on a later
    /// `requestAuthorization()` call that comes back granted.
    public private(set) var authorizationDenied = false

    private let defaults: UserDefaults
    private let poster: any NotificationPosting
    private var task: Task<Void, Never>?
    private var recentlyNotified: [DedupKey: Date] = [:]

    private static let dedupWindow: TimeInterval = 30

    private struct DedupKey: Hashable {
        let runID: String
        let kindClass: String
    }

    private enum Pref {
        case gates, failures, completions
    }

    private struct Mapped {
        let pref: Pref
        let kindClass: String
        let runID: String
        let title: String
        let body: String
    }

    public init(defaults: UserDefaults = .standard, poster: any NotificationPosting = UNCenterNotificationPoster()) {
        self.defaults = defaults
        self.poster = poster
        // Unlike a plain (non-`@Observable`) stored property, `didSet`
        // FIRES for these three assignments: `@Observable` rewrites every
        // stored property into a computed one over private backing storage,
        // and once a property is computed, Swift's usual "no observer calls
        // during a type's own init" carve-out no longer applies — every
        // assignment, `init` included, goes through the same synthesized
        // setter (confirmed empirically against this project's toolchain;
        // do not assume the plain-stored-property carve-out here). That
        // means a persisted `true` pref DOES call `ensureAuthorization()`
        // right here at construction — which is the right behavior, not a
        // bug to route around: it re-validates the actual OS authorization
        // status (a user may have revoked it from System Settings since the
        // last launch) every time the app starts with a pref already on,
        // not just the first time it's ever switched on from this tab.
        notifyGates = defaults.bool(forKey: Keys.gates)
        notifyFailures = defaults.bool(forKey: Keys.failures)
        notifyCompletions = defaults.bool(forKey: Keys.completions)
    }

    /// Pure seam: maps `event` to a pref + dedup key + display copy, checks
    /// the matching pref, and checks/updates the dedup map — all with no
    /// I/O. Returns `nil` when the event isn't one of the four notified
    /// kinds, its pref is off, or the same `(runID, kindClass)` fired within
    /// the last 30s (guards against a reconnect replaying events the
    /// firehose already delivered once).
    public func decision(for event: CPEvent, now: Date) -> NotificationContent? {
        guard let mapped = Self.map(event) else { return nil }
        guard isEnabled(mapped.pref) else { return nil }

        let key = DedupKey(runID: mapped.runID, kindClass: mapped.kindClass)
        if let last = recentlyNotified[key], now.timeIntervalSince(last) < Self.dedupWindow {
            return nil
        }
        recentlyNotified[key] = now
        pruneStale(now: now)

        return NotificationContent(title: mapped.title, body: mapped.body, runID: mapped.runID)
    }

    /// Idempotent: a second `activate` call while already running is a
    /// no-op (same idiom as `HostsFooterStore.activate(client:)`). Spawns
    /// one task that pulls `streamFactory()`'s stream to completion, calling
    /// `decision`/`poster.post` per event; if `streamFactory()` returns
    /// `nil` (backend not configured yet) or the stream itself ends, it
    /// backs off (capped exponential, matching `JSONEventStream`'s own
    /// internal reconnect backoff shape) and tries again. `self` is
    /// captured weakly and re-checked on every event, not held for the
    /// loop's whole lifetime, so a torn-down `RunNotifier` (e.g. mid-test)
    /// can't be kept alive by an in-flight iteration.
    public func activate(streamFactory: @escaping () -> EventStreamClient?) {
        guard task == nil else { return }
        task = Task { [weak self] in
            var backoffSeconds: UInt64 = 1
            let maxBackoffSeconds: UInt64 = 30

            while !Task.isCancelled {
                guard let stream = streamFactory() else {
                    do {
                        try await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
                    } catch {
                        return
                    }
                    backoffSeconds = min(backoffSeconds * 2, maxBackoffSeconds)
                    continue
                }

                backoffSeconds = 1
                for await event in stream.events() {
                    guard !Task.isCancelled, let self else { return }
                    if let content = self.decision(for: event, now: Date()) {
                        await self.poster.post(content)
                    }
                }

                guard !Task.isCancelled else { return }
                do {
                    try await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
                } catch {
                    return
                }
                backoffSeconds = min(backoffSeconds * 2, maxBackoffSeconds)
            }
        }
    }

    public func deactivate() {
        task?.cancel()
        task = nil
    }

    private func isEnabled(_ pref: Pref) -> Bool {
        switch pref {
        case .gates: notifyGates
        case .failures: notifyFailures
        case .completions: notifyCompletions
        }
    }

    private func pruneStale(now: Date) {
        recentlyNotified = recentlyNotified.filter { now.timeIntervalSince($0.value) < Self.dedupWindow }
    }

    private func ensureAuthorization() {
        Task { [weak self] in
            guard let self else { return }
            let granted = await self.poster.requestAuthorization()
            self.authorizationDenied = !granted
        }
    }

    /// Maps the four notified `CPEvent` cases to their pref/dedup-key/copy;
    /// every other case (including `.unknown`) returns `nil` — those never
    /// notify, per the brief's "all other cases → nil".
    private static func map(_ event: CPEvent) -> Mapped? {
        switch event {
        case .stepAwaitingApproval(let runID, let stepID, let reason):
            return Mapped(
                pref: .gates,
                kindClass: "step_awaiting_approval",
                runID: runID,
                title: "Approval needed",
                body: "Step \(stepID): \(reason)"
            )
        case .stepFailed(let runID, let stepID, let error):
            return Mapped(
                pref: .failures,
                kindClass: "step_failed",
                runID: runID,
                title: "Step failed",
                body: "Step \(stepID): \(error)"
            )
        case .runFailed(let runID, let error, _):
            return Mapped(
                pref: .failures,
                kindClass: "run_failed",
                runID: runID,
                title: "Run failed",
                body: error
            )
        case .runCompleted(let runID, let status, _):
            return Mapped(
                pref: .completions,
                kindClass: "run_completed",
                runID: runID,
                title: "Run completed",
                body: "Status: \(status)"
            )
        default:
            return nil
        }
    }
}
