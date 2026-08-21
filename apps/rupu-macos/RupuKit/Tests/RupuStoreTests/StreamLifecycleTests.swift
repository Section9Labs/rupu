import Testing
import Foundation
@testable import RupuStore

/// Thread-safe append-only log for asserting call ordering. `apply`'s
/// declared type is a synchronous `@MainActor` closure (never `async`), so
/// it can't `await` into an `actor`-backed log directly — a lock-protected
/// `@unchecked Sendable` class (the same pattern as
/// `RupuBackendTests.HealthMonitorTests`' `LockedBox`, and
/// `PagedSnapshotTests`' `FlagBox`/`CountBox`) gives every caller — sync
/// `apply` or async `resnapshot`, or the test driver itself — the same
/// append/read API without an extra async hop that would itself blur the
/// very ordering being asserted.
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

@MainActor @Test func firstConnectDoesNotResnapshot() async {
    let log = EventLog()
    let (signals, continuation) = AsyncStream<StreamSignal<FakeEvent>>.makeStream()

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        signals: signals,
        resnapshot: { log.record("resnapshot") },
        apply: { event in log.record("apply:\(event.id)") }
    )

    continuation.yield(.connection(true))
    continuation.yield(.event(FakeEvent(id: "e1")))
    try? await Task.sleep(for: .milliseconds(30))

    #expect(log.snapshot == ["apply:e1"])
    #expect(lifecycle.freshness == .live)

    lifecycle.stop()
    continuation.finish()
}

/// Regression test for the exact race the two-independent-`AsyncStream`
/// design used to have: `.connection(false)`, `.connection(true)`, and
/// `.event` are yielded into the SAME stream back-to-back with zero delay
/// between them — nothing paces the producer to "help" the consumer catch
/// up. `resnapshot()` itself is slow (20ms sleep) so a buggy
/// implementation that let the event slip through ungated would show
/// `apply` before `resnapshot-end` in the log. `start`'s single-stream,
/// single-consumer construction must get this right regardless of timing,
/// not because of it.
///
/// `"disconnect"` is appended to the log by the test itself, synchronously,
/// in the same breath as yielding `.connection(false)` — before the
/// consumer task has had any scheduling opportunity to run at all — so it
/// is deterministically first without needing a sleep either.
@MainActor @Test func reconnectResnapshotsBeforeNextApplyWithZeroDelayBetweenSignals() async {
    let log = EventLog()
    let (signals, continuation) = AsyncStream<StreamSignal<FakeEvent>>.makeStream()

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        signals: signals,
        resnapshot: {
            log.record("resnapshot-start")
            try? await Task.sleep(for: .milliseconds(20))
            log.record("resnapshot-end")
        },
        apply: { event in log.record("apply:\(event.id)") }
    )

    log.record("disconnect")
    continuation.yield(.connection(false))
    continuation.yield(.connection(true))
    continuation.yield(.event(FakeEvent(id: "e1")))
    try? await Task.sleep(for: .milliseconds(60))

    #expect(log.snapshot == ["disconnect", "resnapshot-start", "resnapshot-end", "apply:e1"])
    #expect(lifecycle.freshness == .live)

    lifecycle.stop()
    continuation.finish()
}

/// A second, independent reconnect (disconnect -> connect again) after the
/// first must resnapshot again too — "any reconnect", not just the first
/// one after the pristine initial connect.
@MainActor @Test func everyReconnectResnapshotsNotJustTheFirst() async {
    let log = EventLog()
    let (signals, continuation) = AsyncStream<StreamSignal<FakeEvent>>.makeStream()

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        signals: signals,
        resnapshot: { log.record("resnapshot") },
        apply: { event in log.record("apply:\(event.id)") }
    )

    continuation.yield(.connection(true)) // pristine first connect: no resnapshot
    continuation.yield(.connection(false))
    continuation.yield(.connection(true)) // reconnect #1: resnapshot
    continuation.yield(.connection(false))
    continuation.yield(.connection(true)) // reconnect #2: resnapshot
    continuation.yield(.event(FakeEvent(id: "e1")))
    try? await Task.sleep(for: .milliseconds(30))

    #expect(log.snapshot == ["resnapshot", "resnapshot", "apply:e1"])

    lifecycle.stop()
    continuation.finish()
}

@MainActor @Test func stopCancelsConsumption() async {
    let log = EventLog()
    let (signals, continuation) = AsyncStream<StreamSignal<FakeEvent>>.makeStream()

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        signals: signals,
        resnapshot: { log.record("resnapshot") },
        apply: { event in log.record("apply:\(event.id)") }
    )

    continuation.yield(.connection(true))
    try? await Task.sleep(for: .milliseconds(20))
    lifecycle.stop()

    continuation.yield(.event(FakeEvent(id: "after-stop")))
    continuation.yield(.connection(false))
    try? await Task.sleep(for: .milliseconds(20))

    #expect(log.snapshot == [])

    continuation.finish()
}

/// Regression-shaped test for the same self->task->closure->self retain
/// pattern `HealthMonitorTests.droppingWithoutStopDoesNotLeak` covers: the
/// consumer task captures `self` weakly, so a caller that drops its last
/// strong reference without calling `stop()` must still let the instance
/// deallocate once the task's next loop iteration notices `self` is gone.
@MainActor @Test func deallocatesWithoutExplicitStop() async {
    weak var weakLifecycle: StreamLifecycle?
    let (signals, continuation) = AsyncStream<StreamSignal<FakeEvent>>.makeStream()

    do {
        let lifecycle = StreamLifecycle()
        weakLifecycle = lifecycle
        lifecycle.start(signals: signals, resnapshot: {}, apply: { _ in })
        continuation.yield(.connection(true))
        try? await Task.sleep(for: .milliseconds(20))
    }
    // `lifecycle` just went out of scope with no `stop()` call. Nudge the
    // stream so the consumer task's next loop iteration notices `self` is
    // nil.
    continuation.yield(.connection(false))
    continuation.yield(.event(FakeEvent(id: "nudge")))
    try? await Task.sleep(for: .milliseconds(50))

    #expect(weakLifecycle == nil)

    continuation.finish()
}
