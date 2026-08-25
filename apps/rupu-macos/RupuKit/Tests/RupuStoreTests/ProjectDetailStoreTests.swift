import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test doubles

/// Thread-safe call counter/log — same rationale as
/// `SessionDetailStoreTests.Counter`/`PathLog`: a plain captured `var`
/// can't cross into a `@Sendable` fetch closure under Swift 6 strict
/// concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

private final class LimitLog: @unchecked Sendable {
    private let lock = NSLock()
    private var v: [Int] = []
    func record(_ limit: Int) { lock.withLock { v.append(limit) } }
    var snapshot: [Int] { lock.withLock { v } }
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - Fixture builders

private func usage(costUSD: Double? = 0.5) -> APIUsageSummary {
    APIUsageSummary(inputTokens: 100, outputTokens: 50, cachedTokens: 0, totalTokens: 150, costUSD: costUSD, priced: costUSD != nil, runs: 1)
}

private func projectRow(wsID: String = "ws-1") -> APIProjectRow {
    APIProjectRow(wsID: wsID, name: "rupu", runCount: 14, lastRunAt: "2026-08-20T12:00:00Z", usage: usage())
}

private func projectDetail(wsID: String = "ws-1") -> APIProjectDetail {
    APIProjectDetail(
        project: projectRow(wsID: wsID),
        runs: APIProjectRunsSummary(total: 14, running: 1, byStatus: ["completed": 10], bySurface: APIProjectRunsBySurface(workflow: 9, autoflow: 5)),
        sessions: APIProjectSessionsSummary(total: 3, active: 1),
        coverage: APIProjectCoverageSummary(targets: 4, findings: 7),
        recentRuns: [],
        usage: usage()
    )
}

private func runRow(id: String) -> APIRunListRow {
    APIRunListRow(
        id: id, workflowName: "nightly-health", status: "completed", startedAt: "2026-08-20T12:00:00Z",
        finishedAt: "2026-08-20T12:06:00Z", trigger: "cron", usage: usage(), turns: 4, durationMS: 360_000, hostID: "local"
    )
}

private func sessionRow(id: String) -> APISessionRow {
    APISessionRow(
        sessionID: id, agentName: "rupuso", model: "claude-sonnet-4-6", providerName: "anthropic",
        totalTurns: 3, totalTokensIn: 100, totalTokensOut: 50, totalTokensCached: 0,
        createdAt: "2026-08-20T11:00:00Z", updatedAt: "2026-08-20T12:00:00Z",
        activeRunID: nil, lastError: nil, target: "main", workspaceID: "ws-1",
        scope: "active", usage: nil, hostID: "local"
    )
}

private func agentDef(name: String) -> AgentDefinition {
    AgentDefinition(
        name: name, slug: name, description: nil, provider: nil, model: nil, effort: nil, maxTokens: nil,
        tools: [], scope: "project", scopeKind: "project", scopeID: "ws-1", runCount: 0, lastRun: nil
    )
}

private func workflowDef(name: String) -> WorkflowDefinition {
    WorkflowDefinition(name: name, scope: "project", scopeKind: "project", scopeID: "ws-1", runCount: 0, lastRun: nil, autoflowEnabled: nil)
}

private func autoflowDef(name: String) -> AutoflowDefinition {
    AutoflowDefinition(name: name, slug: name, trigger: "cron", scope: "project", scopeKind: "project", scopeID: "ws-1", enabled: true)
}

private func findings(count: Int) -> APIFindings {
    APIFindings(
        findings: (0..<count).map { i in
            APIFinding(
                id: "f-\(i)", summary: "finding \(i)", severity: "high", scope: "target", filePath: nil, lineRange: nil,
                project: "ws-1", workflowName: nil, permalink: nil, rationale: "because", declaredAt: "2026-08-20T12:00:00Z"
            )
        },
        summary: APIFindingsSummary(total: count, critical: 0, high: count, medium: 0, low: 0, info: 0)
    )
}

// MARK: - makeStore

@MainActor
private func makeStore(
    detailResult: @escaping @Sendable () async throws -> APIProjectDetail = { projectDetail() },
    runsResult: @escaping @Sendable (Int, Int) async throws -> [APIRunListRow] = { _, _ in [] },
    sessionsResult: @escaping @Sendable (Int, Int) async throws -> [APISessionRow] = { _, _ in [] },
    agentsResult: @escaping @Sendable () async throws -> [AgentDefinition] = { [] },
    workflowsResult: @escaping @Sendable () async throws -> [WorkflowDefinition] = { [] },
    autoflowsResult: @escaping @Sendable () async throws -> [AutoflowDefinition] = { [] },
    findingsResult: @escaping @Sendable () async throws -> APIFindings = { findings(count: 0) }
) -> ProjectDetailStore {
    ProjectDetailStore(
        wsID: "ws-1",
        fetchDetail: detailResult,
        fetchRuns: runsResult,
        fetchSessions: sessionsResult,
        fetchAgents: agentsResult,
        fetchWorkflows: workflowsResult,
        fetchAutoflows: autoflowsResult,
        fetchFindings: findingsResult
    )
}

// MARK: - activate() / detail

@MainActor @Test func activateLoadsDetailOnlyLeavingEveryLazyTabAtLoading() async {
    let store = makeStore()
    await store.activate()

    guard case .content(let detail) = store.detail else {
        Issue.record("expected .content")
        return
    }
    #expect(detail.project.wsID == "ws-1")
    guard case .loading = store.runs else { Issue.record("expected runs still .loading"); return }
    guard case .loading = store.sessions else { Issue.record("expected sessions still .loading"); return }
    guard case .loading = store.agents else { Issue.record("expected agents still .loading"); return }
    guard case .loading = store.workflows else { Issue.record("expected workflows still .loading"); return }
    guard case .loading = store.autoflows else { Issue.record("expected autoflows still .loading"); return }
    guard case .loading = store.findings else { Issue.record("expected findings still .loading"); return }
}

@MainActor @Test func activateDetailFailureSurfacesAsFailed() async {
    let store = makeStore(detailResult: { throw StubError(description: "detail boom") })
    await store.activate()
    guard case .failed(let message) = store.detail else {
        Issue.record("expected .failed")
        return
    }
    #expect(message.contains("detail boom"))
}

@MainActor @Test func activateCancellationLeavesDetailUntouched() async {
    let store = makeStore(detailResult: { throw CancellationError() })
    await store.activate()
    guard case .loading = store.detail else {
        Issue.record("expected .loading (untouched)")
        return
    }
}

// MARK: - Lazy per-tab loads

@MainActor @Test func loadRunsIfNeededFetchesOnceAndIsANoOpOnASecondCall() async {
    let counter = Counter()
    let store = makeStore(runsResult: { _, _ in
        counter.increment()
        return [runRow(id: "run-1")]
    })

    await store.loadRunsIfNeeded()
    #expect(counter.value == 1)
    #expect(store.runs.value?.count == 1)

    await store.loadRunsIfNeeded()
    #expect(counter.value == 1, "a second loadRunsIfNeeded() must not refetch")
}

@MainActor @Test func loadRunsIfNeededSendsTheWindowSizeAsLimit() async {
    let log = LimitLog()
    let store = makeStore(runsResult: { offset, limit in
        log.record(limit)
        #expect(offset == 0)
        return []
    })
    await store.loadRunsIfNeeded()
    #expect(log.snapshot == [ProjectDetailStore.windowSize])
    #expect(store.runsShowingAll == false)
}

@MainActor @Test func showAllRunsRefetchesWithTheShowAllLimitEvenAfterAlreadyLoaded() async {
    let log = LimitLog()
    let store = makeStore(runsResult: { _, limit in
        log.record(limit)
        return []
    })
    await store.loadRunsIfNeeded()
    await store.showAllRuns()
    #expect(log.snapshot == [ProjectDetailStore.windowSize, ProjectDetailStore.showAllLimit])
    #expect(store.runsShowingAll == true)

    // A subsequent loadRunsIfNeeded() (e.g. re-selecting the tab) stays a
    // no-op — "showing all" doesn't reset the lazy-load flag.
    await store.loadRunsIfNeeded()
    #expect(log.snapshot.count == 2)
}

@MainActor @Test func loadRunsForcesARefetchEvenWhenAlreadyRequested() async {
    let counter = Counter()
    let store = makeStore(runsResult: { _, _ in
        counter.increment()
        return []
    })
    await store.loadRunsIfNeeded()
    await store.loadRuns()
    #expect(counter.value == 2)
}

@MainActor @Test func runsFailureSurfacesIndependentlyOfDetail() async {
    let store = makeStore(
        detailResult: { projectDetail() },
        runsResult: { _, _ in throw StubError(description: "runs boom") }
    )
    await store.activate()
    await store.loadRunsIfNeeded()

    guard case .content = store.detail else { Issue.record("expected detail .content"); return }
    guard case .failed(let message) = store.runs else { Issue.record("expected runs .failed"); return }
    #expect(message.contains("runs boom"))
}

@MainActor @Test func loadSessionsIfNeededAndShowAllSessionsMirrorRunsWindowingContract() async {
    let log = LimitLog()
    let store = makeStore(sessionsResult: { _, limit in
        log.record(limit)
        return [sessionRow(id: "sess-1")]
    })
    await store.loadSessionsIfNeeded()
    #expect(log.snapshot == [ProjectDetailStore.windowSize])
    #expect(store.sessions.value?.count == 1)
    #expect(store.sessionsShowingAll == false)

    await store.showAllSessions()
    #expect(log.snapshot == [ProjectDetailStore.windowSize, ProjectDetailStore.showAllLimit])
    #expect(store.sessionsShowingAll == true)
}

@MainActor @Test func loadAgentsIfNeededFetchesOnceAndEmptyResultSetsEmpty() async {
    let counter = Counter()
    let store = makeStore(agentsResult: {
        counter.increment()
        return []
    })
    await store.loadAgentsIfNeeded()
    guard case .empty = store.agents else { Issue.record("expected .empty"); return }
    await store.loadAgentsIfNeeded()
    #expect(counter.value == 1)
}

@MainActor @Test func loadWorkflowsIfNeededFetchesOnceAndDecodesRows() async {
    let counter = Counter()
    let store = makeStore(workflowsResult: {
        counter.increment()
        return [workflowDef(name: "nightly-health")]
    })
    await store.loadWorkflowsIfNeeded()
    #expect(store.workflows.value?.map(\.name) == ["nightly-health"])
    await store.loadWorkflowsIfNeeded()
    #expect(counter.value == 1)
}

@MainActor @Test func loadAutoflowsIfNeededFetchesOnceAndDecodesRows() async {
    let counter = Counter()
    let store = makeStore(autoflowsResult: {
        counter.increment()
        return [autoflowDef(name: "issue-triage")]
    })
    await store.loadAutoflowsIfNeeded()
    #expect(store.autoflows.value?.map(\.name) == ["issue-triage"])
    await store.loadAutoflowsIfNeeded()
    #expect(counter.value == 1)
}

@MainActor @Test func loadFindingsIfNeededFetchesOnceAndDecodesSummary() async {
    let counter = Counter()
    let store = makeStore(findingsResult: {
        counter.increment()
        return findings(count: 3)
    })
    await store.loadFindingsIfNeeded()
    #expect(store.findings.value?.summary.total == 3)
    await store.loadFindingsIfNeeded()
    #expect(counter.value == 1)
}

// MARK: - activate() resets lazy flags for repeatability

@MainActor @Test func activateAgainResetsLazyFlagsSoATabRefetchesOnNextSelection() async {
    let counter = Counter()
    let store = makeStore(runsResult: { _, _ in
        counter.increment()
        return []
    })
    await store.loadRunsIfNeeded()
    #expect(counter.value == 1)

    await store.activate()
    await store.loadRunsIfNeeded()
    #expect(counter.value == 2, "activate() again must let a lazy tab refetch rather than staying latched")
}
