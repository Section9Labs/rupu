import Testing
import CoreGraphics
@testable import RupuBuilder
import RupuFlowKit

/// Pure-helper coverage for the Palette tab (Task 12): `filteredCatalog
/// (query:catalog:)`, `catalogSections(_:)` (the WORK/ORCHESTRATION split),
/// `addToCanvasDropPoint(nodes:)`, and `canvasPoint(fromBuilderPoint:
/// canvasFrame:scrollOffset:)` — no SwiftUI render pass, same "assert the
/// pure seam, not the body" convention `CanvasGeometryTests.swift` follows.

// MARK: - filteredCatalog

@Test func filteredCatalogReturnsAllTenEntriesForAnEmptyQuery() {
    #expect(filteredCatalog(query: "").count == 10)
}

@Test func filteredCatalogTreatsAWhitespaceOnlyQueryAsEmpty() {
    #expect(filteredCatalog(query: "   ").count == 10)
}

@Test func filteredCatalogMatchesByLabelCaseInsensitively() {
    // "panel" appears nowhere else in the catalog's labels/taglines/what
    // blurbs (unlike "step", which shows up inside several other entries'
    // what-blurbs — "sub-steps", "next steps", etc.) — an unambiguous
    // single-entry match on the LABEL itself.
    let results = filteredCatalog(query: "PANEL")
    #expect(results.map(\.kind) == [.panel])
}

@Test func filteredCatalogMatchesByKindTaglineCaseInsensitively() {
    // `for_each`'s tagline (KindVisuals.swift) is "over a list" — no other
    // entry's label/tagline/what contains this exact phrase.
    let results = filteredCatalog(query: "OVER A LIST")
    #expect(results.map(\.kind) == [.forEach])
}

@Test func filteredCatalogMatchesByWhatBlurbCaseInsensitively() {
    // Unique substring of `approval_gate`'s `what` blurb.
    let results = filteredCatalog(query: "AUTO-APPROVE FROM AN EXPRESSION")
    #expect(results.map(\.kind) == [.approvalGate])
}

@Test func filteredCatalogReturnsEmptyWhenNothingMatches() {
    #expect(filteredCatalog(query: "zzz-nonexistent-zzz").isEmpty)
}

@Test func filteredCatalogPreservesCatalogOrderAmongMatches() {
    // "a" appears in several labels/taglines; the result must stay in
    // `blockCatalog`'s own order, not get reshuffled.
    let results = filteredCatalog(query: "a")
    #expect(results.map(\.kind) == blockCatalog.filter { entry in
        entry.label.lowercased().contains("a")
            || kindVisual(entry.kind).tagline.lowercased().contains("a")
            || entry.what.lowercased().contains("a")
    }.map(\.kind))
}

// MARK: - catalogSections

@Test func catalogSectionsSplitsAllTenKindsExactlyOnceByFamily() {
    let sections = catalogSections(blockCatalog)
    let all = sections.work.map(\.kind) + sections.orchestration.map(\.kind)
    #expect(all.count == 10)
    #expect(Set(all).count == 10)
}

@Test func catalogSectionsWorkMembershipMatchesTheSpecTable() {
    let sections = catalogSections(blockCatalog)
    #expect(Set(sections.work.map(\.kind)) == Set([.step, .forEach, .parallel, .panel, .action, .run]))
}

@Test func catalogSectionsOrchestrationMembershipMatchesTheSpecTable() {
    let sections = catalogSections(blockCatalog)
    #expect(Set(sections.orchestration.map(\.kind)) == Set([.branch, .split, .join, .approvalGate]))
}

@Test func catalogSectionsPreservesBlockCatalogOrderWithinEachSection() {
    let sections = catalogSections(blockCatalog)
    #expect(sections.work.map(\.kind) == [.step, .forEach, .parallel, .panel, .action, .run])
    #expect(sections.orchestration.map(\.kind) == [.branch, .approvalGate, .split, .join])
}

@Test func catalogSectionsEveryEntryAgreesWithItsOwnKindVisualFamily() {
    // Membership is DERIVED from `kindVisual(_:).family` — never a locally
    // hardcoded kind list — so this must hold for every entry regardless of
    // the table above.
    let sections = catalogSections(blockCatalog)
    for entry in sections.work {
        #expect(kindVisual(entry.kind).family == .work)
    }
    for entry in sections.orchestration {
        #expect(kindVisual(entry.kind).family == .orchestration)
    }
}

// MARK: - addToCanvasDropPoint

@Test func addToCanvasDropPointIsAFixedPointWhenTheGraphIsEmpty() {
    #expect(addToCanvasDropPoint(nodes: []) == CGPoint(x: 200, y: 200))
}

@Test func addToCanvasDropPointSitsPastTheRightmostNodeWithClearance() {
    let a = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 300, y: 50))
    let b = GraphNode(id: "b", data: StepNodeData(id: "b", kind: .step), position: CGPoint(x: 700, y: 90))
    #expect(addToCanvasDropPoint(nodes: [a, b]) == CGPoint(x: 700 + 260, y: 200))
}

@Test func addToCanvasDropPointIgnoresNodeYPositions() {
    let a = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 500, y: 9000))
    #expect(addToCanvasDropPoint(nodes: [a]) == CGPoint(x: 760, y: 200))
}

// MARK: - canvasPoint(fromBuilderPoint:canvasFrame:scrollOffset:)

@Test func canvasPointReturnsNilOutsideTheCanvasFrame() {
    let frame = CGRect(x: 320, y: 46, width: 800, height: 600)
    #expect(canvasPoint(fromBuilderPoint: CGPoint(x: 10, y: 10), canvasFrame: frame, scrollOffset: .zero) == nil)
}

@Test func canvasPointConvertsAnInsideFramePointToLocalCoordinates() {
    let frame = CGRect(x: 320, y: 46, width: 800, height: 600)
    let p = canvasPoint(fromBuilderPoint: CGPoint(x: 420, y: 146), canvasFrame: frame, scrollOffset: .zero)
    #expect(p == CGPoint(x: 100, y: 100))
}

@Test func canvasPointAddsTheScrollOffsetWhenGiven() {
    let frame = CGRect(x: 0, y: 0, width: 800, height: 600)
    let p = canvasPoint(fromBuilderPoint: CGPoint(x: 50, y: 50), canvasFrame: frame, scrollOffset: CGPoint(x: 200, y: 300))
    #expect(p == CGPoint(x: 250, y: 350))
}

@Test func canvasPointClampsANegativeResultToZero() {
    let frame = CGRect(x: 0, y: 0, width: 800, height: 600)
    let p = canvasPoint(fromBuilderPoint: CGPoint(x: 0, y: 0), canvasFrame: frame, scrollOffset: CGPoint(x: -50, y: -50))
    #expect(p == CGPoint(x: 0, y: 0))
}

@Test func canvasPointTreatsTheFrameOriginAsInside() {
    let frame = CGRect(x: 0, y: 0, width: 800, height: 600)
    #expect(canvasPoint(fromBuilderPoint: CGPoint(x: 0, y: 0), canvasFrame: frame, scrollOffset: .zero) != nil)
}

@Test func canvasPointReturnsNilExactlyAtTheFramesFarEdge() {
    // `CGRect.contains` is half-open (max-exclusive) — a point exactly on
    // the far edge is outside.
    let frame = CGRect(x: 0, y: 0, width: 800, height: 600)
    #expect(canvasPoint(fromBuilderPoint: CGPoint(x: 800, y: 300), canvasFrame: frame, scrollOffset: .zero) == nil)
}
