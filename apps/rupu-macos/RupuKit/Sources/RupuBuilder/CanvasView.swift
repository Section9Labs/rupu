import SwiftUI
import RupuDesign
import RupuFlowKit

// The Workflow Builder's canvas (macOS design plan, Task 11): a scrollable
// `ZStack` — dot-grid background, `EdgeLayer`, then every node positioned
// absolutely at `node.position` — with node drag, port-drag-to-connect, and
// canvas-focused keyboard shortcuts (Esc clears selection, Delete/Backspace
// deletes the selection). Task 10 shipped a dot-grid PLACEHOLDER directly in
// `WorkflowBuilderScreen`; this file is the real thing, and
// `WorkflowBuilderScreen` now embeds it instead.

/// The fixed node box size (spec constant, also `RupuFlowKit.AutoLayout`'s
/// own `NODE_W`/`NODE_H` — duplicated here rather than exported from
/// `RupuFlowKit`, which stays a pure geometry/model module with no reason to
/// expose a SwiftUI-facing layout constant).
let NODE_W: CGFloat = 176
let NODE_H: CGFloat = 68

/// Extra clearance, in points, added past the farthest node's far edge when
/// computing `contentSize` — enough room to drop a new node past the last
/// one without immediately needing to scroll.
private let CONTENT_MARGIN: CGFloat = 120
private let CONTENT_MIN = CGSize(width: 1200, height: 800)

/// The canvas' content size: big enough to hold every node plus
/// `CONTENT_MARGIN` of clearance past the farthest one, floored at
/// `CONTENT_MIN` so an empty or small graph still gets a comfortably large
/// canvas to work in.
func contentSize(nodes: [GraphNode]) -> CGSize {
    guard !nodes.isEmpty else { return CONTENT_MIN }
    let maxX = nodes.map { $0.position.x + NODE_W + CONTENT_MARGIN }.max() ?? CONTENT_MIN.width
    let maxY = nodes.map { $0.position.y + NODE_H + CONTENT_MARGIN }.max() ?? CONTENT_MIN.height
    return CGSize(width: max(CONTENT_MIN.width, maxX), height: max(CONTENT_MIN.height, maxY))
}

/// The node whose `NODE_W`×`NODE_H` frame (top-left at `node.position`)
/// contains `point`, or `nil` outside every node. Nodes are searched in
/// REVERSE array order — the canvas paints nodes in array order, so the
/// LAST node in the array is the topmost on screen, and a click/port-drop on
/// an overlap must hit whatever is actually drawn on top.
func nodeHit(at point: CGPoint, nodes: [GraphNode]) -> String? {
    for node in nodes.reversed() {
        let frame = CGRect(x: node.position.x, y: node.position.y, width: NODE_W, height: NODE_H)
        if frame.contains(point) { return node.id }
    }
    return nil
}

/// Resolves a shape-relative `HandleAnchor` into an absolute canvas point
/// given the node's own frame — the one place `side`/`fraction`/`inset`
/// become real coordinates. Mirrors the web editor's equivalent handle-style
/// computation (`nodeShapes.ts` consumers), just producing a point instead
/// of a CSS `style` object.
func anchorPoint(for anchor: HandleAnchor, nodeFrame: CGRect) -> CGPoint {
    switch anchor.side {
    case .left:
        return CGPoint(x: nodeFrame.minX + anchor.inset, y: nodeFrame.minY + nodeFrame.height * anchor.fraction)
    case .right:
        return CGPoint(x: nodeFrame.maxX - anchor.inset, y: nodeFrame.minY + nodeFrame.height * anchor.fraction)
    case .bottom:
        return CGPoint(x: nodeFrame.minX + nodeFrame.width * anchor.fraction, y: nodeFrame.maxY - anchor.inset)
    }
}

/// The absolute canvas point a port-drag preview line should originate
/// from: `nodeID`'s own `arm` source anchor (or its single default source
/// when `arm` is `nil`), resolved against that node's current frame. `nil`
/// when `nodeID` no longer exists (the node was deleted mid-drag).
func portOrigin(nodeID: String, arm: String?, nodes: [GraphNode]) -> CGPoint? {
    guard let node = nodes.first(where: { $0.id == nodeID }) else { return nil }
    let geo = shapeFor(kindVisual(node.data.kind).shape, w: NODE_W, h: NODE_H)
    let anchor = geo.sources.first(where: { $0.arm == arm })?.anchor
        ?? geo.sources.first?.anchor
        ?? HandleAnchor(side: .right)
    let frame = CGRect(x: node.position.x, y: node.position.y, width: NODE_W, height: NODE_H)
    return anchorPoint(for: anchor, nodeFrame: frame)
}

/// In-flight port-drag state: which node/arm the drag started from, and the
/// current pointer location (canvas-absolute — the drag gesture that
/// produces this uses `.named("canvasSpace")`, see `NodeView.swift`'s port
/// dot gesture).
struct PortDrag: Equatable {
    var sourceID: String
    var arm: String?
    var current: CGPoint
}

/// A dot-grid background at a 22pt pitch — moved here verbatim from
/// `WorkflowBuilderScreen`'s Task 10 placeholder (`canvasPlaceholder`). One
/// 1×1 `rupuBorder` circle per grid intersection, drawn directly with
/// `Canvas` rather than a per-dot SwiftUI view — a `ForEach` over a
/// viewport-sized dot grid would be thousands of views for no visual
/// benefit.
struct DotGrid: View {
    var body: some View {
        Canvas { context, size in
            let pitch: CGFloat = 22
            var x: CGFloat = pitch / 2
            while x < size.width {
                var y: CGFloat = pitch / 2
                while y < size.height {
                    let dot = Path(ellipseIn: CGRect(x: x - 0.5, y: y - 0.5, width: 1, height: 1))
                    context.fill(dot, with: .color(Color.rupuBorder))
                    y += pitch
                }
                x += pitch
            }
        }
        .background(Color.rupuBg)
    }
}

struct CanvasView: View {
    @Bindable var store: BuilderStore

    @State private var portDrag: PortDrag?
    @FocusState private var canvasFocused: Bool

    var body: some View {
        ScrollView([.horizontal, .vertical]) {
            let size = contentSize(nodes: store.graph.nodes)
            ZStack(alignment: .topLeading) {
                DotGrid()
                    .frame(width: size.width, height: size.height)
                    .contentShape(Rectangle())
                    .onTapGesture {
                        store.select(nil)
                        canvasFocused = true
                    }

                EdgeLayer(edges: store.graph.edges, nodes: store.graph.nodes)
                    .frame(width: size.width, height: size.height)
                    .allowsHitTesting(false)

                ForEach(store.graph.nodes) { node in
                    NodeView(
                        node: node,
                        selected: store.selectedID == node.id,
                        onSelect: {
                            store.select(node.id)
                            canvasFocused = true
                        },
                        onMoveEnded: { translation in
                            handleMoveEnded(node: node, translation: translation)
                        },
                        onPortDragChanged: { arm, point in
                            portDrag = PortDrag(sourceID: node.id, arm: arm, current: point)
                        },
                        onPortDragEnded: { arm, point in
                            handlePortDragEnded(sourceID: node.id, arm: arm, at: point)
                        }
                    )
                    .offset(x: node.position.x, y: node.position.y)
                }

                portDragPreview
            }
            .frame(width: size.width, height: size.height)
            .coordinateSpace(name: "canvasSpace")
        }
        // Keyboard invariant: the canvas only receives `.onKeyPress` events
        // while IT holds focus (`canvasFocused`) — a click inside the canvas
        // (background or a node) grabs focus explicitly above; a rail text
        // field (Task 12/13) holds its OWN focus while it's being edited,
        // which SwiftUI focus is exclusive about, so Delete/Backspace typed
        // into a text field can never reach `deleteSelection()` here. Esc/
        // Delete are wired ONLY on this focusable wrapper, never globally.
        .focusable()
        .focused($canvasFocused)
        .onKeyPress(.escape) {
            store.select(nil)
            return .handled
        }
        .onKeyPress(.delete) {
            store.deleteSelection()
            return .handled
        }
        .onKeyPress(.deleteForward) {
            store.deleteSelection()
            return .handled
        }
    }

    @ViewBuilder
    private var portDragPreview: some View {
        if let portDrag, let origin = portOrigin(nodeID: portDrag.sourceID, arm: portDrag.arm, nodes: store.graph.nodes) {
            let (c1, c2) = bezierPoints(from: origin, to: portDrag.current)
            Path { path in
                path.move(to: origin)
                path.addCurve(to: portDrag.current, control1: c1, control2: c2)
            }
            .stroke(Color.rupuBrand, style: StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [4, 3]))
            .allowsHitTesting(false)
        }
    }

    /// Node body drag end: clamp the new position to `>= 0` on both axes
    /// (spec — a node can never be dragged into negative canvas space) and
    /// commit through `store.moveNode(id:to:)`.
    private func handleMoveEnded(node: GraphNode, translation: CGSize) {
        let newPosition = CGPoint(
            x: max(0, node.position.x + translation.width),
            y: max(0, node.position.y + translation.height)
        )
        store.moveNode(id: node.id, to: newPosition)
    }

    /// Port-drag release: hit-test every node's frame at the drop point
    /// (`nodeHit`); a hit on a DIFFERENT node calls `store.connect` (whose
    /// rejections — self-loop, duplicate, cycle — surface through
    /// `store.commitError`, rendered by `WorkflowBuilderScreen`'s banner).
    /// A drop outside any node, or back onto the source node itself, is
    /// silently discarded — no connection was ever drawn there.
    private func handlePortDragEnded(sourceID: String, arm: String?, at point: CGPoint) {
        portDrag = nil
        guard let targetID = nodeHit(at: point, nodes: store.graph.nodes), targetID != sourceID else { return }
        store.connect(source: sourceID, target: targetID, arm: arm)
    }
}
