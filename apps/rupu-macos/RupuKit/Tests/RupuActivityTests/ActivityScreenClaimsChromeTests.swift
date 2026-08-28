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

    // MARK: - isCyclesActive (perf & interaction arc, Plan 5 Task 4b) — same
    // contract as `isClaimsActive` above, for the third sub-tab.

    @Test func cyclesFlipsTrueOnlyForAutoflowsKindWithCyclesSubTab() {
        #expect(ActivityScreen.isCyclesActive(kind: .autoflows, subTab: .cycles) == true)
    }

    @Test func cyclesStaysFalseForAutoflowsKindWithRunsOrClaimsSubTab() {
        #expect(ActivityScreen.isCyclesActive(kind: .autoflows, subTab: .runs) == false)
        #expect(ActivityScreen.isCyclesActive(kind: .autoflows, subTab: .claims) == false)
    }

    @Test func cyclesStaysFalseForEveryOtherKindRegardlessOfSubTab() {
        for kind in RunKindFilter.allCases where kind != .autoflows {
            for subTab in AutoflowsSubTab.allCases {
                #expect(ActivityScreen.isCyclesActive(kind: kind, subTab: subTab) == false, "\(kind) + \(subTab)")
            }
        }
    }

    /// The two flags are mutually exclusive for every `(kind, subTab)`
    /// combination — a state where both (or neither, outside `.autoflows`)
    /// claim to be "active" would mean two different Runs-only-chrome
    /// suppressions disagreeing about what's actually on screen.
    @Test func claimsAndCyclesActiveAreNeverBothTrueForTheSameState() {
        for kind in RunKindFilter.allCases {
            for subTab in AutoflowsSubTab.allCases {
                let claims = ActivityScreen.isClaimsActive(kind: kind, subTab: subTab)
                let cycles = ActivityScreen.isCyclesActive(kind: kind, subTab: subTab)
                #expect(!(claims && cycles), "\(kind) + \(subTab)")
            }
        }
    }
}
