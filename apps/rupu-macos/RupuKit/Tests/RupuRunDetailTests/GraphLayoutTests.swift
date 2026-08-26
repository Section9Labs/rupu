import Testing
import Foundation
import RupuAPI
import RupuStore
@testable import RupuRunDetail

/// Fixture-driven tests for `layoutGraph`. `run_graph.json` carries all
/// seven `APIStepNode` kinds (`step`/`for_each`/`parallel`/`panel`/
/// `gate`/`action`/`run`), a result only for the first node ("plan"), and
/// units for the `for_each` node ("fan") with one finished (`success:
/// true`) and one still in-flight (`success: null`).
private func loadGraph() throws -> APIRunGraph {
    try JSONDecoder().decode(APIRunGraph.self, from: Fixtures.data("run_graph.json"))
}

/// Convenience overload so most tests don't have to spell out the four new
/// (empty-by-default) live-overlay parameters.
private func layout(
    _ graph: APIRunGraph,
    liveStates: [String: NodeState] = [:],
    liveUnits: [String: [Int: UnitLiveState]] = [:],
    panelRounds: [String: PanelRoundState] = [:],
    stepTranscripts: [String: String] = [:]
) -> [GraphNodeVM] {
    layoutGraph(
        nodes: graph.workflow.steps,
        results: graph.stepResults,
        units: graph.units,
        liveStates: liveStates,
        liveUnits: liveUnits,
        panelRounds: panelRounds,
        stepTranscripts: stepTranscripts
    )
}

@Test func kindsMapCorrectlyIncludingUnknownFallback() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    let byID = Dictionary(uniqueKeysWithValues: vms.map { ($0.id, $0) })

    #expect(byID["plan"]?.kind == .step)
    #expect(byID["fan"]?.kind == .forEach)
    #expect(byID["par"]?.kind == .parallel)
    #expect(byID["review"]?.kind == .panel)
    #expect(byID["approve"]?.kind == .gate)
    #expect(byID["create_pr"]?.kind == .action)
    #expect(byID["build"]?.kind == .run)
}

@Test func unknownNodeKindFallsBackToStep() {
    let node = APIStepNode(
        id: "mystery",
        kind: "wat",
        agent: nil,
        forEach: nil,
        parallel: nil,
        panelists: nil,
        gate: nil,
        action: nil,
        approvalGate: nil
    )
    let vms = layoutGraph(
        nodes: [node],
        results: [],
        units: [],
        liveStates: [:],
        liveUnits: [:],
        panelRounds: [:],
        stepTranscripts: [:]
    )
    #expect(vms[0].kind == .step)
}

@Test func statePrecedenceLiveBeatsResultBeatsPending() throws {
    let graph = try loadGraph()

    // "plan" has a result (.done(success: true)) but no live entry -> result wins.
    let resultOnly = layout(graph)
    #expect(resultOnly.first { $0.id == "plan" }?.state == .done(success: true))

    // "plan" also has a live entry -> live wins outright over the result.
    let liveWins = layout(graph, liveStates: ["plan": .running])
    #expect(liveWins.first { $0.id == "plan" }?.state == .running)

    // "par" has neither a result nor a live entry -> pending.
    #expect(resultOnly.first { $0.id == "par" }?.state == .pending)
}

@Test func pausedIsAValidLiveState() throws {
    let graph = try loadGraph()
    let vms = layout(graph, liveStates: ["review": .paused])
    #expect(vms.first { $0.id == "review" }?.state == .paused)
}

@Test func forEachPendingParentPromotesToRunningWhenAnyUnitInFlight() throws {
    let graph = try loadGraph()

    // REST-only: "fan" has one done unit (index 0) and one in-flight unit
    // (index 1, success: nil) -> promotes .pending -> .running.
    let restOnly = layout(graph)
    #expect(restOnly.first { $0.id == "fan" }?.state == .running)
}

@Test func forEachStaysPendingWhenNoUnitIsInFlight() {
    // A for_each node with zero fanout units never promotes.
    let node = APIStepNode(
        id: "fan2",
        kind: "for_each",
        agent: "reviewer",
        forEach: "{{ files }}",
        parallel: nil,
        panelists: nil,
        gate: nil,
        action: nil,
        approvalGate: nil
    )
    let vms = layoutGraph(
        nodes: [node],
        results: [],
        units: [],
        liveStates: [:],
        liveUnits: [:],
        panelRounds: [:],
        stepTranscripts: [:]
    )
    #expect(vms[0].state == .pending)
}

@Test func forEachPromotionDoesNotOverrideAnExistingLiveState() throws {
    let graph = try loadGraph()
    // "fan" has an in-flight unit, but a live state already says .gatePending
    // -> live still wins outright (promotion only applies when state is
    // still the default .pending).
    let vms = layout(graph, liveStates: ["fan": .gatePending])
    #expect(vms.first { $0.id == "fan" }?.state == .gatePending)
}

@Test func fanoutOverlayLiveKeyAndSuccessWinOverRestRow() throws {
    let graph = try loadGraph()
    // REST row for index 0 ("crates/a") is done/success: true with no key.
    // Live overlay for index 0 supplies a key and flips success to false
    // (still in flight per REST, but live has since observed completion).
    let vms = layout(graph, liveUnits: [
        "fan": [0: UnitLiveState(key: "unit-abc", transcriptPath: "t/live0.jsonl", success: false)]
    ])
    let fan = try #require(vms.first { $0.id == "fan" })
    let fanout = try #require(fan.fanout)
    let unit0 = try #require(fanout.units.first { $0.id == 0 })

    #expect(unit0.key == "unit-abc")
    #expect(unit0.transcriptPath == "t/live0.jsonl")
    #expect(unit0.state == .done(success: false))
}

/// Whole-branch review fix (Important): without a live overlay, the key
/// chain's middle rung — the REST row's own `item` (the underlying
/// `for_each:` list value, web parity) — is what a unit falls back to, not
/// straight to the positional `"#index"` label. `run_graph.json`'s "fan"
/// units carry `item: "crates/a"`/`"crates/b"`.
@Test func fanoutOverlayFallsBackToRESTItemLabelWithoutLiveOverlay() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    let fan = try #require(vms.first { $0.id == "fan" })
    let fanout = try #require(fan.fanout)

    let unit0 = try #require(fanout.units.first { $0.id == 0 })
    let unit1 = try #require(fanout.units.first { $0.id == 1 })
    #expect(unit0.key == "crates/a")
    #expect(unit1.key == "crates/b")
    // Unit 1 has no terminal success in REST (nil) -> still running.
    #expect(unit1.state == .running)
    #expect(unit0.state == .done(success: true))
}

/// The key chain's ultimate fallback — `"#index"` — is only reached when
/// neither a live `unit_key` NOR a REST `item` is available (a unit row
/// with `item: nil`, e.g. decoded from a wire value that wasn't a string —
/// see `APIUnitRow.init(from:)`'s doc comment).
@Test func fanoutFallsBackToIndexKeyWhenNeitherLiveKeyNorRESTItemIsPresent() throws {
    let node = APIStepNode(
        id: "fan3",
        kind: "for_each",
        agent: "reviewer",
        forEach: "{{ files }}",
        parallel: nil,
        panelists: nil,
        gate: nil,
        action: nil,
        approvalGate: nil
    )
    let unit = APIUnitRow(stepID: "fan3", index: 0, runID: nil, transcriptPath: "t/u0.jsonl", success: true, host: nil, item: nil)
    let vms = layoutGraph(
        nodes: [node],
        results: [],
        units: [unit],
        liveStates: [:],
        liveUnits: [:],
        panelRounds: [:],
        stepTranscripts: [:]
    )
    let fan = try #require(vms.first { $0.id == "fan3" })
    let fanout = try #require(fan.fanout)
    let unit0 = try #require(fanout.units.first { $0.id == 0 })
    #expect(unit0.key == "#0")
}

@Test func fanoutCountsAreDerivedNotStored() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    let fan = try #require(vms.first { $0.id == "fan" })
    let fanout = try #require(fan.fanout)

    #expect(fanout.total == 2)
    #expect(fanout.done == 1)
    #expect(fanout.failed == 0)
    #expect(fanout.running == 1)
}

@Test func fanoutOverlayCanIntroduceAUnitBeyondTheRestSnapshot() throws {
    let graph = try loadGraph()
    // Live overlay adds a third unit (index 2) never seen in the REST rows
    // at all -- e.g. a `unit_started` event that outran the next poll.
    let vms = layout(graph, liveUnits: [
        "fan": [2: UnitLiveState(key: "unit-new", transcriptPath: "t/u2.jsonl", success: nil)]
    ])
    let fan = try #require(vms.first { $0.id == "fan" })
    let fanout = try #require(fan.fanout)
    #expect(fanout.total == 3)
    let unit2 = try #require(fanout.units.first { $0.id == 2 })
    #expect(unit2.key == "unit-new")
    #expect(unit2.state == .running)
}

@Test func nonForEachNodesHaveNilFanout() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    #expect(vms.first { $0.id == "plan" }?.fanout == nil)
    #expect(vms.first { $0.id == "par" }?.fanout == nil)
}

@Test func subStepsFromParallelBranches() throws {
    let graph = try loadGraph()
    let vms = layout(graph, liveStates: ["par_a": .running])
    let par = try #require(vms.first { $0.id == "par" })

    #expect(par.subSteps.count == 2)
    let a = try #require(par.subSteps.first { $0.id == "par_a" })
    let b = try #require(par.subSteps.first { $0.id == "par_b" })
    #expect(a.agent == "alpha")
    #expect(a.state == .running) // live wins
    #expect(b.agent == "beta")
    #expect(b.state == .pending) // no result, no live -> pending
}

@Test func subStepsFromPanelistsUseNameAsBothIDAndAgent() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    let review = try #require(vms.first { $0.id == "review" })

    #expect(review.subSteps.count == 2)
    let panelistA = try #require(review.subSteps.first { $0.id == "panelist-a" })
    #expect(panelistA.agent == "panelist-a")
    #expect(panelistA.state == .pending)
}

@Test func nonFanoutNodesHaveEmptySubSteps() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    #expect(vms.first { $0.id == "plan" }?.subSteps.isEmpty == true)
    #expect(vms.first { $0.id == "approve" }?.subSteps.isEmpty == true)
}

@Test func gateFlagsThreadFromApprovalGate() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    let approve = try #require(vms.first { $0.id == "approve" })

    #expect(approve.gateAuto == true)
    #expect(approve.gateHasOnReject == true)
}

@Test func gateFlagsDefaultFalseWithoutApprovalGate() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    let plan = try #require(vms.first { $0.id == "plan" })

    #expect(plan.gateAuto == false)
    #expect(plan.gateHasOnReject == false)
}

@Test func actionNameThreadsFromStepNode() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    let byID = Dictionary(uniqueKeysWithValues: vms.map { ($0.id, $0) })

    #expect(byID["create_pr"]?.actionName == "scm.prs.create")
    #expect(byID["build"]?.actionName == "cargo build")
    #expect(byID["plan"]?.actionName == nil)
}

@Test func panelRoundPassesThroughForPanelNode() throws {
    let graph = try loadGraph()
    let vms = layout(graph, panelRounds: ["review": PanelRoundState(round: 2, maxIterations: 5)])
    let review = try #require(vms.first { $0.id == "review" })

    #expect(review.panelRound?.round == 2)
    #expect(review.panelRound?.maxIterations == 5)
    #expect(review.panelGate?.maxIterations == 5)
    #expect(review.panelGate?.untilSeverity == "high")
    #expect(review.panelGate?.fixWith == "fixer")
}

@Test func panelRoundIsNilWithoutLiveEntry() throws {
    let graph = try loadGraph()
    let vms = layout(graph)
    #expect(vms.first { $0.id == "review" }?.panelRound == nil)
}

@Test func stepTranscriptThreadsWhenKnownLive() throws {
    let graph = try loadGraph()
    let vms = layout(graph, stepTranscripts: ["plan": "live/plan.jsonl"])
    #expect(vms.first { $0.id == "plan" }?.transcriptPath == "live/plan.jsonl")
    // No live transcript for "fan" -> nil (the fixture's result-backed
    // transcript_path lives on APIStepResult, not surfaced here directly).
    #expect(vms.first { $0.id == "fan" }?.transcriptPath == nil)
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
    let vms = layoutGraph(
        nodes: [node],
        results: [],
        units: [],
        liveStates: [:],
        liveUnits: [:],
        panelRounds: [:],
        stepTranscripts: [:]
    )
    #expect(vms.count == 1)
    #expect(vms[0].state == .pending)
}
