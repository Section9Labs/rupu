import Foundation
import Observation

/// Coordinates a store's "live tail" against a resumable event stream: on
/// the very first connection there's nothing to resynchronize (the store's
/// initial `PagedSnapshot.refresh()` already established a baseline), but
/// every reconnect after a disconnect means events may have been missed
/// while offline — so `resnapshot()` must run, and fully complete, before
/// any event that arrived on the new connection is applied.
///
/// `start` takes raw `AsyncStream`s rather than a concrete stream type or a
/// streaming protocol, so it stays fully decoupled from `RupuAPI`'s
/// `JSONEventStream`/`Decodable` machinery and is trivially driven by a
/// scripted fake in tests (no networking, no `Decodable` conformance
/// needed on a test payload type). Production callers bridge a
/// `JSONEventStream<T>` in with `connectionBridge()` below, which supplies
/// the `onConnectionChange` closure `JSONEventStream`'s `init` already
/// accepts as a plain `let` — this deliberately avoids turning that
/// property into a `var`, which would break `JSONEventStream`'s existing
/// `Sendable` conformance (a class conforming to `Sendable` — not
/// `@unchecked Sendable` — may only store `let` properties of `Sendable`
/// type; see Package.swift's `JSONEventStream: Sendable` declaration).
@MainActor
@Observable
public final class StreamLifecycle {
    public enum Freshness: Equatable, Sendable {
        case idle, live, stale
    }

    public private(set) var freshness: Freshness = .idle

    private var connectionTask: Task<Void, Never>?
    private var eventsTask: Task<Void, Never>?
    private var pendingResnapshot: Task<Void, Never>?
    private var hasConnectedBefore = false

    public init() {}

    /// Begins consuming `events` and `connectionEvents` concurrently. A
    /// second call tears down and replaces any prior run (mirrors
    /// `BackendController.configureEmbedded`'s idempotent-restart style),
    /// resetting the first-connect/no-resnapshot bookkeeping for the new
    /// run.
    public func start<T: Sendable>(
        events: AsyncStream<T>,
        connectionEvents: AsyncStream<Bool>,
        resnapshot: @escaping @Sendable () async -> Void,
        apply: @escaping @MainActor (T) -> Void
    ) {
        stop()
        hasConnectedBefore = false
        freshness = .idle

        connectionTask = Task { [weak self] in
            for await connected in connectionEvents {
                // Re-acquired weakly every iteration (not bound once above
                // the loop) so a caller that drops its last strong
                // reference without calling `stop()` still lets this task
                // notice `self` is gone and unwind on its next tick,
                // rather than the task/self pair keeping each other alive
                // forever — same pattern as Phase 1's `HealthMonitor`.
                guard let self else { return }
                await self.handleConnectionChange(connected, resnapshot: resnapshot)
            }
        }

        eventsTask = Task { [weak self] in
            for await event in events {
                guard let self else { return }
                await self.applyGated(event, apply: apply)
            }
        }
    }

    /// Cancels both consumer tasks and any in-flight resnapshot.
    /// Idempotent — safe to call more than once, or when nothing was ever
    /// started.
    public func stop() {
        connectionTask?.cancel()
        eventsTask?.cancel()
        connectionTask = nil
        eventsTask = nil
        pendingResnapshot?.cancel()
        pendingResnapshot = nil
    }

    private func handleConnectionChange(_ connected: Bool, resnapshot: @escaping @Sendable () async -> Void) async {
        guard connected else {
            freshness = .stale
            return
        }
        if hasConnectedBefore {
            // A fresh `Task` is the gate `applyGated` awaits before letting
            // the next event through: `connectionTask` runs this method
            // sequentially (one `for await` iteration at a time), so
            // there's never more than one in-flight resnapshot to race
            // against here.
            let task = Task { await resnapshot() }
            pendingResnapshot = task
            await task.value
            pendingResnapshot = nil
        } else {
            hasConnectedBefore = true
        }
        freshness = .live
    }

    private func applyGated<T: Sendable>(_ event: T, apply: @MainActor (T) -> Void) async {
        if let pending = pendingResnapshot {
            await pending.value
        }
        apply(event)
    }
}

extension StreamLifecycle {
    /// Builds the `(onChange, events)` pair a production caller needs to
    /// bridge a `JSONEventStream<T>`'s connection signal into
    /// `start(events:connectionEvents:resnapshot:apply:)`: pass `onChange`
    /// as the stream's `onConnectionChange` init argument, and `events` as
    /// this method's own `connectionEvents` argument.
    ///
    /// ```swift
    /// let (onChange, connectionEvents) = StreamLifecycle.connectionBridge()
    /// let stream = JSONEventStream<CPEvent>(url: url, token: token, onConnectionChange: onChange)
    /// lifecycle.start(events: stream.events(), connectionEvents: connectionEvents, resnapshot: ..., apply: ...)
    /// ```
    public static func connectionBridge() -> (onChange: @Sendable (Bool) -> Void, events: AsyncStream<Bool>) {
        let (stream, continuation) = AsyncStream<Bool>.makeStream()
        let onChange: @Sendable (Bool) -> Void = { value in
            continuation.yield(value)
        }
        return (onChange, stream)
    }
}
