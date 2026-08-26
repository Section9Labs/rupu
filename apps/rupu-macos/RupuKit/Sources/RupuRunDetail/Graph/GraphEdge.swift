import SwiftUI
import RupuDesign
import RupuStore

/// One linear-chain connector between two adjacent `GraphNodeVM`s: a 40pt
/// horizontal 2pt line with a closed arrowhead, vertically centered on the
/// cards it joins (the enclosing `HStack`'s default cross-axis centering
/// does the vertical alignment — this view's own frame only needs to be
/// tall enough for the arrowhead, not the full card height). Ports the web's
/// `RunGraph.tsx` edge memo — same source-kind-accent / awaiting-wins /
/// traversed-vs-ghosted / marching-ants rules — from ReactFlow's
/// `smoothstep` + `MarkerType.ArrowClosed` to a plain SwiftUI shape, since
/// this canvas lays nodes out in a single row rather than a free graph.
///
/// Color: `target.state == .gatePending` wins outright (amber reads as
/// "needs you" regardless of what precedes it, per the web comment this
/// ports); otherwise `source.kind.accent`, at full alpha when the edge is
/// "traversed" (`target.state` is `.running` or `.done`, matching the web's
/// `traversed = active || targetState === 'done'` — a `.gatePending` target
/// is NOT traversed on its own, it only ever renders full-alpha via the
/// amber branch above) and `opacity(0.35)` (ghosted) otherwise.
///
/// Motion: marching ants — `stroke-dasharray [7, 7]` animated via
/// `strokeDashPhase` from 0 to -28 over a 0.7s linear repeat-forever — fire
/// on the same two "live frontier" states, `.running`/`.gatePending`;
/// reduced motion freezes the dash phase at 0 (still dashed, just static),
/// following the same onAppear + onChange restart discipline as the node
/// cards' pulses (`StepNodeCard.swift`'s `RingPulse`,
/// `ContainerNodes.swift`'s `PanelLoopSpinner`).
struct GraphEdge: View {
    let source: GraphNodeVM
    let target: GraphNodeVM

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var phase: CGFloat = 0

    private let length: CGFloat = 40
    private let headSize: CGFloat = 6
    private let lineWidth: CGFloat = 2

    var body: some View {
        ZStack {
            EdgeShaft(length: length, headSize: headSize)
                .stroke(color, style: StrokeStyle(lineWidth: lineWidth, dash: isAnts ? [7, 7] : [], dashPhase: phase))
            EdgeArrowhead(length: length, headSize: headSize)
                .fill(color)
        }
        .frame(width: length, height: headSize)
        .onAppear { updateAnts() }
        .onChange(of: isAnts) { _, _ in updateAnts() }
        .onChange(of: reduceMotion) { _, _ in updateAnts() }
    }

    private var color: Color {
        if target.state == .gatePending {
            return Color.status(.awaiting)
        }
        return source.kind.accent.opacity(isTraversed ? 1 : 0.35)
    }

    private var isTraversed: Bool {
        switch target.state {
        case .running, .done: true
        default: false
        }
    }

    private var isAnts: Bool {
        switch target.state {
        case .running, .gatePending: true
        default: false
        }
    }

    private func updateAnts() {
        guard isAnts, !reduceMotion else {
            phase = 0
            return
        }
        phase = 0
        withAnimation(.linear(duration: 0.7).repeatForever(autoreverses: false)) {
            phase = -28
        }
    }
}

/// The connector's shaft: a horizontal line from the leading edge to just
/// short of the arrowhead, vertically centered in the shared frame.
private struct EdgeShaft: Shape {
    let length: CGFloat
    let headSize: CGFloat

    func path(in rect: CGRect) -> Path {
        let midY = rect.midY
        var path = Path()
        path.move(to: CGPoint(x: 0, y: midY))
        path.addLine(to: CGPoint(x: length - headSize, y: midY))
        return path
    }
}

/// The connector's closed arrowhead triangle at the trailing edge — ports
/// ReactFlow's `MarkerType.ArrowClosed`, always solid-filled (never dashed),
/// same color as the shaft it terminates.
private struct EdgeArrowhead: Shape {
    let length: CGFloat
    let headSize: CGFloat

    func path(in rect: CGRect) -> Path {
        let midY = rect.midY
        let baseX = length - headSize
        var path = Path()
        path.move(to: CGPoint(x: baseX, y: midY - headSize / 2))
        path.addLine(to: CGPoint(x: length, y: midY))
        path.addLine(to: CGPoint(x: baseX, y: midY + headSize / 2))
        path.closeSubpath()
        return path
    }
}
