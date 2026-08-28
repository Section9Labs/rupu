import CoreGraphics
import Testing

@testable import RupuFlowKit

private func graph(_ yaml: String) throws -> WorkflowGraph {
    yamlToGraph(try YAMLParser.parse(yaml))
}

@Test func autoLayoutColumnsFollowLongestPath() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b, c]}\n  - {id: b, agent: x, prompt: p, next: [d]}\n  - {id: c, agent: x, prompt: p}\n  - {id: d, agent: x, prompt: p}\n")
    let laid = autoLayout(nodes: g.nodes, edges: g.edges)
    let x = Dictionary(uniqueKeysWithValues: laid.map { ($0.id, $0.position.x) })
    #expect(x["a"]! < x["b"]!)
    #expect(x["b"]! == x["c"]!)
    #expect(x["b"]! < x["d"]!)
}

@Test func autoLayoutKeepsExistingPositions() throws {
    var g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p}\n  - {id: b, agent: x, prompt: p}\n")
    g.nodes[0].position = CGPoint(x: 500, y: 500)
    let laid = autoLayout(nodes: g.nodes, edges: g.edges)
    #expect(laid[0].position == CGPoint(x: 500, y: 500))
}
