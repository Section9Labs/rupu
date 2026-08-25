import Testing
@testable import RupuSecurity

/// `FindingsWindow`/`FindingsWindowFooter` — the pure, client-side render
/// cap `FindingsTabView.table(_:)` applies after sorting (see
/// `FindingsWindow`'s doc comment for why this exists: GUI validation on a
/// real 385+-row workspace found the un-windowed, un-contained table
/// blowing past its scroll region and breaking the whole shell's layout —
/// `RootView`'s sidebar rail painted blank while Security was frontmost).
/// The layout containment fix itself (`ScrollView` + `LazyVStack`) isn't
/// unit-testable — SwiftUI layout geometry needs a real window/host to
/// measure, which this headless `swift test` target has none of. Manual
/// verification path: `make macos-run`, open Security's Findings tab
/// against a workspace with 200+ findings (or temporarily lower
/// `FindingsWindow.size` for a smaller repro), confirm the table scrolls
/// within its panel, the sidebar stays visible throughout, and the "Show
/// all N findings" footer both appears at the right threshold and reveals
/// every row when tapped.
@Suite struct FindingsWindowTests {
    // MARK: - window(_:showingAll:)

    @Test func windowReturnsEveryRowUnchangedWhenTotalIsAtOrBelowTheCap() {
        let rows = Array(0..<FindingsWindow.size)
        #expect(FindingsWindow.window(rows, showingAll: false) == rows)
    }

    @Test func windowReturnsExactlyTheFirstCapSizeElementsWhenOverTheCap() {
        let rows = Array(0..<(FindingsWindow.size + 50))
        let windowed = FindingsWindow.window(rows, showingAll: false)
        #expect(windowed.count == FindingsWindow.size)
        #expect(windowed == Array(0..<FindingsWindow.size))
    }

    /// Windowing takes the ARRAY'S LEADING elements verbatim — it never
    /// reorders. Combined with `FindingsTabView.table(_:)` always calling
    /// `sortRows` before `FindingsWindow.window`, this is what makes the
    /// visible top-N respect whichever column/direction is active, rather
    /// than an insertion-order slice that happens to ignore sort.
    @Test func windowPreservesInputOrderRatherThanReordering() {
        let sortedDescending = Array(stride(from: 300, through: 1, by: -1))
        let windowed = FindingsWindow.window(sortedDescending, showingAll: false)
        #expect(windowed.first == 300)
        #expect(windowed.last == 300 - FindingsWindow.size + 1)
    }

    @Test func windowShowingAllBypassesTheCapEntirely() {
        let rows = Array(0..<(FindingsWindow.size * 3))
        #expect(FindingsWindow.window(rows, showingAll: true) == rows)
    }

    @Test func windowOnAnEmptyArrayIsEmptyRegardlessOfShowingAll() {
        let empty: [Int] = []
        #expect(FindingsWindow.window(empty, showingAll: false).isEmpty)
        #expect(FindingsWindow.window(empty, showingAll: true).isEmpty)
    }

    // MARK: - WindowFooter.resolve(total:showingAll:)

    @Test func footerIsHiddenWhenTotalIsAtOrBelowTheCap() {
        #expect(FindingsWindowFooter.resolve(total: FindingsWindow.size, showingAll: false) == .hidden)
        #expect(FindingsWindowFooter.resolve(total: FindingsWindow.size - 1, showingAll: false) == .hidden)
        #expect(FindingsWindowFooter.resolve(total: 0, showingAll: false) == .hidden)
    }

    /// The label commits to exactly what tapping it will produce — this
    /// table has no server-side cap standing between "the button" and
    /// "literally every finding" (unlike `ProjectDetailStore`'s
    /// `ShowAllFooter`, which sometimes has to say "show first N of M"
    /// when the real total exceeds what a re-fetch could reach): every
    /// finding is already client-side, so the label always says "all".
    @Test func footerShowsAButtonNamingTheRealTotalWhenOverTheCap() {
        let total = FindingsWindow.size + 185 // 385, matching the real incident's scale
        #expect(FindingsWindowFooter.resolve(total: total, showingAll: false) == .button(label: "Show all 385 findings"))
    }

    @Test func footerFormatsALargeTotalWithGrouping() {
        #expect(FindingsWindowFooter.resolve(total: 1234, showingAll: false) == .button(label: "Show all 1,234 findings"))
    }

    @Test func footerIsHiddenOnceShowingAllEvenWithATotalOverTheCap() {
        #expect(FindingsWindowFooter.resolve(total: FindingsWindow.size + 185, showingAll: true) == .hidden)
    }
}
