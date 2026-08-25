import SwiftUI
import AppKit

/// Resolves to `l` under Aqua and `d` under Dark Aqua. RGB triples (not hex) so every value in
/// this file reads directly off `docs/macOS_design/V2-CONTRACT.md`'s `r g b | r g b` rows without
/// a hex-decode step.
func dynamicColor(_ l: (UInt8, UInt8, UInt8), _ d: (UInt8, UInt8, UInt8)) -> Color {
    Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let rgb = isDark ? d : l
        return NSColor(srgbRed: CGFloat(rgb.0) / 255, green: CGFloat(rgb.1) / 255, blue: CGFloat(rgb.2) / 255, alpha: 1)
    })
}

/// v2 token set, ported verbatim from `crates/rupu-cp/web/src/styles.css` (lines 16-103) — see
/// `docs/macOS_design/V2-CONTRACT.md` for the source table.
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

public enum Severity: String, CaseIterable, Sendable { case crit, high, med, low, info }

public extension Severity {
    /// Maps `rupu-coverage`'s wire vocabulary (the `Severity` enum on the
    /// Rust side, `#[serde(rename_all = "lowercase")]` over
    /// `Info|Low|Medium|High|Critical` — see `crates/rupu-coverage/src/
    /// catalog/types.rs`) onto this app's own case names. The two
    /// vocabularies only agree on `high`/`low`/`info`; `critical`/`medium`
    /// don't match `crit`/`med`, so a naive `Severity(rawValue:)` (the
    /// pre-fix code) silently mapped every critical and medium finding to
    /// `.info`.
    ///
    /// **Shared home (Phase 5B, Task 3 — the "severity lift").** Moved here
    /// from `RupuRunDetail/RunDetailTabs.swift`'s `FindingsTabContent.
    /// severity(for:)` — and the byte-for-byte duplicate that had grown in
    /// `RupuProjects/ProjectDetailScreen.swift`'s `ProjectFindingsTabContent`
    /// — because `RupuDesign` already owns `Severity` and `Color.
    /// severity(_:)`; the wire-mapping belongs beside the tokens it feeds,
    /// not re-derived in every view module that renders a finding. Same
    /// "lift a view-module mapping into the layer that already owns the
    /// type" precedent `ActivityStatus.displayLabel` set when it moved from
    /// `RupuActivity/ActivityTable.swift` into `RupuStore/ActivityRow.swift`.
    /// Both call sites now delegate to this init; see `FindingsSeverityTests`
    /// → `SeverityWireMappingTests.swift` (`RupuDesignTests`) for the pinned
    /// wire-string table this fix protects.
    ///
    /// `nil`-free: an unrecognized wire string (future severity, decode
    /// drift) falls back to `.info` — the same "never crash on new data"
    /// posture the rest of this app takes — but every value `rupu-coverage`'s
    /// `Severity` actually serializes today round-trips exactly.
    init(wireString raw: String) {
        switch raw {
        case "critical": self = .crit
        case "high": self = .high
        case "medium": self = .med
        case "low": self = .low
        case "info": self = .info
        default: self = .info
        }
    }
}

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

/// Trigger kind for a workflow run — `manual` | `cron` | `event`. Mirrors the
/// web's `TriggerChip` tone mapping (`crates/rupu-cp/web/src/components/TriggerChip.tsx`:
/// `manual` → neutral, `cron` → violet, `event` → sky).
public enum TriggerKind: String, CaseIterable, Sendable {
    case manual, cron, event
}

public extension Color {
    /// Trigger palette, read from the web's CSS custom properties
    /// (`crates/rupu-cp/web/src/styles.css`) at the exact channels
    /// `TriggerChip`/`ThroughputChart` resolve to for each tone:
    /// - `manual` = neutral = `--c-ink-dim` (`rupuDim` above — same RGB pair).
    /// - `cron` = violet = `--c-brand-600` (`rupuBrand600` above — same RGB pair).
    /// - `event` = sky = `--c-info` (`rupuInfo` above — same RGB pair; the web
    ///   notes there is no dedicated "sky" token and `info` is the closest
    ///   existing blue, kept distinct from `status.running`).
    /// Both light (`:root`) and dark (`[data-theme="dark"]`) values are defined
    /// on the web side, so both are ported here — no single-theme fallback
    /// needed.
    static func trigger(_ kind: TriggerKind) -> Color {
        switch kind {
        case .manual: rupuDim
        case .cron: rupuBrand600
        case .event: rupuInfo
        }
    }
}
