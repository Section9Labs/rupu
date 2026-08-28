import Testing

@testable import RupuFlowKit

private func graph(_ yaml: String) throws -> WorkflowGraph {
    yamlToGraph(try YAMLParser.parse(yaml))
}

@Test func fullRoundTripIsCanonicalFixedPoint() throws {
    for (name, text) in YAMLGolden.samples {
        let g1 = try graph(text)
        guard case .object(let obj) = graphToWorkflowObject(g1) else {
            Issue.record("serialize failed for \(name)")
            continue
        }
        let dumped = YAMLEmitter.dump(obj)
        let g2 = try graph(dumped)
        // Same steps, same kinds, same edge set, same meta.
        #expect(g1.nodes.map(\.data) == g2.nodes.map(\.data), "step drift in \(name)")
        #expect(Set(g1.edges.map(\.id)) == Set(g2.edges.map(\.id)), "edge drift in \(name)")
        #expect(g1.meta == g2.meta, "meta drift in \(name)")
        // Fixed point: serializing again emits identical YAML.
        guard case .object(let obj2) = graphToWorkflowObject(g2) else {
            Issue.record("re-serialize failed for \(name)")
            continue
        }
        #expect(YAMLEmitter.dump(obj2) == dumped, "canonical YAML not stable for \(name)")
    }
}

@Test func topoOrderTiebreaksOnPositionThenID() {
    var nodes = [
        GraphNode(id: "b", data: .init(id: "b", kind: .step), position: .init(x: 0, y: 100)),
        GraphNode(id: "a", data: .init(id: "a", kind: .step), position: .init(x: 0, y: 0)),
    ]
    if case .order(let o) = topoSort(nodes: nodes, edges: []) {
        #expect(o.map(\.id) == ["a", "b"])
    } else {
        Issue.record("unexpected cycle")
    }
    nodes[0].position = .zero
    if case .order(let o) = topoSort(nodes: nodes, edges: []) {
        #expect(o.map(\.id) == ["a", "b"])  // id tiebreak
    } else {
        Issue.record("unexpected cycle")
    }
}

@Test func cycleBlocksSerialization() throws {
    let g = try graph("name: t\nsteps:\n  - {id: a, agent: x, prompt: p, next: [b]}\n  - {id: b, agent: x, prompt: p, next: [a]}\n")
    guard case .failure(let msg) = graphToWorkflowObject(g) else {
        Issue.record("expected failure")
        return
    }
    #expect(msg.hasPrefix("Cannot serialize: cycle through "))
}

@Test func gateAlwaysEmitsApprovalBlock() throws {
    let g = try graph("name: t\nsteps:\n  - id: gate\n    approval: {}\n")
    guard case .object(let obj) = graphToWorkflowObject(g) else {
        Issue.record("serialize failed")
        return
    }
    #expect(obj["steps"]?.sequenceValue?[0]["approval"] == .mapping([]))
}

/// Regression for a fidelity gap review found in `nodeToStepObject`: the TS
/// source omits `when`/`agent`/`prompt`/`for_each`/`action` when the string
/// is FALSY (empty), not merely when it's `undefined` — `if (d.when)
/// o.when = d.when` (workflowGraph.ts:1044 etc.), not `if (d.when !==
/// undefined)`. A hand-authored `when: ""` (or `agent: ""`/`prompt: ""`/
/// `for_each: ""`/`action: ""`) parses to a non-nil EMPTY Swift string, and
/// a nil-only guard would then re-emit `when: ""` where the web drops the
/// key entirely. Exercises all five call sites: `appendSharedTail`'s
/// `when` (via the for_each-kind step `a`), the step/for_each arm's
/// `agent`/`prompt`/`for_each` (also step `a`), the `action` arm's
/// `action` (step `b`), and the `run` arm's `for_each` (step `c`).
@Test func emptyStringFieldsAreOmittedNotEmittedAsEmptyStrings() throws {
    let y = """
        name: t
        steps:
          - id: a
            agent: ""
            prompt: ""
            when: ""
            for_each: ""
          - id: b
            action: ""
          - id: c
            for_each: ""
            run:
              cmd: echo
        """
    guard case .object(let obj) = graphToWorkflowObject(try graph(y)) else {
        Issue.record("serialize failed")
        return
    }
    let steps = obj["steps"]?.sequenceValue ?? []
    #expect(steps.count == 3)
    #expect(steps[0]["agent"] == nil)
    #expect(steps[0]["prompt"] == nil)
    #expect(steps[0]["when"] == nil)
    #expect(steps[0]["for_each"] == nil)
    #expect(steps[1]["action"] == nil)
    #expect(steps[2]["for_each"] == nil)
}

@Test func loopFeedbackRefDoesNotBlockSerialize() throws {
    let y = """
        name: t
        loops:
          refine:
            nodes: [gen, critique]
            until: ok
            max_iterations: 3
        steps:
          - {id: gen, agent: x, prompt: "use {{ steps.critique.output }}", next: [critique]}
          - {id: critique, agent: x, prompt: p}
        """
    guard case .object = graphToWorkflowObject(try graph(y)) else {
        Issue.record("refine loop must serialize")
        return
    }
}
