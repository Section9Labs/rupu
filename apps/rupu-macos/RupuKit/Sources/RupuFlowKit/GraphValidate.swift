// Graph validation — line-for-line port of `crates/rupu-cp/web/src/lib/
// workflowGraph.ts:1249-1428` (`validateGraph`), minus `validateLoops`
// (loops editing is out of scope this plan — see the Task 6 brief) but
// including `outerCycleEdges` in the order/cycle computation. Message
// strings are verbatim from the TS source.
//
// Two macOS-only extensions (decision in the Task 6 brief, since `run` is a
// first-class `StepKind` here — decision 2 of the plan — where the web never
// detects it as its own kind): a `.run` node with no `runBlock["cmd"]`
// non-empty string needs "run needs a cmd"; a `.action` node with an empty/
// missing `action` needs "action needs a tool" (the web validates this in
// StepForm; here it belongs in `validateGraph` so the canvas's valid-dot is
// honest).

/// Validate every node in `g`, returning a map of step id -> ordered list of
/// human-readable problem messages. An empty result means the graph is
/// clean. Mirrors `workflowGraph.ts`'s `validateGraph` arm-by-arm.
public func validateGraph(_ g: WorkflowGraph) -> [String: [String]] {
    var out: [String: [String]] = [:]
    func add(_ id: String, _ msg: String) {
        out[id, default: []].append(msg)
    }

    var counts: [String: Int] = [:]
    for n in g.nodes { counts[n.id, default: 0] += 1 }
    let nodeIds = Set(g.nodes.map(\.id))

    for n in g.nodes {
        let d = n.data
        switch d.kind {
        case .step, .forEach:
            if d.agent?.isEmpty != false { add(n.id, "needs an agent") }
            if d.prompt?.isEmpty != false { add(n.id, "needs a prompt") }

        case .parallel:
            let subs = d.parallel ?? []
            if subs.isEmpty {
                add(n.id, "needs at least one parallel sub-step")
            } else {
                for (i, s) in subs.enumerated() {
                    let label = s.id.isEmpty ? "#\(i)" : s.id
                    if s.agent.isEmpty { add(n.id, "parallel sub-step \(label) needs an agent") }
                    if s.prompt.isEmpty { add(n.id, "parallel sub-step \(label) needs a prompt") }
                }
            }

        case .panel:
            let p = d.panel
            if p == nil || p!.panelists.isEmpty { add(n.id, "panel needs at least one panelist") }
            if p == nil || p!.subject.isEmpty { add(n.id, "panel needs a subject") }
            if let gate = p?.gate {
                if gate.untilNoFindingsAtSeverityOrAbove == nil || gate.fixWith == nil
                    || gate.maxIterations == nil
                {
                    add(n.id, "gate needs a severity, a fix agent, and max iterations")
                }
            }
            if let mp = p?.maxParallel, mp < 1 {
                add(n.id, "panel `max_parallel` must be at least 1")
            }

        case .branch:
            if d.condition?.isEmpty != false { add(n.id, "branch needs a condition") }
            for t in (d.thenTargets ?? []) + (d.elseTargets ?? []) {
                if !nodeIds.contains(t) { add(n.id, "branch target \(t) is not a known step") }
            }

        case .run:
            let cmd = d.runBlock?["cmd"]?.stringValue
            if cmd?.isEmpty != false { add(n.id, "run needs a cmd") }

        case .action:
            if d.action?.isEmpty != false { add(n.id, "action needs a tool") }

        case .approvalGate, .split, .join:
            break
        }

        if let mp = d.maxParallel, mp < 1 { add(n.id, "`max_parallel` must be at least 1") }
        if (counts[n.id] ?? 0) > 1 { add(n.id, "duplicate step id") }

        // A `notify` row with no `action` (backend `NotifyAction.action` is
        // required) 400s on save rather than failing validation up-front.
        if let notify = d.approvalNotify {
            for (i, entry) in notify.enumerated() {
                if entry["action"]?.stringValue?.isEmpty != false {
                    add(n.id, "notification \(i + 1) needs an action")
                }
            }
        }
        // An action-shaped `on_reject` row (identified by having an `action`
        // key at all, vs. an agent-shaped row) with an empty `action` is the
        // same required-field gap as above. An agent-shaped row (agent/
        // prompt) is validated elsewhere / pre-existing and is left alone
        // here.
        if let onReject = d.approvalOnReject {
            for (i, entry) in onReject.enumerated() {
                if let mapping = entry.mappingValue, mapping.contains(where: { $0.key == "action" }) {
                    if entry["action"]?.stringValue?.isEmpty != false {
                        add(n.id, "on_reject entry \(i + 1) needs an action")
                    }
                }
            }
        }
    }

    // Reference checks: dangling refs (steps.X where X is not a node) and
    // forward refs (X runs AFTER the referencing node — only checkable when
    // there's no cycle, since order is otherwise undefined). `outerCycleEdges`
    // drops a loop's pure feedback data-ref from cycle/order computation
    // (same rationale as `graphToWorkflowObject`'s use, see that call site)
    // so a refine-style loop's own topology doesn't read as globally cyclic.
    let loopsForCycle = g.loops
    let sorted = topoSort(nodes: g.nodes, edges: outerCycleEdges(g.nodes, g.edges, loopsForCycle))
    let pos: [String: Int]? = {
        if case .order(let order) = sorted {
            var m: [String: Int] = [:]
            for (i, n) in order.enumerated() { m[n.id] = i }
            return m
        }
        return nil
    }()
    for n in g.nodes {
        let here = pos?[n.id]
        let ownLoop = loopOfStep(loopsForCycle, n.id)
        for ref in extractStepRefs(n.data) {
            if !nodeIds.contains(ref) {
                add(n.id, "references unknown step \(ref)")
                continue
            }
            // A reference to a FELLOW loop member is never a "runs later"
            // bug — it's either a normal within-iteration read or the loop's
            // own controlled cross-iteration feedback (spec §2d), both
            // legitimate. Only a reference to something outside the loop (or
            // when neither side is in a loop) is checked for order.
            if let ownLoop, ownLoop.nodes.contains(ref) { continue }
            if let pos, let here {
                if let there = pos[ref], there > here {
                    add(n.id, "references steps.\(ref) which runs later")
                }
            }
        }
    }

    // Graph-mode checks (Phase 1 non-linear orchestration): only meaningful
    // once a workflow has explicit edges at all — a legacy edge-free
    // workflow can't author a cycle/dangling-target/degenerate-split-join
    // through this vocabulary, so these mirror the backend's
    // `validate_graph` gate exactly and never fire on a legacy workflow
    // (spec §2/§4 compat).
    if hasExplicitEdges(g.nodes) {
        // Cycle: mirrors the backend's `WorkflowCycle` check. `sorted`
        // (above) already ran Kahn's algorithm over `g.edges` (==
        // `deriveEdges(g.nodes)` by the graph's own invariant); any node
        // topoSort couldn't place still has unresolved in-degree, i.e. is
        // part of a cycle. A reconverging diamond (a→b, a→c, b→d, c→d) is
        // NOT a cycle — Kahn's tracks per-node in-degree, not pairwise
        // adjacency, so it drains cleanly.
        if case .cycle(let ids) = sorted {
            for id in ids { add(id, "part of a cycle — steps must form a DAG") }
        }

        // Unknown edge target: a `next`/`split`/`depends_on` id that isn't a
        // known node.
        for n in g.nodes {
            for t in n.data.next ?? [] {
                if !nodeIds.contains(t) { add(n.id, "edge target `\(t)` is not a known step") }
            }
            if n.data.kind == .split {
                for t in n.data.split ?? [] {
                    if !nodeIds.contains(t) { add(n.id, "edge target `\(t)` is not a known step") }
                }
            }
            for p in n.data.dependsOn ?? [] {
                if !nodeIds.contains(p) { add(n.id, "depends_on `\(p)` is not a known step") }
            }
        }

        // Self-loop: a `next`/`split`/`depends_on` entry that targets the
        // node's own id. Mirrors the backend's `EdgeSelfLoop` check
        // (workflow.rs). Reads the raw node fields directly rather than
        // `g.edges` — `deriveEdges`'s `addEdge` silently drops `source ===
        // target` edges (correct for rendering, since there's nothing to
        // draw), so a self-loop never reaches `g.edges` and the cycle/
        // unknown-target checks above never see it.
        for n in g.nodes {
            for t in n.data.next ?? [] where t == n.id {
                add(n.id, "an edge cannot target its own step")
            }
            if n.data.kind == .split {
                for t in n.data.split ?? [] where t == n.id {
                    add(n.id, "an edge cannot target its own step")
                }
            }
            for p in n.data.dependsOn ?? [] where p == n.id {
                add(n.id, "an edge cannot target its own step")
            }
        }

        // Degenerate split/join: fanning out to (or in from) fewer than 2
        // steps isn't doing real orchestration work — a plain `next` chain
        // would do the same job with less ceremony. Low-severity (not a
        // save-blocking error like the checks above), so distinct wording.
        for n in g.nodes {
            if n.data.kind == .split {
                if (n.data.split ?? []).count < 2 { add(n.id, "a split should fan out to 2+ steps") }
            } else if n.data.kind == .join {
                let inbound = g.edges.filter { $0.target == n.id }.count
                if inbound < 2 { add(n.id, "a join should have 2+ inbound paths") }
            }
        }
    }

    // NOTE: `validateLoops` (Phase 3 §2f, TS-source unconditional) is
    // deliberately NOT ported this plan — loops editing is out of scope
    // (Task 6 brief decision 3).

    return out
}
