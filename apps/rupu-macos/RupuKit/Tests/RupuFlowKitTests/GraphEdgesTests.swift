import Testing

@testable import RupuFlowKit

private func graph(_ yaml: String) throws -> WorkflowGraph {
    yamlToGraph(try YAMLParser.parse(yaml))
}

@Test func legacyModeChainsConsecutivePairs() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n")
    #expect(g.edges.map { "\($0.source)->\($0.target)" } == ["a->b", "b->c"])
}

@Test func graphModeIgnoresListOrder() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [c]}\n  - {id: b, agent: x, prompt: p}\n  - {id: c, agent: x, prompt: p}\n")
    #expect(g.edges.map { "\($0.source)->\($0.target)" } == ["a->c"])
}

@Test func dataRefEdgesInferredInBothModes() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: \"use {{ steps.a.output }}\"}\n")
    #expect(g.edges.count == 1)  // chain edge a->b dedupes with the data-ref
}

@Test func branchArmsAreLabelled() throws {
    let g = try graph("name: t\nsteps:\n  - id: r\n    branch: {condition: c, then: [a], else: [b]}\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    let arms = g.edges.filter { $0.branchArm != nil }
    #expect(arms.map(\.label) == ["true", "false"])
}

@Test func materializeLegacyChainWritesExplicitNext() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    let m = materializeLegacyChain(g.nodes)
    #expect(m[0].data.next == ["b"])
    #expect(m[1].data.next == [])
}

@Test func canConnectRejectsCycleDuplicateSelf() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p}\n")
    #expect(canConnect(source: "a", target: "a", edges: g.edges, arm: nil) == .selfLoop("A step can't depend on itself."))
    #expect(canConnect(source: "a", target: "b", edges: g.edges, arm: nil) == .duplicate("These steps are already connected."))
    #expect(canConnect(source: "b", target: "a", edges: g.edges, arm: nil) == .cycle("This would create a cycle — steps must form a DAG."))
    #expect(canConnect(source: "a", target: "b", edges: g.edges, arm: "then") == .ok)  // arm-distinct duplicate check
}
