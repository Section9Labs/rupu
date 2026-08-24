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

/// Same pattern as `ActivityStoreTests`/`PagedSnapshotTests`' `FlagBox`: a
/// lock-protected boolean a `@Sendable` closure (here, an `AsyncStream`
/// continuation's `onTermination`) can flip from off the main actor.
private final class FlagBox: @unchecked Sendable {
    private let lock = NSLock()
    private var flag = false

    var value: Bool {
        get { lock.withLock { flag } }
        set { lock.withLock { flag = newValue } }
    }
}

private struct FakeEvent: Sendable, Equatable {
    let id: String
}

/// De-flakes "wait for an async background effect to land" — same recipe
/// (and rationale) as `ActivityStoreTests.pollUntil`/`expectEventually`.
/// Fixed-duration sleeps raced the consumer task under this suite's full
/// parallel run (316 tests): `timeout` is generous precisely so a slow
/// scheduler still passes, while a genuinely stuck condition still fails
/// instead of hanging forever.
@MainActor
private func pollUntil(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ condition: () -> Bool
) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while true {
        if condition() { return true }
        if ContinuousClock.now >= deadline { return false }
        try? await Task.sleep(for: interval)
    }
}

/// `pollUntil` plus a descriptive failure on timeout, so a genuine
/// regression reads as "timed out waiting for: ..." rather than a bare
/// boolean mismatch at some unrelated line below.
@MainActor
private func expectEventually(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ description: String,
    sourceLocation: SourceLocation = #_sourceLocation,
    _ condition: () -> Bool
) async {
    let succeeded = await pollUntil(timeout: timeout, interval: interval, condition)
    #expect(succeeded, "timed out waiting for: \(description)", sourceLocation: sourceLocation)
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
    await expectEventually("the lone event is applied, with no resnapshot ever recorded in front of it") {
        log.snapshot == ["apply:e1"]
    }

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

    let expected = ["disconnect", "resnapshot-start", "resnapshot-end", "apply:e1"]
    await expectEventually("resnapshot (including its 20ms sleep) completes before the trailing event is applied") {
        log.snapshot == expected
    }

    #expect(log.snapshot == expected)
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

    let expected = ["resnapshot", "resnapshot", "apply:e1"]
    await expectEventually("both reconnects resnapshot before the trailing event is applied") {
        log.snapshot == expected
    }

    #expect(log.snapshot == expected)

    lifecycle.stop()
    continuation.finish()
}

@MainActor @Test func stopCancelsConsumption() async {
    let log = EventLog()
    let (signals, continuation) = AsyncStream<StreamSignal<FakeEvent>>.makeStream()
    let terminated = FlagBox()
    continuation.onTermination = { _ in terminated.value = true }

    let lifecycle = StreamLifecycle()
    lifecycle.start(
        signals: signals,
        resnapshot: { log.record("resnapshot") },
        apply: { event in log.record("apply:\(event.id)") }
    )

    continuation.yield(.connection(true))
    await expectEventually("the pristine first connect reaches .live before stop() is called") {
        lifecycle.freshness == .live
    }
    #expect(terminated.value == false)

    lifecycle.stop()
    // `stop()`'s `Task.cancel()` only *records* cancellation — Swift's
    // cooperative cancellation means the consumer loop doesn't actually
    // unwind until its next suspension point notices it, which is exactly
    // the "stop/send race" this test regresses on: a fixed sleep here could
    // still lose the race under load. `onTermination` fires precisely when
    // the stream's consumer really is gone — this codebase's established
    // observable for that (cribbed from
    // `ActivityStoreTests.deactivateStopsTheStream`, which polls the same
    // signal for `deactivate()`) — so polling it is a genuine proof, not a
    // longer guess.
    await expectEventually("the consumer task's loop actually unwinds after stop()") {
        terminated.value == true
    }

    continuation.yield(.event(FakeEvent(id: "after-stop")))
    continuation.yield(.connection(false))
    // Short settle: `onTermination` already proves no consumer remains to
    // call `apply`, so nothing here is load-bearing for correctness — it's
    // just deliberate margin before reading the log.
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
        await expectEventually("the pristine first connect is processed while `lifecycle` is still in scope") {
            lifecycle.freshness == .live
        }
    }
    // `lifecycle` just went out of scope with no `stop()` call. Nudge the
    // stream so the consumer task's next loop iteration notices `self` is
    // nil.
    continuation.yield(.connection(false))
    continuation.yield(.event(FakeEvent(id: "nudge")))

    let deallocated = await pollUntil { weakLifecycle == nil }
    #expect(deallocated, "expected `lifecycle` to deallocate once its consumer task's weak `self` capture noticed it was gone")

    continuation.finish()
}
