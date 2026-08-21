import Foundation
import Observation

/// One element of the single ordered signal stream `StreamLifecycle.start`
/// consumes. Folding connection state and decoded events into one
/// `AsyncStream` (rather than two independent ones) is what makes
/// "resnapshot completes before the next apply" true *by construction*: a
/// single `AsyncStream` delivers elements to its one consumer in the exact
/// order `.yield()` was called at the producer, so there is nothing left to
/// race — no gate variable, no scheduling assumption between two
/// independently-resumed consumer tasks.
public enum StreamSignal<T: Sendable>: Sendable {
    case connection(Bool)
    case event(T)
}

/// Coordinates a store's "live tail" against a resumable event stream: on
/// the very first connection there's nothing to resynchronize (the store's
/// initial `PagedSnapshot.refresh()` already established a baseline), but
/// every reconnect after a disconnect means events may have been missed
/// while offline — so `resnapshot()` must run, and fully complete, before
/// any event that arrived on the new connection is applied.
///
/// `start` consumes one `AsyncStream<StreamSignal<T>>` with a single
/// consumer task processing signals strictly in the order they were
/// yielded — a `.connection(true)` that follows an earlier `.connection`
/// signal (i.e. any reconnect) `await`s `resnapshot()` to completion before
/// the loop moves on to the next signal, so a subsequent `.event` in the
/// same stream can never be applied ahead of it. This stays fully decoupled
/// from `RupuAPI`'s `JSONEventStream`/`Decodable` machinery, so it's
/// trivially driven by a scripted fake in tests (no networking, no
/// `Decodable` conformance needed on a test payload type). Production
/// callers bridge a `JSONEventStream<T>` in with `makeSignalBridge()`
/// below, which supplies the `onConnectionChange` closure
/// `JSONEventStream`'s `init` already accepts as a plain `let` — this
/// deliberately avoids turning that property into a `var`, which would
/// break `JSONEventStream`'s existing `Sendable` conformance (a class
/// conforming to `Sendable` — not `@unchecked Sendable` — may only store
/// `let` properties of `Sendable` type; see Package.swift's
/// `JSONEventStream: Sendable` declaration).
@MainActor
@Observable
public final class StreamLifecycle {
    public enum Freshness: Equatable, Sendable {
        case idle, live, stale
    }

    public private(set) var freshness: Freshness = .idle

    private var consumerTask: Task<Void, Never>?

    /// `nil` until the first `.connection` signal of this `start()` run is
    /// processed. Distinguishing "never seen a connection signal at all"
    /// from "currently disconnected" is what makes the resnapshot rule
    /// exactly match its spec: a pristine `.connection(true)` — the very
    /// first signal this run ever sees — skips resnapshot; a
    /// `.connection(true)` preceded by *any* earlier signal (including a
    /// `.connection(false)` with no prior `true` at all) does not, because
    /// textually that's still "a connect after a previous disconnect".
    private var lastKnownConnected: Bool?

    public init() {}

    /// Begins consuming `signals`. A second call tears down and replaces
    /// any prior run (mirrors `BackendController.configureEmbedded`'s
    /// idempotent-restart style), resetting the first-connect/no-resnapshot
    /// bookkeeping for the new run.
    public func start<T: Sendable>(
        signals: AsyncStream<StreamSignal<T>>,
        resnapshot: @escaping @Sendable () async -> Void,
        apply: @escaping @MainActor (T) -> Void
    ) {
        stop()
        lastKnownConnected = nil
        freshness = .idle

        consumerTask = Task { [weak self] in
            for await signal in signals {
                // Re-acquired weakly every iteration (not bound once above
                // the loop) so a caller that drops its last strong
                // reference without calling `stop()` still lets this task
                // notice `self` is gone and unwind on its next tick,
                // rather than the task/self pair keeping each other alive
                // forever — same pattern as Phase 1's `HealthMonitor`.
                guard let self else { return }
                await self.handle(signal, resnapshot: resnapshot, apply: apply)
            }
        }
    }

    /// Cancels the consumer task. Idempotent — safe to call more than once,
    /// or when nothing was ever started.
    ///
    /// Cancellation here is *cooperative*, not preemptive: it stops the
    /// consumer from picking up its *next* signal, but if a `resnapshot()`
    /// (or `apply`) call is already mid-flight when `stop()` is called,
    /// and that closure doesn't itself check `Task.isCancelled`, it will
    /// still run to completion after `stop()` returns. `resnapshot`
    /// closures that care about this (e.g. one doing real I/O worth
    /// aborting) should check `Task.isCancelled` themselves.
    public func stop() {
        consumerTask?.cancel()
        consumerTask = nil
    }

    private func handle<T: Sendable>(
        _ signal: StreamSignal<T>,
        resnapshot: @escaping @Sendable () async -> Void,
        apply: @MainActor (T) -> Void
    ) async {
        switch signal {
        case .connection(let connected):
            let isPristineFirstSignal = connected && lastKnownConnected == nil
            lastKnownConnected = connected
            guard connected else {
                freshness = .stale
                return
            }
            if !isPristineFirstSignal {
                await resnapshot()
            }
            freshness = .live
        case .event(let value):
            apply(value)
        }
    }
}

extension StreamLifecycle {
    /// Bridges a `JSONEventStream<T>` into the single ordered `signals`
    /// stream `start(signals:...)` needs. Two-phase because
    /// `JSONEventStream.init` needs `onConnectionChange` as an argument,
    /// which must exist before the stream itself does:
    ///
    /// ```swift
    /// let (onChange, continuation, signals) = StreamLifecycle.makeSignalBridge(CPEvent.self)
    /// let stream = JSONEventStream<CPEvent>(url: url, token: token, onConnectionChange: onChange)
    /// let pump = Task {
    ///     for await event in stream.events() { continuation.yield(.event(event)) }
    ///     continuation.finish()
    /// }
    /// lifecycle.start(signals: signals, resnapshot: ..., apply: ...)
    /// ```
    ///
    /// `onChange` yields directly into `continuation`, synchronously, from
    /// wherever `JSONEventStream` calls it — so a connection signal for a
    /// given attempt always lands in `signals` before any frame of that
    /// same attempt (`JSONEventStream.runOnce()` calls
    /// `onConnectionChange(true)` before decoding any line, and
    /// `onConnectionChange(false)` only after that attempt's frame loop has
    /// fully exited — Task 3's construction).
    ///
    /// The one honest caveat: forwarding `stream.events()`'s frames into
    /// `continuation` needs its own pump task (shown above), a second,
    /// independently-scheduled hop. So a *trailing* frame from a dying
    /// connection can, in rare cases, still land in `signals` a beat after
    /// that same connection's `false` signal, if the pump hasn't drained it
    /// yet. This never affects the invariant `start` guarantees — that a
    /// *reconnect* always resnapshots before the next event after it is
    /// applied — since it only concerns stale frames from the connection
    /// that just ended, never events from the new one (the new connection's
    /// `true` signal and its own frames still go through this same
    /// synchronous-then-pumped path, in order, relative to each other).
    public static func makeSignalBridge<T: Sendable>(_ type: T.Type) -> (
        onChange: @Sendable (Bool) -> Void,
        continuation: AsyncStream<StreamSignal<T>>.Continuation,
        signals: AsyncStream<StreamSignal<T>>
    ) {
        let (stream, continuation) = AsyncStream<StreamSignal<T>>.makeStream()
        let onChange: @Sendable (Bool) -> Void = { connected in
            continuation.yield(.connection(connected))
        }
        return (onChange, continuation, stream)
    }
}
