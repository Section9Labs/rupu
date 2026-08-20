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

private func hex(_ h: UInt32) -> (r: Int, g: Int, b: Int) {
    (Int((h >> 16) & 0xFF), Int((h >> 8) & 0xFF), Int(h & 0xFF))
}

private func expectMatches(_ color: Color, light: UInt32, dark: UInt32, name: String,
                            sourceLocation: SourceLocation = #_sourceLocation) {
    let lightResolved = components(of: color, appearance: .aqua)
    let darkResolved = components(of: color, appearance: .darkAqua)
    let lightExpected = hex(light)
    let darkExpected = hex(dark)
    #expect(lightResolved == lightExpected, "\(name): light appearance mismatch", sourceLocation: sourceLocation)
    #expect(darkResolved == darkExpected, "\(name): dark appearance mismatch", sourceLocation: sourceLocation)
}

/// The 12 base tokens from HANDOFF's table. `rupuBrand` is intentionally identical in both
/// appearances; `rupuBrandHi` is intentionally asymmetric — both are exercised by this same loop.
private let baseTokens: [(name: String, color: Color, light: UInt32, dark: UInt32)] = [
    ("rupuBg", .rupuBg, 0xFAFAFA, 0x0A0A0A),
    ("rupuPanel", .rupuPanel, 0xFFFFFF, 0x141416),
    ("rupuSurface", .rupuSurface, 0xF1F5F9, 0x1B1B1F),
    ("rupuHover", .rupuHover, 0xE2E8F0, 0x232327),
    ("rupuActive", .rupuActive, 0xCBD5E1, 0x2E2E33),
    ("rupuBorder", .rupuBorder, 0xE5E7EB, 0x26262A),
    ("rupuBorderStrong", .rupuBorderStrong, 0xCBD5E1, 0x3F3F46),
    ("rupuInk", .rupuInk, 0x0F172A, 0xF5F5F5),
    ("rupuDim", .rupuDim, 0x64748B, 0xA1A1AA),
    ("rupuMute", .rupuMute, 0x94A3B8, 0x71717A),
    ("rupuBrand", .rupuBrand, 0x7C3AED, 0x7C3AED),
    ("rupuBrandHi", .rupuBrandHi, 0x6D28D9, 0xA78BFA),
]

private let statusHex: [RunTone: (light: UInt32, dark: UInt32)] = [
    .run: (0x3B82F6, 0x60A5FA),
    .done: (0x16A34A, 0x4ADE80),
    .fail: (0xDC2626, 0xF87171),
    .waiting: (0xD97706, 0xFBBF24),
    .pause: (0x0891B2, 0x22D3EE),
]

private let severityHex: [Severity: (light: UInt32, dark: UInt32)] = [
    .crit: (0x9333EA, 0xA855F7),
    .high: (0xDC2626, 0xF87171),
    .med: (0xEA580C, 0xFB923C),
    .low: (0xCA8A04, 0xFACC15),
    .info: (0x64748B, 0x94A3B8),
]

@Test func baseTokensResolvePerAppearance() {
    for token in baseTokens {
        expectMatches(token.color, light: token.light, dark: token.dark, name: token.name)
    }
}

@Test func statusColorsResolvePerAppearance() {
    // Iterating RunTone.allCases (rather than hand-picking one case) also guards against a
    // future case being added to the enum without a matching row in `statusHex`.
    for tone in RunTone.allCases {
        guard let expected = statusHex[tone] else {
            Issue.record("no expected hex recorded for RunTone.\(tone)")
            continue
        }
        expectMatches(Color.status(tone), light: expected.light, dark: expected.dark, name: "status(.\(tone))")
    }
}

@Test func severityColorsResolvePerAppearance() {
    for severity in Severity.allCases {
        guard let expected = severityHex[severity] else {
            Issue.record("no expected hex recorded for Severity.\(severity)")
            continue
        }
        expectMatches(Color.severity(severity), light: expected.light, dark: expected.dark,
                      name: "severity(.\(severity))")
    }
}
