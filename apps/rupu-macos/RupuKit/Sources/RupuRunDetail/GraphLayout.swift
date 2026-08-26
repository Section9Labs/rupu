import RupuAPI
import RupuStore

/// One branch/panelist lane's view model — a leaf under a `parallel`/`panel`
/// node's `subSteps`.
public struct SubStepVM: Identifiable, Equatable, Sendable {
    public let id: String
    public let agent: String
    public let state: NodeState

    public init(id: String, agent: String, state: NodeState) {
        self.id = id
        self.agent = agent
        self.state = state
    }
}

/// One fan-out unit's view model — a leaf under a `for_each` node's
/// `fanout.units`. `id` is the unit's static REST `index` (stable identity
/// across a REST snapshot + live overlay merge, per the sortable-tables-by-
/// identity convention elsewhere in this app); `key` is the *display* label,
/// live `unit_key` when known, else `"#\(index)"`.
public struct UnitVM: Identifiable, Equatable, Sendable {
    public let id: Int
    public let key: String
    public let state: NodeState
    public let transcriptPath: String?

    public init(id: Int, key: String, state: NodeState, transcriptPath: String?) {
        self.id = id
        self.key = key
        self.state = state
        self.transcriptPath = transcriptPath
    }
}

/// A `for_each` node's fan-out summary. Counts are derived from `units`
/// every time this is built — never stored/cached independently — so they
/// can never drift out of sync with the unit list they summarize.
public struct FanoutVM: Equatable, Sendable {
    public let units: [UnitVM]
    public let done: Int
    public let failed: Int
    public let running: Int
    public let total: Int

    public init(units: [UnitVM], done: Int, failed: Int, running: Int, total: Int) {
        self.units = units
        self.done = done
        self.failed = failed
        self.running = running
        self.total = total
    }
}

/// One node's render-ready view model for `StepGraphView`.
public struct GraphNodeVM: Identifiable, Equatable, Sendable {
    public let id: String
    public let kind: StepKind
    public let agentLabel: String?
    public let state: NodeState
    public let actionName: String?
    public let gateAuto: Bool
    public let gateHasOnReject: Bool
    public let subSteps: [SubStepVM]
    public let fanout: FanoutVM?
    public let panelRound: PanelRoundState?
    public let panelGate: APIPanelGate?
    public let transcriptPath: String?

    public init(
        id: String,
        kind: StepKind,
        agentLabel: String?,
        state: NodeState,
        actionName: String?,
        gateAuto: Bool,
        gateHasOnReject: Bool,
        subSteps: [SubStepVM],
        fanout: FanoutVM?,
        panelRound: PanelRoundState?,
        panelGate: APIPanelGate?,
        transcriptPath: String?
    ) {
        self.id = id
        self.kind = kind
        self.agentLabel = agentLabel
        self.state = state
        self.actionName = actionName
        self.gateAuto = gateAuto
        self.gateHasOnReject = gateHasOnReject
        self.subSteps = subSteps
        self.fanout = fanout
        self.panelRound = panelRound
        self.panelGate = panelGate
        self.transcriptPath = transcriptPath
    }
}

/// Walks the static workflow DAG (`nodes`) and produces one `GraphNodeVM`
/// per node, in DAG order. Pure: no I/O, no clock reads — every bit of
/// "is this thing happening right now" comes in through the four live-
/// overlay parameters, supplied by the caller from live run/event state.
///
/// Per node, in priority order:
/// 1. `liveStates[node.id]` wins outright (covers `.running`, `.gatePending`,
///    and `.paused`, which this function never infers on its own).
/// 2. Else a matching `APIStepResult` (by `stepID`) settles it:
///    `skipped` → `.skipped`, otherwise → `.done(success:)`.
/// 3. Else `.pending` — including the very first node with no results and
///    no live state at all. We never guess `.running` from position alone.
///
/// ONE exception layered on top of that precedence (mirrors the web's
/// `runGraphModel.ts` phase 5, L296-301): a `for_each` node that is still
/// `.pending` after steps 1-3 above gets promoted to `.running` when any of
/// its fan-out units is in flight (REST `success == nil`, or a live overlay
/// with `success == nil`). This only ever promotes `.pending` — an existing
/// live state (including a live `.pending`, if one ever existed) is never
/// overridden by this rule.
///
/// `fanout` is built from `units` filtered to this node's `stepID`, keyed by
/// `index` and overlaid by `liveUnits[stepID]` per index: a live entry wins
/// its `transcriptPath`/`success` outright, and its `key` only when
/// non-nil (REST rows carry no `unit_key`, so most of the time the display
/// key falls back to positional `"#\(index)"`). A `liveUnits` index with no
/// matching REST row still produces a unit (covers a live `unit_started`
/// that outran the next REST poll). Nodes with zero units end up with
/// `fanout == nil` (`for_each`, or a `for_each` finished with no rows this
/// fixture never has — treated the same way).
///
/// `subSteps` comes from `node.parallel` (id + agent literally as given) or
/// `node.panelists` (each name doubling as both id and agent) — whichever
/// is populated; `[]` for every other node kind. Each leaf's state follows
/// the same precedence as the parent: `liveStates[subID]` > a result keyed
/// by that sub-id > `.pending`.
public func layoutGraph(
    nodes: [APIStepNode],
    results: [APIStepResult],
    units: [APIUnitRow],
    liveStates: [String: NodeState],
    liveUnits: [String: [Int: UnitLiveState]],
    panelRounds: [String: PanelRoundState],
    stepTranscripts: [String: String]
) -> [GraphNodeVM] {
    var resultsByStepID: [String: APIStepResult] = [:]
    for result in results {
        resultsByStepID[result.stepID] = result
    }

    var unitsByStepID: [String: [Int: APIUnitRow]] = [:]
    for unit in units {
        unitsByStepID[unit.stepID, default: [:]][unit.index] = unit
    }

    func resolvedState(for id: String) -> NodeState {
        if let live = liveStates[id] {
            return live
        }
        if let result = resultsByStepID[id] {
            return result.skipped ? .skipped : .done(success: result.success)
        }
        return .pending
    }

    func fanout(for stepID: String) -> FanoutVM? {
        let restUnits = unitsByStepID[stepID] ?? [:]
        let liveOverlay = liveUnits[stepID] ?? [:]

        var indices = Set(restUnits.keys)
        indices.formUnion(liveOverlay.keys)
        guard !indices.isEmpty else { return nil }

        let unitVMs: [UnitVM] = indices.sorted().map { index in
            let restRow = restUnits[index]
            let live = liveOverlay[index]

            let key = live?.key ?? "#\(index)"
            let transcriptPath = live?.transcriptPath ?? restRow?.transcriptPath
            let success = live?.success ?? restRow?.success
            let state: NodeState = success == nil ? .running : .done(success: success!)

            return UnitVM(id: index, key: key, state: state, transcriptPath: transcriptPath)
        }

        let done = unitVMs.filter { if case .done(true) = $0.state { true } else { false } }.count
        let failed = unitVMs.filter { if case .done(false) = $0.state { true } else { false } }.count
        let running = unitVMs.filter { $0.state == .running }.count

        return FanoutVM(units: unitVMs, done: done, failed: failed, running: running, total: unitVMs.count)
    }

    func subSteps(for node: APIStepNode) -> [SubStepVM] {
        if let parallel = node.parallel {
            return parallel.map { sub in
                SubStepVM(id: sub.id, agent: sub.agent, state: resolvedState(for: sub.id))
            }
        }
        if let panelists = node.panelists {
            return panelists.map { name in
                SubStepVM(id: name, agent: name, state: resolvedState(for: name))
            }
        }
        return []
    }

    return nodes.map { node in
        var state = resolvedState(for: node.id)
        let nodeFanout = fanout(for: node.id)

        if node.kind == "for_each", state == .pending, let nodeFanout,
           nodeFanout.units.contains(where: { $0.state == .running }) {
            state = .running
        }

        return GraphNodeVM(
            id: node.id,
            kind: StepKind(raw: node.kind),
            agentLabel: node.agent,
            state: state,
            actionName: node.action,
            gateAuto: node.approvalGate?.autoApprove ?? false,
            gateHasOnReject: node.approvalGate?.hasOnReject ?? false,
            subSteps: subSteps(for: node),
            fanout: nodeFanout,
            panelRound: panelRounds[node.id],
            panelGate: node.gate,
            transcriptPath: stepTranscripts[node.id]
        )
    }
}
