import Testing

@testable import RupuFlowKit

private func graph(_ yaml: String) throws -> WorkflowGraph {
    yamlToGraph(try YAMLParser.parse(yaml))
}

@Test func stepNeedsAgentAndPrompt() throws {
    let g = try graph("name: t\nsteps:\n  - id: a\n")
    #expect(validateGraph(g)["a"]?.sorted() == ["needs a prompt", "needs an agent"])
}

@Test func cleanGraphIsEmpty() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n")
    #expect(validateGraph(g).isEmpty)
}

@Test func forwardRefCycleSuppressed() throws {
    // Web parity: the backward data-ref forms a 2-cycle with the legacy chain
    // edge; topo order is undefined, so neither the forward-ref message nor
    // (legacy mode) a cycle message fires. Documents workflowGraph.ts behavior.
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: \"{{ steps.b.output }}\"}\n  - {id: b, agent: x, prompt: p}\n")
    #expect(validateGraph(g)["a"] == nil)
}

@Test func unknownRefFlagged() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: \"{{ steps.ghost.output }}\"}\n")
    #expect(validateGraph(g)["a"] == ["references unknown step ghost"])
}

@Test func graphModeChecks() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [ghost, a]}\n  - id: s\n    split: [a]\n  - id: j\n    join: {}\n")
    let p = validateGraph(g)
    #expect(p["a"]?.contains("edge target `ghost` is not a known step") == true)
    #expect(p["a"]?.contains("an edge cannot target its own step") == true)
    #expect(p["s"]?.contains("a split should fan out to 2+ steps") == true)
    #expect(p["j"]?.contains("a join should have 2+ inbound paths") == true)
}

@Test func duplicateIDFlagged() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: a, agent: x, prompt: q}\n")
    #expect(validateGraph(g)["a"]?.contains("duplicate step id") == true)
}

@Test func panelBranchParallelChecks() throws {
    let g = try graph("name: t\nsteps:\n  - id: p\n    panel: {panelists: [], subject: \"\"}\n  - id: b\n    branch: {then: [ghost]}\n  - id: par\n    parallel: []\n")
    let out = validateGraph(g)
    #expect(out["p"]?.contains("panel needs at least one panelist") == true)
    #expect(out["b"]?.contains("branch needs a condition") == true)
    #expect(out["b"]?.contains("branch target ghost is not a known step") == true)
    #expect(out["par"]?.contains("needs at least one parallel sub-step") == true)
}

@Test func runAndActionExtensions() throws {
    let g = try graph("name: t\nsteps:\n  - id: r\n    run: {}\n  - id: c\n    action: \"\"\n")
    let out = validateGraph(g)
    #expect(out["r"] == ["run needs a cmd"])
    #expect(out["c"] == ["action needs a tool"])
}
