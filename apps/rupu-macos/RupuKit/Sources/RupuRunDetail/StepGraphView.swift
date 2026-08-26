import SwiftUI
import RupuDesign
import RupuStore

/// Horizontal step graph: one capsule per `GraphNodeVM`, joined by thin
/// connectors, scrollable when the workflow has more nodes than fit the
/// window. No unit tests here by design (per the brief) — `GraphLayoutTests`
/// covers the pure logic this view only renders.
///
/// Flows-composition Task 4: each capsule is now tappable — `onSelect` fires
/// with the tapped node's `id` (a step id), which `RunDetailScreen` wires to
/// `RunDetailStore.select(step:)` — and the node matching `selectedID` (the
/// store's `selectedStepID`) renders a 1px `Color.rupuBrand` ring so the tab
/// panel's "which step is this following" stays visually anchored to the
/// graph.
public struct StepGraphView: View {
    private let nodes: [GraphNodeVM]
    private let selectedID: String?
    private let onSelect: (String) -> Void

    public init(nodes: [GraphNodeVM], selectedID: String? = nil, onSelect: @escaping (String) -> Void = { _ in }) {
        self.nodes = nodes
        self.selectedID = selectedID
        self.onSelect = onSelect
    }

    public var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(Array(nodes.enumerated()), id: \.element.id) { index, node in
                    if index > 0 {
                        Connector()
                    }
                    GraphNodeCapsule(node: node, isSelected: node.id == selectedID)
                        .contentShape(Rectangle())
                        .onTapGesture { onSelect(node.id) }
                }
            }
            .padding(16)
        }
    }
}

/// 1px connector line between two node capsules.
private struct Connector: View {
    var body: some View {
        Rectangle()
            .fill(Color.rupuBorderStrong)
            .frame(width: 24, height: 1)
    }
}

private struct GraphNodeCapsule: View {
    let node: GraphNodeVM
    let isSelected: Bool

    var body: some View {
        VStack(spacing: 6) {
            StatusGlyph(state: node.state)
            VStack(spacing: 2) {
                Eyebrow(node.kind.label)
                if let agentLabel = node.agentLabel {
                    Text(agentLabel)
                        .font(.noteText)
                        .foregroundStyle(Color.rupuInk)
                        .lineLimit(1)
                }
                if let fanout = node.fanout {
                    // TODO(Task 3): temporary done/total readout — the real
                    // fan-out visual (per-unit chips, failed count) lands
                    // when this view is properly rewritten.
                    Text("\(fanout.done)/\(fanout.total)")
                        .font(.dataMono(11))
                        .foregroundStyle(Color.rupuDim)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(minWidth: 84)
        .opacity(node.state == .skipped ? 0.4 : 1)
        .panelStyle(.innerCard)
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(Color.rupuBrand, lineWidth: isSelected ? 1 : 0)
        )
    }
}

/// The per-state status indicator: a filled circle for terminal states, a
/// pulsing dot/ring for the two "something is happening" states, and a
/// dashed outline for not-yet-reached nodes.
private struct StatusGlyph: View {
    let state: NodeState

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isPulsing = false

    private let diameter: CGFloat = 14

    var body: some View {
        ZStack {
            switch state {
            case .done(let success):
                let tone: StatusTone = success ? .done : .failed
                Icon(StatusDescriptor.descriptor(for: tone).icon, size: diameter)
                    .foregroundStyle(Color.status(tone))
            case .running:
                Circle()
                    .fill(Color.status(.running))
                    .frame(width: diameter, height: diameter)
                    .scaleEffect(pulseActive ? 1.3 : 1)
                    .opacity(pulseActive ? 0.5 : 1)
            case .gatePending:
                Circle()
                    .strokeBorder(Color.status(.awaiting), lineWidth: 2)
                    .frame(width: diameter, height: diameter)
                    .scaleEffect(pulseActive ? 1.3 : 1)
                    .opacity(pulseActive ? 0.5 : 1)
            case .paused:
                // TODO(Task 3): temporary glyph — a dedicated paused
                // treatment lands when this view is properly rewritten.
                Circle()
                    .strokeBorder(Color.status(.paused), lineWidth: 2)
                    .frame(width: diameter, height: diameter)
            case .pending:
                Circle()
                    .strokeBorder(Color.rupuBorder, style: StrokeStyle(lineWidth: 1.5, dash: [3, 2]))
                    .frame(width: diameter, height: diameter)
            case .skipped:
                Circle()
                    .strokeBorder(Color.rupuMute, lineWidth: 1.5)
                    .frame(width: diameter, height: diameter)
            }
        }
        .onAppear {
            updatePulse()
        }
        // Review fix (minor rider): the pulse used to only ever start in
        // `.onAppear` — a node that was `.pending` when this view first
        // appeared and later transitions to `.running`/`.gatePending` in
        // place (the graph updates `NodeState`s live; this view isn't
        // re-created per state) never got the animation, since `.onAppear`
        // doesn't fire again. Re-running the same start/stop decision on
        // every `state` change covers that transition, and on every
        // `reduceMotion` change covers the flip-mid-animation edge (motion
        // getting reduced while a node is mid-pulse, or un-reduced while
        // one is still animated, in either case landing on the correct
        // resting/animating form rather than whatever `.onAppear` last
        // decided).
        .onChange(of: state) { _, _ in
            updatePulse()
        }
        .onChange(of: reduceMotion) { _, _ in
            updatePulse()
        }
    }

    /// Pulse only fires when motion isn't reduced; otherwise the glyph
    /// renders in its resting (non-scaled, fully opaque) form.
    private var pulseActive: Bool { isPulsing && !reduceMotion }

    /// Starts (or restarts) the repeating pulse animation for an animated
    /// state, or immediately resets to the resting form otherwise. Safe to
    /// call redundantly — e.g. a `state` change between two animated states
    /// (`.running` -> `.gatePending`) leaves an already-running pulse alone
    /// rather than restarting its cycle.
    private func updatePulse() {
        guard !reduceMotion, isAnimatedState else {
            isPulsing = false
            return
        }
        guard !isPulsing else { return }
        withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) {
            isPulsing = true
        }
    }

    private var isAnimatedState: Bool {
        switch state {
        case .running, .gatePending: true
        default: false
        }
    }
}
