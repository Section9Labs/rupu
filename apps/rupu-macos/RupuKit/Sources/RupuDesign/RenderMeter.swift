import Foundation
import os

#if DEBUG
/// DEBUG-only render-frequency instrument (Plan 5, Task 1 — allocation-storm fixes). Meant to sit
/// as one line at the very top of a hot SwiftUI view's `body` — `RenderMeter.tick("Sidebar")` —
/// so a perf pass can see (via `count(for:)` in tests, or the `os_signpost` events in Instruments)
/// how often a given view actually re-renders. Deliberately NOT a general-purpose logging
/// facility: no formatting, no payload, just "this body ran, one more time" — cheap enough to
/// leave inserted at the four call sites this task adds it to (`Sidebar`, `ActivityTable`,
/// `TranscriptFeed`, `StepGraphView`) without materially affecting the very renders it's
/// measuring.
///
/// Release builds never see any of this — see the `#else` branch at the bottom of this file,
/// which compiles every `tick(_:)` call site down to nothing.
public enum RenderMeter {
    private static let signpostLog = OSLog(subsystem: "com.section9labs.rupu", category: "RenderMeter")

    /// Guards `counts` below. A plain `NSLock` (not an actor) because `tick(_:)` must be callable
    /// synchronously from a non-`async` SwiftUI `body`, from whatever thread SwiftUI happens to
    /// evaluate that body on.
    private static let lock = NSLock()

    /// `nonisolated(unsafe)`: mutated only under `lock`, from `tick(_:)`/`reset()` below — the
    /// compiler can't see that discipline, hence the escape hatch, but every access is manually
    /// serialized.
    nonisolated(unsafe) private static var counts: [String: Int] = [:]

    /// Bumps the counter for `label` by one and emits a matching `os_signpost` event so Instruments
    /// can graph render bursts across a session. Safe to call from any thread.
    public static func tick(_ label: StaticString) {
        let key = "\(label)"
        lock.lock()
        counts[key, default: 0] += 1
        lock.unlock()
        os_signpost(.event, log: signpostLog, name: label)
    }

    /// Test-only accessor for the current tick count of `label`. Internal (not `public`) — reaches
    /// only as far as this package's own `@testable import RupuDesign` tests.
    static func count(for label: StaticString) -> Int {
        let key = "\(label)"
        lock.lock()
        defer { lock.unlock() }
        return counts[key] ?? 0
    }

    /// Test-only reset — clears every counted label so tests don't bleed counts into each other.
    /// Internal for the same reason as `count(for:)`.
    static func reset() {
        lock.lock()
        counts = [:]
        lock.unlock()
    }
}
#else
/// Release-build stand-in: every call site compiles to an empty, `@inline(__always)` no-op — no
/// counter, no lock, no signpost. `count(for:)`/`reset()` are deliberately NOT provided here
/// (nothing outside `#if DEBUG` reads them; `RupuDesignTests` — a debug-configuration test
/// target — always sees the `#if DEBUG` branch above, which is what `make macos-test` exercises).
public enum RenderMeter {
    @inline(__always)
    public static func tick(_ label: StaticString) {}
}
#endif
