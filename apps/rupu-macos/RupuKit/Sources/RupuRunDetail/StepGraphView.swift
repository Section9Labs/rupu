import SwiftUI
import RupuDesign
import RupuStore

/// Horizontal step graph: one kind-routed node card per `GraphNodeVM`,
/// joined by animated `GraphEdge` connectors, scrollable when the workflow
/// has more nodes than fit the window. No unit tests here by design (per
/// the brief) — `GraphLayoutTests` covers the pure `layoutGraph` logic this
/// view only renders.
///
/// Each node routes to its Task-3 card by `kind`: `.parallel` ->
/// `ParallelNodeCard`, `.panel` -> `PanelNodeCard`, `.forEach` ->
/// `FanoutNodeView` (which itself branches on `fanout` being nil/populated/
/// large for the placeholder/grid/collapsed presentations), everything else
/// (`.step`, `.gate`, `.action`, `.run`) -> `StepNodeCard`. A tap anywhere on
/// a card's background selects the node (`onSelect(node.id)`); the one
/// exception is a fan-out unit square, which calls `onSelectUnit(stepID,
/// index)` instead — its own `.onTapGesture` (`ContainerNodes.swift`'s
/// `unitSquare`) sits nested inside this view's card-level tap gesture, so a
/// tap that lands on a unit square is consumed there and never bubbles up to
/// also fire `onSelect` (mirrors the web's explicit body-click skip for
/// fan-out/panel nodes in `RunGraph.tsx`'s `handleNodeClick`).
public struct StepGraphView: View {
    private let nodes: [GraphNodeVM]
    private let selectedID: String?
    private let onSelect: (String) -> Void
    private let onSelectUnit: (String, Int) -> Void

    public init(
        nodes: [GraphNodeVM],
        selectedID: String? = nil,
        onSelect: @escaping (String) -> Void = { _ in },
        onSelectUnit: @escaping (String, Int) -> Void = { _, _ in }
    ) {
        self.nodes = nodes
        self.selectedID = selectedID
        self.onSelect = onSelect
        self.onSelectUnit = onSelectUnit
    }

    public var body: some View {
        // RenderMeter seam (Plan 5, Task 1) — one line, safe to delete.
        let _ = RenderMeter.tick("StepGraphView")
        ScrollView(.horizontal, showsIndicators: false) {
            // `LazyHStack`, not `HStack` (perf & interaction arc, Plan 5
            // Task 3): a long-running workflow's step graph can grow wide
            // enough that off-screen node cards no longer need to be
            // instantiated/laid out on every re-render. Node identity stays
            // step-id-stable (`id: \.element.id`, unchanged) — Plan 6's
            // animation work needs that to keep working across a lazy
            // container the same way it would under a plain `HStack`.
            LazyHStack(spacing: 0) {
                ForEach(Array(nodes.enumerated()), id: \.element.id) { index, node in
                    if index > 0 {
                        GraphEdge(source: nodes[index - 1], target: node)
                    }
                    nodeCard(for: node)
                        .contentShape(Rectangle())
                        .onTapGesture { onSelect(node.id) }
                }
            }
            .padding(16)
        }
    }

    @ViewBuilder
    private func nodeCard(for node: GraphNodeVM) -> some View {
        let isSelected = node.id == selectedID
        switch node.kind {
        case .parallel:
            ParallelNodeCard(node: node, isSelected: isSelected)
        case .panel:
            PanelNodeCard(node: node, isSelected: isSelected)
        case .forEach:
            FanoutNodeView(node: node, isSelected: isSelected) { index in
                onSelectUnit(node.id, index)
            }
        case .step, .gate, .action, .run:
            StepNodeCard(node: node, isSelected: isSelected)
        }
    }
}
