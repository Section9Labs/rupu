import CoreGraphics

// autoLayout — positions unpositioned graph nodes left-to-right by longest-
// path layering, and leaves any already-positioned node exactly where it
// is. Used both for a brand-new workflow (every node lands at `.zero`) and
// for the YAML-reload reconcile path (surviving nodes keep their on-screen
// position; newly-appeared nodes get a slot).
//
// Unlike the web editor's `workflowLayout.ts` (which delegates to `dagre`),
// this is a small hand-rolled layering pass: no third-party Swift
// dependencies are allowed in RupuKit, and the box-size variance dagre
// optimizes for (parallel/panel grow with content) isn't needed here — every
// node reserves the same fixed box, so a simpler longest-path column
// assignment is sufficient.

/// Base step card box.
private let NODE_W: CGFloat = 176
private let NODE_H: CGFloat = 68
private let GAP_X: CGFloat = 84
private let GAP_Y: CGFloat = 46
private let ORIGIN_X: CGFloat = 60
private let ORIGIN_Y: CGFloat = 60

/// Position every `.zero`-positioned node left-to-right; leave every
/// already-positioned node untouched. Returns a NEW array in the SAME order
/// as `nodes`; inputs are never mutated.
///
/// Column assignment is longest-path layering: `column(n)` is the length of
/// the longest edge-path reaching `n` from any root (an in-degree-0 node) —
/// `column(root) = 0`, `column(n) = max(column(p) for p in predecessors(n))
/// + 1`. Row assignment, within a column, orders nodes by the mean y of
/// their direct predecessors (already final by the time their column is
/// processed, since predecessors always sit in a strictly earlier column),
/// then by id for a deterministic tiebreak.
///
/// Cycle-safe: if the node set isn't a DAG (`topoSort` reports a cycle),
/// column assignment falls back to each node's index in the input array —
/// still deterministic, just without the longest-path property.
public func autoLayout(nodes: [GraphNode], edges: [GraphEdge]) -> [GraphNode] {
    guard !nodes.isEmpty else { return [] }

    let ids = Set(nodes.map(\.id))
    let relevantEdges = edges.filter { ids.contains($0.source) && ids.contains($0.target) && $0.source != $0.target }

    var predecessors: [String: [String]] = [:]
    for e in relevantEdges {
        predecessors[e.target, default: []].append(e.source)
    }

    var column: [String: Int] = [:]
    switch topoSort(nodes: nodes, edges: relevantEdges) {
    case .order(let sorted):
        for n in sorted {
            let preds = predecessors[n.id] ?? []
            if preds.isEmpty {
                column[n.id] = 0
            } else {
                column[n.id] = (preds.compactMap { column[$0] }.max() ?? -1) + 1
            }
        }
    case .cycle:
        for (i, n) in nodes.enumerated() {
            column[n.id] = i
        }
    }

    var byColumn: [Int: [GraphNode]] = [:]
    for n in nodes {
        byColumn[column[n.id] ?? 0, default: []].append(n)
    }

    var finalY: [String: CGFloat] = [:]
    var resultById: [String: GraphNode] = [:]

    for col in byColumn.keys.sorted() {
        let ordered = byColumn[col]!.sorted { a, b in
            let ay = meanPredecessorY(a.id, predecessors, finalY)
            let by = meanPredecessorY(b.id, predecessors, finalY)
            if ay != by { return ay < by }
            return a.id < b.id
        }
        for (row, n) in ordered.enumerated() {
            var placed = n
            if n.position == .zero {
                placed.position = CGPoint(
                    x: ORIGIN_X + CGFloat(col) * (NODE_W + GAP_X),
                    y: ORIGIN_Y + CGFloat(row) * (NODE_H + GAP_Y)
                )
            }
            finalY[n.id] = placed.position.y
            resultById[n.id] = placed
        }
    }

    return nodes.map { resultById[$0.id] ?? $0 }
}

/// The mean y of `id`'s direct predecessors' FINAL positions (already
/// assigned — predecessors always sit in a strictly earlier column, which is
/// processed first). `0` when `id` has no predecessors.
private func meanPredecessorY(_ id: String, _ predecessors: [String: [String]], _ finalY: [String: CGFloat]) -> CGFloat {
    let ys = (predecessors[id] ?? []).compactMap { finalY[$0] }
    guard !ys.isEmpty else { return 0 }
    return ys.reduce(0, +) / CGFloat(ys.count)
}
