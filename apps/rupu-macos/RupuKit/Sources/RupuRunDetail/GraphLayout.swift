import RupuAPI
import RupuStore

/// One node's render-ready view model for `StepGraphView`.
public struct GraphNodeVM: Identifiable, Equatable, Sendable {
    public let id: String
    public let kindLabel: String
    public let agentLabel: String?
    public let state: NodeState
    public let laneCount: Int
    public let unitProgress: (done: Int, total: Int)?

    public init(
        id: String,
        kindLabel: String,
        agentLabel: String?,
        state: NodeState,
        laneCount: Int,
        unitProgress: (done: Int, total: Int)?
    ) {
        self.id = id
        self.kindLabel = kindLabel
        self.agentLabel = agentLabel
        self.state = state
        self.laneCount = laneCount
        self.unitProgress = unitProgress
    }

    // Manual `==` because `(done: Int, total: Int)?` — a tuple — has no
    // synthesized Equatable conformance.
    public static func == (lhs: GraphNodeVM, rhs: GraphNodeVM) -> Bool {
        lhs.id == rhs.id
            && lhs.kindLabel == rhs.kindLabel
            && lhs.agentLabel == rhs.agentLabel
            && lhs.state == rhs.state
            && lhs.laneCount == rhs.laneCount
            && lhs.unitProgress?.done == rhs.unitProgress?.done
            && lhs.unitProgress?.total == rhs.unitProgress?.total
            && (lhs.unitProgress == nil) == (rhs.unitProgress == nil)
    }
}

/// Walks the static workflow DAG (`nodes`) and produces one `GraphNodeVM`
/// per node, in DAG order. Pure: no I/O, no clock reads — every bit of
/// "is this thing happening right now" comes in through `liveStates`,
/// supplied by the caller from live run/event state.
///
/// Per node, in priority order:
/// 1. `liveStates[node.id]` wins outright (covers `.running` and
///    `.gatePending`, which this function never infers on its own).
/// 2. Else a matching `APIStepResult` (by `stepID`) settles it:
///    `skipped` → `.skipped`, otherwise → `.done(success:)`.
/// 3. Else `.pending` — including the very first node with no results and
///    no live state at all. We never guess `.running` from position alone.
///
/// `laneCount` is the branch/panelist fan-out width (`parallel.count` or
/// `panelists.count`), defaulting to 1 for single-lane node kinds.
///
/// `unitProgress` is derived from `units` filtered to this node's
/// `stepID`: `nil` when there are no matching units (most node kinds),
/// otherwise `(done, total)` where `done` counts units whose `success` is
/// non-nil (a unit synthesized from an in-flight event carries
/// `success: nil` until it finishes).
public func layoutGraph(
    nodes: [APIStepNode],
    results: [APIStepResult],
    units: [APIUnitRow],
    liveStates: [String: NodeState]
) -> [GraphNodeVM] {
    var resultsByStepID: [String: APIStepResult] = [:]
    for result in results {
        resultsByStepID[result.stepID] = result
    }

    var unitsByStepID: [String: [APIUnitRow]] = [:]
    for unit in units {
        unitsByStepID[unit.stepID, default: []].append(unit)
    }

    return nodes.map { node in
        let state: NodeState
        if let live = liveStates[node.id] {
            state = live
        } else if let result = resultsByStepID[node.id] {
            state = result.skipped ? .skipped : .done(success: result.success)
        } else {
            state = .pending
        }

        let laneCount = node.parallel?.count ?? node.panelists?.count ?? 1

        let unitProgress: (done: Int, total: Int)?
        if let nodeUnits = unitsByStepID[node.id], !nodeUnits.isEmpty {
            let done = nodeUnits.filter { $0.success != nil }.count
            unitProgress = (done: done, total: nodeUnits.count)
        } else {
            unitProgress = nil
        }

        return GraphNodeVM(
            id: node.id,
            kindLabel: kindLabel(for: node.kind),
            agentLabel: node.agent,
            state: state,
            laneCount: max(laneCount, 1),
            unitProgress: unitProgress
        )
    }
}

/// Human-readable label for a `StepNodeDto.kind` string.
private func kindLabel(for kind: String) -> String {
    switch kind {
    case "step": "Step"
    case "for_each": "For Each"
    case "parallel": "Parallel"
    case "panel": "Panel"
    case "gate": "Gate"
    case "action": "Action"
    case "run": "Run"
    default: kind.capitalized
    }
}
