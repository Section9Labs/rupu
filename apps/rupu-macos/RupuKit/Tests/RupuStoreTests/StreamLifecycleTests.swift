import Testing
import Foundation
@testable import RupuStore

/// Thread-safe append-only log for asserting call ordering across
/// `StreamLifecycle`'s two concurrently-running consumer tasks.
/// `apply`'s declared type is a synchronous `@MainActor` closure (never
/// `async`), so it can't `await` into an `actor`-backed log directly — a
/// lock-protected `@unchecked Sendable` class (the same pattern as
/// `RupuBackendTests.HealthMonitorTests`' `LockedBox`, and this file's own
/// `PagedSnapshotTests.FlagBox`) gives every caller — sync `apply` or
/// async `resnapshot` — the same append/read API without an extra async
/// hop that would itself blur the very ordering being asserted.
private final class EventLog: @unchecked Sendable {
    private let lock = NSLock()
    private var entries: [String] = []

    func record(_ entry: String) {
        lock.withLock { entries.append(entry) }
    }

    var snapshot: [String] {
        lock.withLock { entries }
    }
}

private struct FakeEvent: Sendable, Equatable {
    let id: String
}

private func makeFakeStreams() -> (
    events: AsyncStream<FakeEvent>, eventsContinuation: AsyncStream<FakeEvent>.Continuation,
    connectionEvents: AsyncStream<Bool>, connectionContinuation: AsyncStream<Bool>.Continuation
) {
    let (events, eventsContinuation) = AsyncStream<FakeEvent>.makeStream()
    let (connectionEvents, connectionContinuation) = AsyncStream<Bool>.makeStream()
    return (events, eventsContinuation, connectionEvents, connectionContinuation)
}

@MainActor @Test func firstConnectDoesNotResnapshot() async {
    let log = EventLog()
    let (events, eventsContinuation, connectionEvents, connectionContinuation) = makeFakeStreams()

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        events: events,
        connectionEvents: connectionEvents,
        resnapshot: { log.record("resnapshot") },
        apply: { event in log.record("apply:\(event.id)") }
    )

    connectionContinuation.yield(true)
    try? await Task.sleep(for: .milliseconds(20))
    #expect(lifecycle.freshness == .live)

    eventsContinuation.yield(FakeEvent(id: "e1"))
    try? await Task.sleep(for: .milliseconds(20))

    #expect(log.snapshot == ["apply:e1"])

    lifecycle.stop()
    eventsContinuation.finish()
    connectionContinuation.finish()
}

@MainActor @Test func reconnectResnapshotsBeforeNextApplyEvenWhenEventArrivesImmediately() async {
    let log = EventLog()
    let (events, eventsContinuation, connectionEvents, connectionContinuation) = makeFakeStreams()

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        events: events,
        connectionEvents: connectionEvents,
        resnapshot: {
            log.record("resnapshot-start")
            try? await Task.sleep(for: .milliseconds(30))
            log.record("resnapshot-end")
        },
        apply: { event in log.record("apply:\(event.id)") }
    )

    // First connect: no resnapshot.
    connectionContinuation.yield(true)
    try? await Task.sleep(for: .milliseconds(20))
    eventsContinuation.yield(FakeEvent(id: "e1"))
    try? await Task.sleep(for: .milliseconds(20))

    // Disconnect, then reconnect. The next event is yielded almost
    // immediately after the reconnect signal — racing the resnapshot's
    // 30ms sleep above — to prove `apply` is actually gated on
    // resnapshot's completion, not just incidentally ordered after it.
    connectionContinuation.yield(false)
    try? await Task.sleep(for: .milliseconds(10))
    #expect(lifecycle.freshness == .stale)

    connectionContinuation.yield(true)
    try? await Task.sleep(for: .milliseconds(5))
    eventsContinuation.yield(FakeEvent(id: "e2"))
    try? await Task.sleep(for: .milliseconds(80))

    #expect(log.snapshot == ["apply:e1", "resnapshot-start", "resnapshot-end", "apply:e2"])
    #expect(lifecycle.freshness == .live)

    lifecycle.stop()
    eventsContinuation.finish()
    connectionContinuation.finish()
}

@MainActor @Test func stopCancelsConsumption() async {
    let log = EventLog()
    let (events, eventsContinuation, connectionEvents, connectionContinuation) = makeFakeStreams()

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        events: events,
        connectionEvents: connectionEvents,
        resnapshot: { log.record("resnapshot") },
        apply: { event in log.record("apply:\(event.id)") }
    )

    connectionContinuation.yield(true)
    try? await Task.sleep(for: .milliseconds(20))
    lifecycle.stop()

    eventsContinuation.yield(FakeEvent(id: "after-stop"))
    connectionContinuation.yield(false)
    try? await Task.sleep(for: .milliseconds(20))

    #expect(log.snapshot == [])

    eventsContinuation.finish()
    connectionContinuation.finish()
}

/// Regression-shaped test for the same self→task→closure→self retain
/// pattern `HealthMonitorTests.droppingWithoutStopDoesNotLeak` covers:
/// both consumer tasks capture `self` weakly, so a caller that drops its
/// last strong reference without calling `stop()` must still let the
/// instance deallocate once each task's next loop iteration notices
/// `self` is gone.
@MainActor @Test func deallocatesWithoutExplicitStop() async {
    weak var weakLifecycle: StreamLifecycle?
    let (events, eventsContinuation, connectionEvents, connectionContinuation) = makeFakeStreams()

    do {
        let lifecycle = StreamLifecycle()
        weakLifecycle = lifecycle
        lifecycle.start(
            events: events,
            connectionEvents: connectionEvents,
            resnapshot: {},
            apply: { _ in }
        )
        connectionContinuation.yield(true)
        try? await Task.sleep(for: .milliseconds(20))
    }
    // `lifecycle` just went out of scope with no `stop()` call. Nudge both
    // streams so each consumer task's next loop iteration notices `self`
    // is nil.
    connectionContinuation.yield(false)
    eventsContinuation.yield(FakeEvent(id: "nudge"))
    try? await Task.sleep(for: .milliseconds(50))

    #expect(weakLifecycle == nil)

    eventsContinuation.finish()
    connectionContinuation.finish()
}
