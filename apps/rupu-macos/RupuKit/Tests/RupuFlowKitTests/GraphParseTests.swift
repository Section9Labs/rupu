import Testing

@testable import RupuFlowKit

private func graph(_ yaml: String) throws -> WorkflowGraph {
    yamlToGraph(try YAMLParser.parse(yaml))
}

@Test func kindPrecedence() throws {
    let g = try graph(
        """
        name: t
        steps:
          - id: a
            agent: x
            prompt: p
          - id: b
            agent: x
            prompt: p
            for_each: "{{ inputs.files }}"
          - id: c
            parallel:
              - id: s1
                agent: x
                prompt: p
          - id: d
            panel:
              panelists: [r1]
              subject: s
          - id: e
            branch:
              condition: c
              then: [a]
          - id: f
            approval:
              prompt: ok?
          - id: g
            action: github.comment
            with: {body: hi}
          - id: h
            run:
              cmd: semgrep
              args: [scan]
          - id: i
            split: [a, b]
          - id: j
            join: {}
        """)
    #expect(
        g.nodes.map(\.data.kind) == [
            .step, .forEach, .parallel, .panel, .branch, .approvalGate, .action, .run, .split, .join,
        ])
    #expect(g.nodes[7].data.runBlock?["cmd"] == .string("semgrep"))
    #expect(g.nodes[9].data.hasJoin && g.nodes[9].data.joinWait == nil)
}

@Test func agentWithApprovalIsNotAGate() throws {
    let g = try graph("name: t\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    approval:\n      required: true\n")
    #expect(g.nodes[0].data.kind == .step)
    #expect(g.nodes[0].data.approvalRequired == true)
}

@Test func passthroughCapturesUnmodelledKeys() throws {
    let g = try graph("name: t\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    contract:\n      format: json\n")
    #expect(g.nodes[0].data.rawPassthrough?.first?.key == "contract")
}

@Test func metaRestKeepsTopLevelOrderVerbatim() throws {
    let g = try graph(
        "name: t\ndescription: d\ntrigger:\n  kind: cron\n  cron: \"0 3 * * *\"\ninputs:\n  files:\n    type: list\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n"
    )
    #expect(g.meta.rest.map(\.key) == ["trigger", "inputs"])
}

@Test func loopsParseSorted() throws {
    let g = try graph(
        "name: t\nloops:\n  zeta:\n    nodes: [a, b]\n    until: done\n    max_iterations: 3\n  alpha:\n    nodes: [c, d]\n    until: ok\n    max_iterations: 2\n    on_max: proceed\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n"
    )
    #expect(g.loops.map(\.name) == ["alpha", "zeta"])
    #expect(g.loops[0].onMax == "proceed")
    #expect(g.loops[1].onMax == "fail")
}
