import Testing
@testable import RupuDesign

/// `RenderMeter` (Plan 5, Task 1) — the DEBUG-only render-frequency seam. `make macos-test` always
/// builds in a Debug configuration, so this test only ever exercises the `#if DEBUG` branch in
/// `RenderMeter.swift`; the release (`#else`) branch is covered by inspection — see that file's
/// doc comment — since it compiles `tick(_:)` down to an empty `@inline(__always)` no-op with no
/// counter/lock/signpost to assert against.
///
/// Kept as one `@Test` (not split per-assertion) deliberately: `RenderMeter`'s counters are
/// process-global state, and Swift Testing runs `@Test` functions concurrently by default —
/// splitting this across multiple tests would race on `reset()`/`tick(_:)` against each other.
@Test func renderMeterCountsPerLabelAndResets() {
    RenderMeter.reset()

    #expect(RenderMeter.count(for: "RenderMeterTests.alpha") == 0)
    RenderMeter.tick("RenderMeterTests.alpha")
    RenderMeter.tick("RenderMeterTests.alpha")
    RenderMeter.tick("RenderMeterTests.alpha")
    #expect(RenderMeter.count(for: "RenderMeterTests.alpha") == 3)

    // Distinct labels count independently.
    RenderMeter.tick("RenderMeterTests.beta")
    RenderMeter.tick("RenderMeterTests.gamma")
    RenderMeter.tick("RenderMeterTests.gamma")
    #expect(RenderMeter.count(for: "RenderMeterTests.beta") == 1)
    #expect(RenderMeter.count(for: "RenderMeterTests.gamma") == 2)
    #expect(RenderMeter.count(for: "RenderMeterTests.alpha") == 3, "unrelated label untouched by beta/gamma ticks")

    RenderMeter.reset()
    #expect(RenderMeter.count(for: "RenderMeterTests.alpha") == 0)
    #expect(RenderMeter.count(for: "RenderMeterTests.beta") == 0)
    #expect(RenderMeter.count(for: "RenderMeterTests.gamma") == 0)
}
