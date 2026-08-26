import CoreGraphics

/// Parses an SVG path `d` attribute (the command subset lucide icons use: `M/m L/l H/h V/v C/c S/s
/// Q/q T/t A/a Z/z`) into a `CGPath`, scaled from the icon's original viewBox into an arbitrary
/// destination rect.
///
/// `SVGPath` is deliberately the only public surface — the intermediate drawing-op representation
/// stays internal so `cgPath(in:viewBox:)` is the one thing callers (and tests) go through.
///
/// `Sendable`: every stored value (`DrawOp`'s `CGPoint` payloads) is itself `Sendable`, and this
/// conformance is what lets `Icon.swift`'s `IconShape.parsedPaths` cache a `[LucideIcon:
/// [SVGPath]]` table as a plain `static let` — without it, Swift 6 treats that dictionary as
/// non-concurrency-safe global mutable state.
public struct SVGPath: Sendable {
    enum DrawOp: Sendable {
        case moveTo(CGPoint)
        case lineTo(CGPoint)
        case curveTo(CGPoint, CGPoint, CGPoint)
        case quadCurveTo(CGPoint, CGPoint)
        case closePath
    }

    let ops: [DrawOp]

    /// Parses `d`. Returns `nil` if `d` is empty, doesn't start with a moveto, uses an unrecognized
    /// command letter, or a command is missing required numeric arguments.
    public init?(d: String) {
        guard let parsed = SVGPath.parse(d) else { return nil }
        ops = parsed
    }

    /// Renders the parsed path into `rect`, scaling from the `viewBox × viewBox` coordinate space
    /// the `d` string was authored in (lucide icons: 24).
    public func cgPath(in rect: CGRect, viewBox: CGFloat) -> CGPath {
        let path = CGMutablePath()
        guard viewBox > 0 else { return path }
        let sx = rect.width / viewBox
        let sy = rect.height / viewBox
        func t(_ p: CGPoint) -> CGPoint {
            CGPoint(x: rect.minX + p.x * sx, y: rect.minY + p.y * sy)
        }
        for op in ops {
            switch op {
            case .moveTo(let p):
                path.move(to: t(p))
            case .lineTo(let p):
                path.addLine(to: t(p))
            case .curveTo(let c1, let c2, let p):
                path.addCurve(to: t(p), control1: t(c1), control2: t(c2))
            case .quadCurveTo(let c, let p):
                path.addQuadCurve(to: t(p), control: t(c))
            case .closePath:
                path.closeSubpath()
            }
        }
        return path
    }

    // MARK: - Parsing

    private static func parse(_ d: String) -> [DrawOp]? {
        var scanner = PathScanner(d)
        scanner.skipSeparators()
        guard let first = scanner.peekCommand(), first == "M" || first == "m" else { return nil }

        var ops: [DrawOp] = []
        var current = CGPoint.zero
        var subpathStart = CGPoint.zero
        var lastCubicControl: CGPoint?
        var lastQuadControl: CGPoint?

        func resolved(_ x: Double, _ y: Double, relative: Bool) -> CGPoint {
            relative ? CGPoint(x: current.x + x, y: current.y + y) : CGPoint(x: x, y: y)
        }

        scanner.skipSeparators()
        while !scanner.isAtEnd {
            guard let cmd = scanner.nextCommand() else { return nil }
            let relative = cmd.isLowercase
            switch Character(cmd.uppercased()) {
            case "M":
                guard let x = scanner.nextNumber(), let y = scanner.nextNumber() else { return nil }
                let p = resolved(x, y, relative: relative)
                ops.append(.moveTo(p))
                current = p
                subpathStart = p
                lastCubicControl = nil
                lastQuadControl = nil
                while scanner.hasMoreNumbers() {
                    guard let x2 = scanner.nextNumber(), let y2 = scanner.nextNumber() else { return nil }
                    let p2 = resolved(x2, y2, relative: relative)
                    ops.append(.lineTo(p2))
                    current = p2
                }

            case "L":
                repeat {
                    guard let x = scanner.nextNumber(), let y = scanner.nextNumber() else { return nil }
                    let p = resolved(x, y, relative: relative)
                    ops.append(.lineTo(p))
                    current = p
                    lastCubicControl = nil
                    lastQuadControl = nil
                } while scanner.hasMoreNumbers()

            case "H":
                repeat {
                    guard let x = scanner.nextNumber() else { return nil }
                    let newX = relative ? current.x + x : x
                    let p = CGPoint(x: newX, y: current.y)
                    ops.append(.lineTo(p))
                    current = p
                    lastCubicControl = nil
                    lastQuadControl = nil
                } while scanner.hasMoreNumbers()

            case "V":
                repeat {
                    guard let y = scanner.nextNumber() else { return nil }
                    let newY = relative ? current.y + y : y
                    let p = CGPoint(x: current.x, y: newY)
                    ops.append(.lineTo(p))
                    current = p
                    lastCubicControl = nil
                    lastQuadControl = nil
                } while scanner.hasMoreNumbers()

            case "C":
                repeat {
                    guard let x1 = scanner.nextNumber(), let y1 = scanner.nextNumber(),
                        let x2 = scanner.nextNumber(), let y2 = scanner.nextNumber(),
                        let x = scanner.nextNumber(), let y = scanner.nextNumber()
                    else { return nil }
                    let c1 = resolved(x1, y1, relative: relative)
                    let c2 = resolved(x2, y2, relative: relative)
                    let p = resolved(x, y, relative: relative)
                    ops.append(.curveTo(c1, c2, p))
                    current = p
                    lastCubicControl = c2
                    lastQuadControl = nil
                } while scanner.hasMoreNumbers()

            case "S":
                repeat {
                    guard let x2 = scanner.nextNumber(), let y2 = scanner.nextNumber(),
                        let x = scanner.nextNumber(), let y = scanner.nextNumber()
                    else { return nil }
                    let c1 = lastCubicControl.map { CGPoint(x: 2 * current.x - $0.x, y: 2 * current.y - $0.y) } ?? current
                    let c2 = resolved(x2, y2, relative: relative)
                    let p = resolved(x, y, relative: relative)
                    ops.append(.curveTo(c1, c2, p))
                    current = p
                    lastCubicControl = c2
                    lastQuadControl = nil
                } while scanner.hasMoreNumbers()

            case "Q":
                repeat {
                    guard let x1 = scanner.nextNumber(), let y1 = scanner.nextNumber(),
                        let x = scanner.nextNumber(), let y = scanner.nextNumber()
                    else { return nil }
                    let c = resolved(x1, y1, relative: relative)
                    let p = resolved(x, y, relative: relative)
                    ops.append(.quadCurveTo(c, p))
                    current = p
                    lastQuadControl = c
                    lastCubicControl = nil
                } while scanner.hasMoreNumbers()

            case "T":
                repeat {
                    guard let x = scanner.nextNumber(), let y = scanner.nextNumber() else { return nil }
                    let c = lastQuadControl.map { CGPoint(x: 2 * current.x - $0.x, y: 2 * current.y - $0.y) } ?? current
                    let p = resolved(x, y, relative: relative)
                    ops.append(.quadCurveTo(c, p))
                    current = p
                    lastQuadControl = c
                    lastCubicControl = nil
                } while scanner.hasMoreNumbers()

            case "A":
                repeat {
                    guard let rx = scanner.nextNumber(), let ry = scanner.nextNumber(),
                        let rot = scanner.nextNumber(), let largeArc = scanner.nextFlag(),
                        let sweep = scanner.nextFlag(), let x = scanner.nextNumber(), let y = scanner.nextNumber()
                    else { return nil }
                    let end = resolved(x, y, relative: relative)
                    appendArc(
                        &ops, from: current, rx: rx, ry: ry, xAxisRotationDegrees: rot,
                        largeArc: largeArc == 1, sweep: sweep == 1, to: end
                    )
                    current = end
                    lastCubicControl = nil
                    lastQuadControl = nil
                } while scanner.hasMoreNumbers()

            case "Z":
                ops.append(.closePath)
                current = subpathStart
                lastCubicControl = nil
                lastQuadControl = nil

            default:
                return nil
            }
            scanner.skipSeparators()
        }

        guard case .moveTo = ops.first else { return nil }
        return ops
    }
}

/// Appends the cubic-Bézier approximation of one elliptical-arc segment (`A`/`a` semantics) to
/// `ops`, per the SVG spec's endpoint-to-center parameterization:
/// https://www.w3.org/TR/SVG/implnote.html#ArcConversionEndpointToCenter
private func appendArc(
    _ ops: inout [SVGPath.DrawOp],
    from start: CGPoint,
    rx rxIn: Double,
    ry ryIn: Double,
    xAxisRotationDegrees: Double,
    largeArc: Bool,
    sweep: Bool,
    to end: CGPoint
) {
    // Identical endpoints: per spec, no arc is drawn.
    if start.x == end.x && start.y == end.y { return }

    var rx = abs(rxIn)
    var ry = abs(ryIn)
    if rx == 0 || ry == 0 {
        ops.append(.lineTo(end))
        return
    }

    let phi = xAxisRotationDegrees * .pi / 180
    let cosPhi = cos(phi)
    let sinPhi = sin(phi)

    let dx2 = Double(start.x - end.x) / 2
    let dy2 = Double(start.y - end.y) / 2
    let x1p = cosPhi * dx2 + sinPhi * dy2
    let y1p = -sinPhi * dx2 + cosPhi * dy2

    // Step 2: correct out-of-range radii.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry)
    if lambda > 1 {
        let s = lambda.squareRoot()
        rx *= s
        ry *= s
    }

    // Step 3 + 4: center.
    let sign: Double = (largeArc != sweep) ? 1 : -1
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p
    let co = den == 0 ? 0 : sign * max(0, num / den).squareRoot()
    let cxp = co * (rx * y1p) / ry
    let cyp = co * -(ry * x1p) / rx

    let cx = cosPhi * cxp - sinPhi * cyp + Double(start.x + end.x) / 2
    let cy = sinPhi * cxp + cosPhi * cyp + Double(start.y + end.y) / 2

    func angleBetween(_ ux: Double, _ uy: Double, _ vx: Double, _ vy: Double) -> Double {
        let dot = ux * vx + uy * vy
        let len = (ux * ux + uy * uy).squareRoot() * (vx * vx + vy * vy).squareRoot()
        guard len > 0 else { return 0 }
        var a = acos(max(-1, min(1, dot / len)))
        if ux * vy - uy * vx < 0 { a = -a }
        return a
    }

    let theta1 = angleBetween(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    var dTheta = angleBetween((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if !sweep && dTheta > 0 { dTheta -= 2 * .pi }
    if sweep && dTheta < 0 { dTheta += 2 * .pi }

    // Subdivide into <=90° spans for a faithful cubic-Bézier approximation.
    let segmentCount = max(1, Int(ceil(abs(dTheta) / (.pi / 2))))
    let delta = dTheta / Double(segmentCount)
    let alpha = tan(delta / 4) * 4.0 / 3.0

    func pointAndTangent(_ t: Double) -> (point: CGPoint, tangent: CGPoint) {
        let ct = cos(t)
        let st = sin(t)
        let ex = rx * ct
        let ey = ry * st
        let dex = -rx * st
        let dey = ry * ct
        let x = cosPhi * ex - sinPhi * ey + cx
        let y = sinPhi * ex + cosPhi * ey + cy
        let dx = cosPhi * dex - sinPhi * dey
        let dy = sinPhi * dex + cosPhi * dey
        return (CGPoint(x: x, y: y), CGPoint(x: dx, y: dy))
    }

    var theta = theta1
    for _ in 0..<segmentCount {
        let thetaNext = theta + delta
        let p1 = pointAndTangent(theta)
        let p2 = pointAndTangent(thetaNext)
        let c1 = CGPoint(x: p1.point.x + alpha * p1.tangent.x, y: p1.point.y + alpha * p1.tangent.y)
        let c2 = CGPoint(x: p2.point.x - alpha * p2.tangent.x, y: p2.point.y - alpha * p2.tangent.y)
        ops.append(.curveTo(c1, c2, p2.point))
        theta = thetaNext
    }
}

/// Hand-rolled scanner over an SVG path `d` string. Not `Foundation.Scanner`-based: arc flags
/// (`large-arc-flag`/`sweep-flag`) are single `0`/`1` digits that real-world paths pack directly
/// against the following token with no separator (e.g. `"...0 0-1.93 1.46..."`), which a
/// general-purpose number scanner would misparse — `nextFlag()` reads exactly one digit and stops.
private struct PathScanner {
    private let chars: [Character]
    private var idx = 0

    init(_ s: String) {
        chars = Array(s)
    }

    private static let separators: Set<Character> = [" ", "\t", "\n", "\r", ","]
    private static let digits: Set<Character> = Set("0123456789")
    private static let commandLetters = Set("MmLlHhVvCcSsQqTtAaZz")

    mutating func skipSeparators() {
        while idx < chars.count, PathScanner.separators.contains(chars[idx]) {
            idx += 1
        }
    }

    var isAtEnd: Bool {
        idx >= chars.count
    }

    private func peek() -> Character? {
        idx < chars.count ? chars[idx] : nil
    }

    /// Peeks (without consuming) whether the next character is a command letter, after skipping
    /// separators.
    mutating func peekCommand() -> Character? {
        skipSeparators()
        guard let c = peek(), PathScanner.commandLetters.contains(c) else { return nil }
        return c
    }

    /// Consumes one command letter, if the scanner is positioned at one.
    mutating func nextCommand() -> Character? {
        skipSeparators()
        guard let c = peek(), PathScanner.commandLetters.contains(c) else { return nil }
        idx += 1
        return c
    }

    /// True if, after skipping separators, the next character could start a number — used to
    /// detect the SVG "implicit repeat" continuation (extra coordinate groups after a command with
    /// no new command letter).
    mutating func hasMoreNumbers() -> Bool {
        skipSeparators()
        guard let c = peek() else { return false }
        return PathScanner.digits.contains(c) || c == "." || c == "+" || c == "-"
    }

    /// Consumes a floating point number (optional sign, digits, optional fraction, optional
    /// exponent). Returns `nil` without consuming if no valid number starts here.
    mutating func nextNumber() -> Double? {
        skipSeparators()
        let start = idx
        var i = idx
        if i < chars.count, chars[i] == "+" || chars[i] == "-" { i += 1 }
        var sawDigit = false
        while i < chars.count, PathScanner.digits.contains(chars[i]) {
            i += 1
            sawDigit = true
        }
        if i < chars.count, chars[i] == "." {
            i += 1
            while i < chars.count, PathScanner.digits.contains(chars[i]) {
                i += 1
                sawDigit = true
            }
        }
        guard sawDigit else {
            idx = start
            return nil
        }
        if i < chars.count, chars[i] == "e" || chars[i] == "E" {
            var j = i + 1
            if j < chars.count, chars[j] == "+" || chars[j] == "-" { j += 1 }
            if j < chars.count, PathScanner.digits.contains(chars[j]) {
                j += 1
                while j < chars.count, PathScanner.digits.contains(chars[j]) { j += 1 }
                i = j
            }
        }
        guard let value = Double(String(chars[start..<i])) else {
            idx = start
            return nil
        }
        idx = i
        return value
    }

    /// Consumes exactly one arc flag digit (`0` or `1`), never extending into further digits.
    mutating func nextFlag() -> Double? {
        skipSeparators()
        guard let c = peek(), c == "0" || c == "1" else { return nil }
        idx += 1
        return c == "1" ? 1 : 0
    }
}
