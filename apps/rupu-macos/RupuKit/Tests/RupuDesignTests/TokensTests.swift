import AppKit
import SwiftUI
import Testing
@testable import RupuDesign

/// Resolves a `Color` to its 8-bit sRGB components under a forced appearance.
/// Headless `swift test` has no window server, so we push the appearance onto
/// the current-drawing-appearance stack rather than relying on NSApp.effectiveAppearance.
private func components(of color: Color, appearance name: NSAppearance.Name) -> (r: Int, g: Int, b: Int) {
    let appearance = NSAppearance(named: name)!
    var resolved: NSColor?
    appearance.performAsCurrentDrawingAppearance {
        resolved = NSColor(color).usingColorSpace(.sRGB)
    }
    guard let c = resolved else {
        Issue.record("failed to resolve color under \(name.rawValue)")
        return (-1, -1, -1)
    }
    return (
        Int((c.redComponent * 255).rounded()),
        Int((c.greenComponent * 255).rounded()),
        Int((c.blueComponent * 255).rounded())
    )
}

private func expectMatches(_ color: Color, light: (Int, Int, Int), dark: (Int, Int, Int), name: String,
                            sourceLocation: SourceLocation = #_sourceLocation) {
    let lightResolved = components(of: color, appearance: .aqua)
    let darkResolved = components(of: color, appearance: .darkAqua)
    #expect(lightResolved == light, "\(name): light appearance mismatch", sourceLocation: sourceLocation)
    #expect(darkResolved == dark, "\(name): dark appearance mismatch", sourceLocation: sourceLocation)
}

/// Every base (non-status, non-severity) token from token-table.md's neutrals/brand/semantic row —
/// `rupuBrand` is intentionally identical in both appearances (brand500); every other brand step
/// and every semantic pair is asymmetric. Light|dark RGB triples are transcribed verbatim from the
/// table — cross-checked against `crates/rupu-cp/web/src/styles.css` lines 16-103.
private let baseTokens: [(name: String, color: Color, light: (Int, Int, Int), dark: (Int, Int, Int))] = [
    ("rupuBg", .rupuBg, (250, 250, 250), (10, 10, 10)),
    ("rupuPanel", .rupuPanel, (255, 255, 255), (20, 20, 22)),
    ("rupuSurface", .rupuSurface, (241, 245, 249), (27, 27, 31)),
    ("rupuSurfaceHover", .rupuSurfaceHover, (226, 232, 240), (35, 35, 39)),
    ("rupuSurfaceActive", .rupuSurfaceActive, (203, 213, 225), (46, 46, 51)),
    ("rupuBorder", .rupuBorder, (229, 231, 235), (38, 38, 42)),
    ("rupuBorderStrong", .rupuBorderStrong, (203, 213, 225), (63, 63, 70)),
    ("rupuInk", .rupuInk, (15, 23, 42), (245, 245, 245)),
    ("rupuDim", .rupuDim, (100, 116, 139), (161, 161, 170)),
    ("rupuMute", .rupuMute, (148, 163, 184), (113, 113, 122)),
    ("rupuBrand50", .rupuBrand50, (245, 243, 255), (33, 23, 56)),
    ("rupuBrand100", .rupuBrand100, (237, 233, 254), (45, 33, 74)),
    ("rupuBrand", .rupuBrand, (124, 58, 237), (124, 58, 237)),
    ("rupuBrand600", .rupuBrand600, (109, 40, 217), (124, 58, 237)),
    ("rupuBrand700", .rupuBrand700, (91, 33, 182), (167, 139, 250)),
    ("rupuErr", .rupuErr, (220, 38, 38), (248, 113, 113)),
    ("rupuErrBg", .rupuErrBg, (254, 242, 242), (43, 22, 22)),
    ("rupuOk", .rupuOk, (22, 163, 74), (74, 222, 128)),
    ("rupuOkBg", .rupuOkBg, (240, 253, 244), (18, 40, 27)),
    ("rupuWarn", .rupuWarn, (217, 119, 6), (251, 191, 36)),
    ("rupuWarnBg", .rupuWarnBg, (255, 251, 235), (48, 36, 12)),
    ("rupuInfo", .rupuInfo, (37, 99, 235), (96, 165, 250)),
    ("rupuInfoBg", .rupuInfoBg, (239, 246, 255), (20, 32, 54)),
]

private let statusRGB: [StatusTone: (light: (Int, Int, Int), dark: (Int, Int, Int))] = [
    .running: ((59, 130, 246), (96, 165, 250)),
    .done: ((34, 197, 94), (74, 222, 128)),
    .failed: ((239, 68, 68), (248, 113, 113)),
    .awaiting: ((245, 158, 11), (251, 191, 36)),
    .paused: ((6, 182, 212), (34, 211, 238)),
    .pending: ((148, 163, 184), (113, 113, 122)),
    .skipped: ((203, 213, 225), (82, 82, 91)),
    .cancelled: ((100, 116, 139), (161, 161, 170)),
    .rejected: ((239, 68, 68), (248, 113, 113)), // = failed, same RGB
]

private let severityRGB: [Severity: (light: (Int, Int, Int), dark: (Int, Int, Int))] = [
    .crit: ((147, 51, 234), (168, 85, 247)),
    .high: ((220, 38, 38), (248, 113, 113)),
    .med: ((234, 88, 12), (251, 146, 60)),
    .low: ((202, 138, 4), (250, 204, 21)),
    .info: ((100, 116, 139), (148, 163, 184)),
]

private let severityBgRGB: [Severity: (light: (Int, Int, Int), dark: (Int, Int, Int))] = [
    .crit: ((250, 245, 255), (42, 28, 56)),
    .high: ((254, 242, 242), (48, 24, 24)),
    .med: ((255, 247, 237), (48, 32, 18)),
    .low: ((254, 252, 232), (46, 40, 16)),
    .info: ((248, 250, 252), (30, 31, 35)),
]

@Test func baseTokensResolvePerAppearance() {
    for token in baseTokens {
        expectMatches(token.color, light: token.light, dark: token.dark, name: token.name)
    }
}

@Test func statusColorsResolvePerAppearance() {
    // Iterating StatusTone.allCases (rather than hand-picking cases) also guards against a
    // future case being added to the enum without a matching row in `statusRGB`.
    for tone in StatusTone.allCases {
        guard let expected = statusRGB[tone] else {
            Issue.record("no expected RGB recorded for StatusTone.\(tone)")
            continue
        }
        expectMatches(Color.status(tone), light: expected.light, dark: expected.dark, name: "status(.\(tone))")
    }
}

@Test func severityColorsResolvePerAppearance() {
    for severity in Severity.allCases {
        guard let expected = severityRGB[severity] else {
            Issue.record("no expected RGB recorded for Severity.\(severity)")
            continue
        }
        expectMatches(Color.severity(severity), light: expected.light, dark: expected.dark,
                      name: "severity(.\(severity))")
    }
}

@Test func severityBgColorsResolvePerAppearance() {
    for severity in Severity.allCases {
        guard let expected = severityBgRGB[severity] else {
            Issue.record("no expected RGB recorded for Severity.\(severity) bg")
            continue
        }
        expectMatches(Color.severityBg(severity), light: expected.light, dark: expected.dark,
                      name: "severityBg(.\(severity))")
    }
}

/// The deprecated `RunTone` shim (Task 5 deletes both it and this test) must still resolve to
/// the same colors as the `StatusTone` it maps onto — a regression guard while call sites migrate.
@available(*, deprecated, message: "exercises the deprecated RunTone shim intentionally")
@Test func deprecatedRunToneShimMapsToStatusTone() {
    let table: [(RunTone, StatusTone)] = [
        (.run, .running),
        (.done, .done),
        (.fail, .failed),
        (.waiting, .awaiting),
        (.pause, .paused),
    ]
    for (legacy, modern) in table {
        let expected = statusRGB[modern]!
        expectMatches(Color.status(legacy), light: expected.light, dark: expected.dark,
                      name: "status(RunTone.\(legacy))")
    }
}
