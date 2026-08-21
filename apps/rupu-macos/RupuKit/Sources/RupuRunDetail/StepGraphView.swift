import SwiftUI
import RupuDesign
import RupuStore

/// Horizontal step graph: one capsule per `GraphNodeVM`, joined by thin
/// connectors, scrollable when the workflow has more nodes than fit the
/// window. No unit tests here by design (per the brief) — `GraphLayoutTests`
/// covers the pure logic this view only renders.
public struct StepGraphView: View {
    private let nodes: [GraphNodeVM]

    public init(nodes: [GraphNodeVM]) {
        self.nodes = nodes
    }

    public var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 0) {
                ForEach(Array(nodes.enumerated()), id: \.element.id) { index, node in
                    if index > 0 {
                        Connector()
                    }
                    GraphNodeCapsule(node: node)
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

    var body: some View {
        VStack(spacing: 6) {
            StatusGlyph(state: node.state)
            VStack(spacing: 2) {
                MicroLabel(node.kindLabel)
                    .foregroundStyle(Color.rupuDim)
                if let agentLabel = node.agentLabel {
                    Text(agentLabel)
                        .font(.system(size: 11))
                        .foregroundStyle(Color.rupuInk)
                        .lineLimit(1)
                }
                if let unitProgress = node.unitProgress {
                    Text("\(unitProgress.done)/\(unitProgress.total)")
                        .font(.numeral(size: 11))
                        .foregroundStyle(Color.rupuDim)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(minWidth: 84)
        .opacity(node.state == .skipped ? 0.4 : 1)
        .panelStyle(.innerCard)
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
                Circle()
                    .fill(Color.status(success ? .done : .fail))
                    .frame(width: diameter, height: diameter)
                Image(systemName: success ? "checkmark" : "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(.white)
            case .running:
                Circle()
                    .fill(Color.status(.run))
                    .frame(width: diameter, height: diameter)
                    .scaleEffect(pulseActive ? 1.3 : 1)
                    .opacity(pulseActive ? 0.5 : 1)
            case .gatePending:
                Circle()
                    .strokeBorder(Color.status(.waiting), lineWidth: 2)
                    .frame(width: diameter, height: diameter)
                    .scaleEffect(pulseActive ? 1.3 : 1)
                    .opacity(pulseActive ? 0.5 : 1)
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
