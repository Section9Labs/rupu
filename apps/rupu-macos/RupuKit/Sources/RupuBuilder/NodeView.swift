import SwiftUI
import RupuDesign
import RupuFlowKit
import RupuStore

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
    /// Run-mode overlay inputs (Task 14) — `nil`/`.design` for every
    /// Design-mode render, which is untouched by either. `overlayState` is
    /// this node's own resolved `NodeState` (already looked up by the
    /// caller from `BuilderStore.runOverlay?.states[node.id]`, defaulting
    /// to `.pending` when the run overlay has no entry for this step —
    /// see `RunOverlayModel.swift`'s doc comment on why a missing key means
    /// "not reached yet", not "unknown"); `unitProgress` is this node's
    /// `(done, total)` fan-out count, `nil` for every non-`for_each` node or
    /// a `for_each` with no unit rows yet.
    let mode: BuilderStore.Mode
    let overlayState: NodeState?
    let unitProgress: (done: Int, total: Int)?
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

    private var runOverlayActive: Bool { mode == .run }

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
        .opacity(runContentOpacity)
        .overlay(runPendingRing)
        .overlay(alignment: .topTrailing) { runStateBadge }
        .offset(dragTranslation)
        .contentShape(Rectangle())
        .onTapGesture { onSelect() }
        .gesture(bodyDrag)
        .onHover { hovering = $0 }
    }

    // MARK: - Run-mode overlay (Task 14)

    /// `.pending` -> 0.6 opacity, `.skipped` -> 0.45, everything else (and
    /// Design mode, and no overlay entry at all outside `.run`) -> 1 —
    /// per the brief's exact glyph language.
    private var runContentOpacity: CGFloat {
        guard runOverlayActive, let overlayState else { return 1 }
        switch overlayState {
        case .pending: return 0.6
        case .skipped: return 0.45
        default: return 1
        }
    }

    /// `.pending`'s dashed ring — drawn OUTSIDE the node's own shape stroke,
    /// so it reads as an overlay rather than replacing the kind-accent
    /// silhouette stroke underneath it.
    @ViewBuilder
    private var runPendingRing: some View {
        if runOverlayActive, overlayState == .pending {
            SilhouetteShape(name: visual.shape)
                .stroke(Color.rupuBorder, style: StrokeStyle(lineWidth: 1.4, dash: [3, 2]))
        } else if runOverlayActive, overlayState == .skipped {
            SilhouetteShape(name: visual.shape)
                .stroke(Color.rupuMute, lineWidth: 1.4)
        }
    }

    /// The top-right corner badge: a green check for a SUCCESSFUL `.done`,
    /// a red X for a FAILED one (review fix, Finding 1 — codebase precedent
    /// is `StatusDescriptor.descriptor(for:).icon`/`StatusPill`'s own
    /// `.failed -> .xCircle`; a recolored checkmark read as "failed but
    /// still somehow a check", which is exactly backwards), a pulsing dot
    /// for `.running`/`.gatePending`. Nothing for `.pending`/`.skipped`
    /// (those read purely through `runContentOpacity`/`runPendingRing`
    /// above) or Design mode.
    @ViewBuilder
    private var runStateBadge: some View {
        if runOverlayActive, let overlayState, let icon = badgeIcon(for: overlayState) {
            // `runStrokeColor` already resolves the exact same
            // done(success)/running/gatePending -> status-color mapping
            // this badge needs — reused rather than re-deriving it a
            // second time.
            Icon(icon, size: 14)
                .foregroundStyle(runStrokeColor ?? Color.status(.done))
                .padding(3)
        } else if runOverlayActive, overlayState == .running {
            RunPulseDot(tone: .running)
                .padding(5)
        } else if runOverlayActive, overlayState == .gatePending {
            RunPulseDot(tone: .awaiting)
                .padding(5)
        }
    }

    /// The node's own accent/selection stroke — `.done`/`.running`/
    /// `.gatePending` swap it to the state's status color per the brief;
    /// every other state (including no overlay entry, or Design mode)
    /// keeps the plain kind-accent/selection stroke `silhouetteBody`
    /// already draws.
    private var runStrokeColor: Color? {
        guard runOverlayActive, let overlayState else { return nil }
        switch overlayState {
        case .done(let success): return Color.status(success ? .done : .failed)
        case .running: return Color.status(.running)
        case .gatePending: return Color.status(.awaiting)
        default: return nil
        }
    }

    // MARK: - Silhouette style (current)

    private var silhouetteBody: some View {
        ZStack(alignment: .topLeading) {
            SilhouetteShape(name: visual.shape)
                .fill(Color.rupuPanel)
            // 1.4pt kind-accent stroke normally; selected swaps to a 2pt
            // BRAND stroke (not accent) plus the outer brand shadow the
            // `.shadow(...)` modifier on `body` adds — spec's selection
            // treatment is a brand highlight, not a thicker accent ring.
            // Run mode's `runStrokeColor` (done/running/gatePending) wins
            // over both when present — selection still takes the thicker
            // 2pt line width, just recolored.
            SilhouetteShape(name: visual.shape)
                .stroke(
                    runStrokeColor ?? (selected ? Color.rupuBrand : accent),
                    lineWidth: selected ? 2 : 1.4
                )
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
            // Selection border goes brand (not the kind accent) per spec —
            // same rule the silhouette body's own stroke follows above; the
            // accent stays visible via the top bar fill below either way.
            RoundedRectangle(cornerRadius: 6)
                .stroke(selected ? Color.rupuBrand : accent, lineWidth: selected ? 2 : 1.4)
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
        // Task 14: a `for_each` node with a live fan-out count replaces its
        // usual sub-line ("over <list>"/agent name) with "n/m items" while
        // Run mode has progress to show — everything else (every other
        // kind, Design mode, or a `for_each` with no `unitProgress` entry
        // yet) keeps `subLine(for:)`'s normal design-mode text unchanged.
        let sub: (text: String, mono: Bool)
        if runOverlayActive, let unitProgress {
            sub = ("\(unitProgress.done)/\(unitProgress.total) items", false)
        } else {
            sub = subLine(for: node.data)
        }
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
                // `.highPriorityGesture`, NOT plain `.gesture` — a port dot
                // sits INSIDE the node body, which already carries its own
                // plain `.gesture(bodyDrag)` (see `body` below). SwiftUI's
                // default gesture composition does not guarantee the
                // descendant wins a same-type ambiguity (two `DragGesture`s
                // recognizing the same touch); per Apple's own
                // `highPriorityGesture(_:)` docs, this modifier is exactly
                // what makes a subview's gesture take precedence over an
                // ancestor's — without it, a drag started on the dot risks
                // being captured by `bodyDrag` instead, silently turning
                // every connect-drag into a node move. This can't be
                // exercised by the pure-geometry test suite (no SwiftUI
                // render pass here); confirm empirically in the running app.
                .highPriorityGesture(
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

/// Pure seam (review fix, Finding 1) so the badge-icon choice is testable
/// without a SwiftUI render pass: `.checkCircle2` for a SUCCESSFUL `.done`,
/// `.xCircle` for a FAILED one — mirrors `RupuDesign.StatusDescriptor.
/// descriptor(for:).icon`/`StatusPill`'s own `.failed -> .xCircle` mapping,
/// which the badge previously diverged from (a red-recolored checkmark
/// instead of an X). `nil` for every other state — `.running`/`.gatePending`
/// render `RunPulseDot` instead (see `runStateBadge`), `.pending`/`.skipped`
/// render no badge at all.
func badgeIcon(for state: NodeState) -> LucideIcon? {
    switch state {
    case .done(let success): success ? .checkCircle2 : .xCircle
    default: nil
    }
}

/// Run mode's top-right node badge for `.running`/`.gatePending` (Task 14):
/// an 8pt filled dot pulsing scale 1 -> 1.3, opacity 1 -> 0.5, on a
/// `.easeInOut(duration: 1.2)` repeat-forever — static (no animation at all)
/// under Reduce Motion. Same start/stop-on-`onAppear`-and-`onChange`
/// discipline as `RupuRunDetail/Graph/StepNodeCard.swift`'s `RingPulse`
/// (this view's closest sibling): restart only at the animated <-> static
/// boundary, never on every render, and react to a live Reduce Motion
/// toggle exactly the way that view does.
struct RunPulseDot: View {
    let tone: StatusTone

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var isPulsing = false

    private let size: CGFloat = 8

    var body: some View {
        Circle()
            .fill(Color.status(tone))
            .frame(width: size, height: size)
            .scaleEffect(active ? 1.3 : 1)
            .opacity(active ? 0.5 : 1)
            .onAppear { update() }
            .onChange(of: reduceMotion) { _, _ in update() }
    }

    private var active: Bool { isPulsing && !reduceMotion }

    private func update() {
        guard !reduceMotion else {
            isPulsing = false
            return
        }
        guard !isPulsing else { return }
        withAnimation(.easeInOut(duration: 1.2).repeatForever(autoreverses: true)) {
            isPulsing = true
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
