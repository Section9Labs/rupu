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
