import Testing
import CoreGraphics
@testable import RupuBuilder
import RupuFlowKit

/// Pure-helper coverage for the Step form (macOS Workflow Builder, Task 13):
/// `derivedRows(for:graph:)` (branch/split/join read-only rows), the `run:`
/// COMMAND setter (`settingRunCommand`, which must preserve sibling `run:`
/// keys via `YAMLValue.mapping(settingKey:to:)`), and the action `with:`
/// sub-editor's parse-or-hold helper (`parseWithText`). No SwiftUI render
/// pass — same "test the pure seam, not the body" convention every other
/// `RupuBuilderTests` file already follows (see `PaletteTests.swift`).

// MARK: - derivedRows — branch

@Test func derivedRowsForBranchListsThenAndElseTargets() {
    var data = StepNodeData(id: "route", kind: .branch)
    data.condition = "x == 1"
    data.thenTargets = ["a", "b"]
    data.elseTargets = ["c"]
    let node = GraphNode(id: "route", data: data, position: .zero)
    let graph = WorkflowGraph(nodes: [node], edges: [], meta: WorkflowMeta(name: "wf"))

    let rows = derivedRows(for: node, graph: graph)
    #expect(rows.map(\.label) == ["then", "else"])
    #expect(rows.map(\.value) == ["a, b", "c"])
}

@Test func derivedRowsForBranchShowsPlaceholderWhenTargetsAreEmpty() {
    let data = StepNodeData(id: "route", kind: .branch)
    let node = GraphNode(id: "route", data: data, position: .zero)
    let graph = WorkflowGraph(nodes: [node], edges: [], meta: WorkflowMeta(name: "wf"))

    let rows = derivedRows(for: node, graph: graph)
    #expect(rows.map(\.label) == ["then", "else"])
    #expect(rows.map(\.value) == ["—", "—"])
}

// MARK: - derivedRows — split

@Test func derivedRowsForSplitListsDataSplitTargets() {
    var data = StepNodeData(id: "fanout", kind: .split)
    data.split = ["review_a", "review_b"]
    let node = GraphNode(id: "fanout", data: data, position: .zero)
    let graph = WorkflowGraph(nodes: [node], edges: [], meta: WorkflowMeta(name: "wf"))

    let rows = derivedRows(for: node, graph: graph)
    #expect(rows.map(\.label) == ["targets"])
    #expect(rows.map(\.value) == ["review_a, review_b"])
}

@Test func derivedRowsForSplitShowsPlaceholderWhenEmpty() {
    let data = StepNodeData(id: "fanout", kind: .split)
    let node = GraphNode(id: "fanout", data: data, position: .zero)
    let graph = WorkflowGraph(nodes: [node], edges: [], meta: WorkflowMeta(name: "wf"))

    #expect(derivedRows(for: node, graph: graph).map(\.value) == ["—"])
}

// MARK: - derivedRows — join

@Test func derivedRowsForJoinListsInboundEdgeSourcesAsWaitsOn() {
    var joinData = StepNodeData(id: "barrier", kind: .join)
    joinData.hasJoin = true
    let joinNode = GraphNode(id: "barrier", data: joinData, position: .zero)

    var aData = StepNodeData(id: "a", kind: .step)
    aData.next = ["barrier"]
    let aNode = GraphNode(id: "a", data: aData, position: .zero)

    var bData = StepNodeData(id: "b", kind: .step)
    bData.next = ["barrier"]
    let bNode = GraphNode(id: "b", data: bData, position: .zero)

    let graph = withDerivedEdges(meta: WorkflowMeta(name: "wf"), nodes: [aNode, bNode, joinNode], loops: [])

    let rows = derivedRows(for: joinNode, graph: graph)
    #expect(rows.map(\.label) == ["waits on"])
    #expect(rows.map(\.value) == ["a, b"])
}

@Test func derivedRowsForJoinShowsPlaceholderWithNoInboundEdges() {
    let joinData = StepNodeData(id: "barrier", kind: .join)
    let node = GraphNode(id: "barrier", data: joinData, position: .zero)
    let graph = WorkflowGraph(nodes: [node], edges: [], meta: WorkflowMeta(name: "wf"))

    #expect(derivedRows(for: node, graph: graph).map(\.value) == ["—"])
}

// MARK: - derivedRows — every other kind

@Test func derivedRowsIsEmptyForEveryOtherKind() {
    for kind: StepKind in [.step, .forEach, .parallel, .panel, .approvalGate, .action, .run] {
        let data = StepNodeData(id: "n", kind: kind)
        let node = GraphNode(id: "n", data: data, position: .zero)
        let graph = WorkflowGraph(nodes: [node], edges: [], meta: WorkflowMeta(name: "wf"))
        #expect(derivedRows(for: node, graph: graph).isEmpty, "expected no derived rows for \(kind)")
    }
}

// MARK: - settingRunCommand

@Test func settingRunCommandPreservesSiblingRunKeys() {
    var data = StepNodeData(id: "scan", kind: .run)
    data.runBlock = .mapping([
        (key: "cmd", value: .string("semgrep")),
        (key: "args", value: .sequence([.string("scan"), .string("--config"), .string("auto")])),
    ])

    let updated = settingRunCommand(data, to: "eslint")

    #expect(updated.runBlock?["cmd"] == .string("eslint"))
    #expect(updated.runBlock?["args"] == .sequence([.string("scan"), .string("--config"), .string("auto")]))
}

@Test func settingRunCommandCreatesAMappingWhenRunBlockIsNil() {
    let data = StepNodeData(id: "scan", kind: .run)

    let updated = settingRunCommand(data, to: "eslint")

    #expect(updated.runBlock == .mapping([(key: "cmd", value: .string("eslint"))]))
}

@Test func settingRunCommandLeavesEveryOtherFieldUntouched() {
    var data = StepNodeData(id: "scan", kind: .run)
    data.when = "always"

    let updated = settingRunCommand(data, to: "eslint")

    #expect(updated.id == "scan")
    #expect(updated.when == "always")
}

// MARK: - parseWithText

@Test func parseWithTextEmptyClearsTheField() {
    #expect(parseWithText("") == .success(nil))
    #expect(parseWithText("   \n  ") == .success(nil))
}

@Test func parseWithTextValidYAMLParsesToAMapping() {
    let result = parseWithText("body: \"Done.\"\nclosed: true")
    #expect(result == .success(.mapping([(key: "body", value: .string("Done.")), (key: "closed", value: .bool(true))])))
}

@Test func parseWithTextInvalidYAMLFailsWithoutCommitting() {
    let result = parseWithText("body: [unterminated")
    guard case .failure(let message) = result else {
        Issue.record("expected a failure result, got \(result)")
        return
    }
    #expect(!message.isEmpty)
}
