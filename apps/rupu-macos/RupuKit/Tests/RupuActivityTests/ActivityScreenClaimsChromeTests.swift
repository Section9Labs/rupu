import Testing
@testable import RupuActivity
import RupuStore

/// Exercises `ActivityScreen.isClaimsActive(kind:subTab:)` (review fix,
/// round 1) — the single pure flag that suppresses `FilterBar`'s Runs-only
/// chrome (status chips / live-tail toggle / "+N new runs" pill) and this
/// screen's own stale/pending-hosts stream banners whenever the autoflows
/// kind's Claims sub-tab is the one actually showing, per the no-dead-
/// controls rule (those controls act on `ActivityStore`'s `ActivityTable`,
/// which isn't even on screen at that point). Asserted directly, the same
/// "view-member pure logic gets its own testable static func" idiom
/// `ClaimTableRow`'s seams and `RunDetailScreen.unrecognizedStatusRaw`
/// already establish.
@Suite
@MainActor
struct ActivityScreenClaimsChromeTests {
    @Test func flipsTrueOnlyForAutoflowsKindWithClaimsSubTab() {
        #expect(ActivityScreen.isClaimsActive(kind: .autoflows, subTab: .claims) == true)
    }

    @Test func staysFalseForAutoflowsKindWithRunsSubTab() {
        #expect(ActivityScreen.isClaimsActive(kind: .autoflows, subTab: .runs) == false)
    }

    /// Every non-autoflows kind never activates Claims chrome-suppression,
    /// regardless of what `subTab` happens to hold (that state is only ever
    /// reachable/rendered while `kind == .autoflows`, but the flag itself
    /// must stay correct even if a stale value lingers across a kind
    /// switch).
    @Test func staysFalseForEveryOtherKindRegardlessOfSubTab() {
        for kind in RunKindFilter.allCases where kind != .autoflows {
            #expect(ActivityScreen.isClaimsActive(kind: kind, subTab: .claims) == false, "\(kind) + .claims")
            #expect(ActivityScreen.isClaimsActive(kind: kind, subTab: .runs) == false, "\(kind) + .runs")
        }
    }

    /// The flag flips with the sub-tab alone, `kind` held fixed at
    /// `.autoflows` — the exact transition `autoflowsSubTabPicker`'s
    /// selection binding drives.
    @Test func flipsWithSubTabAloneWhenKindIsHeldFixed() {
        var subTab = AutoflowsSubTab.runs
        #expect(ActivityScreen.isClaimsActive(kind: .autoflows, subTab: subTab) == false)
        subTab = .claims
        #expect(ActivityScreen.isClaimsActive(kind: .autoflows, subTab: subTab) == true)
        subTab = .runs
        #expect(ActivityScreen.isClaimsActive(kind: .autoflows, subTab: subTab) == false)
    }
}
