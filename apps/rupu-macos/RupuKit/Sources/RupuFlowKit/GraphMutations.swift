import CoreGraphics

// Graph mutation primitives — the canvas-editing verbs (add / connect /
// delete / rename / field-edit a node) as pure `WorkflowGraph -> WorkflowGraph`
// transforms, every one returning a NEW graph built via `withDerivedEdges` so
// edges are always freshly derived, never hand-patched. Callers re-serialize
// via `graphToWorkflowObject` (Task 9) — a serialize failure (a mutation that
// closed a cycle `graphToWorkflowObject`'s own topo-sort would refuse) simply
// rejects the edit at that layer, not here.
//
// Loosely mirrors `crates/rupu-cp/web/src/components/workflow-editor/
// WorkflowEditorGraph.tsx`'s exported pure helpers (`applyAddNodeAt` /
// `applyConnect` / `applyDelete`) and `workflowGraph.ts`'s
// `scrubStepFromLoops`; `applyRename` has no TS counterpart (the web editor
// doesn't support renaming a step id in place) and is a macOS-only addition
// per the Task 8 brief.

// ── applyAdd ──────────────────────────────────────────────────────────────────

/// Smallest free `"\(kind.rawValue)-N"` id (N ≥ 1) not already used by any
/// node in `nodes` — checked against every existing id regardless of that
/// node's own kind, so a graph that already has `step-1` never mints a
/// second `step-1` even if the new node is a different kind (e.g. adding a
/// `for_each` still walks its own `for_each-N` sequence independently).
func smallestFreeID(_ nodes: [GraphNode], prefix: String) -> String {
    let existing = Set(nodes.map(\.id))
    var n = 1
    var candidate = "\(prefix)-\(n)"
    while existing.contains(candidate) {
        n += 1
        candidate = "\(prefix)-\(n)"
    }
    return candidate
}

/// Build the `StepNodeData` for a fresh node of `kind`, seeding the
/// container shapes so the node round-trips through `graphToWorkflowObject`
/// immediately (validated shape, even if empty). A `.approvalGate` node
/// deliberately seeds NOTHING — `nodeToStepObject`'s gate arm emits a bare
/// `approval: {}` purely because `d.kind == .approvalGate`, so the node's
/// identity IS the block's presence, not any field inside it (this is the
/// one place this port diverges from the TS `newNodeData`, which sets
/// `approvalRequired = true`; matches the Task 8 brief's explicit
/// instruction). A `.branch` node likewise seeds nothing — `condition`/
/// `thenTargets`/`elseTargets` all start `nil`, same as any other kind's
/// unset optional fields.
func newNodeData(id: String, kind: StepKind) -> StepNodeData {
    var data = StepNodeData(id: id, kind: kind)
    switch kind {
    case .split:
        data.split = []
    case .join:
        data.hasJoin = true
    case .parallel:
        data.parallel = []
    case .panel:
        data.panel = PanelCfg(panelists: [], subject: "")
    case .run:
        data.runBlock = .mapping([("cmd", .string(""))])
    case .step, .forEach, .branch, .approvalGate, .action:
        break
    }
    return data
}

/// Append a new node of `kind` at an explicit canvas position; returns the
/// updated graph and the new id. Deliberately never wires `next`/`split` — a
/// freshly dropped node is disconnected until a line is drawn from or to it
/// (`applyConnect`).
public func applyAdd(_ g: WorkflowGraph, kind: StepKind, at: CGPoint) -> (WorkflowGraph, newID: String) {
    let id = smallestFreeID(g.nodes, prefix: kind.rawValue)
    let node = GraphNode(id: id, data: newNodeData(id: id, kind: kind), position: at)
    let nodes = g.nodes + [node]
    return (withDerivedEdges(meta: g.meta, nodes: nodes, loops: g.loops), newID: id)
}

// ── applyConnect ──────────────────────────────────────────────────────────────

/// The result of a draw-a-connection gesture: either the new graph, or a
/// human-readable reason the connection was refused (surfaced to the user,
/// never applied).
public enum ConnectOutcome {
    case connected(WorkflowGraph)
    case rejected(String)
}

/// Validate + apply a drawn connection source→target. `arm` is `"then"` /
/// `"else"` for a branch-arm draw (append to that arm's target list) or
/// `nil` for a plain draw (append to `source`'s `next`).
///
/// Validation runs FIRST, against the graph's CURRENT derived edges (`g.edges`
/// — already includes the legacy chain when the graph has no explicit edges
/// yet, since `deriveEdges` draws it). Only once the draw is accepted do we
/// call `materializeLegacyChain` — the Critical-bug guard ported from TS
/// `workflowGraph.ts:807-830`: this connect may be about to author the
/// graph's first explicit edge, which flips `hasExplicitEdges` for the WHOLE
/// graph from legacy to graph mode; materializing BEFORE the new edge is
/// written freezes every OTHER node's implicit chain successor as an
/// explicit `next` so none of them silently lose their edge when the mode
/// flips. Running `canConnect` against the pre-materialize edges (rather
/// than re-deriving after) is deliberate too — they describe the exact same
/// edge set either way, so this is just avoiding redundant work, not a
/// behavioral choice.
public func applyConnect(_ g: WorkflowGraph, source: String, target: String, arm: String?) -> ConnectOutcome {
    switch canConnect(source: source, target: target, edges: g.edges, arm: arm) {
    case .ok:
        break
    case .selfLoop(let reason), .duplicate(let reason), .cycle(let reason):
        return .rejected(reason)
    }

    let baseNodes = materializeLegacyChain(g.nodes)
    let nodes = baseNodes.map { n -> GraphNode in
        guard n.id == source else { return n }
        var copy = n
        if arm == "then" {
            var list = copy.data.thenTargets ?? []
            if !list.contains(target) { list.append(target) }
            copy.data.thenTargets = list
        } else if arm == "else" {
            var list = copy.data.elseTargets ?? []
            if !list.contains(target) { list.append(target) }
            copy.data.elseTargets = list
        } else {
            var list = copy.data.next ?? []
            if !list.contains(target) { list.append(target) }
            copy.data.next = list
        }
        return copy
    }
    return .connected(withDerivedEdges(meta: g.meta, nodes: nodes, loops: g.loops))
}

// ── applyDelete ───────────────────────────────────────────────────────────────

/// Drop `id` from every loop's membership (a loop referencing a now-gone
/// member would otherwise dangle and validate as unknown). A loop left with
/// fewer than 2 members after the drop is removed outright — same as the
/// explicit "remove the group" action's effect. Port of TS
/// `workflowGraph.ts`'s `scrubStepFromLoops`; lives here rather than in
/// `GraphEdges.swift` because it's a mutation-flow helper (only ever called
/// from `applyDelete`), not a piece of edge derivation.
func scrubStepFromLoops(_ loops: [WorkflowLoop], _ id: String) -> [WorkflowLoop] {
    loops
        .map { l -> WorkflowLoop in
            guard l.nodes.contains(id) else { return l }
            var copy = l
            copy.nodes = l.nodes.filter { $0 != id }
            return copy
        }
        .filter { $0.nodes.count >= 2 }
}

/// Remove a node, scrubbing it from every surviving node's `next`/`split`/
/// `dependsOn`/`thenTargets`/`elseTargets` — those arrays, not a stored
/// edges array, are what the deleted node's touching edges derive from, so
/// without this an edge to or from a now-gone node would keep trying to
/// derive. Also scrubs the id from loop membership (`scrubStepFromLoops`).
public func applyDelete(_ g: WorkflowGraph, id: String) -> WorkflowGraph {
    let nodes = g.nodes
        .filter { $0.id != id }
        .map { n -> GraphNode in
            var copy = n
            copy.data.next = copy.data.next?.filter { $0 != id }
            copy.data.split = copy.data.split?.filter { $0 != id }
            copy.data.dependsOn = copy.data.dependsOn?.filter { $0 != id }
            copy.data.thenTargets = copy.data.thenTargets?.filter { $0 != id }
            copy.data.elseTargets = copy.data.elseTargets?.filter { $0 != id }
            return copy
        }
    return withDerivedEdges(meta: g.meta, nodes: nodes, loops: scrubStepFromLoops(g.loops, id))
}

// ── applyRename ───────────────────────────────────────────────────────────────

/// The result of a rename attempt: either the new graph and the slugified id
/// that was actually applied, or a human-readable rejection reason.
public enum RenameOutcome {
    case renamed(WorkflowGraph, id: String)
    case rejected(String)
}

/// Lowercase, map every character outside `[a-z0-9_-]` to `-`, collapse
/// repeated `-` runs to one, then trim leading/trailing `-`. Underscores in
/// the input survive as-is (they're already in the allowed set).
func slugify(_ raw: String) -> String {
    var mapped = ""
    for ch in raw.lowercased() {
        if ch.isASCII, ch.isLetter || ch.isNumber || ch == "_" || ch == "-" {
            mapped.append(ch)
        } else {
            mapped.append("-")
        }
    }

    var collapsed = ""
    var lastWasDash = false
    for ch in mapped {
        if ch == "-" {
            if lastWasDash { continue }
            lastWasDash = true
        } else {
            lastWasDash = false
        }
        collapsed.append(ch)
    }

    while collapsed.hasPrefix("-") { collapsed.removeFirst() }
    while collapsed.hasSuffix("-") { collapsed.removeLast() }
    return collapsed
}

/// Rename step `from` to a slugified version of `rawNew`. Rejects an
/// empty-after-slug result or a collision with any OTHER node's id (a
/// same-as-before "rename" is allowed — it's a no-op). Rewrites every
/// reference: other nodes' `next`/`split`/`dependsOn`/`thenTargets`/
/// `elseTargets`, and loop memberships. Prompt-text `steps.<old>` template
/// references are deliberately NOT rewritten (matches web behavior — the
/// validate layer flags the resulting dangling reference instead).
public func applyRename(_ g: WorkflowGraph, from: String, to rawNew: String) -> RenameOutcome {
    let newID = slugify(rawNew)
    guard !newID.isEmpty else {
        return .rejected("Step id can't be empty.")
    }
    if newID != from, g.nodes.contains(where: { $0.id == newID }) {
        return .rejected("A step with this id already exists.")
    }

    let nodes = g.nodes.map { n -> GraphNode in
        var copy = n
        if n.id == from {
            copy.id = newID
            copy.data.id = newID
        } else {
            copy.data.next = copy.data.next?.map { $0 == from ? newID : $0 }
            copy.data.split = copy.data.split?.map { $0 == from ? newID : $0 }
            copy.data.dependsOn = copy.data.dependsOn?.map { $0 == from ? newID : $0 }
            copy.data.thenTargets = copy.data.thenTargets?.map { $0 == from ? newID : $0 }
            copy.data.elseTargets = copy.data.elseTargets?.map { $0 == from ? newID : $0 }
        }
        return copy
    }
    let loops = g.loops.map { l -> WorkflowLoop in
        guard l.nodes.contains(from) else { return l }
        var copy = l
        copy.nodes = l.nodes.map { $0 == from ? newID : $0 }
        return copy
    }
    return .renamed(withDerivedEdges(meta: g.meta, nodes: nodes, loops: loops), id: newID)
}

// ── applyUpdate ───────────────────────────────────────────────────────────────

/// Replace node `id`'s data wholesale (form edits; branch target edits
/// included, since routing is data-borne). `data.id` wins as the node's id
/// too — this path trusts the caller not to use it for a rename (renames go
/// through `applyRename`, which also rewrites every reference; a bare
/// `applyUpdate` with a changed id would NOT rewrite other nodes' pointers).
public func applyUpdate(_ g: WorkflowGraph, id: String, data: StepNodeData) -> WorkflowGraph {
    let nodes = g.nodes.map { n -> GraphNode in
        guard n.id == id else { return n }
        return GraphNode(id: data.id, data: data, position: n.position)
    }
    return withDerivedEdges(meta: g.meta, nodes: nodes, loops: g.loops)
}
