import Foundation
import Testing
@testable import RupuOverview

/// `OverviewOrderEditor.moved(_:from:to:)` / `reset(_:)` are the reorder
/// sheet's pure seam — `List`'s `.onMove` and the "Reset Order" button both
/// route through these rather than touching `widgetsData` directly, so the
/// reorder mechanism is verifiable without hosting a SwiftUI hierarchy.
/// `@MainActor` on every test per this phase's CI rule: both are statics on
/// a `View` type.
@Suite struct OverviewOrderEditorTests {
    @Test @MainActor func movedAppliesADragReorderToWidgetsOrder() {
        var widgets = OverviewWidgets()
        widgets.order = ["needsYou", "instruments", "charts", "cycles", "fleet"]

        let moved = OverviewOrderEditor.moved(widgets, from: IndexSet(integer: 0), to: 3)

        #expect(moved.order == ["instruments", "charts", "needsYou", "cycles", "fleet"])
    }

    @Test @MainActor func movedLeavesVisibilityTogglesUntouched() {
        var widgets = OverviewWidgets(instruments: false, fleet: false)
        widgets.order = ["needsYou", "instruments", "charts", "cycles", "fleet"]

        let moved = OverviewOrderEditor.moved(widgets, from: IndexSet(integer: 4), to: 0)

        #expect(moved.instruments == false)
        #expect(moved.fleet == false)
        #expect(moved.needsYou == true)
        #expect(moved.charts == true)
        #expect(moved.cycles == true)
    }

    @Test @MainActor func resetRestoresDefaultOrder() {
        var widgets = OverviewWidgets()
        widgets.order = ["fleet", "needsYou", "charts", "instruments", "cycles"]

        let reset = OverviewOrderEditor.reset(widgets)

        #expect(reset.order == OverviewWidgets.defaultOrder)
    }

    @Test @MainActor func resetLeavesVisibilityTogglesUntouched() {
        var widgets = OverviewWidgets(instruments: false, fleet: false)
        widgets.order = ["fleet", "needsYou", "charts", "instruments", "cycles"]

        let reset = OverviewOrderEditor.reset(widgets)

        #expect(reset.instruments == false)
        #expect(reset.fleet == false)
    }
}
