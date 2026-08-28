import SwiftUI
import RupuDesign
import RupuFlowKit

// EdgeLayer — draws every `GraphEdge` as a cubic bezier with an arrowhead,
// plus THEN/ELSE branch-arm labels. Pure geometry (`bezierPoints`/
// `edgeAnchors`) is split out so it's directly testable without a SwiftUI
// render pass; the `Canvas`-based view below is the only consumer.

/// Cubic-bezier control points for a horizontal-ish edge from `from` to
/// `to`: both controls sit at the SAME y as their endpoint, offset
/// horizontally by `min(120, max(40, dx*0.5))` — a floor/ceiling clamp on
/// half the horizontal span, so a very short edge still bulges a visible
/// 40pt and a very long one never bulges past 120pt. The offset is always
/// `>= 0` regardless of `dx`'s sign (a loop-back edge, `to.x < from.x`,
/// still gets the same treatment — both controls bulge OUTWARD from their
/// own endpoint, never inward).
func bezierPoints(from: CGPoint, to: CGPoint) -> (c1: CGPoint, c2: CGPoint) {
    let dx = to.x - from.x
    let offset = min(120, max(40, dx * 0.5))
    let c1 = CGPoint(x: from.x + offset, y: from.y)
    let c2 = CGPoint(x: to.x - offset, y: to.y)
    return (c1, c2)
}

/// Resolves one edge's absolute start/end points against its two nodes'
/// current geometry: `nil` when either endpoint node is missing (a stale
/// edge referencing an already-deleted node — the caller should skip it
/// rather than draw a dangling line). The source anchor is the branch arm's
/// own `SourceAnchor` (falling back to the shape's first/default source if
/// the arm can't be matched — defensive, not expected in practice) for a
/// branch-arm edge, or the node kind's single default source otherwise. The
/// target anchor is always the target node kind's own `target` handle (left
/// side for every shape today, but this stays shape-driven rather than
/// hardcoding `.left` so a future shape with a different target side works
/// without touching this function).
func edgeAnchors(edge: GraphEdge, nodes: [GraphNode]) -> (CGPoint, CGPoint)? {
    guard
        let source = nodes.first(where: { $0.id == edge.source }),
        let target = nodes.first(where: { $0.id == edge.target })
    else { return nil }

    let sourceGeo = shapeFor(kindVisual(source.data.kind).shape, w: NODE_W, h: NODE_H)
    let targetGeo = shapeFor(kindVisual(target.data.kind).shape, w: NODE_W, h: NODE_H)

    let sourceAnchor = sourceGeo.sources.first(where: { $0.arm == edge.branchArm })?.anchor
        ?? sourceGeo.sources.first?.anchor
        ?? HandleAnchor(side: .right)

    let sourceFrame = CGRect(x: source.position.x, y: source.position.y, width: NODE_W, height: NODE_H)
    let targetFrame = CGRect(x: target.position.x, y: target.position.y, width: NODE_W, height: NODE_H)

    let p1 = anchorPoint(for: sourceAnchor, nodeFrame: sourceFrame)
    let p2 = anchorPoint(for: targetGeo.target, nodeFrame: targetFrame)
    return (p1, p2)
}

/// A point at distance `distance` from `from`, along the tangent direction
/// `control - from` — used to place a branch-arm label "~24pt past the
/// source anchor" without walking the actual bezier arc length (the curve
/// is near-tangent to `control - from` right at its start, so this is a
/// close, cheap approximation). Falls back to `from` itself when `control`
/// coincides with it (a degenerate, zero-length tangent).
private func pointAlongTangent(from: CGPoint, control: CGPoint, distance: CGFloat) -> CGPoint {
    let dx = control.x - from.x
    let dy = control.y - from.y
    let length = (dx * dx + dy * dy).squareRoot()
    guard length > 0.0001 else { return from }
    return CGPoint(x: from.x + dx / length * distance, y: from.y + dy / length * distance)
}

/// Unit direction from `a` to `b`; `(1, 0)` when `a == b` (degenerate).
private func unitDirection(from a: CGPoint, to b: CGPoint) -> CGPoint {
    let dx = b.x - a.x
    let dy = b.y - a.y
    let length = (dx * dx + dy * dy).squareRoot()
    guard length > 0.0001 else { return CGPoint(x: 1, y: 0) }
    return CGPoint(x: dx / length, y: dy / length)
}

struct EdgeLayer: View {
    let edges: [GraphEdge]
    let nodes: [GraphNode]

    var body: some View {
        Canvas { context, _ in
            for edge in edges {
                guard let (p1, p2) = edgeAnchors(edge: edge, nodes: nodes) else { continue }
                let (c1, c2) = bezierPoints(from: p1, to: p2)

                var path = Path()
                path.move(to: p1)
                path.addCurve(to: p2, control1: c1, control2: c2)
                context.stroke(path, with: .color(Color.rupuBorderStrong), style: StrokeStyle(lineWidth: 1.5, lineCap: .round))

                // Arrowhead: a 7pt chevron at the target, rotated to the
                // curve's end tangent — the cubic bezier's derivative at
                // t=1 points along `p2 - c2`.
                let tangent = unitDirection(from: c2, to: p2)
                drawArrowhead(context: context, at: p2, direction: tangent)

                if let arm = edge.branchArm {
                    let labelPoint = pointAlongTangent(from: p1, control: c1, distance: 24)
                    let text = Text(arm.uppercased())
                        .font(.dataMono(8))
                        .kerning(0.8)
                        .foregroundColor(Color.status(.done))
                    context.draw(text, at: labelPoint, anchor: .center)
                }
            }
        }
    }

    private func drawArrowhead(context: GraphicsContext, at point: CGPoint, direction: CGPoint) {
        let angle = atan2(direction.y, direction.x)
        let size: CGFloat = 7
        let spread: CGFloat = .pi / 7
        let left = CGPoint(x: point.x - size * cos(angle - spread), y: point.y - size * sin(angle - spread))
        let right = CGPoint(x: point.x - size * cos(angle + spread), y: point.y - size * sin(angle + spread))

        var path = Path()
        path.move(to: left)
        path.addLine(to: point)
        path.addLine(to: right)
        context.stroke(path, with: .color(Color.rupuBorderStrong), style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
    }
}
