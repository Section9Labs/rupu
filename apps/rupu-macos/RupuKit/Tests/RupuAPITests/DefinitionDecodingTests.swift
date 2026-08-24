import Testing
import Foundation
@testable import RupuAPI

@Test func decodesAgentDefinitionsFixture() throws {
    let defs = try JSONDecoder().decode([AgentDefinition].self, from: Fixtures.data("agent_defs.json"))
    #expect(defs.count == 2)

    let reviewer = defs[0]
    #expect(reviewer.name == "code-reviewer")
    #expect(reviewer.slug == "code-reviewer")
    #expect(reviewer.description == "Reviews code for correctness and style")
    #expect(reviewer.provider == "anthropic")
    #expect(reviewer.model == "claude-sonnet-4-6")
    #expect(reviewer.effort == "Medium")
    #expect(reviewer.maxTokens == 8192)
    #expect(reviewer.tools == ["Read", "Grep", "Edit"])
    #expect(reviewer.scope == "global")
    #expect(reviewer.scopeKind == "global")
    #expect(reviewer.scopeID == nil)
    #expect(reviewer.runCount == 3)
    #expect(reviewer.lastRun == "2026-08-20T12:00:00+00:00")

    let triage = defs[1]
    #expect(triage.description == nil)
    #expect(triage.provider == nil)
    #expect(triage.model == nil)
    #expect(triage.effort == nil)
    #expect(triage.maxTokens == nil)
    #expect(triage.tools == [])
    #expect(triage.scope == "widgets")
    #expect(triage.scopeKind == "project")
    #expect(triage.scopeID == "ws_a")
    #expect(triage.runCount == 0)
    #expect(triage.lastRun == nil)
}

@Test func decodesWorkflowDefinitionsFixture() throws {
    let defs = try JSONDecoder().decode([WorkflowDefinition].self, from: Fixtures.data("workflow_defs.json"))
    #expect(defs.count == 2)

    let nightly = defs[0]
    #expect(nightly.name == "nightly-health")
    #expect(nightly.scope == "global")
    #expect(nightly.scopeKind == "global")
    #expect(nightly.scopeID == nil)
    #expect(nightly.runCount == 5)
    #expect(nightly.lastRun == "2026-08-20T12:00:00+00:00")
    #expect(nightly.autoflowEnabled == true)

    let adhoc = defs[1]
    #expect(adhoc.name == "adhoc-build")
    #expect(adhoc.scopeID == "ws_a")
    #expect(adhoc.runCount == 0)
    #expect(adhoc.lastRun == nil)
    #expect(adhoc.autoflowEnabled == nil)
}

@Test func decodesWorkflowDetailFixtureWithBothInputs() throws {
    let detail = try JSONDecoder().decode(WorkflowDetail.self, from: Fixtures.data("workflow_detail.json"))
    #expect(detail.name == "nightly-health")
    #expect(detail.yaml.contains("name: nightly-health"))
    #expect(detail.inputs.count == 2)

    let branch = try #require(detail.inputs["branch"])
    #expect(branch.type == "string")
    #expect(branch.required)
    #expect(branch.default == nil)
    #expect(branch.allowedValues == ["main", "staging"])

    let target = try #require(detail.inputs["target"])
    #expect(target.type == "string")
    #expect(!target.required)
    #expect(target.default == "production")
    #expect(target.allowedValues == [])
}

@Test func decodesToolsFixtureIgnoringInputSchema() throws {
    let response = try JSONDecoder().decode(ToolsListResponse.self, from: Fixtures.data("tools.json"))
    #expect(response.tools.count == 2)
    #expect(response.tools[0].name == "scm.repos.list")
    #expect(response.tools[0].kind == "read")
    #expect(response.tools[1].name == "scm.branches.create")
    #expect(response.tools[1].kind == "write")
}

@Test func decodesHostsFixtureWithVersionAndActiveRunCount() throws {
    let hosts = try JSONDecoder().decode([APIHostRow].self, from: Fixtures.data("hosts.json"))
    #expect(hosts.count == 3)

    let local = hosts[0]
    #expect(local.id == "local")
    #expect(local.version == "0.74.0")
    #expect(local.activeRunCount == 2)

    let mini = hosts[1]
    #expect(mini.activeRunCount == 0)
    #expect(mini.lastSeenAt == "2026-08-20T12:00:00Z")

    let kuki = hosts[2]
    #expect(kuki.status == "offline")
    #expect(kuki.version == nil)
    #expect(kuki.activeRunCount == 0)
}
