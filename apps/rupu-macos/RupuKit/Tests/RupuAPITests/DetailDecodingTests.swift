import Testing
import Foundation
@testable import RupuAPI

@Test func decodesRunDetailFixtureWithAwaitingGate() throws {
    let detail = try JSONDecoder().decode(APIRunDetail.self, from: Fixtures.data("run_detail.json"))
    #expect(detail.run.id == "run-01")
    #expect(detail.run.status == "awaiting_approval")
    #expect(detail.run.workflowName == "nightly-health")
    #expect(detail.run.workspaceID == "ws-1")
    #expect(detail.run.finishedAt == nil)
    #expect(detail.run.awaiting.count == 1)
    #expect(detail.run.awaiting[0].stepID == "gate")
    #expect(detail.run.awaiting[0].prompt == "deploy?")
    #expect(detail.run.permissionMode == "ask")
    #expect(detail.steps.count == 2)
    #expect(detail.steps[0].stepID == "plan")
    #expect(detail.steps[0].success)
    #expect(!detail.steps[0].skipped)
    #expect(detail.steps[0].kind == "linear")
    #expect(detail.steps[0].iterations == 0)
    #expect(detail.steps[1].kind == "panel")
    #expect(detail.steps[1].iterations == 2)
    #expect(detail.usage.totalTokens == 6200)
}

@Test func decodesRunGraphFixtureWithAllSevenNodeKinds() throws {
    let graph = try JSONDecoder().decode(APIRunGraph.self, from: Fixtures.data("run_graph.json"))
    #expect(graph.run.id == "run-02")
    #expect(graph.run.awaiting.isEmpty) // key absent in fixture -> defaults to []
    #expect(graph.stepResults.count == 1)
    #expect(graph.stepResults[0].kind == "linear")
    #expect(graph.stepResults[0].iterations == 0) // key absent -> defaults to 0

    #expect(graph.workflow.steps.count == 7)
    let byID = Dictionary(uniqueKeysWithValues: graph.workflow.steps.map { ($0.id, $0) })

    let step = try #require(byID["plan"])
    #expect(step.kind == "step")
    #expect(step.agent == "rupuso")

    let forEach = try #require(byID["fan"])
    #expect(forEach.kind == "for_each")
    #expect(forEach.forEach == "{{ files }}")
    #expect(forEach.agent == "reviewer")

    let parallel = try #require(byID["par"])
    #expect(parallel.kind == "parallel")
    #expect(parallel.parallel?.count == 2)
    #expect(parallel.parallel?[0].id == "par_a")
    #expect(parallel.parallel?[0].agent == "alpha")

    let panel = try #require(byID["review"])
    #expect(panel.kind == "panel")
    #expect(panel.panelists == ["panelist-a", "panelist-b"])
    #expect(panel.gate?.maxIterations == 5)
    #expect(panel.gate?.untilSeverity == "high")
    #expect(panel.gate?.fixWith == "fixer")

    let gate = try #require(byID["approve"])
    #expect(gate.kind == "gate")
    #expect(gate.approvalGate?.autoApprove == true)
    #expect(gate.approvalGate?.hasOnReject == true)
    #expect(gate.approvalGate?.timeoutSeconds == 3600)

    let action = try #require(byID["create_pr"])
    #expect(action.kind == "action")
    #expect(action.action == "scm.prs.create")

    let run = try #require(byID["build"])
    #expect(run.kind == "run")
    #expect(run.action == "cargo build")
    #expect(run.forEach == "{{ targets }}")

    #expect(graph.units.count == 2)
    #expect(graph.units[0].success == true)
    #expect(graph.units[0].runID == "sub_01")
    #expect(graph.units[1].success == nil) // synthesized unit: success is null
    #expect(graph.units[1].runID == nil)
    #expect(graph.usage.totalTokens == 3700)
}

@Test func decodesNetflowFixtureWithTransportErrorAndRollupErrors() throws {
    let netflow = try JSONDecoder().decode(APINetflow.self, from: Fixtures.data("netflow_run.json"))
    #expect(netflow.flows.count == 2)
    #expect(netflow.asnLoaded)
    #expect(netflow.droppedTotal == 0)

    let ok = netflow.flows[0]
    #expect(ok.outcome == "ok")
    #expect(ok.error == nil)
    #expect(ok.asn?.asn == 15169)
    #expect(ok.asn?.org == "Google LLC")
    #expect(ok.ctx.runID == "run-40")

    let transportError = netflow.flows[1]
    #expect(transportError.outcome == "transport_error")
    #expect(transportError.error == "connection reset")
    #expect(transportError.status == nil)

    let githubRollup = try #require(netflow.hosts.first { $0.host == "api.github.com" })
    #expect(githubRollup.errors == 3)
    #expect(githubRollup.bytesIn == nil)

    let anthropicRollup = try #require(netflow.hosts.first { $0.host == "api.anthropic.com" })
    #expect(anthropicRollup.errors == 0)
    #expect(anthropicRollup.p50MS == 30)
}

@Test func decodesProjectDetailFixtureWithCountsBlocksAndRecentRuns() throws {
    let detail = try JSONDecoder().decode(APIProjectDetail.self, from: Fixtures.data("project_detail.json"))

    // `project` reuses APIProjectRow's partial decode — the fixture's
    // ProjectRow carries more fields (repo_remote, branch, last_active) than
    // that type decodes, and unknown keys are ignored; path/repo_home_url/
    // created_at ARE decoded (final-review fix wave — the Project Detail
    // header's second facts row needs them).
    #expect(detail.project.wsID == "ws-1")
    #expect(detail.project.name == "rupu")
    #expect(detail.project.runCount == 14)
    #expect(detail.project.path == "/Users/matt/Code/rupu")
    #expect(detail.project.repoHomeURL == "https://github.com/section9labs/rupu")
    #expect(detail.project.createdAt == "2026-08-01T09:00:00Z")

    #expect(detail.runs.total == 14)
    #expect(detail.runs.running == 1)
    #expect(detail.runs.byStatus["completed"] == 10)
    #expect(detail.runs.byStatus["failed"] == 2)
    #expect(detail.runs.byStatus["awaiting_approval"] == 1)
    #expect(detail.runs.bySurface.workflow == 9)
    #expect(detail.runs.bySurface.autoflow == 5)

    #expect(detail.sessions.total == 3)
    #expect(detail.sessions.active == 1)

    #expect(detail.coverage.targets == 4)
    #expect(detail.coverage.findings == 7)

    #expect(detail.recentRuns.count == 2)
    #expect(detail.recentRuns[0].id == "run-01")
    #expect(detail.recentRuns[0].hostID == nil) // project routes never inject host_id
    #expect(detail.recentRuns[1].status == "running")

    #expect(detail.usage.totalTokens == 6200)
}

@Test func decodesFindingsFixtureWithSeverityStringsAndRepoScopeNilFilePath() throws {
    let findings = try JSONDecoder().decode(APIFindings.self, from: Fixtures.data("findings_run.json"))
    #expect(findings.findings.count == 2)
    #expect(findings.summary.total == 2)
    #expect(findings.summary.critical == 1)
    #expect(findings.summary.info == 1)

    let critical = findings.findings[0]
    #expect(critical.severity == "critical")
    #expect(critical.scope == "line")
    #expect(critical.filePath == "src/a.rs")
    #expect(critical.lineRange == [17, 19])
    #expect(critical.rationale == "unwrap on an Option that can be None in production")
    #expect(critical.permalink != nil)

    let repoScoped = findings.findings[1]
    #expect(repoScoped.severity == "info")
    #expect(repoScoped.scope == "repo")
    #expect(repoScoped.filePath == nil)
    #expect(repoScoped.workflowName == nil)
    #expect(repoScoped.rationale == "repository has no .github/workflows directory")
}
