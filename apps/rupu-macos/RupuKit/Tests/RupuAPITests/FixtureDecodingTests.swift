import Testing
import Foundation
@testable import RupuAPI

@Test func decodesHostInfoFixture() throws {
    let info = try JSONDecoder().decode(HostInfo.self, from: Fixtures.data("host_info.json"))
    #expect(info.version == "0.71.0")
    #expect(info.capabilities.permissionModes == ["ask", "bypass", "readonly"])
}

@Test func decodesEveryEventFixtureVariant() throws {
    let events = try JSONDecoder().decode([CPEvent].self, from: Fixtures.data("events.json"))
    #expect(events.count >= 18)
    #expect(!events.contains { if case .unknown = $0 { true } else { false } })
    if case let .stepCompleted(runID, stepID, success, durationMS, host) = events[4] {
        #expect(runID == "run-01" && stepID == "plan" && success && durationMS == 4200 && host == "mini")
    } else { Issue.record("events[4] should be step_completed") }
}

@Test func unknownEventTypeDecodesAsUnknownNotError() throws {
    let json = Data(#"{"type":"future_thing","run_id":"r9"}"#.utf8)
    let ev = try JSONDecoder().decode(CPEvent.self, from: json)
    #expect(ev == .unknown(type: "future_thing", runID: "r9"))
}

@Test func decodesProjectsFixture() throws {
    let rows = try JSONDecoder().decode([APIProjectRow].self, from: Fixtures.data("projects.json"))
    #expect(!rows.isEmpty)
    #expect(rows[0].wsID == "ws-1")
    #expect(rows[0].name == "rupu")
    #expect(rows[0].runCount == 14)
    #expect(rows[0].lastRunAt == "2026-08-20T12:00:00Z")
    #expect(rows[0].path == "/Users/matt/Code/rupu")
    #expect(rows[0].repoHomeURL == "https://github.com/section9labs/rupu")
    #expect(rows[0].createdAt == "2026-08-01T09:00:00Z")
}

@Test func decodesDashboardFixture() throws {
    let dashboard = try JSONDecoder().decode(APIDashboardResponse.self, from: Fixtures.data("dashboard.json"))

    #expect(dashboard.hosts.count == 2)
    #expect(dashboard.hosts[0].hostID == "mini" && dashboard.hosts[0].state == "ok")
    #expect(dashboard.hosts[1].hostID == "kuki" && dashboard.hosts[1].state == "offline")
    #expect(dashboard.hosts[1].reason == "connection refused")

    // The poisoned field: findings_open is nil while findings_partial is true.
    #expect(dashboard.findingsOpen == nil)
    #expect(dashboard.findingsPartial == true)
    #expect(dashboard.cyclesPartial == false)
    #expect(dashboard.fleetPartial == false)

    #expect(dashboard.active.running == 3)
    #expect(dashboard.activeLongest?.runID == "run-01")
    #expect(dashboard.activeLongest?.ageMs == 120_000)

    #expect(dashboard.terminalBuckets.count == 2)
    #expect(dashboard.terminalBuckets[0].completed == 4)
    #expect(dashboard.throughputBuckets.count == 2)
    #expect(dashboard.throughputBuckets[0].cron == 3)

    #expect(dashboard.cycles.total == 10)
    #expect(dashboard.cycles.clean == 8)

    #expect(dashboard.fleet.issuesCapped == true)
    #expect(dashboard.fleet.issuesOpen == 120)
}

// MARK: - Phase 5B: global findings, coverage, usage

@Test func decodesFindingsGlobalFixtureAcrossWorkspacesWithLongFormSeverity() throws {
    let findings = try JSONDecoder().decode(APIFindings.self, from: Fixtures.data("findings_global.json"))

    #expect(findings.findings.count == 4)
    #expect(findings.summary.total == 4)
    #expect(findings.summary.critical == 1)
    #expect(findings.summary.high == 1)
    #expect(findings.summary.medium == 1)
    #expect(findings.summary.low == 0)
    #expect(findings.summary.info == 1)

    let critical = findings.findings[0]
    #expect(critical.wsID == "ws-1")
    #expect(critical.project == "rupu")
    #expect(critical.targetID == "auth-core")
    #expect(critical.workflowName == "nightly-security")
    // Long-form severity strings: exactly the wire values `Severity.
    // init(wireString:)` (`RupuDesign`) switches on (critical/high/medium/
    // low/info).
    #expect(critical.severity == "critical")
    // `declared_by` (review fix, Phase 5B Task 3): the real run linkage
    // `FindingOut`'s flattened `FindingRecord` carries — see `APIFinding`'s
    // doc comment for why an earlier pass wrongly believed this didn't
    // exist. `run_9k2f`/`claude-sonnet-4-6`/`workflow` are the fixture's
    // real values (`crates/rupu-cp/tests/macos_fixtures.rs`), not stand-ins.
    #expect(critical.declaredBy.runID == "run_9k2f")
    #expect(critical.declaredBy.model == "claude-sonnet-4-6")
    #expect(critical.declaredBy.surface == "workflow")

    let high = findings.findings[1]
    #expect(high.wsID == "ws-1")
    #expect(high.targetID == "web-api")
    #expect(high.severity == "high")
    #expect(high.filePath == nil)
    #expect(high.declaredBy.runID == "run_9k2f")
    #expect(high.declaredBy.surface == "workflow")

    // Poisoned/nil field: a workspace-2 finding with no joined workflow_name.
    let medium = findings.findings[2]
    #expect(medium.wsID == "ws-2")
    #expect(medium.project == "phi-cell")
    #expect(medium.targetID == "ml-pipeline")
    #expect(medium.workflowName == nil)
    #expect(medium.severity == "medium")
    #expect(medium.declaredBy.runID == "run_am4d")
    #expect(medium.declaredBy.surface == "workflow")

    let info = findings.findings[3]
    #expect(info.severity == "info")
    #expect(info.wsID == "ws-2")
    #expect(info.declaredBy.runID == "run_am4d")
    #expect(info.declaredBy.surface == "workflow")
}

@Test func decodesCoverageSummaryFixture() throws {
    let rows = try JSONDecoder().decode([APICoverageSummary].self, from: Fixtures.data("coverage_summary.json"))
    #expect(rows.count == 3)

    #expect(rows[0].wsID == "ws-1")
    #expect(rows[0].project == "rupu")
    #expect(rows[0].targetID == "auth-core")
    #expect(rows[0].assertionLines == 128)
    #expect(rows[0].hasCatalog == true)
    #expect(rows[0].findings == 3)

    // Poisoned/zero field: a target with no catalog and no findings at all.
    #expect(rows[1].targetID == "web-api")
    #expect(rows[1].hasCatalog == false)
    #expect(rows[1].findings == 0)

    #expect(rows[2].wsID == "ws-2")
    #expect(rows[2].project == "phi-cell")
}

@Test func decodesCoverageDetailFixtureWithAssertionsFindingsAndFileViews() throws {
    let detail = try JSONDecoder().decode(APICoverageDetail.self, from: Fixtures.data("coverage_detail.json"))

    #expect(detail.wsID == "ws-1")
    #expect(detail.project == "rupu")
    #expect(detail.targetID == "auth-core")
    #expect(detail.assertionLines == 128)
    #expect(detail.hasCatalog == true)

    #expect(detail.assertions.count == 2)
    let flagged = detail.assertions[0]
    #expect(flagged.concernID == "timing-attack")
    #expect(flagged.filePath == "src/auth/session.rs")
    #expect(flagged.status == "finding")
    #expect(flagged.evidence.findingIDs == ["fnd_critical_1"])
    #expect(flagged.evidence.lineRanges == [[42, 58]])
    #expect(flagged.declaredBy.runID == "run_9k2f")
    #expect(flagged.declaredBy.surface == "workflow")

    // Poisoned/empty field: a clean assertion whose evidence cites no findings.
    let clean = detail.assertions[1]
    #expect(clean.concernID == "sql-injection")
    #expect(clean.status == "clean")
    #expect(clean.evidence.findingIDs.isEmpty)

    #expect(detail.findings.count == 1)
    let finding = detail.findings[0]
    #expect(finding.id == "fnd_critical_1")
    #expect(finding.severity == "critical")
    #expect(finding.evidence.rationale == "non-constant-time comparison enables a timing side-channel")
    #expect(finding.evidence.references == ["https://cwe.mitre.org/data/definitions/208.html"])

    #expect(detail.files.count == 1)
    let file = detail.files[0]
    #expect(file.path == "src/auth/session.rs")
    #expect(file.strongest == "edit")
    #expect(file.touchModes == ["read", "edit"])
    #expect(file.readLines == [[1, 120], [42, 58]])
    #expect(file.edits == 1)
    #expect(file.grepMatches == 0)
    #expect(file.touchedBy.count == 1)
    #expect(file.touchedBy[0].model == "claude-sonnet-4-6")
}

@Test func decodesCoverageCatalogFixtureAsBareFlatCatalog() throws {
    let catalog = try JSONDecoder().decode(APICoverageCatalog.self, from: Fixtures.data("coverage_catalog.json"))

    #expect(catalog.concerns.count == 2)
    let timing = catalog.concerns[0]
    #expect(timing.id == "timing-attack")
    #expect(timing.name == "Timing side-channel")
    #expect(timing.severity == "critical")
    #expect(timing.applicableGlobs == ["**/auth/**"])
    #expect(timing.minStrength == "read")
    #expect(timing.tags == ["stride:spoofing"])

    // Poisoned/empty field: sql-injection's references/tags are both empty.
    let sqli = catalog.concerns[1]
    #expect(sqli.id == "sql-injection")
    #expect(sqli.references.isEmpty)
    #expect(sqli.tags.isEmpty)

    #expect(catalog.sources["timing-attack"] == "stride")
    #expect(catalog.sources["sql-injection"] == "stride")
    #expect(catalog.renderModes["timing-attack"] == "full")
    #expect(catalog.renderModes["sql-injection"] == "index")
}

@Test func decodesUsageFixtureIncludingUnpricedRowAndHostFreshness() throws {
    let usage = try JSONDecoder().decode(APIUsageResponse.self, from: Fixtures.data("usage.json"))

    #expect(usage.summary.inputTokens == 1_505_000)
    #expect(usage.summary.totalTokens == 1_810_050)
    #expect(usage.summary.costUSD == 12.71)
    // Poisoned field: fleet summary is unpriced overall (one contributing
    // model has no resolvable price) even though its cost isn't nil — a
    // partial total, not a missing one.
    #expect(usage.summary.priced == false)

    #expect(usage.breakdown.count == 3)
    let sonnet = usage.breakdown[0]
    #expect(sonnet.agent == "rupuso")
    #expect(sonnet.model == "claude-sonnet-4-6")
    #expect(sonnet.costUSD == 6.0)
    #expect(sonnet.priced == true)

    // The unpriced row itself: cost_usd is nil, never fabricated as 0.
    let unpriced = usage.breakdown[2]
    #expect(unpriced.model == "llama-3-70b")
    #expect(unpriced.provider == "internal-vllm")
    #expect(unpriced.costUSD == nil)
    #expect(unpriced.priced == false)

    #expect(usage.unpriced.models == ["llama-3-70b"])
    #expect(usage.unpriced.rows == 1)

    #expect(usage.hosts.count == 2)
    #expect(usage.hosts[0].hostID == "local" && usage.hosts[0].state == "ok")
    #expect(usage.hosts[1].hostID == "kuki" && usage.hosts[1].state == "offline")
    #expect(usage.hosts[1].reason == "connection refused")
    #expect(usage.hosts[1].capturedAt == nil)
}

@Test func decodesUsageRunsFixtureAsFlatPerRunModelRows() throws {
    let rows = try JSONDecoder().decode([APIUsageRunRow].self, from: Fixtures.data("usage_runs.json"))
    #expect(rows.count == 6)

    let first = rows[0]
    #expect(first.runID == "run-01")
    #expect(first.workflowName == "nightly-health")
    #expect(first.workspaceID == "ws-1")
    #expect(first.hostID == "local")
    #expect(first.costUSD == 0.12)
    #expect(first.priced == true)

    // Poisoned/nil field: the unpriced run's cost_usd is nil, priced is false.
    let unpriced = rows[5]
    #expect(unpriced.runID == "run-06")
    #expect(unpriced.model == "llama-3-70b")
    #expect(unpriced.costUSD == nil)
    #expect(unpriced.priced == false)
}

@Test func decodesUsageOutliersFixture() throws {
    let outliers = try JSONDecoder().decode([APIOutlierRun].self, from: Fixtures.data("usage_outliers.json"))
    #expect(outliers.count == 2)

    let worst = outliers[0]
    #expect(worst.runID == "run-06")
    #expect(worst.workflowName == "nightly-health")
    #expect(worst.costUSD == 12.5)
    #expect(worst.baselineUSD == 1.2)
    #expect(worst.ratio == 10.416666666666668)

    #expect(outliers[1].runID == "run-09")
    #expect(outliers[1].ratio == 4.0)
}
