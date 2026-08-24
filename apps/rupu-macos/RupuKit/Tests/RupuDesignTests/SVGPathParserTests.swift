import CoreGraphics
import Testing
@testable import RupuDesign

/// A structural snapshot of one `CGPathElement`, so tests can assert against `SVGPath.cgPath`'s
/// output without reaching into `SVGPath`'s (deliberately non-public) internal op list.
private struct PathElement: Equatable {
    enum Kind: Equatable { case moveTo, lineTo, curveTo, quadCurveTo, closePath }
    let kind: Kind
    let points: [CGPoint]
}

private func elements(of path: CGPath) -> [PathElement] {
    var result: [PathElement] = []
    path.applyWithBlock { elementPtr in
        let element = elementPtr.pointee
        switch element.type {
        case .moveToPoint:
            result.append(PathElement(kind: .moveTo, points: [element.points[0]]))
        case .addLineToPoint:
            result.append(PathElement(kind: .lineTo, points: [element.points[0]]))
        case .addCurveToPoint:
            result.append(
                PathElement(kind: .curveTo, points: [element.points[0], element.points[1], element.points[2]])
            )
        case .addQuadCurveToPoint:
            result.append(PathElement(kind: .quadCurveTo, points: [element.points[0], element.points[1]]))
        case .closeSubpath:
            result.append(PathElement(kind: .closePath, points: []))
        @unknown default:
            break
        }
    }
    return result
}

/// Identity transform: a 24×24 rect over a 24 viewBox, so asserted points equal the raw path-space
/// coordinates directly.
private let identityRect = CGRect(x: 0, y: 0, width: 24, height: 24)
private let viewBox: CGFloat = 24

private func close(_ a: CGPoint, _ b: CGPoint, tolerance: CGFloat = 0.0001) -> Bool {
    abs(a.x - b.x) < tolerance && abs(a.y - b.y) < tolerance
}

@Test func parsesSimpleAbsoluteLine() {
    let svg = SVGPath(d: "M3 12h18")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 2)
    #expect(els[0].kind == .moveTo)
    #expect(close(els[0].points[0], CGPoint(x: 3, y: 12)))
    #expect(els[1].kind == .lineTo)
    #expect(close(els[1].points[0], CGPoint(x: 21, y: 12)))
}

@Test func parsesAbsoluteCubicCurve() {
    let svg = SVGPath(d: "M10 10C12 5 18 5 20 10")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 2)
    #expect(els[0].kind == .moveTo)
    #expect(els[1].kind == .curveTo)
    #expect(close(els[1].points[0], CGPoint(x: 12, y: 5)))
    #expect(close(els[1].points[1], CGPoint(x: 18, y: 5)))
    #expect(close(els[1].points[2], CGPoint(x: 20, y: 10)))
}

@Test func parsesQuadraticCurve() {
    let svg = SVGPath(d: "M0 0Q10 20 20 0")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 2)
    #expect(els[1].kind == .quadCurveTo)
    #expect(close(els[1].points[0], CGPoint(x: 10, y: 20)))
    #expect(close(els[1].points[1], CGPoint(x: 20, y: 0)))
}

@Test func parsesSmoothCubicReflection() {
    // S with no preceding C reflects around the current point (control == current point).
    let svg = SVGPath(d: "M0 0S10 10 20 0")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els[1].kind == .curveTo)
    #expect(close(els[1].points[0], CGPoint(x: 0, y: 0)))
}

@Test func parsesRelativeCommands() {
    let svg = SVGPath(d: "M5 5l5 0l0 5z")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 4)
    #expect(els[0].kind == .moveTo)
    #expect(close(els[0].points[0], CGPoint(x: 5, y: 5)))
    #expect(els[1].kind == .lineTo)
    #expect(close(els[1].points[0], CGPoint(x: 10, y: 5)))
    #expect(els[2].kind == .lineTo)
    #expect(close(els[2].points[0], CGPoint(x: 10, y: 10)))
    #expect(els[3].kind == .closePath)
}

@Test func parsesImplicitRepeatedCoordinatesAfterMoveto() {
    // "M1 2 3 4" == moveTo(1,2) then an IMPLICIT lineTo(3,4) — extra coordinate pairs following an
    // M command are treated as lineto per the SVG spec.
    let svg = SVGPath(d: "M1 2 3 4")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 2)
    #expect(els[0].kind == .moveTo)
    #expect(close(els[0].points[0], CGPoint(x: 1, y: 2)))
    #expect(els[1].kind == .lineTo)
    #expect(close(els[1].points[0], CGPoint(x: 3, y: 4)))
}

@Test func parsesImplicitRepeatedLineto() {
    let svg = SVGPath(d: "M0 0L1 1 2 2 3 3")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 4)
    #expect(els[1].kind == .lineTo)
    #expect(close(els[1].points[0], CGPoint(x: 1, y: 1)))
    #expect(els[2].kind == .lineTo)
    #expect(close(els[2].points[0], CGPoint(x: 2, y: 2)))
    #expect(els[3].kind == .lineTo)
    #expect(close(els[3].points[0], CGPoint(x: 3, y: 3)))
}

@Test func parsesMultipleSubpaths() {
    let svg = SVGPath(d: "M0 0L5 0M10 10L15 10")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 4)
    #expect(els[0].kind == .moveTo)
    #expect(els[1].kind == .lineTo)
    #expect(els[2].kind == .moveTo)
    #expect(close(els[2].points[0], CGPoint(x: 10, y: 10)))
    #expect(els[3].kind == .lineTo)
}

@Test func parsesPackedArcFlagsAdjacentToSignedNumber() {
    // Real-world lucide paths pack the two single-digit arc flags with no separator against a
    // following negative number, e.g. "...0 0-1.93 1.46..." (flags "0","0" then x=-1.93, y=1.46).
    // A naive number tokenizer would misparse "0-1.93" or swallow digits across the flag boundary.
    let svg = SVGPath(d: "M22 12a2 2 0 0 0-1.93 1.46")
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    #expect(els.count == 2)
    #expect(els[1].kind == .curveTo)
    // End point of the arc's bezier approximation: relative (-1.93, 1.46) from (22, 12).
    #expect(close(els[1].points[2], CGPoint(x: 20.07, y: 13.46), tolerance: 0.01))
}

@Test func arcConvertedCircleProducesPointsOnTheCircle() {
    // Mirrors the extractor's circle -> two-semicircle-arc conversion: cx=12, cy=12, r=10.
    let d = "M2 12A10 10 0 1 0 22 12A10 10 0 1 0 2 12"
    let svg = SVGPath(d: d)
    #expect(svg != nil)
    let els = elements(of: svg!.cgPath(in: identityRect, viewBox: viewBox))
    let center = CGPoint(x: 12, y: 12)
    let radius: CGFloat = 10
    var checked = 0
    for el in els where el.kind == .curveTo {
        let end = el.points[2]
        let distance = (end - center)
        #expect(abs(distance - radius) < 0.001, "arc endpoint \(end) should sit on the circle")
        checked += 1
    }
    // The circle is drawn as (at least) two arc segments, each themselves subdivided into <=90°
    // Bézier spans — so at least 4 curve endpoints total, all of which must land on the circle.
    #expect(checked >= 4)
}

private func - (a: CGPoint, b: CGPoint) -> CGFloat {
    let dx = a.x - b.x
    let dy = a.y - b.y
    return (dx * dx + dy * dy).squareRoot()
}

@Test func rejectsEmptyPathData() {
    #expect(SVGPath(d: "") == nil)
}

@Test func rejectsPathNotStartingWithMoveto() {
    #expect(SVGPath(d: "L5 5") == nil)
}

@Test func rejectsUnknownCommandLetter() {
    #expect(SVGPath(d: "M1 2X3 4") == nil)
}

@Test func rejectsTruncatedCommandArguments() {
    // Q requires 4 numbers; only 1 is given.
    #expect(SVGPath(d: "M1 2Q3") == nil)
}

@Test func rejectsGarbage() {
    #expect(SVGPath(d: "not a path") == nil)
}
