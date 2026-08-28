@preconcurrency import CoreGraphics

// nodeShapes — pure silhouette geometry for the Flow Designer's `next`
// nodes. Line-for-line port of `crates/rupu-cp/web/src/components/
// workflow-editor/nodeShapes.ts`.
//
// Each node KIND paints a flowchart symbol (see KindVisuals.kindVisual):
// step→rect, branch→vhex, action→parallelogram, approval_gate→trapezoid,
// for_each→hexagon, parallel→subroutine, panel→stacked, split→fanout,
// join→fanin. This module owns the geometry only — no SwiftUI, no colour.
// The view layer paints `path` (plus `extra`) into a layer and positions its
// content inside `safe`.
//
// Two rules encoded here, both from the approved design:
//  1. `safe` is inscribed at the shape's NARROWEST row, so text can never
//     overrun the outline (truncation is bounded by the safe rect, not the
//     bounding box).
//  2. `centered` is part of the shape. A silhouette whose width varies
//     across the text band (the vhex) CENTRES its content — left-aligned
//     text there starts on the slope and reads as spilling outside the
//     outline.

/// Stroke inset, in px — keeps the silhouette stroke off the box edge so it
/// is never half-clipped.
private let I: CGFloat = 2
/// Corner radius of the plain `rect` silhouette (matches the old `.wfx-node`).
private let R: CGFloat = 12
/// Horizontal shear of a parallelogram, per side.
private let SHEAR: CGFloat = 20
/// How far a trapezoid's top edge is inset, per side.
private let TAPER: CGFloat = 26
/// How far a hexagon's left/right points reach in from the box edge.
private let POINT: CGFloat = 22
/// How far a vhex's top/bottom points reach in from the box edge — the same
/// role as `POINT`, rotated 90°: `POINT` insets the hexagon's flat top/bottom
/// edge from the LEFT/RIGHT box edges; `Q` insets the vhex's flat left/right
/// edge from the TOP/BOTTOM box edges. Same self-intersection risk, so it
/// goes through the same `clampInset`/`EDGE_CLAMP_FRACTION` treatment, just
/// against `h` instead of `w`.
private let Q: CGFloat = 22
/// Inset of a subroutine's two vertical rails from the box edge.
private let RAIL: CGFloat = 11
/// Offset of a stacked shape's layers behind its body.
private let LAYER: CGFloat = 9
/// How far a fanout/fanin's single point's OPPOSITE corners are pulled in
/// from the box edge — the fan-shape counterpart of `POINT`/`TAPER`, but
/// cutting from only ONE side of the box (the pointed side) rather than
/// both, since the other side stays a flat, full-height edge.
private let FAN: CGFloat = 30

/// Fraction used for the four insets that cut in from BOTH sides of an edge
/// (`SHEAR`/`TAPER`/`POINT`/`RAIL`/`Q`) — the ones that self-intersect into a
/// bowtie if pushed past 0.5. `0.3` was measured empirically as the point
/// where the palette's 34x20 preview keeps a flat edge wide enough to read
/// as a hexagon/trapezoid rather than degenerating to a diamond/triangle at
/// the true 24x14 CSS display size, while staying well clear of the `0.5`
/// self-intersection boundary. No-op at every real node box.
private let EDGE_CLAMP_FRACTION: CGFloat = 0.3

/// Fraction used for `LAYER` (the `stacked` shape's layer offset). `LAYER`
/// is not an edge inset — it does not cut in from both sides of the same
/// edge, so it cannot invert the polygon's vertex order the way
/// `EDGE_CLAMP_FRACTION`'s four constants can. Its only correctness
/// requirement is that the offset stays smaller than the box itself; `0.5`
/// of the inner span satisfies that with wide margin.
private let LAYER_CLAMP_FRACTION: CGFloat = 0.5

/// Clamp a horizontal/vertical inset constant (`SHEAR`/`TAPER`/`POINT`/
/// `RAIL`/`Q`) to what a small box can actually hold, the same way
/// `roundedRectPath`'s `rad` is clamped below. Each of those constants cuts
/// in from a box edge on BOTH sides; once the box is narrower than roughly
/// `2*CONST + 2*I`, the two insets overlap and the vertex order the shape
/// depends on reverses — a hexagon/trapezoid's top edge runs backwards,
/// producing the self-intersecting bowtie this clamp exists to prevent.
func clampInset(_ value: CGFloat, _ dim: CGFloat, _ fraction: CGFloat) -> CGFloat {
    min(value, (dim - 2 * I) * fraction)
}

/// The flowchart symbol a node kind paints. Geometry lives in `shapeFor`.
public enum ShapeName: String, Sendable, CaseIterable {
    case rect
    case vhex
    case parallelogram
    case trapezoid
    case hexagon
    case subroutine
    case stacked
    case fanout
    case fanin
}

/// Where content may live, in box coordinates.
public struct SafeRect: Equatable, Sendable {
    public var x: CGFloat
    public var y: CGFloat
    public var w: CGFloat
    public var h: CGFloat

    public init(x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat) {
        self.x = x
        self.y = y
        self.w = w
        self.h = h
    }
}

public enum HandleSide: String, Sendable {
    case left
    case right
    case bottom
}

/// A handle position expressed against the box, not a hardcoded percentage
/// of a rectangle. `fraction` replaces the CSS `'50%'` offset (always 0.5
/// today) applied along `side` (top-to-bottom for left/right, left-to-right
/// for bottom).
public struct HandleAnchor: Equatable, Sendable {
    public var side: HandleSide
    public var fraction: CGFloat
    /// Perpendicular inset, in px, in from the box edge along `side` — e.g.
    /// for `side: .right` this becomes a `right: <inset>px` offset instead
    /// of the default `right: 0`. Needed by any shape whose boundary at the
    /// anchored offset is not flush with the box edge (a slanted or
    /// narrowed side). Default 0 (flush with the box edge) when omitted.
    public var inset: CGFloat

    public init(side: HandleSide, fraction: CGFloat = 0.5, inset: CGFloat = 0) {
        self.side = side
        self.fraction = fraction
        self.inset = inset
    }
}

/// A source handle. `arm` is omitted (`nil`) for the single default source;
/// reports the two branch arms ("then"/"else") otherwise — a MODEL CONTRACT
/// (the connect logic reads them) even though their positions are
/// shape-derived.
public struct SourceAnchor: Equatable, Sendable {
    public var arm: String?
    public var anchor: HandleAnchor

    public init(arm: String? = nil, anchor: HandleAnchor) {
        self.arm = arm
        self.anchor = anchor
    }
}

public struct NodeShapeGeometry: Sendable {
    /// Silhouette vertices. `rect` reports its un-rounded corners.
    public var points: [CGPoint]
    /// The filled+stroked silhouette (rounded for `.rect`; the polygon
    /// otherwise).
    public var path: CGPath
    /// Extra paths stroked (never filled) on top — rails, stack layers.
    public var extra: [CGPath]
    public var safe: SafeRect
    /// Whether content should be centered rather than left-aligned — true
    /// only for `.vhex`, whose width varies across the text band.
    public var centered: Bool
    public var target: HandleAnchor
    public var sources: [SourceAnchor]
}

private func polygonPoints(_ points: [(CGFloat, CGFloat)]) -> [CGPoint] {
    points.map { CGPoint(x: $0.0, y: $0.1) }
}

private func polygonPath(_ points: [CGPoint]) -> CGPath {
    let path = CGMutablePath()
    guard let first = points.first else { return path }
    path.move(to: first)
    for p in points.dropFirst() { path.addLine(to: p) }
    path.closeSubpath()
    return path
}

private func linePath(_ a: CGPoint, _ b: CGPoint) -> CGPath {
    let path = CGMutablePath()
    path.move(to: a)
    path.addLine(to: b)
    return path
}

/// Rounded rectangle — the only silhouette whose painted path differs from
/// its polygon (the polygon is the un-rounded box, used for geometry
/// tests).
///
/// The corner radius `R` is fixed at 12px, sized for real node boxes
/// (~176x68+). Painted naively at a small box — e.g. the 34x20
/// palette-chip preview — `R` no longer fits: the straight run between two
/// corner curves would need to run backwards once the box is shorter than
/// `2*R + 2*I`. Clamping to what the box can actually hold keeps the curve
/// monotonic at any size and is a no-op at real node sizes.
private func roundedRectPath(_ w: CGFloat, _ h: CGFloat) -> CGPath {
    let l = I
    let t = I
    let r = w - I
    let b = h - I
    let rad = min(R, (w - 2 * I) / 2, (h - 2 * I) / 2)
    let path = CGMutablePath()
    path.move(to: CGPoint(x: l + rad, y: t))
    path.addLine(to: CGPoint(x: r - rad, y: t))
    path.addQuadCurve(to: CGPoint(x: r, y: t + rad), control: CGPoint(x: r, y: t))
    path.addLine(to: CGPoint(x: r, y: b - rad))
    path.addQuadCurve(to: CGPoint(x: r - rad, y: b), control: CGPoint(x: r, y: b))
    path.addLine(to: CGPoint(x: l + rad, y: b))
    path.addQuadCurve(to: CGPoint(x: l, y: b - rad), control: CGPoint(x: l, y: b))
    path.addLine(to: CGPoint(x: l, y: t + rad))
    path.addQuadCurve(to: CGPoint(x: l + rad, y: t), control: CGPoint(x: l, y: t))
    path.closeSubpath()
    return path
}

private let LEFT_TARGET = HandleAnchor(side: .left)
private let RIGHT_SOURCE: [SourceAnchor] = [SourceAnchor(anchor: HandleAnchor(side: .right))]

/// Geometry for one silhouette at a given box size. Pure.
public func shapeFor(_ shape: ShapeName, w: CGFloat, h: CGFloat) -> NodeShapeGeometry {
    switch shape {
    case .vhex:
        // A vertical hexagon: points on the TOP/BOTTOM (a "decision" shape),
        // distinct from `hexagon`'s points on the LEFT/RIGHT (`for_each`). It
        // is `hexagon`'s geometry rotated 90°: `Q` (vertical) plays the role
        // `POINT` (horizontal) plays there, so it gets the same clamp
        // against the axis it cuts into from both ends — here `h`, not `w`.
        let q = clampInset(Q, h, EDGE_CLAMP_FRACTION)
        let points = polygonPoints([
            (I, q),
            (w / 2, I),
            (w - I, q),
            (w - I, h - q),
            (w / 2, h - I),
            (I, h - q),
        ])
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [],
            // The flat band ([q, h-q]) is exactly where the shape reaches
            // full width — the top/bottom tip triangles converge to the
            // box's full width AT y=q/y=h-q, not past it, so the safe
            // rect's y/h can use that band directly with no extra vertical
            // margin needed (unlike a diamond, whose width shrinks
            // continuously toward its tips). Only the horizontal pad is
            // tunable: ~11px in from each flat side, giving an 11px
            // clearance to the outline — comfortable slack for a realistic
            // branch body (kindpill+id header, `if <condition>` line,
            // true/false port pills).
            safe: SafeRect(x: I + 11, y: q, w: w - 2 * I - 22, h: h - 2 * q),
            centered: true,
            target: LEFT_TARGET,
            sources: [
                SourceAnchor(arm: "then", anchor: HandleAnchor(side: .right)),
                SourceAnchor(arm: "else", anchor: HandleAnchor(side: .bottom)),
            ]
        )

    case .parallelogram:
        let shear = clampInset(SHEAR, w, EDGE_CLAMP_FRACTION)
        let points = polygonPoints([
            (shear, I),
            (w - I, I),
            (w - shear, h - I),
            (I, h - I),
        ])
        // Both slanted sides (p1->p2 on the right, p3->p0 on the left) cross
        // y=h/2 at their parameter midpoint (t=0.5, since y runs linearly
        // from I to h-I and h/2 is that range's own midpoint) — giving
        // boundary x = w - (shear+I)/2 on the right, (shear+I)/2 on the
        // left. Both handles sit at the box edge (x=0 or x=w) by default,
        // so the inset needed to land back on the boundary is the same
        // (shear+I)/2 on both sides.
        let inset = (shear + I) / 2
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [],
            safe: SafeRect(x: shear + 8, y: 11, w: w - 2 * shear - 16, h: h - 22),
            centered: false,
            target: HandleAnchor(side: .left, inset: inset),
            sources: [SourceAnchor(anchor: HandleAnchor(side: .right, inset: inset))]
        )

    case .trapezoid:
        let taper = clampInset(TAPER, w, EDGE_CLAMP_FRACTION)
        let points = polygonPoints([
            (taper, I),
            (w - taper, I),
            (w - I, h - I),
            (I, h - I),
        ])
        // Same midpoint argument as parallelogram above, applied to the
        // trapezoid's own slanted sides: boundary x at y=h/2 is
        // w - (taper+I)/2 on the right, (taper+I)/2 on the left.
        let inset = (taper + I) / 2
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [],
            safe: SafeRect(x: taper + 7, y: 13, w: w - 2 * taper - 14, h: h - 26),
            centered: false,
            target: HandleAnchor(side: .left, inset: inset),
            sources: [SourceAnchor(anchor: HandleAnchor(side: .right, inset: inset))]
        )

    case .hexagon:
        let point = clampInset(POINT, w, EDGE_CLAMP_FRACTION)
        let points = polygonPoints([
            (point, I),
            (w - point, I),
            (w - I, h / 2),
            (w - point, h - I),
            (point, h - I),
            (I, h / 2),
        ])
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [],
            safe: SafeRect(x: point + 7, y: 11, w: w - 2 * point - 14, h: h - 22),
            centered: false,
            target: LEFT_TARGET,
            sources: RIGHT_SOURCE
        )

    case .subroutine:
        let rail = clampInset(RAIL, w, EDGE_CLAMP_FRACTION)
        let points = polygonPoints([
            (I, I),
            (w - I, I),
            (w - I, h - I),
            (I, h - I),
        ])
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [
                linePath(CGPoint(x: rail, y: I), CGPoint(x: rail, y: h - I)),
                linePath(CGPoint(x: w - rail, y: I), CGPoint(x: w - rail, y: h - I)),
            ],
            safe: SafeRect(x: rail + 8, y: 11, w: w - 2 * rail - 16, h: h - 22),
            centered: false,
            target: LEFT_TARGET,
            sources: RIGHT_SOURCE
        )

    case .stacked:
        // body sits down-left; the layers peek out up-right. LAYER offsets
        // one side of BOTH axes (not two, unlike SHEAR/TAPER/POINT/RAIL
        // above), so it is clamped against each axis independently and the
        // tighter of the two wins — at the 34x20 palette box, height (20)
        // is the binding constraint, not width (34).
        let layer = min(
            clampInset(LAYER, w, LAYER_CLAMP_FRACTION),
            clampInset(LAYER, h, LAYER_CLAMP_FRACTION)
        )
        let points = polygonPoints([
            (I, layer + I),
            (w - layer - I, layer + I),
            (w - layer - I, h - I),
            (I, h - I),
        ])
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [
                polygonPathOpen([
                    CGPoint(x: layer, y: I + 3),
                    CGPoint(x: w - I - 3, y: I + 3),
                    CGPoint(x: w - I - 3, y: h - layer),
                ]),
                polygonPathOpen([
                    CGPoint(x: layer - 3, y: I + 6),
                    CGPoint(x: w - I - 6, y: I + 6),
                    CGPoint(x: w - I - 6, y: h - layer - 3),
                ]),
            ],
            safe: SafeRect(x: 13, y: layer + 10, w: w - layer - 24, h: h - layer - 21),
            centered: false,
            target: LEFT_TARGET,
            // The body rect's LEFT edge is at x=I, same as every other
            // shape's un-inset side (rect/hexagon/subroutine all sit I off
            // their box edge too) — no inset needed there. Its RIGHT edge,
            // though, is pulled in by `layer` on top of the usual `I` (to
            // leave room for the stack layers peeking out), so the default
            // right:0 handle lands on the decorative layer stroke, not the
            // body — inset by `layer + I`.
            sources: [SourceAnchor(anchor: HandleAnchor(side: .right, inset: layer + I))]
        )

    case .fanout:
        // split ("one in, many out"): a flat vertical left edge (the single
        // inbound side, un-tapered — same box-edge flush left edge as
        // `rect`) fanning to a single point on the right. Placeholder
        // geometry standing in for a real fan-out symbol; a later pass may
        // refine the exact silhouette.
        let fan = clampInset(FAN, w, EDGE_CLAMP_FRACTION)
        let points = polygonPoints([
            (I, I),
            (w - fan, I),
            (w - I, h / 2),
            (w - fan, h - I),
            (I, h - I),
        ])
        // The right boundary is narrowest (x = w-fan) at y=I/y=h-I and
        // widens toward the point (x = w-I) at y=h/2 — so a safe rect
        // spanning the FULL height (like `rect`'s) stays inside at every
        // row as long as its right edge clears `w-fan` (the global minimum
        // of that boundary), with the left edge flush like `rect`'s (no
        // left-side taper here).
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [],
            safe: SafeRect(x: 15, y: 11, w: w - fan - 22, h: h - 22),
            centered: false,
            target: LEFT_TARGET,
            sources: RIGHT_SOURCE
        )

    case .fanin:
        // join ("many in, one out"): the mirror of `fanout` — a single
        // point on the left fanning in from a flat vertical right edge
        // (the single outbound side). Same placeholder status as `fanout`
        // above.
        let fan = clampInset(FAN, w, EDGE_CLAMP_FRACTION)
        let points = polygonPoints([
            (w - I, I),
            (fan, I),
            (I, h / 2),
            (fan, h - I),
            (w - I, h - I),
        ])
        return NodeShapeGeometry(
            points: points,
            path: polygonPath(points),
            extra: [],
            safe: SafeRect(x: fan + 7, y: 11, w: w - fan - 22, h: h - 22),
            centered: false,
            target: LEFT_TARGET,
            sources: RIGHT_SOURCE
        )

    case .rect:
        let points = polygonPoints([
            (I, I),
            (w - I, I),
            (w - I, h - I),
            (I, h - I),
        ])
        return NodeShapeGeometry(
            points: points,
            path: roundedRectPath(w, h),
            extra: [],
            safe: SafeRect(x: 15, y: 11, w: w - 30, h: h - 22),
            centered: false,
            target: LEFT_TARGET,
            sources: RIGHT_SOURCE
        )
    }
}

/// An open (unclosed) polyline path — used by `extra` (stroke-only rails /
/// stack-layer lines, never filled/closed).
private func polygonPathOpen(_ points: [CGPoint]) -> CGPath {
    let path = CGMutablePath()
    guard let first = points.first else { return path }
    path.move(to: first)
    for p in points.dropFirst() { path.addLine(to: p) }
    return path
}
