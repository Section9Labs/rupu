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

private func expectMatches(_ color: Color, light: UInt32, dark: UInt32,
                            sourceLocation: SourceLocation = #_sourceLocation) {
    let lightResolved = components(of: color, appearance: .aqua)
    let darkResolved = components(of: color, appearance: .darkAqua)
    let lightExpected = hex(light)
    let darkExpected = hex(dark)
    #expect(lightResolved == lightExpected, "light appearance mismatch", sourceLocation: sourceLocation)
    #expect(darkResolved == darkExpected, "dark appearance mismatch", sourceLocation: sourceLocation)
}

@Test func baseTokensResolvePerAppearance() {
    expectMatches(Color.rupuBg, light: 0xFAFAFA, dark: 0x0A0A0A)
    expectMatches(Color.rupuPanel, light: 0xFFFFFF, dark: 0x141416)
    expectMatches(Color.rupuInk, light: 0x0F172A, dark: 0xF5F5F5)
}

@Test func brandHiIsAsymmetricAcrossAppearances() {
    // Unlike rupuBrand (identical in both appearances), rupuBrandHi differs.
    expectMatches(Color.rupuBrandHi, light: 0x6D28D9, dark: 0xA78BFA)
}

@Test func statusColorResolvesPerAppearance() {
    expectMatches(Color.status(.run), light: 0x3B82F6, dark: 0x60A5FA)
}

@Test func severityColorResolvesPerAppearance() {
    expectMatches(Color.severity(.crit), light: 0x9333EA, dark: 0xA855F7)
}
