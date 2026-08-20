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
