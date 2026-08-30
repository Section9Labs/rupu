import CoreGraphics

// Graph model — the pure data shape of the visual workflow editor's graph
// (nodes/edges/meta/loops). Line-for-line port of the TypeScript types at
// `crates/rupu-cp/web/src/lib/workflowGraph.ts:16-155`, with one deliberate
// extension (decision 2 of the macOS Workflow Builder plan): `run` becomes a
// first-class `StepKind` — the web editor never detects a `run:` block as its
// own kind, but on macOS a `run:` mapping (workflow.rs connector-run steps)
// is stored verbatim in `StepNodeData.runBlock` and round-trips as its own
// node kind rather than silently falling into `raw_passthrough`.

/// The kind of node a step maps to on the canvas. Precedence for classifying
/// a raw step (most-specific first) lives in `GraphParse.swift`'s
/// `parseStepData`.
public enum StepKind: String, CaseIterable, Sendable, Equatable {
    case step
    case forEach = "for_each"
    case parallel
    case panel
    case branch
    case approvalGate = "approval_gate"
    case action
    case run
    case split
    case join
}

public struct SubStep: Equatable, Sendable {
    public var id: String
    public var agent: String
    public var prompt: String

    public init(id: String, agent: String, prompt: String) {
        self.id = id
        self.agent = agent
        self.prompt = prompt
    }
}

/// Mirrors the real `PanelGate` schema in `crates/rupu-orchestrator/src/
/// workflow.rs` (serde `deny_unknown_fields`, no aliases) — field names here
/// must match those wire names exactly.
public struct PanelGate: Equatable, Sendable {
    public var untilNoFindingsAtSeverityOrAbove: String?
    public var fixWith: String?
    public var maxIterations: Int?

    public init(
        untilNoFindingsAtSeverityOrAbove: String? = nil,
        fixWith: String? = nil,
        maxIterations: Int? = nil
    ) {
        self.untilNoFindingsAtSeverityOrAbove = untilNoFindingsAtSeverityOrAbove
        self.fixWith = fixWith
        self.maxIterations = maxIterations
    }
}

public struct PanelCfg: Sendable {
    public var panelists: [String]
    public var subject: String
    public var prompt: String?
    public var maxParallel: Int?
    public var gate: PanelGate?
    /// Any panel-level keys not modelled above, captured verbatim on load and
    /// re-emitted on save (source order) — mirrors `StepNodeData.rawPassthrough`
    /// one level deeper so an unmodelled key nested directly under `panel:`
    /// isn't dropped.
    public var rest: [(key: String, value: YAMLValue)]

    public init(
        panelists: [String],
        subject: String,
        prompt: String? = nil,
        maxParallel: Int? = nil,
        gate: PanelGate? = nil,
        rest: [(key: String, value: YAMLValue)] = []
    ) {
        self.panelists = panelists
        self.subject = subject
        self.prompt = prompt
        self.maxParallel = maxParallel
        self.gate = gate
        self.rest = rest
    }
}

extension PanelCfg: Equatable {
    // Tuple-array `rest` doesn't get synthesized `Equatable` conformance.
    public static func == (lhs: PanelCfg, rhs: PanelCfg) -> Bool {
        lhs.panelists == rhs.panelists && lhs.subject == rhs.subject && lhs.prompt == rhs.prompt
            && lhs.maxParallel == rhs.maxParallel && lhs.gate == rhs.gate
            && lhs.rest.count == rhs.rest.count
            && zip(lhs.rest, rhs.rest).allSatisfy { $0.key == $1.key && $0.value == $1.value }
    }
}

/// Mirrors workflow.rs's `JoinWait` (`#[serde(untagged)]`): the bare keyword
/// `"all"`/`"any"`, or the `{ count }` map form.
public enum JoinWait: Equatable, Sendable {
    case all
    case any
    case count(Int)
}

public struct StepNodeData: Sendable {
    public var id: String
    public var kind: StepKind
    public var agent: String?
    public var prompt: String?
    public var when: String?
    public var continueOnError: Bool?
    public var actions: [String]?
    public var forEach: String?
    public var maxParallel: Int?
    public var parallel: [SubStep]?
    public var panel: PanelCfg?
    public var condition: String?
    public var thenTargets: [String]?
    public var elseTargets: [String]?
    public var approvalRequired: Bool?
    public var approvalPrompt: String?
    public var approvalTimeoutSeconds: Int?
    public var approvalAutoApprove: String?
    /// "approve" | "reject" | "fail"
    public var approvalOnTimeout: String?
    public var approvalNotify: [YAMLValue]?
    public var approvalOnReject: [YAMLValue]?
    /// Any keys nested directly under `branch:`/`approval:` not modelled
    /// above, captured verbatim on load and re-emitted on save (source
    /// order) — mirrors the step-level `rawPassthrough` pattern one level
    /// deeper.
    public var branchRest: [(key: String, value: YAMLValue)]?
    public var approvalRest: [(key: String, value: YAMLValue)]?
    public var action: String?
    public var with: YAMLValue?
    /// The whole `run:` mapping, verbatim (macOS decision 2 — the web editor
    /// never detects `run` as its own kind).
    public var runBlock: YAMLValue?
    public var next: [String]?
    public var dependsOn: [String]?
    public var split: [String]?
    public var joinWait: JoinWait?
    /// Whether `join:` is present at all, even a bare `{}`.
    public var hasJoin: Bool
    /// Any step-level keys not modelled above (e.g. `contract:`), captured
    /// verbatim on load and re-emitted on save (source order).
    public var rawPassthrough: [(key: String, value: YAMLValue)]?

    public init(id: String, kind: StepKind) {
        self.id = id
        self.kind = kind
        self.agent = nil
        self.prompt = nil
        self.when = nil
        self.continueOnError = nil
        self.actions = nil
        self.forEach = nil
        self.maxParallel = nil
        self.parallel = nil
        self.panel = nil
        self.condition = nil
        self.thenTargets = nil
        self.elseTargets = nil
        self.approvalRequired = nil
        self.approvalPrompt = nil
        self.approvalTimeoutSeconds = nil
        self.approvalAutoApprove = nil
        self.approvalOnTimeout = nil
        self.approvalNotify = nil
        self.approvalOnReject = nil
        self.branchRest = nil
        self.approvalRest = nil
        self.action = nil
        self.with = nil
        self.runBlock = nil
        self.next = nil
        self.dependsOn = nil
        self.split = nil
        self.joinWait = nil
        self.hasJoin = false
        self.rawPassthrough = nil
    }
}

extension StepNodeData: Equatable {
    // Tuple-array fields (`branchRest`/`approvalRest`/`rawPassthrough`) don't
    // get synthesized `Equatable` conformance.
    public static func == (lhs: StepNodeData, rhs: StepNodeData) -> Bool {
        guard
            lhs.id == rhs.id, lhs.kind == rhs.kind, lhs.agent == rhs.agent, lhs.prompt == rhs.prompt,
            lhs.when == rhs.when, lhs.continueOnError == rhs.continueOnError, lhs.actions == rhs.actions,
            lhs.forEach == rhs.forEach, lhs.maxParallel == rhs.maxParallel, lhs.parallel == rhs.parallel,
            lhs.panel == rhs.panel, lhs.condition == rhs.condition, lhs.thenTargets == rhs.thenTargets,
            lhs.elseTargets == rhs.elseTargets, lhs.approvalRequired == rhs.approvalRequired,
            lhs.approvalPrompt == rhs.approvalPrompt, lhs.approvalTimeoutSeconds == rhs.approvalTimeoutSeconds,
            lhs.approvalAutoApprove == rhs.approvalAutoApprove, lhs.approvalOnTimeout == rhs.approvalOnTimeout,
            lhs.approvalNotify == rhs.approvalNotify, lhs.approvalOnReject == rhs.approvalOnReject,
            lhs.action == rhs.action, lhs.with == rhs.with, lhs.runBlock == rhs.runBlock, lhs.next == rhs.next,
            lhs.dependsOn == rhs.dependsOn, lhs.split == rhs.split, lhs.joinWait == rhs.joinWait,
            lhs.hasJoin == rhs.hasJoin
        else { return false }
        guard tupleArrayEqual(lhs.branchRest, rhs.branchRest) else { return false }
        guard tupleArrayEqual(lhs.approvalRest, rhs.approvalRest) else { return false }
        guard tupleArrayEqual(lhs.rawPassthrough, rhs.rawPassthrough) else { return false }
        return true
    }
}

/// Shared comparator for optional `[(key: String, value: YAMLValue)]` fields
/// (order-sensitive, since these arrays preserve source key order).
func tupleArrayEqual(_ lhs: [(key: String, value: YAMLValue)]?, _ rhs: [(key: String, value: YAMLValue)]?) -> Bool {
    switch (lhs, rhs) {
    case (nil, nil):
        return true
    case (let l?, let r?):
        return l.count == r.count && zip(l, r).allSatisfy { $0.key == $1.key && $0.value == $1.value }
    default:
        return false
    }
}

public struct GraphNode: Equatable, Sendable, Identifiable {
    public var id: String
    public var data: StepNodeData
    public var position: CGPoint

    public init(id: String, data: StepNodeData, position: CGPoint) {
        self.id = id
        self.data = data
        self.position = position
    }
}

public struct GraphEdge: Equatable, Sendable, Identifiable {
    public var id: String
    public var source: String
    public var target: String
    public var label: String?
    /// "then" | "else"
    public var branchArm: String?

    public init(id: String, source: String, target: String, label: String? = nil, branchArm: String? = nil) {
        self.id = id
        self.source = source
        self.target = target
        self.label = label
        self.branchArm = branchArm
    }
}

public struct WorkflowMeta: Sendable {
    public var name: String
    public var description: String?
    /// Every top-level workflow key except `name`/`description`/`steps`/
    /// `loops`, captured verbatim in source order so a round-trip leaves
    /// `trigger`/`inputs`/`defaults`/`autoflow`/etc untouched.
    public var rest: [(key: String, value: YAMLValue)]

    public init(name: String, description: String? = nil, rest: [(key: String, value: YAMLValue)] = []) {
        self.name = name
        self.description = description
        self.rest = rest
    }
}

extension WorkflowMeta: Equatable {
    // Tuple-array `rest` doesn't get synthesized `Equatable` conformance.
    public static func == (lhs: WorkflowMeta, rhs: WorkflowMeta) -> Bool {
        lhs.name == rhs.name && lhs.description == rhs.description && tupleArrayEqual(lhs.rest, rhs.rest)
    }
}

/// A named bounded subgraph loop (workflow.rs `LoopDef`). Field names are the
/// camelCase mirror of the YAML `loops.<name>` map entry (`nodes`/`until`/
/// `max_iterations`/`on_max`) — `name` is the map key, carried here as a
/// field.
///
/// `maxIterations` is `Int?` rather than the TS port's `number` (which
/// represents "missing" as `NaN`): Swift's `Int` has no NaN sentinel, so
/// `nil` stands in for "absent/non-numeric `max_iterations`" instead.
/// Downstream validation (Task 6) treats `nil` the same as the TS side's
/// `!Number.isFinite(...)` check on `NaN` — both flag the loop as needing a
/// `max_iterations` set.
public struct WorkflowLoop: Equatable, Sendable {
    public var name: String
    public var nodes: [String]
    public var until: String
    public var maxIterations: Int?
    /// "fail" | "proceed"
    public var onMax: String

    public init(name: String, nodes: [String], until: String, maxIterations: Int?, onMax: String) {
        self.name = name
        self.nodes = nodes
        self.until = until
        self.maxIterations = maxIterations
        self.onMax = onMax
    }
}

public struct WorkflowGraph: Equatable, Sendable {
    public var nodes: [GraphNode]
    public var edges: [GraphEdge]
    public var meta: WorkflowMeta
    public var loops: [WorkflowLoop]

    public init(nodes: [GraphNode], edges: [GraphEdge], meta: WorkflowMeta, loops: [WorkflowLoop] = []) {
        self.nodes = nodes
        self.edges = edges
        self.meta = meta
        self.loops = loops
    }
}
