import SwiftUI
import AppKit

/// Resolves to `l` under Aqua and `d` under Dark Aqua. RGB triples (not hex) so every value in
/// this file reads directly off token-table.md's `r g b | r g b` rows without a hex-decode step.
func dynamicColor(_ l: (UInt8, UInt8, UInt8), _ d: (UInt8, UInt8, UInt8)) -> Color {
    Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let rgb = isDark ? d : l
        return NSColor(srgbRed: CGFloat(rgb.0) / 255, green: CGFloat(rgb.1) / 255, blue: CGFloat(rgb.2) / 255, alpha: 1)
    })
}

/// v2 token set, ported verbatim from `crates/rupu-cp/web/src/styles.css` (lines 16-103) — see
/// `.superpowers/sdd/2026-08-24-rupu-macos-design-language/token-table.md` for the source table.
public extension Color {
    static let rupuBg = dynamicColor((250, 250, 250), (10, 10, 10))
    static let rupuPanel = dynamicColor((255, 255, 255), (20, 20, 22))
    static let rupuSurface = dynamicColor((241, 245, 249), (27, 27, 31))
    static let rupuSurfaceHover = dynamicColor((226, 232, 240), (35, 35, 39))
    static let rupuSurfaceActive = dynamicColor((203, 213, 225), (46, 46, 51)) // fill-only
    static let rupuBorder = dynamicColor((229, 231, 235), (38, 38, 42))
    static let rupuBorderStrong = dynamicColor((203, 213, 225), (63, 63, 70)) // line/fill-only, never text
    static let rupuInk = dynamicColor((15, 23, 42), (245, 245, 245))
    static let rupuDim = dynamicColor((100, 116, 139), (161, 161, 170))
    static let rupuMute = dynamicColor((148, 163, 184), (113, 113, 122)) // dimmest legal text
    static let rupuBrand50 = dynamicColor((245, 243, 255), (33, 23, 56))
    static let rupuBrand100 = dynamicColor((237, 233, 254), (45, 33, 74))
    /// brand500 — kept as `rupuBrand` (not `rupuBrand500`) so existing call sites survive.
    static let rupuBrand = dynamicColor((124, 58, 237), (124, 58, 237))
    static let rupuBrand600 = dynamicColor((109, 40, 217), (124, 58, 237))
    static let rupuBrand700 = dynamicColor((91, 33, 182), (167, 139, 250))
    static let rupuErr = dynamicColor((220, 38, 38), (248, 113, 113))
    static let rupuErrBg = dynamicColor((254, 242, 242), (43, 22, 22))
    static let rupuOk = dynamicColor((22, 163, 74), (74, 222, 128))
    static let rupuOkBg = dynamicColor((240, 253, 244), (18, 40, 27))
    static let rupuWarn = dynamicColor((217, 119, 6), (251, 191, 36))
    static let rupuWarnBg = dynamicColor((255, 251, 235), (48, 36, 12))
    static let rupuInfo = dynamicColor((37, 99, 235), (96, 165, 250))
    static let rupuInfoBg = dynamicColor((239, 246, 255), (20, 32, 54))
}

/// Legacy 5-tone status enum. Superseded by `StatusTone`; deleted in Task 5 once every call site
/// migrates. Kept alive here only via the deprecated `Color.status(RunTone)` overload below.
@available(*, deprecated, message: "migrate to StatusTone (Task 5)")
public enum RunTone: String, CaseIterable, Sendable { case run, done, fail, waiting = "await", pause }

public enum Severity: String, CaseIterable, Sendable { case crit, high, med, low, info }

/// 9-state run/step/gate lifecycle tone. `rejected` is a distinct case from `failed` (different
/// semantic meaning — a gate decision vs. a run outcome) but renders identically: same RGB pair.
public enum StatusTone: String, CaseIterable, Sendable {
    case running, done, failed, awaiting, paused, pending, skipped, cancelled, rejected
}

public extension Color {
    static func status(_ tone: StatusTone) -> Color {
        switch tone {
        case .running: dynamicColor((59, 130, 246), (96, 165, 250))
        case .done: dynamicColor((34, 197, 94), (74, 222, 128))
        case .failed: dynamicColor((239, 68, 68), (248, 113, 113))
        case .awaiting: dynamicColor((245, 158, 11), (251, 191, 36))
        case .paused: dynamicColor((6, 182, 212), (34, 211, 238))
        case .pending: dynamicColor((148, 163, 184), (113, 113, 122))
        case .skipped: dynamicColor((203, 213, 225), (82, 82, 91))
        case .cancelled: dynamicColor((100, 116, 139), (161, 161, 170))
        case .rejected: dynamicColor((239, 68, 68), (248, 113, 113)) // = failed
        }
    }

    /// Compatibility shim for call sites not yet migrated off `RunTone` — maps onto the
    /// equivalent `StatusTone` color. Deleted alongside `RunTone` in Task 5.
    @available(*, deprecated, message: "migrate to StatusTone (Task 5)")
    static func status(_ tone: RunTone) -> Color {
        switch tone {
        case .run: status(StatusTone.running)
        case .done: status(StatusTone.done)
        case .fail: status(StatusTone.failed)
        case .waiting: status(StatusTone.awaiting)
        case .pause: status(StatusTone.paused)
        }
    }

    static func severity(_ s: Severity) -> Color {
        switch s {
        case .crit: dynamicColor((147, 51, 234), (168, 85, 247))
        case .high: dynamicColor((220, 38, 38), (248, 113, 113))
        case .med: dynamicColor((234, 88, 12), (251, 146, 60))
        case .low: dynamicColor((202, 138, 4), (250, 204, 21))
        case .info: dynamicColor((100, 116, 139), (148, 163, 184))
        }
    }

    static func severityBg(_ s: Severity) -> Color {
        switch s {
        case .crit: dynamicColor((250, 245, 255), (42, 28, 56))
        case .high: dynamicColor((254, 242, 242), (48, 24, 24))
        case .med: dynamicColor((255, 247, 237), (48, 32, 18))
        case .low: dynamicColor((254, 252, 232), (46, 40, 16))
        case .info: dynamicColor((248, 250, 252), (30, 31, 35))
        }
    }
}
