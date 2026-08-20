import SwiftUI
import AppKit

func dynamicColor(light: UInt32, dark: UInt32) -> Color {
    Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return NSColor(srgbHex: isDark ? dark : light)
    })
}

extension NSColor {
    convenience init(srgbHex hex: UInt32) {
        self.init(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                  green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }
}

public extension Color {
    static let rupuBg = dynamicColor(light: 0xFAFAFA, dark: 0x0A0A0A)
    static let rupuPanel = dynamicColor(light: 0xFFFFFF, dark: 0x141416)
    static let rupuSurface = dynamicColor(light: 0xF1F5F9, dark: 0x1B1B1F)
    static let rupuHover = dynamicColor(light: 0xE2E8F0, dark: 0x232327)
    static let rupuActive = dynamicColor(light: 0xCBD5E1, dark: 0x2E2E33)
    static let rupuBorder = dynamicColor(light: 0xE5E7EB, dark: 0x26262A)
    static let rupuBorderStrong = dynamicColor(light: 0xCBD5E1, dark: 0x3F3F46) // lines/fills only, never text
    static let rupuInk = dynamicColor(light: 0x0F172A, dark: 0xF5F5F5)
    static let rupuDim = dynamicColor(light: 0x64748B, dark: 0xA1A1AA)
    static let rupuMute = dynamicColor(light: 0x94A3B8, dark: 0x71717A) // dimmest legal text
    static let rupuBrand = dynamicColor(light: 0x7C3AED, dark: 0x7C3AED)
    static let rupuBrandHi = dynamicColor(light: 0x6D28D9, dark: 0xA78BFA)
}

public enum RunTone: String, CaseIterable, Sendable { case run, done, fail, waiting = "await", pause }
public enum Severity: String, CaseIterable, Sendable { case crit, high, med, low, info }

public extension Color {
    static func status(_ tone: RunTone) -> Color {
        switch tone {
        case .run: dynamicColor(light: 0x3B82F6, dark: 0x60A5FA)
        case .done: dynamicColor(light: 0x16A34A, dark: 0x4ADE80)
        case .fail: dynamicColor(light: 0xDC2626, dark: 0xF87171)
        case .waiting: dynamicColor(light: 0xD97706, dark: 0xFBBF24)
        case .pause: dynamicColor(light: 0x0891B2, dark: 0x22D3EE)
        }
    }
    static func severity(_ s: Severity) -> Color {
        switch s {
        case .crit: dynamicColor(light: 0x9333EA, dark: 0xA855F7)
        case .high: dynamicColor(light: 0xDC2626, dark: 0xF87171)
        case .med: dynamicColor(light: 0xEA580C, dark: 0xFB923C)
        case .low: dynamicColor(light: 0xCA8A04, dark: 0xFACC15)
        case .info: dynamicColor(light: 0x64748B, dark: 0x94A3B8)
        }
    }
}
