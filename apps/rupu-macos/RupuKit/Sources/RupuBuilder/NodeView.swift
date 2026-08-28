import SwiftUI
import RupuDesign
import RupuFlowKit

/// Which node body treatment `NodeView` paints. `.silhouette` (the shipped
/// default) draws the kind's actual flowchart symbol (`ShapePaths.swift`'s
/// `SilhouetteShape`/`SilhouetteExtras`, from `RupuFlowKit.shapeFor`);
/// `.cards` is a flag-gated rounded-rect alternative (6pt radius, 3.5pt
/// accent top bar) kept alive for a possible future style toggle but not
/// wired to any UI control yet.
enum BuilderNodeStyle {
    case silhouette
    case cards

    static let current: BuilderNodeStyle = .silhouette
}

/// One canvas node: 176×68, positioned by the parent (`CanvasView` applies
/// `.offset(x: node.position.x, y: node.position.y)`) — this view only
/// knows its OWN local 0...176 × 0...68 box. Owns its transient drag offset
/// (`@GestureState`, snaps back to zero the instant the drag gesture ends,
/// exactly when `onMoveEnded` commits the real position through the store)
/// and its port-drag gestures (which report canvas-ABSOLUTE points via the
/// `"canvasSpace"` named coordinate space `CanvasView` establishes).
struct NodeView: View {
    let node: GraphNode
    let selected: Bool
    let onSelect: () -> Void
    let onMoveEnded: (CGSize) -> Void
    let onPortDragChanged: (String?, CGPoint) -> Void
    let onPortDragEnded: (String?, CGPoint) -> Void

    @GestureState private var dragTranslation: CGSize = .zero
    @State private var hovering = false

    private var visual: KindVisual { kindVisual(node.data.kind) }
    private var geometry: NodeShapeGeometry { shapeFor(visual.shape, w: NODE_W, h: NODE_H) }
    // `RupuBuilder.` prefix disambiguates from `View`'s own deprecated
    // `accentColor(_:)` method — see `ShapePaths.swift`'s doc comment.
    private var accent: Color { RupuBuilder.accentColor(visual.accent) }

    var body: some View {
        Group {
            switch BuilderNodeStyle.current {
            case .silhouette:
                silhouetteBody
            case .cards:
                cardsBody
            }
        }
        .frame(width: NODE_W, height: NODE_H)
        .shadow(color: selected ? Color.rupuBrand.opacity(0.35) : .clear, radius: selected ? 8 : 0)
        .offset(dragTranslation)
        .contentShape(Rectangle())
        .onTapGesture { onSelect() }
        .gesture(bodyDrag)
        .onHover { hovering = $0 }
    }

    // MARK: - Silhouette style (current)

    private var silhouetteBody: some View {
        ZStack(alignment: .topLeading) {
            SilhouetteShape(name: visual.shape)
                .fill(Color.rupuPanel)
            SilhouetteShape(name: visual.shape)
                .stroke(accent, lineWidth: selected ? 2 : 1.4)
            SilhouetteExtras(name: visual.shape)
                .stroke(Color.rupuBorderStrong, lineWidth: 1)

            nodeContent

            ports
            targetPort
        }
    }

    // MARK: - Cards style (flag-gated alternative)

    private var cardsBody: some View {
        ZStack(alignment: .top) {
            RoundedRectangle(cornerRadius: 6)
                .fill(Color.rupuPanel)
            RoundedRectangle(cornerRadius: 6)
                .stroke(selected ? Color.rupuBrand : Color.rupuBorderStrong, lineWidth: selected ? 2 : 1.4)
            UnevenRoundedRectangle(topLeadingRadius: 6, topTrailingRadius: 6)
                .fill(accent)
                .frame(height: 3.5)

            nodeContent
                .padding(.top, 10)
                .padding(.horizontal, 10)

            ports
            targetPort
        }
        .clipShape(RoundedRectangle(cornerRadius: 6))
    }

    // MARK: - Content

    private var nodeContent: some View {
        let sub = subLine(for: node.data)
        return VStack(alignment: geometry.centered ? .center : .leading, spacing: 3) {
            HStack(spacing: 4) {
                if let icon = LucideIcon(rawValue: visual.iconName) {
                    Icon(icon, size: 11)
                        .foregroundStyle(accent)
                } else {
                    // `LucideIconDataTests` (RupuDesignTests) verifies every
                    // `LucideIcon` case has SVG path data; `kindVisual`'s
                    // `iconName` strings are pinned against real
                    // `LucideIcon.rawValue`s by `KindVisualsTests`
                    // (RupuFlowKitTests) — this branch should be
                    // unreachable in a build where both hold, so it fails
                    // loudly in debug rather than silently dropping the
                    // icon.
                    let _ = assertionFailure("Unknown lucide icon name: \(visual.iconName)")
                    EmptyView()
                }
                Text(node.data.kind.rawValue.uppercased())
                    .font(.dataMono(9))
                    .kerning(0.8)
                    .foregroundStyle(accent)
            }
            Text(node.id)
                .font(.system(size: 12.5, weight: .semibold))
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
            if !sub.text.isEmpty {
                Text(sub.text)
                    .font(sub.mono ? .dataMono(10.5) : .system(size: 10.5))
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(1)
            }
        }
        .frame(width: geometry.safe.w, height: geometry.safe.h, alignment: geometry.centered ? .center : .leading)
        .offset(x: geometry.safe.x, y: geometry.safe.y)
    }

    // MARK: - Ports

    /// Source dots: one 8pt circle per `SourceAnchor`, positioned in LOCAL
    /// box coordinates. A branch node's two anchors also carry a mini
    /// THEN/ELSE label near the dot. Each dot's own `DragGesture` (in the
    /// `"canvasSpace"` named coordinate space `CanvasView` sets up on its
    /// outer content `ZStack`) is what starts a connect-drag — `onChanged`/
    /// `onEnded` report canvas-absolute points straight through, no local
    /// coordinate math needed here.
    private var ports: some View {
        ForEach(Array(geometry.sources.enumerated()), id: \.offset) { _, source in
            let point = anchorPoint(for: source.anchor, nodeFrame: CGRect(x: 0, y: 0, width: NODE_W, height: NODE_H))
            portDot
                .overlay(alignment: labelAlignment(for: source.anchor)) {
                    if let arm = source.arm {
                        Text(arm.uppercased())
                            .font(.dataMono(8))
                            .foregroundStyle(accent)
                            .fixedSize()
                            .offset(labelOffset(for: source.anchor))
                    }
                }
                .position(point)
                .gesture(
                    DragGesture(minimumDistance: 2, coordinateSpace: .named("canvasSpace"))
                        .onChanged { value in onPortDragChanged(source.arm, value.location) }
                        .onEnded { value in onPortDragEnded(source.arm, value.location) }
                )
        }
    }

    /// The target handle — shown only while hovering (spec: "target dot on
    /// the left at hover-time only"), purely a visual affordance that this
    /// node accepts an incoming connection; it carries no gesture of its
    /// own (the DRAG originates from the source's port, not the target's).
    @ViewBuilder
    private var targetPort: some View {
        if hovering {
            let point = anchorPoint(for: geometry.target, nodeFrame: CGRect(x: 0, y: 0, width: NODE_W, height: NODE_H))
            portDot.position(point)
        }
    }

    private var portDot: some View {
        Circle()
            .fill(Color.rupuBorderStrong)
            .overlay(Circle().stroke(accent, lineWidth: 1.4))
            .frame(width: 8, height: 8)
    }

    private func labelAlignment(for anchor: HandleAnchor) -> Alignment {
        switch anchor.side {
        case .left: .leading
        case .right: .trailing
        case .bottom: .bottom
        }
    }

    private func labelOffset(for anchor: HandleAnchor) -> CGSize {
        switch anchor.side {
        case .left: CGSize(width: -18, height: -8)
        case .right: CGSize(width: 18, height: -8)
        case .bottom: CGSize(width: 0, height: 12)
        }
    }

    // MARK: - Body drag

    private var bodyDrag: some Gesture {
        DragGesture(minimumDistance: 2)
            .updating($dragTranslation) { value, state, _ in
                state = value.translation
            }
            .onEnded { value in
                onMoveEnded(value.translation)
            }
    }
}

/// Row 3's content, per kind (Task 11 brief) — `mono` selects `dataMono`
/// (run/action; command/tool identifiers read as data) vs the plain sans
/// sub-line font every other kind uses. An empty `text` means "render no
/// sub-line row at all" (`NodeView.nodeContent` skips it).
func subLine(for data: StepNodeData) -> (text: String, mono: Bool) {
    switch data.kind {
    case .step:
        return (data.agent ?? "", false)
    case .forEach:
        if let agent = data.agent, !agent.isEmpty {
            return (agent, false)
        }
        if let forEach = data.forEach, !forEach.isEmpty {
            return ("over \(forEach)", false)
        }
        return ("", false)
    case .parallel:
        return ("\(data.parallel?.count ?? 0) sub-steps", false)
    case .panel:
        return ("\(data.panel?.panelists.count ?? 0) panelists", false)
    case .branch:
        return ("if \(truncated(data.condition ?? "", 40))", false)
    case .split:
        return ("→ \(data.split?.count ?? 0) targets", false)
    case .join:
        return ("wait: \(joinWaitText(data.joinWait))", false)
    case .approvalGate:
        if let prompt = data.approvalPrompt, !prompt.isEmpty {
            return (truncated(prompt, 40), false)
        }
        return ("human approval", false)
    case .action:
        return (data.action ?? "", true)
    case .run:
        return (runCommandLine(data.runBlock), true)
    }
}

private func truncated(_ s: String, _ limit: Int) -> String {
    guard s.count > limit else { return s }
    return String(s.prefix(limit)) + "…"
}

private func joinWaitText(_ wait: JoinWait?) -> String {
    switch wait {
    case .none, .all: "all"
    case .any: "any"
    case .count(let n): "\(n)"
    }
}

/// `$ <cmd> <args…>` from a `run:` block's verbatim `YAMLValue` mapping —
/// empty when `cmd` isn't a string (an empty/malformed `run:` block).
private func runCommandLine(_ block: YAMLValue?) -> String {
    guard let cmd = block?["cmd"]?.stringValue else { return "" }
    let args = block?["args"]?.sequenceValue?.compactMap(\.stringValue) ?? []
    return (["$", cmd] + args).joined(separator: " ")
}
