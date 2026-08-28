import Testing
import CoreGraphics
@testable import RupuBuilder
import RupuFlowKit

/// Pure-geometry coverage for the canvas (Task 11): `bezierPoints(from:to:)`,
/// `edgeAnchors(edge:nodes:)`, `contentSize(nodes:)`, and `nodeHit(at:nodes:)`
/// — no SwiftUI render pass, same "assert the pure seam, not the body"
/// convention every other screen module's test target follows (see
/// `BuilderScreenWiringTests.swift`'s own file-header comment).

// MARK: - bezierPoints

@Test func bezierPointsClampsToTheMaximumOffset() {
    // dx = 1000 -> dx*0.5 = 500, clamped down to the 120 ceiling.
    let (c1, c2) = bezierPoints(from: CGPoint(x: 0, y: 10), to: CGPoint(x: 1000, y: 40))
    #expect(c1 == CGPoint(x: 120, y: 10))
    #expect(c2 == CGPoint(x: 880, y: 40))
}

@Test func bezierPointsClampsToTheMinimumOffset() {
    // dx = 10 -> dx*0.5 = 5, clamped up to the 40 floor.
    let (c1, c2) = bezierPoints(from: CGPoint(x: 0, y: 0), to: CGPoint(x: 10, y: 0))
    #expect(c1 == CGPoint(x: 40, y: 0))
    #expect(c2 == CGPoint(x: -30, y: 0))
}

@Test func bezierPointsUsesTheHalfDxOffsetInBetween() {
    // dx = 100 -> dx*0.5 = 50, inside [40, 120].
    let (c1, c2) = bezierPoints(from: CGPoint(x: 0, y: 5), to: CGPoint(x: 100, y: 25))
    #expect(c1 == CGPoint(x: 50, y: 5))
    #expect(c2 == CGPoint(x: 50, y: 25))
}

@Test func bezierPointsFloorsTheOffsetEvenForANegativeOrZeroDx() {
    // A backward (loop-back) edge still gets the same [40, 120] treatment —
    // the offset is never negative, so both controls bulge outward.
    let (c1, c2) = bezierPoints(from: CGPoint(x: 400, y: 0), to: CGPoint(x: 0, y: 0))
    #expect(c1 == CGPoint(x: 440, y: 0))
    #expect(c2 == CGPoint(x: -40, y: 0))
}

// MARK: - edgeAnchors

@Test func edgeAnchorsReturnsNilWhenTheSourceNodeIsMissing() {
    let target = GraphNode(id: "t", data: StepNodeData(id: "t", kind: .step), position: .zero)
    let edge = GraphEdge(id: "s->t", source: "s", target: "t")
    #expect(edgeAnchors(edge: edge, nodes: [target]) == nil)
}

@Test func edgeAnchorsReturnsNilWhenTheTargetNodeIsMissing() {
    let source = GraphNode(id: "s", data: StepNodeData(id: "s", kind: .step), position: .zero)
    let edge = GraphEdge(id: "s->t", source: "s", target: "t")
    #expect(edgeAnchors(edge: edge, nodes: [source]) == nil)
}

@Test func edgeAnchorsPlainEdgePicksTheRightSideSourceAndLeftSideTarget() {
    let source = GraphNode(id: "s", data: StepNodeData(id: "s", kind: .step), position: .zero)
    let target = GraphNode(id: "t", data: StepNodeData(id: "t", kind: .step), position: CGPoint(x: 400, y: 0))
    let edge = GraphEdge(id: "s->t", source: "s", target: "t")
    let anchors = edgeAnchors(edge: edge, nodes: [source, target])
    #expect(anchors != nil)
    let (p1, p2) = anchors!
    // `.rect`'s single source sits flush on the right edge (inset 0), at the
    // box's vertical midpoint.
    #expect(p1 == CGPoint(x: NODE_W, y: NODE_H / 2))
    // `.rect`'s target sits flush on the left edge (inset 0) of the SECOND
    // node's own box, offset by its position.
    #expect(p2 == CGPoint(x: 400, y: NODE_H / 2))
}

@Test func edgeAnchorsBranchThenArmExitsTheRightSide() {
    var branchData = StepNodeData(id: "b", kind: .branch)
    branchData.thenTargets = ["t"]
    let branch = GraphNode(id: "b", data: branchData, position: .zero)
    let target = GraphNode(id: "t", data: StepNodeData(id: "t", kind: .step), position: CGPoint(x: 400, y: 300))
    let edge = GraphEdge(id: "b->t:then", source: "b", target: "t", label: "true", branchArm: "then")
    let (p1, _) = edgeAnchors(edge: edge, nodes: [branch, target])!
    #expect(p1 == CGPoint(x: NODE_W, y: NODE_H / 2))
}

@Test func edgeAnchorsBranchElseArmExitsTheBottomSide() {
    var branchData = StepNodeData(id: "b", kind: .branch)
    branchData.elseTargets = ["t"]
    let branch = GraphNode(id: "b", data: branchData, position: CGPoint(x: 20, y: 40))
    let target = GraphNode(id: "t", data: StepNodeData(id: "t", kind: .step), position: CGPoint(x: 400, y: 300))
    let edge = GraphEdge(id: "b->t:else", source: "b", target: "t", label: "false", branchArm: "else")
    let (p1, _) = edgeAnchors(edge: edge, nodes: [branch, target])!
    // `.vhex`'s "else" source is the bottom-side handle: x at the box's
    // horizontal midpoint, y flush with the box's bottom edge (inset 0).
    #expect(p1 == CGPoint(x: 20 + NODE_W / 2, y: 40 + NODE_H))
}

// MARK: - contentSize

@Test func contentSizeFloorsAtTheMinimumCanvasSizeWhenEmpty() {
    #expect(contentSize(nodes: []) == CGSize(width: 1200, height: 800))
}

@Test func contentSizeFloorsAtTheMinimumCanvasSizeForASmallGraph() {
    let node = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 10, y: 10))
    #expect(contentSize(nodes: [node]) == CGSize(width: 1200, height: 800))
}

@Test func contentSizeGrowsPastTheMinimumWithFarNodePositions() {
    let node = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 2000, y: 3000))
    let size = contentSize(nodes: [node])
    #expect(size.width == 2000 + NODE_W + 120)
    #expect(size.height == 3000 + NODE_H + 120)
}

@Test func contentSizeTakesTheMaximumAcrossMultipleNodes() {
    let a = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 2000, y: 10))
    let b = GraphNode(id: "b", data: StepNodeData(id: "b", kind: .step), position: CGPoint(x: 10, y: 3000))
    let size = contentSize(nodes: [a, b])
    #expect(size.width == 2000 + NODE_W + 120)
    #expect(size.height == 3000 + NODE_H + 120)
}

// MARK: - nodeHit

@Test func nodeHitFindsTheNodeWhoseFrameContainsThePoint() {
    let node = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 100, y: 100))
    #expect(nodeHit(at: CGPoint(x: 150, y: 120), nodes: [node]) == "a")
}

@Test func nodeHitReturnsNilOutsideEveryNodeFrame() {
    let node = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 100, y: 100))
    #expect(nodeHit(at: CGPoint(x: 5000, y: 5000), nodes: [node]) == nil)
}

@Test func nodeHitPicksTheLastNodeInArrayOrderOnOverlap() {
    // `a` and `b` overlap; the LAST one in array order (topmost paint order)
    // must win the hit-test.
    let a = GraphNode(id: "a", data: StepNodeData(id: "a", kind: .step), position: CGPoint(x: 0, y: 0))
    let b = GraphNode(id: "b", data: StepNodeData(id: "b", kind: .step), position: CGPoint(x: 50, y: 0))
    let overlapPoint = CGPoint(x: 60, y: 10)
    #expect(nodeHit(at: overlapPoint, nodes: [a, b]) == "b")
    #expect(nodeHit(at: overlapPoint, nodes: [b, a]) == "a")
}

// MARK: - anchorPoint

@Test func anchorPointResolvesLeftSideAtTheFractionAndInset() {
    let frame = CGRect(x: 100, y: 200, width: 176, height: 68)
    let p = anchorPoint(for: HandleAnchor(side: .left, fraction: 0.5, inset: 6), nodeFrame: frame)
    #expect(p == CGPoint(x: 106, y: 234))
}

@Test func anchorPointResolvesRightSideAtTheFractionAndInset() {
    let frame = CGRect(x: 100, y: 200, width: 176, height: 68)
    let p = anchorPoint(for: HandleAnchor(side: .right, fraction: 0.25, inset: 4), nodeFrame: frame)
    #expect(p == CGPoint(x: 272, y: 217))
}

@Test func anchorPointResolvesBottomSideAtTheFractionAndInset() {
    let frame = CGRect(x: 100, y: 200, width: 176, height: 68)
    let p = anchorPoint(for: HandleAnchor(side: .bottom, fraction: 0.75, inset: 2), nodeFrame: frame)
    #expect(p == CGPoint(x: 232, y: 266))
}
