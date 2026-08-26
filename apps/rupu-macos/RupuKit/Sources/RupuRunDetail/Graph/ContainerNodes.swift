import SwiftUI
import RupuAPI
import RupuDesign
import RupuStore

/// Chrome shared by the three container cards (`ParallelNodeCard`, `PanelNodeCard`,
/// `FanoutNodeView`'s grid/collapsed variants): kind-tinted fill (8%) + border (40%), radius 7,
/// min width 170, and the same 2px `rupuBrand` selection ring `LeafNodeChrome` uses
/// (`StepNodeCard.swift`). Unlike `LeafNodeChrome`, there is no accent top-bar or badge row —
/// the container's ENTIRE border/fill carries the kind identity instead, per `ParallelNode.tsx`/
/// `FanoutNode.tsx`'s `data-testid="rg-container"` treatment. No pending-opacity dimming either
/// — parity with the web, which only applies that to the three leaf node components.
struct ContainerChrome<Content: View>: View {
    let accent: Color
    let isSelected: Bool
    let content: Content

    init(accent: Color, isSelected: Bool, @ViewBuilder content: () -> Content) {
        self.accent = accent
        self.isSelected = isSelected
        self.content = content()
    }

    private let radius: CGFloat = 7

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: radius)
        content
            .padding(10)
            .frame(minWidth: 170, alignment: .leading)
            .background(accent.opacity(0.08))
            .clipShape(shape)
            .overlay(shape.stroke(accent.opacity(0.4), lineWidth: 1))
            .overlay {
                if isSelected {
                    shape.stroke(Color.rupuBrand, lineWidth: 2).padding(-2)
                }
            }
    }
}

/// Small (12pt) state-glyph square used by sub-step / gate-line rows in this file — same look as
/// `StateBadge` (`StepNodeCard.swift`) at a smaller fixed size, never pulsing (the ring-pulse is
/// reserved for a leaf card's own primary badge).
private func subGlyph(_ state: NodeState, size: CGFloat = 12) -> some View {
    StateBadge(state: state, size: size, showsPulse: false)
}

// MARK: - ParallelNodeCard

/// A `parallel` step's container card: `parallel · id` header with the `done/total ✓` roll-up
/// (both in the kind's accent color — `dataMono`, not uppercase-tracked: `Eyebrow`
/// (`RupuDesign/Typography.swift`) is this design's ONE sanctioned uppercase-tracked element, so
/// this header stays mixed-case rather than reinventing that treatment for a second color), one
/// bordered chip per `subSteps` entry (12pt state-glyph square + the sub-step's own id), and a
/// "no sub-steps" line when the branch list is empty.
public struct ParallelNodeCard: View {
    private let node: GraphNodeVM
    private let isSelected: Bool

    public init(node: GraphNodeVM, isSelected: Bool) {
        self.node = node
        self.isSelected = isSelected
    }

    private var doneCount: Int {
        node.subSteps.filter {
            if case .done(true) = $0.state { return true }
            return false
        }.count
    }

    public var body: some View {
        ContainerChrome(accent: node.kind.accent, isSelected: isSelected) {
            VStack(alignment: .leading, spacing: 6) {
                header
                subStepList
            }
        }
    }

    private var header: some View {
        HStack(spacing: 6) {
            Text("parallel · \(node.id)")
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 4)
            Text("\(doneCount)/\(node.subSteps.count) ✓")
        }
        .font(.dataMono(10))
        .foregroundStyle(node.kind.accent)
    }

    private var subStepList: some View {
        VStack(alignment: .leading, spacing: 4) {
            if node.subSteps.isEmpty {
                Text("no sub-steps")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
            } else {
                ForEach(node.subSteps) { sub in
                    HStack(spacing: 6) {
                        subGlyph(sub.state)
                        Text(sub.id)
                            .font(.noteText)
                            .foregroundStyle(Color.rupuInk)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                    .padding(.horizontal, 6)
                    .padding(.vertical, 4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.rupuPanel)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.rupuBorder, lineWidth: 1))
                }
            }
        }
    }
}

// MARK: - PanelNodeCard

/// A `panel` step's container card: `panel · id` header (+ `round r/max` in `dataMono` when
/// `panelRound` is known) with a spinning `↻` while `.running`, the gate-condition block sourced
/// from `panelGate` (`gate ≥ untilSeverity · max maxIterations`, falling back to a bare "gate"
/// caption if the DTO never threaded one), and non-interactive panelist chips from `subSteps` —
/// per the brief, transcript selection for panelists is Plan 4 territory, so these chips carry
/// no tap handler at all this task.
public struct PanelNodeCard: View {
    private let node: GraphNodeVM
    private let isSelected: Bool

    public init(node: GraphNodeVM, isSelected: Bool) {
        self.node = node
        self.isSelected = isSelected
    }

    public var body: some View {
        ContainerChrome(accent: node.kind.accent, isSelected: isSelected) {
            VStack(alignment: .leading, spacing: 6) {
                header
                gateLine
                if !node.subSteps.isEmpty {
                    panelistChips
                }
            }
        }
    }

    private var header: some View {
        HStack(spacing: 6) {
            HStack(spacing: 4) {
                Text("panel · \(node.id)")
                if let round = node.panelRound {
                    Text("· round \(round.round)/\(round.maxIterations)")
                }
            }
            .lineLimit(1)
            .truncationMode(.tail)
            Spacer(minLength: 4)
            if node.state == .running {
                PanelLoopSpinner()
            }
        }
        .font(.dataMono(10))
        .foregroundStyle(node.kind.accent)
    }

    private var gateLine: some View {
        HStack(spacing: 6) {
            subGlyph(node.state)
            Text(gateText)
                .font(.metaText)
                .fontWeight(.medium)
                .foregroundStyle(Color.status(.awaiting))
                .lineLimit(1)
                .truncationMode(.tail)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 4)
        .background(Color.status(.awaiting).opacity(0.12))
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(Color.status(.awaiting).opacity(0.45), lineWidth: 1)
        )
    }

    private var gateText: String {
        guard let gate = node.panelGate else { return "gate" }
        return "gate ≥ \(gate.untilSeverity) · max \(gate.maxIterations)"
    }

    private var panelistChips: some View {
        HStack(spacing: 4) {
            ForEach(node.subSteps) { panelist in
                HStack(spacing: 4) {
                    RoundedRectangle(cornerRadius: 2)
                        .fill(Color.status(panelist.state.tone))
                        .frame(width: 6, height: 6)
                    Text(panelist.agent)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.rupuPanel.opacity(0.8))
                .clipShape(RoundedRectangle(cornerRadius: 4))
            }
        }
    }
}

/// Panel-loop `↻` spin cue, mounted only while the panel node is `.running` (its parent's `if
/// node.state == .running` in `PanelNodeCard.header`) — 2.4s linear repeat, static (unrotated)
/// under reduced motion. Same start/stop-on-change discipline as `StepNodeCard.swift`'s
/// `RingPulse`: mount/unmount at the `.running`/not-`.running` boundary handles the state
/// transition (a fresh `onAppear` per mount), and `onChange(of: reduceMotion)` still covers
/// motion getting toggled mid-spin without a remount.
private struct PanelLoopSpinner: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isSpinning = false

    var body: some View {
        Icon(.repeatIcon, size: 12)
            .foregroundStyle(Color.status(.awaiting))
            .rotationEffect(.degrees(active ? 360 : 0))
            .onAppear { update() }
            .onChange(of: reduceMotion) { _, _ in update() }
    }

    private var active: Bool { isSpinning && !reduceMotion }

    private func update() {
        guard !reduceMotion else {
            isSpinning = false
            return
        }
        guard !isSpinning else { return }
        withAnimation(.linear(duration: 2.4).repeatForever(autoreverses: false)) {
            isSpinning = true
        }
    }
}

// MARK: - FanoutNodeView

/// A `for_each` step's three-way card (per `FanoutNode.tsx`), driven entirely by `fanout.total`:
/// zero units gets a state-aware placeholder message; `1...12` units get an inline tappable
/// grid; more than 12 collapses into a summary card with a density preview and an
/// expand-in-place toggle. `onUnitTap` fires with the tapped unit's stable `id` (its REST
/// `index`) for every tappable square, in both the small grid and the large card's expanded
/// grid.
public struct FanoutNodeView: View {
    private let node: GraphNodeVM
    private let isSelected: Bool
    private let onUnitTap: (Int) -> Void

    @State private var isExpanded = false

    public init(node: GraphNodeVM, isSelected: Bool, onUnitTap: @escaping (Int) -> Void) {
        self.node = node
        self.isSelected = isSelected
        self.onUnitTap = onUnitTap
    }

    public var body: some View {
        if let fanout = node.fanout, fanout.total > 0 {
            if fanout.total <= 12 {
                ContainerChrome(accent: node.kind.accent, isSelected: isSelected) {
                    inlineGrid(fanout: fanout)
                }
            } else {
                ContainerChrome(accent: node.kind.accent, isSelected: isSelected) {
                    collapsedCard(fanout: fanout)
                }
            }
        } else {
            ContainerChrome(accent: placeholderTone ?? Color.rupuBorder, isSelected: isSelected) {
                placeholder
            }
        }
    }

    // MARK: total == 0 — state-aware placeholder

    /// `nil` for every "nothing to say about color" state (`.done(true)`, `.skipped`,
    /// `.gatePending`, `.paused`, `.pending`) — those fall back to a neutral border/mute label in
    /// the two call sites above/below, matching the web's `isErr`/`isActive`/neutral 3-way split
    /// collapsed onto this app's `NodeState` vocabulary.
    private var placeholderTone: Color? {
        switch node.state {
        case .running: Color.status(.running)
        case .done(false): Color.status(.failed)
        default: nil
        }
    }

    private var placeholder: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("for each · \(node.id)")
                .font(.dataMono(10))
                .foregroundStyle(placeholderTone ?? Color.rupuMute)
                .lineLimit(1)
                .truncationMode(.tail)
            Text(placeholderMessage)
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
        }
    }

    private var placeholderMessage: String {
        switch node.state {
        case .running: "starting units…"
        case .done(let success): success ? "no units — nothing to fan out" : "failed before fan-out"
        case .skipped: "skipped"
        default: "awaiting units…"
        }
    }

    // MARK: 1...12 units — inline grid

    private func inlineGrid(fanout: FanoutVM) -> some View {
        let cols = min(fanout.total, 8)
        return VStack(alignment: .leading, spacing: 4) {
            gridHeader(fanout: fanout)
            LazyVGrid(columns: Array(repeating: GridItem(.fixed(15), spacing: 3), count: cols), spacing: 3) {
                ForEach(fanout.units) { unit in
                    unitSquare(unit, size: 15)
                }
            }
        }
    }

    private func gridHeader(fanout: FanoutVM) -> some View {
        HStack(spacing: 4) {
            Text("for each · \(node.id) · \(fanout.total)")
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: 4)
            HStack(spacing: 2) {
                Text("\(fanout.done) ✓")
                if fanout.failed > 0 {
                    Text("· \(fanout.failed) ✕")
                        .foregroundStyle(Color.rupuErr)
                }
            }
        }
        .font(.dataMono(10))
        .foregroundStyle(node.kind.accent)
    }

    private func unitSquare(_ unit: UnitVM, size: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: 3)
            .fill(unitFillColor(unit.state))
            .frame(width: size, height: size)
            .contentShape(Rectangle())
            .onTapGesture { onUnitTap(unit.id) }
            .help("\(unit.key) · \(unit.state.stateLabel)")
    }

    /// Web-parity `glyphBg`: a `.pending` unit reads as the muted `skipped` slate fill rather
    /// than the dimmer `pending` glyph color — `layoutGraph` never actually produces a pending
    /// fan-out unit today (every unit is `.running` or `.done`), so this is a defensive port
    /// rather than a reachable branch, kept for the same "never guess a color for a state we
    /// haven't seen" posture the rest of this app takes.
    private func unitFillColor(_ state: NodeState) -> Color {
        state == .pending ? Color.status(.skipped) : Color.status(state.tone)
    }

    // MARK: > 12 units — collapsed summary card

    private func collapsedCard(fanout: FanoutVM) -> some View {
        let pct = fanout.total > 0 ? Int((Double(fanout.done) / Double(fanout.total) * 100).rounded()) : 0
        let pending = max(fanout.total - fanout.done - fanout.failed - fanout.running, 0)
        let preview = Array(fanout.units.prefix(60))

        return VStack(alignment: .leading, spacing: 6) {
            Text("for each · \(node.id)")
                .font(.dataMono(10))
                .foregroundStyle(node.kind.accent)
                .lineLimit(1)
                .truncationMode(.tail)

            HStack(alignment: .lastTextBaseline, spacing: 6) {
                Text("\(fanout.done)")
                    .font(.dataMono(20))
                    .foregroundStyle(Color.rupuInk)
                Text("/ \(fanout.total) units")
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
                Spacer(minLength: 4)
                Text("\(pct)%")
                    .font(.dataMono(13))
                    .foregroundStyle(node.kind.accent)
            }

            progressBar(pct: pct)

            HStack(spacing: 10) {
                countLabel(fanout.done, "done")
                countLabel(fanout.running, "running")
                countLabel(pending, "pending")
                if fanout.failed > 0 {
                    Text("\(fanout.failed) failed")
                        .font(.noteText)
                        .fontWeight(.bold)
                        .foregroundStyle(Color.rupuErr)
                }
            }

            LazyVGrid(columns: Array(repeating: GridItem(.fixed(9), spacing: 2), count: 20), spacing: 2) {
                ForEach(preview) { unit in
                    unitSquare(unit, size: 9)
                }
            }

            Button {
                isExpanded.toggle()
            } label: {
                Text(isExpanded ? "▾ collapse" : "▸ expand all \(fanout.total)")
                    .font(.noteText)
                    .fontWeight(.medium)
                    .foregroundStyle(node.kind.accent)
            }
            .buttonStyle(.plain)

            if isExpanded {
                LazyVGrid(columns: Array(repeating: GridItem(.fixed(15), spacing: 3), count: 8), spacing: 3) {
                    ForEach(fanout.units) { unit in
                        unitSquare(unit, size: 15)
                    }
                }
            }
        }
    }

    private func progressBar(pct: Int) -> some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Color.rupuSurface)
                Capsule()
                    .fill(
                        LinearGradient(
                            colors: [Color.status(.running), Color.status(.done)],
                            startPoint: .leading,
                            endPoint: .trailing
                        )
                    )
                    .frame(width: geo.size.width * CGFloat(pct) / 100)
            }
        }
        .frame(height: 9)
    }

    private func countLabel(_ count: Int, _ label: String) -> some View {
        HStack(spacing: 3) {
            Text("\(count)")
                .fontWeight(.bold)
                .foregroundStyle(Color.rupuInk)
            Text(label)
                .foregroundStyle(Color.rupuDim)
        }
        .font(.noteText)
    }
}
