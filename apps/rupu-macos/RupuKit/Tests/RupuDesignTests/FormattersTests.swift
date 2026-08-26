import Testing
@testable import RupuDesign

@Test func nilCountRendersDash() {
    #expect(Fmt.count(nil) == "—")
    #expect(Fmt.count(0) == "0")
    #expect(Fmt.count(1234) == "1,234")
}
@Test func partialSumsAreMarked() {
    #expect(Fmt.partial(12, isPartial: true) == "12+")
    #expect(Fmt.partial(12, isPartial: false) == "12")
}
@Test func cost() {
    #expect(Fmt.cost(nil) == "—")
    #expect(Fmt.cost(0.12) == "$0.12")
    #expect(Fmt.cost(12.5) == "$12.50")
    #expect(Fmt.cost(0) == "$0.00")
}

// MARK: - Equivalence pins (Plan 5, Task 1 — static-formatter cache)
//
// `Fmt.count`/`Fmt.cost` moved from a fresh `NumberFormatter()` per call to a shared, cached
// `static let` instance (see `Formatters.swift`'s doc comments). These pin exact output strings
// for the brief's representative values so a future formatter-config change can't silently drift
// output — including the en_US_POSIX-pinned decimal separator and 2-digit rounding `cost` relies
// on.

@Test func countEquivalencePinnedValues() {
    #expect(Fmt.count(nil) == "—")
    #expect(Fmt.count(0) == "0")
    #expect(Fmt.count(1) == "1")
    #expect(Fmt.count(999) == "999")
    #expect(Fmt.count(1_000) == "1,000")
    #expect(Fmt.count(1_234_567) == "1,234,567")
}

@Test func costEquivalencePinnedValues() {
    #expect(Fmt.cost(nil) == "—")
    #expect(Fmt.cost(0.0) == "$0.00")
    // NumberFormatter's default rounding mode is `.halfEven` ("banker's rounding"): 0.004999
    // rounds down to 2 fraction digits (well under the 0.005 halfway point either way).
    #expect(Fmt.cost(0.004999) == "$0.00")
    // 1.005 isn't exactly representable in binary floating point — it's a hair under the
    // mathematical 1.005, so `.halfEven` rounding lands on "$1.00", not "$1.01". Pinned here so a
    // switch to a different rounding mode (or to raw arithmetic) can't silently change this.
    #expect(Fmt.cost(1.005) == "$1.00")
    #expect(Fmt.cost(1234.56) == "$1234.56")
}

@Test func durations() {
    #expect(Fmt.duration(ms: 850) == "0.9s")
    #expect(Fmt.duration(ms: 4_200) == "4.2s")
    #expect(Fmt.duration(ms: 72_000) == "1m 12s")
    #expect(Fmt.duration(ms: 3_720_000) == "1h 2m")
}

@Test func durationCascadesAtTheSixtySecondBoundary() {
    // Just under: rounds to 59.9s, stays in decisecond display.
    #expect(Fmt.duration(ms: 59_949) == "59.9s")
    // 59_950...59_999ms round to 60.0s at the decisecond level, which must cascade
    // to the minute/second format rather than rendering the out-of-range "60.0s".
    #expect(Fmt.duration(ms: 59_950) == "1m 0s")
    #expect(Fmt.duration(ms: 59_999) == "1m 0s")
    #expect(Fmt.duration(ms: 60_000) == "1m 0s")
}

@Test func durationCascadesAtTheOneHourBoundary() {
    // Just under: rounds down to 3599s = 59m 59s, stays in minute/second display.
    #expect(Fmt.duration(ms: 3_599_499) == "59m 59s")
    // 3_599_500...3_599_999ms round to 3600.0s at the whole-second level, which must
    // cascade to the hour/minute format. `deciseconds` and `totalSeconds` are rounded
    // independently from `ms` here, so this boundary must not inherit any rounding
    // error from the decisecond branch above it.
    #expect(Fmt.duration(ms: 3_599_500) == "1h 0m")
    #expect(Fmt.duration(ms: 3_599_999) == "1h 0m")
    #expect(Fmt.duration(ms: 3_600_000) == "1h 0m")
}
