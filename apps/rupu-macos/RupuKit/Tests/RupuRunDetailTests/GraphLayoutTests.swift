import Testing
import Foundation
import RupuAPI
@testable import RupuRunDetail

/// Fixture-driven tests for `layoutGraph`. `run_graph.json` carries all
/// seven `APIStepNode` kinds (`step`/`for_each`/`parallel`/`panel`/
/// `gate`/`action`/`run`), a result only for the first node ("plan"), and
/// units for the `for_each` node ("fan") with one finished (`success:
/// true`) and one still in-flight (`success: null`).
private func loadGraph() throws -> APIRunGraph {
    try JSONDecoder().decode(APIRunGraph.self, from: Fixtures.data("run_graph.json"))
}

@Test func kindLabelsCoverAllSevenNodeKinds() throws {
    let graph = try loadGraph()
    let vms = layoutGraph(nodes: graph.workflow.steps, results: graph.stepResults, units: graph.units, liveStates: [:])
    let labelByID = Dictionary(uniqueKeysWithValues: vms.map { ($0.id, $0.kindLabel) })

    #expect(labelByID["plan"] == "Step")
    #expect(labelByID["fan"] == "For Each")
    #expect(labelByID["par"] == "Parallel")
    #expect(labelByID["review"] == "Panel")
    #expect(labelByID["approve"] == "Gate")
    #expect(labelByID["create_pr"] == "Action")
    #expect(labelByID["build"] == "Run")
}

@Test func resultBackedNodeIsDone() throws {
    let graph = try loadGraph()
    let vms = layoutGraph(nodes: graph.workflow.steps, results: graph.stepResults, units: graph.units, liveStates: [:])
    let plan = try #require(vms.first { $0.id == "plan" })
    #expect(plan.state == .done(success: true))
}

@Test func gateAwaitingApprovalIsInjectedViaLiveStates() throws {
    let graph = try loadGraph()
    let vms = layoutGraph(
        nodes: graph.workflow.steps,
        results: graph.stepResults,
        units: graph.units,
        liveStates: ["approve": .gatePending]
    )
    let approve = try #require(vms.first { $0.id == "approve" })
    #expect(approve.state == .gatePending)
}

@Test func nodesWithoutResultsOrLiveStatesArePending() throws {
    let graph = try loadGraph()
    let vms = layoutGraph(
        nodes: graph.workflow.steps,
        results: graph.stepResults,
        units: graph.units,
        liveStates: ["approve": .gatePending]
    )
    let byID = Dictionary(uniqueKeysWithValues: vms.map { ($0.id, $0) })

    // "fan", "par", "review" have no matching result and no liveStates entry.
    #expect(byID["fan"]?.state == .pending)
    #expect(byID["par"]?.state == .pending)
    #expect(byID["review"]?.state == .pending)
    // Nodes downstream of the gate: no result, no liveStates entry -> pending.
    #expect(byID["create_pr"]?.state == .pending)
    #expect(byID["build"]?.state == .pending)
}

@Test func forEachNodeUnitProgressCountsNonNilSuccess() throws {
    let graph = try loadGraph()
    let vms = layoutGraph(nodes: graph.workflow.steps, results: graph.stepResults, units: graph.units, liveStates: [:])
    let fan = try #require(vms.first { $0.id == "fan" })
    let progress = try #require(fan.unitProgress)
    #expect(progress.done == 1)
    #expect(progress.total == 2)
}

@Test func laneCountReflectsParallelBranchesAndPanelists() throws {
    let graph = try loadGraph()
    let vms = layoutGraph(nodes: graph.workflow.steps, results: graph.stepResults, units: graph.units, liveStates: [:])
    let byID = Dictionary(uniqueKeysWithValues: vms.map { ($0.id, $0) })

    #expect(byID["par"]?.laneCount == 2) // two `parallel` branches
    #expect(byID["review"]?.laneCount == 2) // two panelists
    #expect(byID["plan"]?.laneCount == 1) // plain step, single lane
}

@Test func nodesWithoutMatchingUnitsHaveNilUnitProgress() throws {
    let graph = try loadGraph()
    let vms = layoutGraph(nodes: graph.workflow.steps, results: graph.stepResults, units: graph.units, liveStates: [:])
    let plan = try #require(vms.first { $0.id == "plan" })
    #expect(plan.unitProgress == nil)
}

@Test func emptyResultsAndLiveStatesLeaveFirstNodePendingNeverRunning() {
    let node = APIStepNode(
        id: "solo",
        kind: "step",
        agent: "rupuso",
        forEach: nil,
        parallel: nil,
        panelists: nil,
        gate: nil,
        action: nil,
        approvalGate: nil
    )
    let vms = layoutGraph(nodes: [node], results: [], units: [], liveStates: [:])
    #expect(vms.count == 1)
    #expect(vms[0].state == .pending)
}
