import Foundation

// Edge derivation + connect validation + loop-aware cycle helpers. Line-for-
// line port of `crates/rupu-cp/web/src/lib/workflowGraph.ts:566-668,
// 756-928, 1197-1243`.

// ── extractStepRefs ───────────────────────────────────────────────────────────

/// Scan every template string carried by a node (prompt, for_each, when,
/// condition, each sub-step prompt, panel subject/prompt) for `steps.<id>`
/// references and return the unique referenced ids, in first-seen order —
/// mirrors the TS `Set` (insertion-ordered) semantics exactly.
public func extractStepRefs(_ d: StepNodeData) -> [String] {
    // Constructed locally (rather than as a file-scope `let`) because
    // `Regex` isn't `Sendable`, and this package builds under Swift 6
    // strict concurrency — a global `let` would trip the
    // not-concurrency-safe diagnostic.
    let stepRefRegex = /steps\.([A-Za-z0-9_-]+)/
    var buckets: [String?] = [d.prompt, d.forEach, d.when, d.condition]
    if let parallel = d.parallel {
        for s in parallel { buckets.append(s.prompt) }
    }
    if let panel = d.panel {
        buckets.append(panel.subject)
        buckets.append(panel.prompt)
    }

    var seen = Set<String>()
    var ids: [String] = []
    for case let text? in buckets {
        for match in text.matches(of: stepRefRegex) {
            let ref = String(match.output.1)
            if seen.insert(ref).inserted { ids.append(ref) }
        }
    }
    return ids
}

// ── hasExplicitEdges / materializeLegacyChain ────────────────────────────────

/// A workflow "has explicit edges" (is a graph workflow rather than a legacy
/// linear one) when any node declares `next:`, `split:`, `join:`, or
/// `depends_on:` — mirrors workflow.rs `workflow_has_explicit_edges` exactly.
/// Our Swift model makes `run` a first-class `StepKind`, which does NOT
/// affect this check (unlike `split`/`join`, `run` never implies explicit
/// edges).
public func hasExplicitEdges(_ nodes: [GraphNode]) -> Bool {
    nodes.contains { n in
        (n.data.next?.isEmpty == false) || n.data.kind == .split || n.data.kind == .join
            || (n.data.dependsOn?.isEmpty == false)
    }
}

/// The legacy->graph migration primitive: a no-op when `nodes` already has
/// explicit edges; otherwise returns a copy where every non-`branch` node's
/// `next` is set to exactly what `deriveEdges`'s legacy chain loop would
/// already have drawn for it — `[the next node in list order]`, `[]` for the
/// last node. A `branch` node is left untouched (no `next` written, even
/// `[]`): its routing is its `then`/`else` targets.
public func materializeLegacyChain(_ nodes: [GraphNode]) -> [GraphNode] {
    guard !hasExplicitEdges(nodes) else { return nodes }
    return nodes.enumerated().map { i, n in
        guard n.data.kind != .branch else { return n }
        var copy = n
        copy.data.next = i < nodes.count - 1 ? [nodes[i + 1].id] : []
        return copy
    }
}

// ── deriveEdges ───────────────────────────────────────────────────────────────

/// The ONE producer of canvas edges. Pure function of the ordered node list,
/// branching on `hasExplicitEdges`:
///
/// **Graph mode** (any node has `next`/`split`/`join`/`depends_on`): edges
/// come ONLY from explicit connections — (a) each node's `next` targets, (b)
/// a `split` node's fan-out targets, (c) `depends_on`'s inbound edges —
/// UNION (d) inferred data-ref edges (X->Y whenever Y references steps.X)
/// UNION (e) branch-arm edges. List order contributes NOTHING.
///
/// **Legacy mode** (no node has any of those): a chain edge between each
/// consecutive pair (declared order), plus the same data-ref and branch-arm
/// edges — the compat guarantee for edge-free workflows authored before
/// non-linear orchestration existed.
public func deriveEdges(_ nodes: [GraphNode]) -> [GraphEdge] {
    let ids = Set(nodes.map(\.id))
    var edges: [GraphEdge] = []
    var seen = Set<String>()

    func addEdge(_ source: String, _ target: String, label: String? = nil, branch: String? = nil) {
        // Label is part of the dedupe key so a labeled branch-arm edge never
        // collapses onto a plain chain/data-ref edge (or onto the other arm)
        // that happens to connect the same pair of nodes.
        let key = "\(source)->\(target)::\(label ?? "")"
        if source == target || seen.contains(key) { return }
        seen.insert(key)
        let id = branch != nil ? "\(source)->\(target):\(branch!)" : "\(source)->\(target)"
        edges.append(GraphEdge(id: id, source: source, target: target, label: label, branchArm: branch))
    }

    let graphMode = hasExplicitEdges(nodes)

    if graphMode {
        // (a) explicit `next` edges, (b) `split` fan-out edges, (c)
        // `depends_on` inbound edges — the symmetric inverse of `next`,
        // authored on the TARGET node: `depends_on: [p]` renders as
        // `p -> thisNode`.
        for n in nodes {
            for t in n.data.next ?? [] where ids.contains(t) {
                addEdge(n.id, t)
            }
            if n.data.kind == .split {
                for t in n.data.split ?? [] where ids.contains(t) {
                    addEdge(n.id, t)
                }
            }
            for p in n.data.dependsOn ?? [] where ids.contains(p) {
                addEdge(p, n.id)
            }
        }
    } else {
        // Legacy: a chain edge between each consecutive pair (declared order).
        for i in 0..<max(0, nodes.count - 1) {
            addEdge(nodes[i].id, nodes[i + 1].id)
        }
    }

    // Data-ref edges X->Y whenever Y references steps.X and X exists —
    // inferred in BOTH modes. Dedupe collapses onto any chain/next/split
    // edge that already connects the same pair.
    for n in nodes {
        for ref in extractStepRefs(n.data) where ids.contains(ref) {
            addEdge(ref, n.id)
        }
    }

    // Branch-arm edges: a `branch` node points at each of its then/else
    // targets with a label so the renderer can draw true/false arms
    // distinctly — in BOTH modes (a branch is an explicit connection either
    // way).
    for n in nodes where n.data.kind == .branch {
        for t in n.data.thenTargets ?? [] where ids.contains(t) {
            addEdge(n.id, t, label: "true", branch: "then")
        }
        for t in n.data.elseTargets ?? [] where ids.contains(t) {
            addEdge(n.id, t, label: "false", branch: "else")
        }
    }

    return edges
}

/// Build a `WorkflowGraph` whose edges are derived from its nodes — the only
/// correct way to construct/return a graph.
public func withDerivedEdges(meta: WorkflowMeta, nodes: [GraphNode], loops: [WorkflowLoop]) -> WorkflowGraph {
    WorkflowGraph(nodes: nodes, edges: deriveEdges(nodes), meta: meta, loops: loops)
}

// ── loop-aware cycle helpers ─────────────────────────────────────────────────

/// Generic cycle check over a small edge list: returns the set of node ids
/// that are part of AT LEAST one cycle (Kahn's algorithm — same technique
/// `topoSort` uses). Kept generic (raw `(String, String)` pairs, not
/// `GraphEdge`) so it works over collapsed super-node ids too.
func cyclicNodes<S: Sequence>(_ nodeIds: S, _ edges: [(String, String)]) -> Set<String> where S.Element == String {
    var indeg: [String: Int] = [:]
    var adj: [String: [String]] = [:]
    for id in nodeIds { indeg[id] = 0 }
    for (a, b) in edges {
        guard indeg[a] != nil, indeg[b] != nil else { continue }
        adj[a, default: []].append(b)
        indeg[b, default: 0] += 1
    }

    var queue = indeg.filter { $0.value == 0 }.map(\.key)
    var done = Set<String>()
    var head = 0
    while head < queue.count {
        let id = queue[head]
        head += 1
        done.insert(id)
        for t in adj[id] ?? [] {
            indeg[t, default: 0] -= 1
            if indeg[t] == 0 { queue.append(t) }
        }
    }

    var out = Set<String>()
    for id in indeg.keys where !done.contains(id) { out.insert(id) }
    return out
}

/// A loop's members' internal CONTROL edges only — `next`/`split`/branch-
/// arm/`depends_on`, both endpoints members of `memberIds` — mirroring
/// workflow.rs `loop_control_edges` exactly (control edges, NOT the inferred
/// data-ref edges `deriveEdges` also produces; a feedback back-reference is a
/// data ref to a non-ancestor and must NOT be treated as an internal control
/// edge or every refine-style loop would false-positive as cyclic).
func loopControlEdges(_ nodes: [GraphNode], _ memberIds: Set<String>) -> [(String, String)] {
    var out: [(String, String)] = []
    for n in nodes where memberIds.contains(n.id) {
        for t in n.data.next ?? [] where memberIds.contains(t) && t != n.id {
            out.append((n.id, t))
        }
        if n.data.kind == .split {
            for t in n.data.split ?? [] where memberIds.contains(t) && t != n.id {
                out.append((n.id, t))
            }
        }
        for t in n.data.thenTargets ?? [] where memberIds.contains(t) && t != n.id {
            out.append((n.id, t))
        }
        for t in n.data.elseTargets ?? [] where memberIds.contains(t) && t != n.id {
            out.append((n.id, t))
        }
        for p in n.data.dependsOn ?? [] where memberIds.contains(p) && p != n.id {
            out.append((p, n.id))
        }
    }
    return out
}

/// `edges` (typically `deriveEdges(nodes)`) with every EDGE fully inside a
/// SINGLE loop dropped UNLESS it's a genuine internal control edge
/// (`loopControlEdges`). This is the client mirror of the backend's
/// collapsed-graph semantics applied to cycle detection specifically: a
/// loop's legitimate feedback data-ref (e.g. `gen`'s prompt reading a prior
/// iteration's `steps.critique.output`, a REAL edge in `deriveEdges`) must
/// NOT count toward the OUTER graph's acyclicity, or a refine-style loop's
/// own intended mechanic would falsely trip the general cycle check and
/// BLOCK SAVING (`graphToWorkflowObject`). A genuine internal cycle (`next:
/// a→b; b→a` both in the same loop) is still caught by the loop's own
/// internal-acyclicity check (Task 6), which is scoped to control edges only
/// anyway. Edges that cross a loop boundary, connect two different loops, or
/// don't touch a loop at all are always kept unchanged.
func outerCycleEdges(_ nodes: [GraphNode], _ edges: [GraphEdge], _ loops: [WorkflowLoop]) -> [GraphEdge] {
    guard !loops.isEmpty else { return edges }
    var membership: [String: String] = [:]
    for l in loops { for id in l.nodes { membership[id] = l.name } }

    var controlPairs = Set<String>()
    for l in loops {
        let memberSet = Set(l.nodes)
        for (a, b) in loopControlEdges(nodes, memberSet) {
            controlPairs.insert("\(a)->\(b)")
        }
    }

    return edges.filter { e in
        guard let a = membership[e.source] else { return true }
        let b = membership[e.target]
        if a != b { return true }
        return controlPairs.contains("\(e.source)->\(e.target)")
    }
}

/// The loop `stepId` belongs to, if any. `nil` for a step in no loop —
/// callers don't need to special-case "not in a loop" beyond an optional
/// check. Port of `workflowGraph.ts:488-490`.
public func loopOfStep(_ loops: [WorkflowLoop], _ stepId: String) -> WorkflowLoop? {
    loops.first { $0.nodes.contains(stepId) }
}

// ── canConnect ────────────────────────────────────────────────────────────────

/// The result of a `canConnect` check — mirrors the TS `{ ok: true } | {
/// ok: false, reason, kind }` shape as a single enum, carrying the reason
/// string on each rejection case.
public enum ConnectVerdict: Equatable {
    case ok
    case selfLoop(String)
    case duplicate(String)
    case cycle(String)
}

/// Whether dragging a new edge source→target is allowed: no self-loops, no
/// duplicates, and the result must stay a DAG. We reject if `target` can
/// already reach `source` (a DFS over existing edges) — adding source→target
/// would then close a cycle.
///
/// `arm` distinguishes WHICH logical edge is being drawn: a plain connect
/// (`arm` nil) duplicate-checks only against other plain edges; a branch-arm
/// connect (`arm: "then" | "else"`) duplicate-checks only against an
/// existing edge tagged with that SAME arm — under the derived-edges model
/// every consecutive node pair carries a chain edge, so a branch arm and a
/// chain edge to the same target are two DISTINCT logical edges, not a
/// duplicate.
public func canConnect(source: String, target: String, edges: [GraphEdge], arm: String?) -> ConnectVerdict {
    if source == target { return .selfLoop("A step can't depend on itself.") }
    if edges.contains(where: { $0.source == source && $0.target == target && $0.branchArm == arm }) {
        return .duplicate("These steps are already connected.")
    }

    var adj: [String: [String]] = [:]
    for e in edges {
        adj[e.source, default: []].append(e.target)
    }

    var seen = Set<String>()
    var stack = [target]
    while let cur = stack.popLast() {
        if cur == source {
            return .cycle("This would create a cycle — steps must form a DAG.")
        }
        if seen.contains(cur) { continue }
        seen.insert(cur)
        stack.append(contentsOf: adj[cur] ?? [])
    }
    return .ok
}
