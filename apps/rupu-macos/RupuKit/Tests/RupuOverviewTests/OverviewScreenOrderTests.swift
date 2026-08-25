import Testing
@testable import RupuOverview

/// `OverviewScreen.mergedBlockOrder(_:)` / `needsYouPrecedesMergedGroup(_:)`
/// are the pure half of Task 3's order-driven rendering — extracted as
/// static funcs on the `View` type itself (same "view-member pure logic
/// gets its own testable static func" idiom `SituationRoomScreen.shouldActivate`
/// and `RunDetailScreen.unrecognizedStatusRaw` already establish), so these
/// are tagged `@MainActor` per this phase's CI rule even though the bodies
/// touch no SwiftUI state.
@Suite struct OverviewScreenOrderTests {
    @Test @MainActor func mergedBlockOrderFiltersOutNeedsYou() {
        let order = ["fleet", "needsYou", "charts", "instruments", "cycles"]

        #expect(OverviewScreen.mergedBlockOrder(order) == ["fleet", "charts", "instruments", "cycles"])
    }

    @Test @MainActor func mergedBlockOrderPreservesAnArbitraryDragOrder() {
        let order = ["cycles", "needsYou", "fleet", "charts", "instruments"]

        #expect(OverviewScreen.mergedBlockOrder(order) == ["cycles", "fleet", "charts", "instruments"])
    }

    @Test @MainActor func needsYouPrecedesTheMergedGroupWhenItsFirst() {
        #expect(OverviewScreen.needsYouPrecedesMergedGroup(OverviewWidgets.defaultOrder))
    }

    @Test @MainActor func needsYouDoesNotPrecedeTheMergedGroupWhenDraggedToTheEnd() {
        let order = ["instruments", "charts", "cycles", "fleet", "needsYou"]

        #expect(!OverviewScreen.needsYouPrecedesMergedGroup(order))
    }

    @Test @MainActor func needsYouPositionFollowsWhicheverMergedBlockComesFirstEvenMidGroup() {
        // "needsYou" dragged between "charts" and "cycles" — the merged
        // group can't actually be split (it shares one loading/error gate),
        // so this counts as "after" the group, since "charts" — the
        // group's first surviving id in this order — comes before it.
        let order = ["charts", "needsYou", "cycles", "instruments", "fleet"]

        #expect(!OverviewScreen.needsYouPrecedesMergedGroup(order))
    }

    @Test @MainActor func needsYouPositionDefaultsToFirstWhenMissingFromOrder() {
        // Shouldn't happen in practice — `OverviewWidgets.normalized(_:)`
        // guarantees the full canonical set — but an honest default beats
        // a crash if that invariant is ever violated.
        #expect(OverviewScreen.needsYouPrecedesMergedGroup(["instruments", "charts"]))
    }
}
