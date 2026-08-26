import SwiftUI
import RupuDesign

/// The run graph's kind vocabulary — a Swift-side mirror of the web CP's `StepKind`
/// (`crates/rupu-cp/web/src/lib/workflowGraph.ts`) restricted to the seven kinds a
/// `StepNodeDto.kind` string can actually carry (`step`/`for_each`/`parallel`/`panel`/`gate`/
/// `action`/`run`). The editor-only kinds (`branch`/`split`/`join`) have no run-model
/// counterpart, so they aren't cases here — same restriction the web `kindBridge.ts` module
/// applies via its `RunKind` type.
public enum StepKind: String, CaseIterable, Sendable {
    case step
    case forEach = "for_each"
    case parallel
    case panel
    case gate
    case action
    case run

    /// Maps a raw `StepNodeDto.kind` string onto a `StepKind`, falling back to `.step` for
    /// anything unrecognized — the same fallback the web bridge's `STEP_KIND` lookup would hit
    /// if `RunKind` ever grew a member ahead of this enum (there, a compile-time exhaustiveness
    /// failure; here, deliberately permissive since raw strings arrive over the wire).
    public init(raw: String) {
        self = StepKind(rawValue: raw) ?? .step
    }
}

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
