import Testing
@testable import RupuProjects

/// Smoke coverage for the tab vocabulary — the screens themselves (`Root
/// View`-composed SwiftUI) aren't independently unit-testable, same as
/// every other screen module in this codebase (`RupuRunDetail`/
/// `RupuOverview` have no screen-level tests either); the load-bearing
/// logic (sorting, windowing, lazy loading) lives in `RupuStore`'s
/// `ProjectsStore`/`ProjectDetailStore`, covered by `RupuStoreTests`.
@Test func everyProjectDetailTabHasATitle() {
    for tab in ProjectDetailTab.allCases {
        #expect(!tab.title.isEmpty)
    }
}

@Test func projectDetailTabRawValuesAreStableForTaskIDConcatenation() {
    // `ProjectDetailScreen.tabPanel` folds `tab.rawValue` into a `.task(id:)`
    // string key — a value here must stay a plain lowercase identifier
    // (no `|`, since that's the separator `tabPanel` uses to join it with
    // `wsID`) for that id to keep meaning what it says.
    for tab in ProjectDetailTab.allCases {
        #expect(!tab.rawValue.contains("|"))
    }
}
