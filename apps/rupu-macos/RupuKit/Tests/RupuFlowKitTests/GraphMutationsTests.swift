import Testing

@testable import RupuFlowKit

private func graph(_ yaml: String) throws -> WorkflowGraph {
    yamlToGraph(try YAMLParser.parse(yaml))
}

@Test func addAllocatesSmallestFreeID() throws {
    let g0 = try graph("name: t\nsteps:\n  - {id: step-1, agent: x, prompt: p}\n")
    let (g1, id) = applyAdd(g0, kind: .step, at: .init(x: 10, y: 10))
    #expect(id == "step-2")
    #expect(g1.nodes.count == 2)
}

@Test func addedGateRoundTripsImmediately() throws {
    let (g, _) = applyAdd(try graph("name: t\nsteps: []\n"), kind: .approvalGate, at: .zero)
    guard case .object(let obj) = graphToWorkflowObject(g) else { Issue.record("gate must serialize"); return }
    #expect(obj["steps"]?.sequenceValue?[0]["approval"] != nil)
}

@Test func connectMaterializesLegacyChainFirst() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n")
    guard case .connected(let g2) = applyConnect(g, source: "a", target: "c", arm: nil) else { Issue.record("connect refused"); return }
    // The pre-existing implicit a->b and b->c chain must survive as explicit next.
    let ids = Set(g2.edges.map { "\($0.source)->\($0.target)" })
    #expect(ids == ["a->b", "b->c", "a->c"])
}

@Test func connectRejectsCycle() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n")
    guard case .rejected(let why) = applyConnect(g, source: "b", target: "a", arm: nil) else { Issue.record("expected reject"); return }
    #expect(why == "This would create a cycle — steps must form a DAG.")
}

@Test func branchArmConnectWritesTargets() throws {
    let g = try graph("name: t\nsteps:\n  - id: r\n    branch: {condition: c}\n  - {id: a, agent: x, prompt: p}\n")
    guard case .connected(let g2) = applyConnect(g, source: "r", target: "a", arm: "then") else { Issue.record("refused"); return }
    #expect(g2.nodes[0].data.thenTargets == ["a"])
}

@Test func deleteScrubsEveryReference() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n  - id: r\n    branch: {condition: c, then: [b], else: [a]}\n  - id: s\n    split: [b, a]\n")
    let g2 = applyDelete(g, id: "b")
    #expect(!g2.nodes.contains { $0.id == "b" })
    #expect(g2.nodes.first { $0.id == "a" }?.data.next == [])
    #expect(g2.nodes.first { $0.id == "r" }?.data.thenTargets == nil || g2.nodes.first { $0.id == "r" }!.data.thenTargets! == [])
    #expect(g2.nodes.first { $0.id == "s" }?.data.split == ["a"])
}

@Test func renameSlugifiesAndRewritesEdges() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n")
    guard case .renamed(let g2, let id) = applyRename(g, from: "b", to: "My Step!!") else { Issue.record("refused"); return }
    #expect(id == "my-step")
    #expect(g2.nodes.first { $0.id == "a" }?.data.next == ["my-step"])
}

@Test func renameRejectsDuplicate() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    guard case .rejected = applyRename(g, from: "b", to: "a") else { Issue.record("expected reject"); return }
}

// ── extra coverage beyond the brief's Step-1 tests ──────────────────────────

@Test func addSeedsPerKindIdentityFields() throws {
    let g0 = try graph("name: t\nsteps: []\n")
    let (gSplit, splitID) = applyAdd(g0, kind: .split, at: .zero)
    #expect(gSplit.nodes.first { $0.id == splitID }?.data.split == [])

    let (gJoin, joinID) = applyAdd(g0, kind: .join, at: .zero)
    #expect(gJoin.nodes.first { $0.id == joinID }?.data.hasJoin == true)

    let (gParallel, parallelID) = applyAdd(g0, kind: .parallel, at: .zero)
    #expect(gParallel.nodes.first { $0.id == parallelID }?.data.parallel == [])

    let (gPanel, panelID) = applyAdd(g0, kind: .panel, at: .zero)
    #expect(gPanel.nodes.first { $0.id == panelID }?.data.panel == PanelCfg(panelists: [], subject: ""))

    let (gRun, runID) = applyAdd(g0, kind: .run, at: .zero)
    #expect(gRun.nodes.first { $0.id == runID }?.data.runBlock == .mapping([("cmd", .string(""))]))
}

@Test func connectDuplicateIsRejected() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n")
    guard case .rejected(let why) = applyConnect(g, source: "a", target: "b", arm: nil) else { Issue.record("expected reject"); return }
    #expect(why == "These steps are already connected.")
}

@Test func connectSelfLoopIsRejected() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n")
    guard case .rejected(let why) = applyConnect(g, source: "a", target: "a", arm: nil) else { Issue.record("expected reject"); return }
    #expect(why == "A step can't depend on itself.")
}

@Test func deleteShrinksLoopBelowTwoAndDropsIt() throws {
    let g = try graph(
        "name: t\nloops:\n  refine: {nodes: [a, b], until: c, max_iterations: 3, on_max: fail}\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n"
    )
    let g2 = applyDelete(g, id: "b")
    #expect(g2.loops.isEmpty)
}

@Test func renameRewritesLoopMembership() throws {
    let g = try graph(
        "name: t\nloops:\n  refine: {nodes: [a, b, c], until: cond, max_iterations: 3, on_max: fail}\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n"
    )
    guard case .renamed(let g2, let id) = applyRename(g, from: "b", to: "reviewed") else { Issue.record("refused"); return }
    #expect(id == "reviewed")
    #expect(g2.loops.first?.nodes == ["a", "reviewed", "c"])
}

@Test func renameToSameIDIsANoOp() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n")
    guard case .renamed(let g2, let id) = applyRename(g, from: "a", to: "a") else { Issue.record("refused"); return }
    #expect(id == "a")
    #expect(g2.nodes.map(\.id) == ["a"])
}

@Test func renameRejectsEmptyAfterSlug() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n")
    guard case .rejected = applyRename(g, from: "a", to: "!!!") else { Issue.record("expected reject"); return }
}

@Test func updateReplacesNodeData() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n")
    var newData = StepNodeData(id: "a", kind: .step)
    newData.agent = "y"
    newData.prompt = "q"
    let g2 = applyUpdate(g, id: "a", data: newData)
    #expect(g2.nodes.first { $0.id == "a" }?.data.agent == "y")
    #expect(g2.nodes.first { $0.id == "a" }?.data.prompt == "q")
}

@Test func deleteScrubsDependsOnReference() throws {
    let g = try graph(
        "name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p, depends_on: [a]}\n"
    )
    let g2 = applyDelete(g, id: "a")
    #expect(g2.nodes.first { $0.id == "b" }?.data.dependsOn == [])
}

@Test func renameRejectsUnknownFrom() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n")
    guard case .rejected = applyRename(g, from: "nope", to: "whatever") else { Issue.record("expected reject"); return }
}

// ── loops survive add / connect / update (Task 8 review fix) ───────────────

private let loopedGraphYAML =
    "name: t\nloops:\n  refine: {nodes: [a, b], until: cond, max_iterations: 3, on_max: fail}\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n"

// A third, non-member node `c` gives `applyConnect` somewhere to draw a
// FRESH edge (a->b is already an implicit chain edge, so connecting them
// again would just read as a "duplicate" — this keeps the loop-survival
// assertion independent of that unrelated rejection path).
private let loopedGraphWithSpareNodeYAML =
    "name: t\nloops:\n  refine: {nodes: [a, b], until: cond, max_iterations: 3, on_max: fail}\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n"

@Test func addPreservesLoops() throws {
    let g = try graph(loopedGraphYAML)
    let (g2, _) = applyAdd(g, kind: .step, at: .zero)
    #expect(g2.loops == g.loops)
}

@Test func connectPreservesLoops() throws {
    let g = try graph(loopedGraphWithSpareNodeYAML)
    guard case .connected(let g2) = applyConnect(g, source: "a", target: "c", arm: nil) else {
        Issue.record("connect refused")
        return
    }
    #expect(g2.loops == g.loops)
    #expect(g2.edges.contains { $0.source == "a" && $0.target == "c" })
}

@Test func updatePreservesLoops() throws {
    let g = try graph(loopedGraphYAML)
    var newData = StepNodeData(id: "a", kind: .step)
    newData.agent = "y"
    newData.prompt = "q"
    let g2 = applyUpdate(g, id: "a", data: newData)
    #expect(g2.loops == g.loops)
}
