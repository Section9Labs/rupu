import SwiftUI
import RupuDesign
import RupuStore

// MARK: - NodeState -> node-card presentation

/// `NodeState` -> presentation bridge for every run-graph node card. This is
/// the STATE channel of the two-channel color rule (glyph badge + state
/// label) — kind identity (accent bar + kind pill) always comes from
/// `StepKind` (`KindBridge.swift`), never from here. Shared by both
/// `StepNodeCard` (this file) and the container cards (`ContainerNodes.swift`)
/// so a sub-step chip / fan-out unit square reads its color/label the exact
/// same way the parent card's own badge does.
extension NodeState {
    var tone: StatusTone {
        switch self {
        case .done(let success): success ? .done : .failed
        case .running: .running
        case .gatePending: .awaiting
        case .paused: .paused
        case .pending: .pending
        case .skipped: .skipped
        }
    }

    /// Web-parity lowercase state label (`stepStyle.ts`'s `GLYPH_LABEL`), except `.pending`
    /// reads `"—"` (`StepNode.tsx:63`: `node.state === 'pending' ? '—' : s.label`) — this app's
    /// null-discipline convention for "haven't reached this yet", not literally the word
    /// "pending".
    var stateLabel: String {
        switch self {
        case .done(let success): success ? "done" : "failed"
        case .running: "running"
        case .gatePending: "awaiting"
        case .paused: "paused"
        case .pending: "—"
        case .skipped: "skipped"
        }
    }

    /// The two "something is happening" states — drives the ring-pulse / panel spin and the
    /// `rg-pulse-run`/`rg-pulse-await` class toggle on the web side.
    var isPulsing: Bool {
        switch self {
        case .running, .gatePending: true
        default: false
        }
    }
}

// MARK: - Shared card chrome

/// Chrome for the three leaf-node variants (`StepNodeCard`'s `.step`/`.run` default, `.gate`,
/// `.action`): `panel` fill, 1px border (dashed for gates), radius 7, a 3px `kind.accent` top
/// bar, min width 170, pending at 75% opacity, and a 2px `rupuBrand` selection ring drawn just
/// outside the border. Containers (`ContainerNodes.swift`) paint their own kind-tinted
/// fill/border with no accent bar or badge row, so they don't share this.
struct LeafNodeChrome<Content: View>: View {
    let kind: StepKind
    let state: NodeState
    let isSelected: Bool
    let isGate: Bool
    let content: Content

    init(
        kind: StepKind,
        state: NodeState,
        isSelected: Bool,
        isGate: Bool = false,
        @ViewBuilder content: () -> Content
    ) {
        self.kind = kind
        self.state = state
        self.isSelected = isSelected
        self.isGate = isGate
        self.content = content()
    }

    private let radius: CGFloat = 7
    private let accentBarHeight: CGFloat = 3

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: radius)
        ZStack(alignment: .top) {
            Color.rupuPanel
            Rectangle()
                .fill(kind.accent)
                .frame(height: accentBarHeight)
            VStack(alignment: .leading, spacing: 6) {
                content
            }
            .padding(.horizontal, 10)
            .padding(.top, accentBarHeight + 6)
            .padding(.bottom, 8)
        }
        .frame(minWidth: 170, alignment: .leading)
        .clipShape(shape)
        .overlay(
            shape.stroke(
                Color.rupuBorder,
                style: isGate ? StrokeStyle(lineWidth: 1, dash: [4, 3]) : StrokeStyle(lineWidth: 1)
            )
        )
        .overlay {
            if isSelected {
                shape.stroke(Color.rupuBrand, lineWidth: 2).padding(-2)
            }
        }
        .opacity(state == .pending ? 0.75 : 1)
    }
}

/// Row-1 15pt state badge: state-color fill, white glyph icon (`StatusDescriptor`'s icon) at
/// 9pt, with a ring-pulse mounted behind it while `.running`/`.gatePending`. `size`/`showsPulse`
/// let container cards (`ContainerNodes.swift`) reuse the same glyph-square look at a smaller
/// scale (sub-step chips, panel gate line, fan-out unit squares) without the pulse — the brief
/// only asks for the ring-pulse on the primary node badge.
struct StateBadge: View {
    let state: NodeState
    var size: CGFloat = 15
    var showsPulse: Bool = true

    var body: some View {
        ZStack {
            if showsPulse, state.isPulsing {
                RingPulse(state: state, size: size)
            }
            RoundedRectangle(cornerRadius: 3)
                .fill(Color.status(state.tone))
                .frame(width: size, height: size)
                .overlay(
                    Icon(StatusDescriptor.descriptor(for: state.tone).icon, size: size * 0.6)
                        .foregroundStyle(.white)
                )
        }
    }
}

/// Ring-pulse behind a running/awaiting badge: an expanding stroked circle (scale 1 -> ~1.5,
/// fading out) on a 1.7s ease-in-out repeat — port of the web's `rg-pulse-run`/`rg-pulse-await`
/// CSS keyframes (`.gatePending` renders in the `.awaiting` color, everything else in
/// `.running`). Reuses the restart discipline every animated element in this run graph follows
/// (see also `GraphEdge.swift`'s marching ants, `ContainerNodes.swift`'s `PanelLoopSpinner`):
/// start/stop on `onAppear` + `onChange(of:)` for both `state` and reduce-motion,
/// and never restart an already-running pulse between two animated states — mounting this view
/// only at the animated <-> non-animated boundary (its parent's `if state.isPulsing` in
/// `StateBadge`) means a `.running` -> `.gatePending` transition keeps this view's identity, so
/// only the `onChange(of: state)` branch runs (a no-op restart-guard hit, not a fresh mount).
private struct RingPulse: View {
    let state: NodeState
    let size: CGFloat

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isPulsing = false

    var body: some View {
        Circle()
            .stroke(ringColor, lineWidth: 2)
            .frame(width: size, height: size)
            .scaleEffect(active ? 1.5 : 1)
            .opacity(active ? 0 : 0.85)
            .allowsHitTesting(false)
            .onAppear { update() }
            .onChange(of: state) { _, _ in update() }
            .onChange(of: reduceMotion) { _, _ in update() }
    }

    private var ringColor: Color {
        state == .gatePending ? Color.status(.awaiting) : Color.status(.running)
    }

    private var active: Bool { isPulsing && !reduceMotion }

    private func update() {
        guard !reduceMotion, state.isPulsing else {
            isPulsing = false
            return
        }
        guard !isPulsing else { return }
        withAnimation(.easeInOut(duration: 1.7).repeatForever(autoreverses: true)) {
            isPulsing = true
        }
    }
}

/// Row-2 kind pill: `kind.icon` at 10pt + `kind.label`, `metaText`, on a 14% tint of
/// `kind.accent`, radius 4. The KIND channel of the two-channel rule — never colored by state.
struct KindPillView: View {
    let kind: StepKind

    var body: some View {
        HStack(spacing: 4) {
            Icon(kind.icon, size: 10)
            Text(kind.label)
        }
        .font(.metaText)
        .foregroundStyle(kind.accent)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(kind.accent.opacity(0.14))
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

/// Neutral `rupuSurface` chip — the agent-name chip on step/action cards. `metaText`/`rupuDim`,
/// kept distinct from `Badge` (always `dataMono`) per the brief's exact typography pin for this
/// element.
struct SurfaceChip: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.metaText)
            .foregroundStyle(Color.rupuDim)
            .lineLimit(1)
            .truncationMode(.tail)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.rupuSurface)
            .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

/// Row 1 shared by every leaf variant: badge · headline (truncating) · state label.
private struct NodeCardHeadline: View {
    let state: NodeState
    let headline: String
    let headlineFont: Font

    var body: some View {
        HStack(spacing: 6) {
            StateBadge(state: state)
            Text(headline)
                .font(headlineFont)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 4)
            Text(state.stateLabel)
                .font(.metaText)
                .foregroundStyle(Color.status(state.tone))
        }
    }
}

// MARK: - StepNodeCard

/// The run graph's leaf-node card: dispatches to the `.gate`/`.action` anatomy for those two
/// kinds, and the plain step anatomy for everything else (`.step`, `.run`, and any future kind
/// this app hasn't special-cased — mirrors the web's `flowKind` switch, whose `default` case
/// also falls through to `StepNode`). Container kinds (`.parallel`/`.panel`/`.forEach`) never
/// reach this view — `ContainerNodes.swift` renders those.
public struct StepNodeCard: View {
    private let node: GraphNodeVM
    private let isSelected: Bool

    public init(node: GraphNodeVM, isSelected: Bool) {
        self.node = node
        self.isSelected = isSelected
    }

    public var body: some View {
        switch node.kind {
        case .gate: gateBody
        case .action: actionBody
        default: stepBody
        }
    }

    private var stepBody: some View {
        LeafNodeChrome(kind: node.kind, state: node.state, isSelected: isSelected) {
            NodeCardHeadline(state: node.state, headline: node.id, headlineFont: .uiText)
            HStack(spacing: 6) {
                KindPillView(kind: node.kind)
                if let agentLabel = node.agentLabel {
                    SurfaceChip(text: agentLabel)
                }
            }
        }
    }

    private var gateBody: some View {
        LeafNodeChrome(kind: node.kind, state: node.state, isSelected: isSelected, isGate: true) {
            NodeCardHeadline(state: node.state, headline: node.id, headlineFont: .uiText)
            HStack(spacing: 6) {
                KindPillView(kind: node.kind)
                if node.gateAuto {
                    Badge("auto")
                }
            }
            if node.gateHasOnReject {
                Text("↳ on reject")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
            }
        }
    }

    private var actionBody: some View {
        LeafNodeChrome(kind: node.kind, state: node.state, isSelected: isSelected) {
            NodeCardHeadline(
                state: node.state,
                headline: node.actionName ?? node.id,
                headlineFont: .dataMono(12)
            )
            if node.actionName != nil {
                Text(node.id)
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
            }
            HStack(spacing: 6) {
                KindPillView(kind: node.kind)
                Badge("connector")
            }
        }
    }
}
