import Testing
import RupuAPI
@testable import RupuOverview

/// `CycleSummaryFigures.compute(cycles:partial:)` is the pure, testable seam
/// behind `CycleSummaryLine` — a plain struct static, not a `View` member,
/// so none of these need `@MainActor` (CI rule: only tests touching a
/// `View`-type member do). Final-review (Task 6): the controller ruling that
/// `partial` should suffix the clean/with-failures FIGURES with `Fmt.
/// partial`'s trailing `+`, not just drive the line's separate "(partial)"
/// caption.

@Test func computeAppliesPlusSuffixToCleanAndWithFailuresWhenPartial() {
    let cycles = APICycleCounts(total: 8, clean: 6, withFailures: 2)
    let figures = CycleSummaryFigures.compute(cycles: cycles, partial: true)

    #expect(figures.total == "8", "total is always a complete sum — never suffixed, partial or not")
    #expect(figures.clean == "6+")
    #expect(figures.withFailures == "2+")
}

@Test func computeHasNoSuffixOnAnyFigureWhenNotPartial() {
    let cycles = APICycleCounts(total: 8, clean: 6, withFailures: 2)
    let figures = CycleSummaryFigures.compute(cycles: cycles, partial: false)

    #expect(figures.total == "8")
    #expect(figures.clean == "6")
    #expect(figures.withFailures == "2")
}

@Test func computeRendersNilCleanAndWithFailuresAsEmDashRegardlessOfPartial() {
    // `nil` means no host in the merge reported the breakdown at all —
    // there is no sum to mark as incomplete when there isn't a sum, so the
    // em dash never grows a `+`, whether `partial` is set or not.
    let cycles = APICycleCounts(total: 3, clean: nil, withFailures: nil)

    let whenPartial = CycleSummaryFigures.compute(cycles: cycles, partial: true)
    #expect(whenPartial.clean == "—")
    #expect(whenPartial.withFailures == "—")

    let whenNotPartial = CycleSummaryFigures.compute(cycles: cycles, partial: false)
    #expect(whenNotPartial.clean == "—")
    #expect(whenNotPartial.withFailures == "—")
}
