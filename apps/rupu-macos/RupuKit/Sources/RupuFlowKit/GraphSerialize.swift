// Topological ordering + graph -> YAMLValue serialization. Line-for-line
// port of `crates/rupu-cp/web/src/lib/workflowGraph.ts:958-1010, 1013-1199,
// 481-497` (topoSort / nodeToStepObject / graphToWorkflowObject /
// loopsToObject).

// ── topoSort ──────────────────────────────────────────────────────────────────

/// Result of `topoSort`: either a valid topological ordering, or (if the
/// graph isn't a DAG) the ids of the nodes still left unordered.
public enum TopoResult: Equatable {
    case order([GraphNode])
    case cycle([String])
}

/// Kahn's algorithm with a deterministic tiebreak. Among in-degree-0 "ready"
/// nodes we always pick the one with the smallest `position.y`, then
/// `position.x`, then `id` (lexicographic) — so the output is stable and
/// layout-aware.
public func topoSort(nodes: [GraphNode], edges: [GraphEdge]) -> TopoResult {
    var byId: [String: GraphNode] = [:]
    var indeg: [String: Int] = [:]
    for n in nodes {
        byId[n.id] = n
        indeg[n.id] = 0
    }
    var adj: [String: [String]] = [:]
    for e in edges {
        guard byId[e.source] != nil, byId[e.target] != nil else { continue }
        adj[e.source, default: []].append(e.target)
        indeg[e.target, default: 0] += 1
    }

    func cmp(_ a: GraphNode, _ b: GraphNode) -> Bool {
        if a.position.y != b.position.y { return a.position.y < b.position.y }
        if a.position.x != b.position.x { return a.position.x < b.position.x }
        return a.id < b.id
    }

    var order: [GraphNode] = []
    var ready: [GraphNode] = nodes.filter { (indeg[$0.id] ?? 0) == 0 }
    while !ready.isEmpty {
        ready.sort(by: cmp)
        let n = ready.removeFirst()
        order.append(n)
        for t in adj[n.id] ?? [] {
            indeg[t, default: 0] -= 1
            if indeg[t] == 0, let tn = byId[t] {
                ready.append(tn)
            }
        }
    }

    if order.count != byId.count {
        let done = Set(order.map(\.id))
        return .cycle(nodes.filter { !done.contains($0.id) }.map(\.id))
    }
    return .order(order)
}

// ── nodeToStepObject ──────────────────────────────────────────────────────────

private func joinWaitValue(_ w: JoinWait) -> YAMLValue {
    switch w {
    case .all: return .string("all")
    case .any: return .string("any")
    case .count(let n): return .mapping([("count", .int(n))])
    }
}

/// `when` / `continue_on_error` / `actions` — the fields shared by every
/// kind EXCEPT `parallel` (a genuine TS-source asymmetry, ported verbatim:
/// the `parallel` arm never emits these).
private func appendSharedTail(_ o: inout [(key: String, value: YAMLValue)], _ d: StepNodeData) {
    if let when = d.when { o.append(("when", .string(when))) }
    if d.continueOnError == true { o.append(("continue_on_error", .bool(true))) }
    if let actions = d.actions, !actions.isEmpty {
        o.append(("actions", .sequence(actions.map { .string($0) })))
    }
}

private func appendRest(_ o: inout [(key: String, value: YAMLValue)], _ rest: [(key: String, value: YAMLValue)]?) {
    guard let rest else { return }
    for (k, v) in rest where !o.contains(where: { $0.key == k }) {
        o.append((k, v))
    }
}

/// Serialize one node back to a YAML-step object, including ONLY set fields
/// (nil / empty arrays / empty strings are omitted, except where noted).
/// Per-kind emit arms exactly mirror the TS source, plus the `run` kind
/// extension (macOS decision 2): the `run:` block is emitted verbatim, along
/// with `for_each`/`max_parallel` (a `run:` step composes with `for_each`
/// the same way a plain step does) and the shared when/continue_on_error/
/// actions tail.
func nodeToStepObject(_ d: StepNodeData) -> YAMLValue {
    var o: [(key: String, value: YAMLValue)] = [("id", .string(d.id))]

    switch d.kind {
    case .parallel:
        let subs = (d.parallel ?? []).map { s -> YAMLValue in
            var so: [(key: String, value: YAMLValue)] = [("id", .string(s.id))]
            if !s.agent.isEmpty { so.append(("agent", .string(s.agent))) }
            if !s.prompt.isEmpty { so.append(("prompt", .string(s.prompt))) }
            return .mapping(so)
        }
        o.append(("parallel", .sequence(subs)))
        if let maxParallel = d.maxParallel { o.append(("max_parallel", .int(maxParallel))) }
        // NOTE: no when/continue_on_error/actions here — the TS source's
        // `parallel` arm genuinely never emits them; ported verbatim.

    case .panel:
        let p = d.panel ?? PanelCfg(panelists: [], subject: "")
        var po: [(key: String, value: YAMLValue)] = [
            ("panelists", .sequence(p.panelists.map { .string($0) })),
            ("subject", .string(p.subject)),
        ]
        if let prompt = p.prompt, !prompt.isEmpty { po.append(("prompt", .string(prompt))) }
        if let maxParallel = p.maxParallel { po.append(("max_parallel", .int(maxParallel))) }
        if let gate = p.gate {
            var go: [(key: String, value: YAMLValue)] = []
            if let v = gate.untilNoFindingsAtSeverityOrAbove {
                go.append(("until_no_findings_at_severity_or_above", .string(v)))
            }
            if let v = gate.fixWith { go.append(("fix_with", .string(v))) }
            if let v = gate.maxIterations { go.append(("max_iterations", .int(v))) }
            po.append(("gate", .mapping(go)))
        }
        appendRest(&po, p.rest)
        o.append(("panel", .mapping(po)))
        appendSharedTail(&o, d)

    case .branch:
        var bo: [(key: String, value: YAMLValue)] = []
        if let condition = d.condition { bo.append(("condition", .string(condition))) }
        if let then = d.thenTargets, !then.isEmpty { bo.append(("then", .sequence(then.map { .string($0) }))) }
        if let els = d.elseTargets, !els.isEmpty { bo.append(("else", .sequence(els.map { .string($0) }))) }
        appendRest(&bo, d.branchRest)
        o.append(("branch", .mapping(bo)))

    case .action:
        if let action = d.action { o.append(("action", .string(action))) }
        if let with = d.with { o.append(("with", with)) }
        appendSharedTail(&o, d)

    case .run:
        if let runBlock = d.runBlock { o.append(("run", runBlock)) }
        if let forEach = d.forEach { o.append(("for_each", .string(forEach))) }
        if let maxParallel = d.maxParallel { o.append(("max_parallel", .int(maxParallel))) }
        appendSharedTail(&o, d)

    case .split:
        // `split` orchestration node — its whole identity is the `split:`
        // array (fan-out targets); ALWAYS emitted, even empty.
        o.append(("split", .sequence((d.split ?? []).map { .string($0) })))
        appendSharedTail(&o, d)

    case .join:
        // `join` (barrier) orchestration node — `wait` is omitted when not
        // set on load, so a bare `join: {}` round-trips as-is.
        var jo: [(key: String, value: YAMLValue)] = []
        if let wait = d.joinWait { jo.append(("wait", joinWaitValue(wait))) }
        o.append(("join", .mapping(jo)))
        appendSharedTail(&o, d)

    case .approvalGate:
        // standalone gate NODE — its whole identity is the `approval:` block
        // assembled below; it carries no agent/prompt.
        appendSharedTail(&o, d)

    case .step, .forEach:
        if let agent = d.agent { o.append(("agent", .string(agent))) }
        if let prompt = d.prompt { o.append(("prompt", .string(prompt))) }
        appendSharedTail(&o, d)
        if let forEach = d.forEach { o.append(("for_each", .string(forEach))) }
        if let maxParallel = d.maxParallel { o.append(("max_parallel", .int(maxParallel))) }
        if let with = d.with { o.append(("with", with)) }
    }

    // `next` (explicit successor edges) applies to any step kind — omitted
    // when empty so a legacy node (no `next:` on load) round-trips clean.
    if let next = d.next, !next.isEmpty { o.append(("next", .sequence(next.map { .string($0) }))) }

    // `depends_on` (explicit predecessor edges) — same omit-when-empty rule.
    if let dependsOn = d.dependsOn, !dependsOn.isEmpty {
        o.append(("depends_on", .sequence(dependsOn.map { .string($0) })))
    }

    // Approval applies to any step kind. A gate NODE ALWAYS emits an
    // `approval:` block (it is the node's identity); other kinds emit only
    // when there's an inline approval to say.
    let hasGateExtras =
        d.approvalAutoApprove != nil || d.approvalOnTimeout != nil || (d.approvalNotify?.isEmpty == false)
        || (d.approvalOnReject?.isEmpty == false)
    let hasApprovalRest = d.approvalRest?.isEmpty == false
    if d.kind == .approvalGate || d.approvalRequired == true || d.approvalPrompt != nil
        || d.approvalTimeoutSeconds != nil || hasGateExtras || hasApprovalRest
    {
        var ap: [(key: String, value: YAMLValue)] = []
        if d.approvalRequired == true { ap.append(("required", .bool(true))) }
        if let prompt = d.approvalPrompt { ap.append(("prompt", .string(prompt))) }
        if let timeout = d.approvalTimeoutSeconds { ap.append(("timeout_seconds", .int(timeout))) }
        if let auto = d.approvalAutoApprove { ap.append(("auto_approve", .string(auto))) }
        if let onTimeout = d.approvalOnTimeout { ap.append(("on_timeout", .string(onTimeout))) }
        if let notify = d.approvalNotify, !notify.isEmpty { ap.append(("notify", .sequence(notify))) }
        if let onReject = d.approvalOnReject, !onReject.isEmpty { ap.append(("on_reject", .sequence(onReject))) }
        appendRest(&ap, d.approvalRest)
        o.append(("approval", .mapping(ap)))
    }

    // Spread unmodeled keys (e.g. `contract:`) back, never clobbering modeled
    // ones.
    appendRest(&o, d.rawPassthrough)

    return .mapping(o)
}

// ── loopsToObject ─────────────────────────────────────────────────────────────

/// Serialize `[WorkflowLoop]` back to the `loops:` map shape (name-sorted,
/// mirroring `BTreeMap`'s serialized order), field order `nodes` / `until` /
/// `max_iterations` / `on_max` matching workflow.rs `LoopDef`'s declaration
/// order. `on_max` is ALWAYS emitted. `max_iterations` is emitted only when
/// non-nil — `WorkflowLoop.maxIterations` is `Int?` in this port (nil stands
/// in for the TS side's `NaN` "missing" sentinel; see its doc comment in
/// `GraphModel.swift`).
private func loopsToObject(_ loops: [WorkflowLoop]) -> YAMLValue {
    var out: [(key: String, value: YAMLValue)] = []
    for l in loops.sorted(by: { $0.name < $1.name }) {
        var lo: [(key: String, value: YAMLValue)] = [
            ("nodes", .sequence(l.nodes.map { .string($0) })),
            ("until", .string(l.until)),
        ]
        if let maxIterations = l.maxIterations { lo.append(("max_iterations", .int(maxIterations))) }
        lo.append(("on_max", .string(l.onMax)))
        out.append((l.name, .mapping(lo)))
    }
    return .mapping(out)
}

// ── graphToWorkflowObject ─────────────────────────────────────────────────────

/// Result of `graphToWorkflowObject`: either the serialized workflow object,
/// or (if the graph isn't a DAG) a human-readable failure message.
public enum SerializeResult {
    case object(YAMLValue)
    case failure(String)
}

/// Serialize the graph back to a workflow object. Steps are emitted in topo
/// order (so the YAML reads top-to-bottom in execution order). Key order of
/// the result is the round-trip contract: `name` first, then `description`
/// (if set), then all `meta.rest` keys verbatim, then `loops` (only when
/// non-empty), then `steps` last.
public func graphToWorkflowObject(_ g: WorkflowGraph) -> SerializeResult {
    // `outerCycleEdges` drops a loop's pure feedback data-ref from cycle
    // detection — without it, a legitimate refine-style loop (`gen` reading
    // a prior iteration's `steps.critique.output`, which `deriveEdges` sees
    // as a real `critique->gen` edge) would falsely read as cyclic and BLOCK
    // SAVING. A genuine internal cycle among loop members is still caught by
    // Task 6's loop validation.
    let sorted = topoSort(nodes: g.nodes, edges: outerCycleEdges(g.nodes, deriveEdges(g.nodes), g.loops))
    switch sorted {
    case .cycle(let ids):
        return .failure("Cannot serialize: cycle through " + ids.joined(separator: ", "))
    case .order(let order):
        let steps = order.map { nodeToStepObject($0.data) }

        var obj: [(key: String, value: YAMLValue)] = [("name", .string(g.meta.name))]
        if let description = g.meta.description { obj.append(("description", .string(description))) }
        for (k, v) in g.meta.rest { obj.append((k, v)) }
        if !g.loops.isEmpty { obj.append(("loops", loopsToObject(g.loops))) }
        obj.append(("steps", .sequence(steps)))
        return .object(.mapping(obj))
    }
}
