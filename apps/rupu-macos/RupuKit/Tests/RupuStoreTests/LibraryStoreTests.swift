import Testing
import Foundation
@testable import RupuStore
import RupuAPI
import RupuDesign

// MARK: - Fixture builders

private func agentDef(
    name: String = "code-reviewer",
    scope: String = "global",
    scopeKind: String = "global",
    scopeID: String? = nil,
    mode: String? = nil
) -> AgentDefinition {
    AgentDefinition(
        name: name, slug: name, description: nil, provider: nil, model: nil, effort: nil, maxTokens: nil,
        mode: mode, tools: [], scope: scope, scopeKind: scopeKind, scopeID: scopeID, runCount: 0, lastRun: nil
    )
}

private func workflowDef(
    name: String = "nightly-health",
    scope: String = "global",
    scopeKind: String = "global",
    scopeID: String? = nil,
    autoflowEnabled: Bool? = nil
) -> WorkflowDefinition {
    WorkflowDefinition(
        name: name, scope: scope, scopeKind: scopeKind, scopeID: scopeID, runCount: 0, lastRun: nil,
        autoflowEnabled: autoflowEnabled
    )
}

private func autoflowDef(
    name: String = "nightly-health",
    scope: String = "global",
    scopeKind: String = "global",
    scopeID: String? = nil,
    enabled: Bool = true
) -> AutoflowDefinition {
    AutoflowDefinition(
        name: name, slug: name, trigger: "cron", scope: scope, scopeKind: scopeKind, scopeID: scopeID, enabled: enabled
    )
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

/// Same rationale as every other store test's `Counter` in this module.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

/// Same per-file `pollUntil`/`expectEventually` convention `FleetStoreTests`/
/// `ActivityStoreTests` establish.
@MainActor
private func pollUntil(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ condition: () -> Bool
) async -> Bool {
    let deadline = ContinuousClock.now + timeout
    while true {
        if condition() { return true }
        if ContinuousClock.now >= deadline { return false }
        try? await Task.sleep(for: interval)
    }
}

@MainActor
private func expectEventually(
    timeout: Duration = .seconds(5),
    interval: Duration = .milliseconds(10),
    _ description: String,
    sourceLocation: SourceLocation = #_sourceLocation,
    _ condition: () -> Bool
) async {
    let succeeded = await pollUntil(timeout: timeout, interval: interval, condition)
    #expect(succeeded, "timed out waiting for: \(description)", sourceLocation: sourceLocation)
}

@MainActor
private func makeStore(
    fetchAgents: @escaping @Sendable () async throws -> [AgentDefinition] = { [agentDef()] },
    fetchWorkflows: @escaping @Sendable () async throws -> [WorkflowDefinition] = { [workflowDef()] },
    fetchAutoflows: @escaping @Sendable () async throws -> [AutoflowDefinition] = { [autoflowDef()] },
    postSetAutoflowEnabled: @escaping @Sendable (String, String?, String?, Bool) async throws -> AutoflowSetEnabledResponse = {
        name, _, _, enabled in AutoflowSetEnabledResponse(name: name, enabled: enabled)
    },
    pendingActions: PendingActions = PendingActions()
) -> LibraryStore {
    LibraryStore(
        fetchAgents: fetchAgents, fetchWorkflows: fetchWorkflows, fetchAutoflows: fetchAutoflows,
        postSetAutoflowEnabled: postSetAutoflowEnabled, pendingActions: pendingActions
    )
}

// MARK: - activate() — three independent blocks

@MainActor @Test func activateLoadsAllThreeListsIntoContent() async {
    let store = makeStore(
        fetchAgents: { [agentDef(name: "a1"), agentDef(name: "a2")] },
        fetchWorkflows: { [workflowDef(name: "w1")] },
        fetchAutoflows: { [autoflowDef(name: "af1")] }
    )
    await store.activate()

    #expect(store.agents.value?.map(\.name) == ["a1", "a2"])
    #expect(store.workflows.value?.map(\.name) == ["w1"])
    #expect(store.autoflows.value?.map(\.name) == ["af1"])
}

@MainActor @Test func activateWithEmptyResultsSetsEmptyIndependentlyPerBlock() async {
    let store = makeStore(fetchAgents: { [] }, fetchWorkflows: { [] }, fetchAutoflows: { [] })
    await store.activate()

    #expect(store.agents.isEmptyState)
    #expect(store.workflows.isEmptyState)
    #expect(store.autoflows.isEmptyState)
}

/// One block failing never blanks the other two — same per-block-
/// independence contract `FleetStore`/`ProjectDetailStore` already carry.
@MainActor @Test func oneBlockFailingLeavesTheOtherTwoUnaffected() async {
    let store = makeStore(
        fetchAgents: { throw StubError(description: "agents down") },
        fetchWorkflows: { [workflowDef()] },
        fetchAutoflows: { [autoflowDef()] }
    )
    await store.activate()

    guard case .failed(let message) = store.agents else {
        Issue.record("expected agents .failed")
        return
    }
    #expect(message.contains("agents down"))
    #expect(store.workflows.value != nil)
    #expect(store.autoflows.value != nil)
}

@MainActor @Test func libraryStoreActivateIsRepeatableAndReloadsFromScratch() async {
    let counter = Counter()
    let store = makeStore(fetchAgents: {
        counter.increment() == 1 ? [agentDef(name: "first")] : [agentDef(name: "second")]
    })
    await store.activate()
    #expect(store.agents.value?.map(\.name) == ["first"])

    await store.activate()
    #expect(store.agents.value?.map(\.name) == ["second"])
}

/// `loadWorkflows()` — the narrower single-block reload `RupuBuilder.
/// SettingsTab` (the Workflow Builder's successor to the deleted
/// `RupuLibrary.WorkflowDetailScreen`, which called this same method)
/// calls instead of a full `activate()` — leaves `agents`/`autoflows` alone
/// (still `.loading`, never fetched).
@MainActor @Test func loadWorkflowsAloneNeverTouchesAgentsOrAutoflows() async {
    let agentsCounter = Counter()
    let store = makeStore(fetchAgents: {
        agentsCounter.increment()
        return [agentDef()]
    })

    await store.loadWorkflows()

    #expect(store.workflows.value != nil)
    if case .loading = store.agents {} else { Issue.record("expected agents to still be .loading") }
    if case .loading = store.autoflows {} else { Issue.record("expected autoflows to still be .loading") }
    #expect(agentsCounter.value == 0) // fetchAgents was never invoked by loadWorkflows()
}

/// `loadAgents()` — the single-block reload the Agents tab's failed-block
/// Retry button calls (public like `loadWorkflows`, promoted for exactly
/// that affordance) — retries a `.failed` block to `.content` without ever
/// fetching the sibling blocks.
@MainActor @Test func loadAgentsAloneRetriesAFailedAgentsBlock() async {
    let agentsCounter = Counter()
    let workflowsCounter = Counter()
    let store = makeStore(
        fetchAgents: {
            if agentsCounter.increment() == 1 { throw StubError(description: "agents down") }
            return [agentDef(name: "recovered")]
        },
        fetchWorkflows: {
            workflowsCounter.increment()
            return [workflowDef()]
        }
    )

    await store.loadAgents()
    guard case .failed(let message) = store.agents else {
        Issue.record("expected agents to be .failed, got \(store.agents)")
        return
    }
    #expect(message.contains("agents down"))

    await store.loadAgents()

    #expect(store.agents.value?.map(\.name) == ["recovered"])
    if case .loading = store.workflows {} else { Issue.record("expected workflows to still be .loading") }
    if case .loading = store.autoflows {} else { Issue.record("expected autoflows to still be .loading") }
    #expect(workflowsCounter.value == 0)
}

/// Same contract as `loadAgentsAloneRetriesAFailedAgentsBlock`, for the
/// Autoflows tab's failed block.
@MainActor @Test func loadAutoflowsAloneRetriesAFailedAutoflowsBlock() async {
    let autoflowsCounter = Counter()
    let agentsCounter = Counter()
    let store = makeStore(
        fetchAgents: {
            agentsCounter.increment()
            return [agentDef()]
        },
        fetchAutoflows: {
            if autoflowsCounter.increment() == 1 { throw StubError(description: "autoflows down") }
            return [autoflowDef(name: "recovered")]
        }
    )

    await store.loadAutoflows()
    guard case .failed = store.autoflows else {
        Issue.record("expected autoflows to be .failed, got \(store.autoflows)")
        return
    }

    await store.loadAutoflows()

    #expect(store.autoflows.value?.map(\.name) == ["recovered"])
    if case .loading = store.agents {} else { Issue.record("expected agents to still be .loading") }
    #expect(agentsCounter.value == 0)
}

// MARK: - setAutoflowEnabled — pending state

@MainActor @Test func setAutoflowEnabledBeginsPendingImmediately() async {
    let pendingActions = PendingActions()
    let store = makeStore(
        postSetAutoflowEnabled: { _, _, _, _ in
            try await Task.sleep(for: .seconds(60)) // never returns in this test's window
            return AutoflowSetEnabledResponse(name: "unused", enabled: false)
        },
        pendingActions: pendingActions
    )
    await store.activate()

    let key = ActionKey.autoflow(name: "nightly-health", scopeKind: "global", scopeID: nil, verb: .setEnabled)
    let task = Task { await store.setAutoflowEnabled(name: "nightly-health", scopeKind: "global", scopeID: nil, enabled: false) }
    await expectEventually("setAutoflowEnabled begins the key before the POST resolves") {
        if case .pending = pendingActions.state(key) { return true }
        return false
    }
    task.cancel()
}

/// **Immediate confirmation** — unlike `FleetStore.removeHost`'s confirm-on-
/// refetch, the response body itself is the truth; no second fetch is
/// needed or performed. Also verifies the in-place patch on `autoflows`.
@MainActor @Test func setAutoflowEnabledSuccessConfirmsImmediatelyOffTheResponseAndPatchesTheRow() async {
    let pendingActions = PendingActions()
    let store = makeStore(
        fetchAutoflows: { [autoflowDef(name: "nightly-health", scopeKind: "global", scopeID: nil, enabled: true)] },
        pendingActions: pendingActions
    )
    await store.activate()
    #expect(store.autoflows.value?.first?.enabled == true)

    await store.setAutoflowEnabled(name: "nightly-health", scopeKind: "global", scopeID: nil, enabled: false)

    let key = ActionKey.autoflow(name: "nightly-health", scopeKind: "global", scopeID: nil, verb: .setEnabled)
    #expect(pendingActions.state(key) == .confirmed)
    #expect(store.autoflows.value?.first?.enabled == false)
}

/// The row patch also reaches a matching `workflows` row (the detail
/// screen's toggle acts on `workflows`, not `autoflows`).
@MainActor @Test func setAutoflowEnabledPatchesAMatchingWorkflowsRowToo() async {
    let store = makeStore(
        fetchWorkflows: { [workflowDef(name: "nightly-health", scopeKind: "global", scopeID: nil, autoflowEnabled: true)] }
    )
    await store.activate()

    await store.setAutoflowEnabled(name: "nightly-health", scopeKind: "global", scopeID: nil, enabled: false)

    #expect(store.workflows.value?.first?.autoflowEnabled == false)
}

/// Two repos defining the same autoflow name never cross-toggle: the patch
/// only applies to the row whose `scopeKind`/`scopeID` match exactly what
/// was passed to `setAutoflowEnabled`, never a bare name match.
@MainActor @Test func setAutoflowEnabledNeverPatchesADifferentlyScopedSameNamedRow() async {
    let store = makeStore(
        fetchAutoflows: {
            [
                autoflowDef(name: "nightly-health", scope: "repo-a", scopeKind: "project", scopeID: "ws-a", enabled: true),
                autoflowDef(name: "nightly-health", scope: "repo-b", scopeKind: "project", scopeID: "ws-b", enabled: true),
            ]
        }
    )
    await store.activate()

    await store.setAutoflowEnabled(name: "nightly-health", scopeKind: "project", scopeID: "ws-a", enabled: false)

    let rows = store.autoflows.value ?? []
    #expect(rows.first(where: { $0.scopeID == "ws-a" })?.enabled == false)
    #expect(rows.first(where: { $0.scopeID == "ws-b" })?.enabled == true)
}

/// **Review fix, round 1**: the UI pending/failure ledger must isolate two
/// same-named, differently-scoped rows exactly like the data patch already
/// does (`setAutoflowEnabledNeverPatchesADifferentlyScopedSameNamedRow`
/// above covers the row content; this covers `PendingActions` itself) —
/// toggling repo A's row must never show repo B's same-named row as
/// pending, nor paint A's failure message onto B's row. Verifies both
/// directions: A's begin-pending state, and A's failure, both leave B at
/// `.idle` throughout.
@MainActor @Test func setAutoflowEnabledNeverCrossPollutesADifferentlyScopedSameNamedRowsPendingState() async {
    let pendingActions = PendingActions()
    let store = makeStore(
        fetchAutoflows: {
            [
                autoflowDef(name: "nightly-health", scope: "repo-a", scopeKind: "project", scopeID: "ws-a", enabled: true),
                autoflowDef(name: "nightly-health", scope: "repo-b", scopeKind: "project", scopeID: "ws-b", enabled: true),
            ]
        },
        postSetAutoflowEnabled: { _, _, _, _ in
            try await Task.sleep(for: .seconds(60)) // never returns in this test's window
            return AutoflowSetEnabledResponse(name: "unused", enabled: false)
        },
        pendingActions: pendingActions
    )
    await store.activate()

    let keyA = ActionKey.autoflow(name: "nightly-health", scopeKind: "project", scopeID: "ws-a", verb: .setEnabled)
    let keyB = ActionKey.autoflow(name: "nightly-health", scopeKind: "project", scopeID: "ws-b", verb: .setEnabled)
    #expect(keyA != keyB)

    let task = Task { await store.setAutoflowEnabled(name: "nightly-health", scopeKind: "project", scopeID: "ws-a", enabled: false) }
    await expectEventually("A's key begins pending before its POST resolves") {
        if case .pending = pendingActions.state(keyA) { return true }
        return false
    }
    #expect(pendingActions.state(keyB) == .idle)
    task.cancel()

    // Same isolation on the failure path: a synchronous failing POST for A
    // must never touch B's (still-untouched) ledger entry either.
    let failingStore = makeStore(
        fetchAutoflows: {
            [
                autoflowDef(name: "nightly-health", scope: "repo-a", scopeKind: "project", scopeID: "ws-a", enabled: true),
                autoflowDef(name: "nightly-health", scope: "repo-b", scopeKind: "project", scopeID: "ws-b", enabled: true),
            ]
        },
        postSetAutoflowEnabled: { _, _, _, _ in throw StubError(description: "repo A only") },
        pendingActions: pendingActions
    )
    await failingStore.activate()
    await failingStore.setAutoflowEnabled(name: "nightly-health", scopeKind: "project", scopeID: "ws-a", enabled: false)

    guard case .failed(let message) = pendingActions.state(keyA) else {
        Issue.record("expected A's key .failed, got \(pendingActions.state(keyA))")
        return
    }
    #expect(message.contains("repo A only"))
    #expect(pendingActions.state(keyB) == .idle)
}

/// A POST failure fails the key with the error message and leaves the row
/// untouched — nothing was actually toggled server-side.
@MainActor @Test func setAutoflowEnabledFailureFailsTheKeyAndLeavesTheRowUntouched() async {
    let pendingActions = PendingActions()
    let store = makeStore(
        fetchAutoflows: { [autoflowDef(name: "nightly-health", enabled: true)] },
        postSetAutoflowEnabled: { _, _, _, _ in throw StubError(description: "definition not found") },
        pendingActions: pendingActions
    )
    await store.activate()

    await store.setAutoflowEnabled(name: "nightly-health", scopeKind: "global", scopeID: nil, enabled: false)

    let key = ActionKey.autoflow(name: "nightly-health", scopeKind: "global", scopeID: nil, verb: .setEnabled)
    guard case .failed(let message) = pendingActions.state(key) else {
        Issue.record("expected .failed, got \(pendingActions.state(key))")
        return
    }
    #expect(message.contains("definition not found"))
    #expect(store.autoflows.value?.first?.enabled == true)
}

// MARK: - agentPermissionTone(mode:) — pure mapping

@MainActor @Test func agentPermissionToneMapsEachKnownModeToTheUmbrellaToneRule() {
    #expect(agentPermissionTone(mode: "readonly") == .done)
    #expect(agentPermissionTone(mode: "ask") == .awaiting)
    #expect(agentPermissionTone(mode: "bypass") == .failed)
}

/// `nil` (no frontmatter override) never guesses a tone — see the
/// function's doc comment for why asserting "ask" here would be dishonest.
@MainActor @Test func agentPermissionToneReturnsNilForNoOverride() {
    #expect(agentPermissionTone(mode: nil) == nil)
}

/// An unrecognized string (a value `rupu_agent::permission::parse_mode`
/// wouldn't recognize either) gets the same "no tone" treatment as `nil` —
/// never a guessed/default tone for garbage input.
@MainActor @Test func agentPermissionToneReturnsNilForAnUnrecognizedString() {
    #expect(agentPermissionTone(mode: "somethingElse") == nil)
}

// MARK: - BlockState test helper

private extension BlockState {
    var isEmptyState: Bool {
        if case .empty = self { return true }
        return false
    }
}
