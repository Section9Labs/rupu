import SwiftUI
import RupuDesign
import RupuStore

/// The kind channel of the run graph's two-channel color rule: `accent`/`icon` paint the accent
/// bar and kind pill, distinct from run *state* (pending/running/done/...), which colors the
/// glyph and state label elsewhere. Values port `kindVisuals.ts`'s `KIND_ACCENT`/`KIND_ICON`/
/// the run graph's `LABELS` table (`kindBridge.ts`) for this kind subset.
public extension StepKind {
    var accent: Color {
        switch self {
        case .step: .status(.running)
        case .forEach: .rupuBrand
        case .parallel: .severity(.crit)
        case .panel: .status(.awaiting)
        case .gate: .status(.paused)
        case .action: .severity(.info)
        case .run: .severity(.med)
        }
    }

    var icon: LucideIcon {
        switch self {
        case .step: .bot
        case .forEach: .repeatIcon
        case .parallel: .columns3
        case .panel: .shieldCheck
        case .gate: .userCheck
        case .action: .zap
        case .run: .terminal
        }
    }

    var label: String {
        switch self {
        case .step: "step"
        case .forEach: "for each"
        case .parallel: "parallel"
        case .panel: "panel"
        case .gate: "gate"
        case .action: "action"
        case .run: "run"
        }
    }
}
