import Foundation
import Observation
import RupuAPI

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

/// Non-prompting authorization read. Distinct from a `Bool` so `.notDetermined`
/// (never asked yet) can't be conflated with `.denied` (asked, said no) —
/// conflating them would either show the "go to System Settings" banner
/// before the user has ever been prompted, or hide a genuine denial.
public enum NotificationAuthorizationStatus: Sendable, Equatable {
    case notDetermined
    case authorized
    case denied
}

/// Seam between `RunNotifier` and the real OS notification center.
/// `UNUserNotificationCenter` throws (no bundle context) under `swift test`
/// — every unit test run is in exactly that situation — so `RunNotifier`
/// never talks to it directly, only through this protocol. Tests inject a
/// recorder.
///
/// The prod implementation (`UNCenterNotificationPoster`) deliberately does
/// NOT live in this module — it lives in the App target
/// (`App/NotificationPoster.swift`), the only place that's always inside a
/// real bundle. Combined with `RunNotifier.init`'s `poster` parameter having
/// no default, that's not just a comment's claim: nothing in `RupuStore` (or
/// anything that imports it, including every test target) can even NAME a
/// concrete `UNUserNotificationCenter`-backed type, let alone construct one
/// by accident.
public protocol NotificationPosting: Sendable {
    /// Requests `.alert`/`.sound` authorization (a no-op re-ask, never a
    /// second system prompt, once the user has already decided) and reports
    /// whether it's currently granted.
    func requestAuthorization() async -> Bool
    /// Posts one local notification immediately (no trigger).
    func post(_ content: NotificationContent) async
    /// Reads the current authorization status WITHOUT prompting — never
    /// shows a system dialog, unlike `requestAuthorization()`. This is what
    /// lets `RunNotifier` keep `authorizationDenied` in sync with a status
    /// the user changed from System Settings directly, without this app
    /// ever calling `requestAuthorization()` again.
    func currentAuthorizationStatus() async -> NotificationAuthorizationStatus
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
            guard !isInitializing, notifyGates, !oldValue else { return }
            ensureAuthorization()
        }
    }

    /// Post on `.stepFailed`/`.runFailed`. Backed by `"notify.failures"`;
    /// defaults OFF.
    public var notifyFailures: Bool = false {
        didSet {
            defaults.set(notifyFailures, forKey: Keys.failures)
            guard !isInitializing, notifyFailures, !oldValue else { return }
            ensureAuthorization()
        }
    }

    /// Post on `.runCompleted` — a run that finishes as completed,
    /// cancelled, or rejected. A FAILED run never emits `.runCompleted`; it
    /// emits `.runFailed` instead (covered by `notifyFailures`), so this
    /// pref never doubles up with that one. Backed by `"notify.completions"`;
    /// defaults OFF.
    public var notifyCompletions: Bool = false {
        didSet {
            defaults.set(notifyCompletions, forKey: Keys.completions)
            guard !isInitializing, notifyCompletions, !oldValue else { return }
            ensureAuthorization()
        }
    }

    /// `true` once a `currentAuthorizationStatus()`/`requestAuthorization()`
    /// check has come back denied — the Settings tab shows a banner with a
    /// System Settings deep link while this is `true`. Kept in sync (both
    /// directions — a stale `true` OR a stale `false`) by
    /// `syncAuthorizationStatus()`, a non-prompting check called from
    /// `activate()` (once, at app-lifetime startup) and from
    /// `NotificationsTab`'s `onAppear` (every time the tab is shown) — that
    /// is what catches the user re-enabling, or revoking, notifications from
    /// System Settings directly, without ever touching a pref in this app
    /// again.
    public private(set) var authorizationDenied = false

    private let defaults: UserDefaults
    private let poster: any NotificationPosting
    private var task: Task<Void, Never>?

    /// `true` only while `init` is running. Restoring persisted prefs in
    /// `init` still fires `didSet` — `@Observable` rewrites stored
    /// properties into computed ones over private backing storage, so
    /// Swift's usual "no observer calls during a type's own init" carve-out
    /// doesn't apply once a property is macro-rewritten (confirmed
    /// empirically against this project's toolchain). This flag is the
    /// explicit guard that keeps `init` a pure "read prefs, touch nothing
    /// else" operation: no authorization request, no OS call, of any kind,
    /// ever happens before `init` returns.
    private var isInitializing = true

    /// Bumped by both `activate()` and `deactivate()`. The running loop
    /// re-checks this against the value it captured at spawn time on every
    /// iteration — see `activate`'s doc comment for why `Task.isCancelled`
    /// alone isn't enough to rule out a stale loop still doing work for a
    /// moment after a `deactivate()` immediately followed by a re-`activate()`.
    private var activationGeneration = 0

    /// Non-private (unlike everything else here) specifically so
    /// `RunNotifierTests` can assert on it directly via `@testable import` —
    /// `private` stays file-scoped even under `@testable`, so the dedup
    /// map's actual contents would otherwise be unobservable from a test.
    var recentlyNotified: [DedupKey: Date] = [:]

    private static let dedupWindow: TimeInterval = 30

    /// Non-private for the same `@testable`-visibility reason as
    /// `recentlyNotified` above. `stepID` is `nil` for the two run-scoped
    /// kinds (`runFailed`/`runCompleted`) and non-nil for the two
    /// step-scoped kinds (`stepAwaitingApproval`/`stepFailed`) — two
    /// DIFFERENT gates parked on the same run must each notify (they're
    /// different, still-actionable asks), so the dedup key has to include
    /// which step a step-scoped event is about, not just which run.
    struct DedupKey: Hashable {
        let runID: String
        let kindClass: String
        let stepID: String?
    }

    private enum Pref {
        case gates, failures, completions
    }

    private struct Mapped {
        let pref: Pref
        let kindClass: String
        let runID: String
        let stepID: String?
        let title: String
        let body: String
    }

    public init(defaults: UserDefaults = .standard, poster: any NotificationPosting) {
        self.defaults = defaults
        self.poster = poster
        // Reads only — see `isInitializing`'s doc comment for why this is
        // guaranteed to never call `ensureAuthorization()` (or anything
        // else UN*-shaped), no matter what these three read back as.
        notifyGates = defaults.bool(forKey: Keys.gates)
        notifyFailures = defaults.bool(forKey: Keys.failures)
        notifyCompletions = defaults.bool(forKey: Keys.completions)
        isInitializing = false
    }

    /// Pure seam: maps `event` to a pref + dedup key + display copy, checks
    /// the matching pref, and checks/updates the dedup map — all with no
    /// I/O. Returns `nil` when the event isn't one of the four notified
    /// kinds, its pref is off, or the same dedup key fired within the last
    /// 30s (guards against a reconnect replaying events the firehose
    /// already delivered once).
    public func decision(for event: CPEvent, now: Date) -> NotificationContent? {
        guard let mapped = Self.map(event) else { return nil }
        guard isEnabled(mapped.pref) else { return nil }

        let key = DedupKey(runID: mapped.runID, kindClass: mapped.kindClass, stepID: mapped.stepID)
        if let last = recentlyNotified[key], now.timeIntervalSince(last) < Self.dedupWindow {
            return nil
        }
        recentlyNotified[key] = now
        pruneStale(now: now)

        return NotificationContent(title: mapped.title, body: mapped.body, runID: mapped.runID)
    }

    /// Idempotent: a second `activate` call while already running is a
    /// no-op (same idiom as `HostsFooterStore.activate(client:)`). Spawns
    /// one task that syncs `authorizationDenied` once (non-prompting; see
    /// that property's doc comment) and then pulls `streamFactory()`'s
    /// stream to completion, calling `decision`/`poster.post` per event.
    ///
    /// If `streamFactory()` returns `nil` (backend not configured yet) or
    /// the stream itself ends, this backs off (capped exponential) and
    /// tries again — `JSONEventStream` already reconnects internally on its
    /// own capped backoff (see that type's `events()`), so in practice this
    /// outer retry is a safety net for cases the inner reconnect can't
    /// cover, not the primary reconnect path. `backoffSeconds` resets to 1
    /// only once an event actually arrives, not merely once
    /// `streamFactory()` hands back a non-nil stream — a stream that
    /// connects but never emits anything shouldn't be treated as healthy
    /// just because it exists.
    ///
    /// `self` is captured weakly and re-checked on every event, not held
    /// for the loop's whole lifetime, so a torn-down `RunNotifier` (e.g.
    /// mid-test) can't be kept alive by an in-flight iteration. Every
    /// re-check also compares `activationGeneration` against the value
    /// captured when this specific loop was spawned: `Task.isCancelled`
    /// alone only reflects THIS task's own cancellation flag, propagated
    /// cooperatively — a `deactivate()` immediately followed by a
    /// re-`activate()` bumps the generation synchronously (on `MainActor`,
    /// so strictly before the new loop can start), which means a stale loop
    /// that hasn't yet reached its next cancellation checkpoint still
    /// recognizes itself as superseded the moment it resumes, rather than
    /// possibly processing one more event as a second, overlapping live
    /// subscriber.
    public func activate(streamFactory: @escaping () -> EventStreamClient?) {
        guard task == nil else { return }
        activationGeneration += 1
        let generation = activationGeneration

        // `notifier` (not `self`) is deliberately the name every checkpoint
        // below unwraps into: `self` stays the ORIGINAL weak-optional
        // capture for the whole closure body, so each checkpoint re-reads
        // it fresh rather than reusing a strong local from an earlier
        // checkpoint that would otherwise keep `self` alive for however
        // long the surrounding scope takes to unwind.
        task = Task { [weak self] in
            if let notifier = self {
                await notifier.syncAuthorizationStatus()
            }

            var backoffSeconds: UInt64 = 1
            let maxBackoffSeconds: UInt64 = 30

            while !Task.isCancelled {
                guard let notifier = self, notifier.activationGeneration == generation else { return }

                guard let stream = streamFactory() else {
                    do {
                        try await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
                    } catch {
                        return
                    }
                    backoffSeconds = min(backoffSeconds * 2, maxBackoffSeconds)
                    continue
                }

                for await event in stream.events() {
                    guard !Task.isCancelled, let notifier = self, notifier.activationGeneration == generation else { return }
                    backoffSeconds = 1
                    if let content = notifier.decision(for: event, now: Date()) {
                        await notifier.poster.post(content)
                    }
                }

                guard !Task.isCancelled, let notifier = self, notifier.activationGeneration == generation else { return }
                do {
                    try await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
                } catch {
                    return
                }
                backoffSeconds = min(backoffSeconds * 2, maxBackoffSeconds)
            }
        }
    }

    /// `RunNotifier` is app-lifetime (owned once by `RupuApp`, activated on
    /// the first healthy backend connection, never explicitly torn down by
    /// any production caller) — this exists for symmetry with `activate()`
    /// and for tests. Bumping `activationGeneration` here, synchronously
    /// before `task?.cancel()`, is what makes an immediate re-`activate()`
    /// safe: see `activate`'s doc comment for the race it closes.
    public func deactivate() {
        activationGeneration += 1
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

    /// Non-prompting: reads `poster.currentAuthorizationStatus()` and syncs
    /// `authorizationDenied` to match. Safe to call as often as needed
    /// (`activate()` calls it once per activation; `NotificationsTab` calls
    /// it on every `onAppear`) — it never shows a system prompt, so there's
    /// no "asked too many times" concern the way `ensureAuthorization()`
    /// has.
    public func syncAuthorizationStatus() async {
        let status = await poster.currentAuthorizationStatus()
        authorizationDenied = (status == .denied)
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
                stepID: stepID,
                title: "Approval needed",
                body: "Step \(stepID): \(reason)"
            )
        case .stepFailed(let runID, let stepID, let error):
            return Mapped(
                pref: .failures,
                kindClass: "step_failed",
                runID: runID,
                stepID: stepID,
                title: "Step failed",
                body: "Step \(stepID): \(error)"
            )
        case .runFailed(let runID, let error, _):
            return Mapped(
                pref: .failures,
                kindClass: "run_failed",
                runID: runID,
                stepID: nil,
                title: "Run failed",
                body: error
            )
        case .runCompleted(let runID, let status, _):
            return Mapped(
                pref: .completions,
                kindClass: "run_completed",
                runID: runID,
                stepID: nil,
                title: "Run completed",
                body: "Status: \(status)"
            )
        default:
            return nil
        }
    }
}
