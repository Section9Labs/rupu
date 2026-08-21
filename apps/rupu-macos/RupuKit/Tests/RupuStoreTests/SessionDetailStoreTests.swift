import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test doubles

/// Thread-safe call counter — same rationale as `ActivityStoreTests.Counter`
/// / `RunDetailStoreTests.Counter`: a plain captured `var` can't cross into a
/// `@Sendable` fetch closure under Swift 6 strict concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    func increment() { lock.withLock { v += 1 } }
    var value: Int { lock.withLock { v } }
}

private final class PathLog: @unchecked Sendable {
    private let lock = NSLock()
    private var v: [String] = []
    func record(_ path: String) { lock.withLock { v.append(path) } }
    var snapshot: [String] { lock.withLock { v } }
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - Fixture builders

private func sessionRow(
    sessionID: String = "sess-1",
    activeRunID: String? = nil,
    lastError: String? = nil
) -> APISessionRow {
    APISessionRow(
        sessionID: sessionID, agentName: "rupuso", model: "claude-sonnet-4-6", providerName: "anthropic",
        totalTurns: 3, totalTokensIn: 5000, totalTokensOut: 1200, totalTokensCached: 300,
        createdAt: "2026-08-20T11:00:00Z", updatedAt: "2026-08-20T12:00:00Z",
        activeRunID: activeRunID, lastError: lastError, target: "main", workspaceID: "ws-1",
        scope: "active", usage: nil, hostID: "local"
    )
}

private func runRow(
    runID: String,
    prompt: String = "hello",
    transcriptPath: String = "t/\(UUID().uuidString).jsonl",
    status: String? = "ok",
    error: String? = nil
) -> APISessionRunRow {
    APISessionRunRow(
        runID: runID, prompt: prompt, transcriptPath: transcriptPath, status: status,
        startedAt: "2026-08-20T11:00:00Z", completedAt: "2026-08-20T11:01:00Z",
        tokensIn: 100, tokensOut: 200, durationMS: 1000, error: error
    )
}

@MainActor
private func makeStore(
    sessionResult: @escaping @Sendable () async throws -> APISessionRow,
    runsResult: @escaping @Sendable () async throws -> [APISessionRunRow] = { [] },
    transcriptResult: @escaping @Sendable (String) async throws -> APITranscriptPage = { _ in APITranscriptPage(events: [], summary: nil) }
) -> SessionDetailStore {
    SessionDetailStore(
        sessionID: "sess-1",
        fetchSession: sessionResult,
        fetchRuns: runsResult,
        fetchTranscript: transcriptResult
    )
}

// MARK: - (a) activate populates session + runs (fixture-driven for session)

@MainActor @Test func activatePopulatesSessionFromFixtureAndRunsFromFakeClosure() async throws {
    let sessionRows = try JSONDecoder().decode([APISessionRow].self, from: Fixtures.data("session_rows.json"))
    let fixtureSession = try #require(sessionRows.first)

    let runs = [runRow(runID: "run-1"), runRow(runID: "run-2")]
    let store = makeStore(
        sessionResult: { fixtureSession },
        runsResult: { runs }
    )

    await store.activate()

    #expect(store.session.value == fixtureSession)
    #expect(store.runs.value?.map(\.runID) == ["run-1", "run-2"])
}

@MainActor @Test func activateOneBlockFailingLeavesTheOtherAsItResolved() async {
    let store = makeStore(
        sessionResult: { throw StubError(description: "session endpoint down") },
        runsResult: { [runRow(runID: "run-1")] }
    )

    await store.activate()

    guard case .failed(let message) = store.session else {
        Issue.record("expected session to be .failed, got \(store.session)")
        return
    }
    #expect(message.contains("session endpoint down"))
    #expect(store.runs.value?.map(\.runID) == ["run-1"])
}

@MainActor @Test func activateWithNoRunsLeavesRunsEmptyAndNeverFocuses() async {
    let transcriptCalls = Counter()
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: { [] },
        transcriptResult: { _ in
            transcriptCalls.increment()
            return APITranscriptPage(events: [], summary: nil)
        }
    )

    await store.activate()

    guard case .empty = store.runs else {
        Issue.record("expected runs to be .empty, got \(store.runs)")
        return
    }
    #expect(transcriptCalls.value == 0)
    #expect(store.focusedRunID == nil)
    #expect(store.transcript.isEmpty)
}

// MARK: - (b) activate's default focus is the newest run (last in array order)

@MainActor @Test func activateFocusesTheNewestRunByDefault() async {
    let requestedPaths = PathLog()
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: {
            [
                runRow(runID: "run-1", transcriptPath: "t/run-1.jsonl"),
                runRow(runID: "run-2", transcriptPath: "t/run-2.jsonl"),
                runRow(runID: "run-3", transcriptPath: "t/run-3.jsonl"),
            ]
        },
        transcriptResult: { path in
            requestedPaths.record(path)
            return APITranscriptPage(events: [.assistantMessage(content: "hi from \(path)", thinking: nil)], summary: nil)
        }
    )

    await store.activate()

    #expect(requestedPaths.snapshot == ["t/run-3.jsonl"])
    #expect(store.focusedRunID == "run-3")
    #expect(store.transcript == [.assistantMessage(content: "hi from t/run-3.jsonl", thinking: nil)])
}

// MARK: - (c) focusRun loads that run's transcript snapshot

@MainActor @Test func focusRunLoadsTheGivenRunsTranscriptSnapshot() async {
    let requestedPaths = PathLog()
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: {
            [
                runRow(runID: "run-1", transcriptPath: "t/run-1.jsonl"),
                runRow(runID: "run-2", transcriptPath: "t/run-2.jsonl"),
            ]
        },
        transcriptResult: { path in
            requestedPaths.record(path)
            if path == "t/run-1.jsonl" {
                return APITranscriptPage(events: [.assistantMessage(content: "run one", thinking: nil)], summary: nil)
            }
            return APITranscriptPage(events: [.assistantMessage(content: "run two", thinking: nil)], summary: nil)
        }
    )

    await store.activate() // default-focuses run-2 (newest)
    #expect(store.focusedRunID == "run-2")

    await store.focusRun(runRow(runID: "run-1", transcriptPath: "t/run-1.jsonl"))

    #expect(store.focusedRunID == "run-1")
    #expect(store.transcript == [.assistantMessage(content: "run one", thinking: nil)])
    #expect(requestedPaths.snapshot == ["t/run-2.jsonl", "t/run-1.jsonl"])
}

@MainActor @Test func focusRunTranscriptFailureBlanksTheFeedRatherThanLeavingStaleEvents() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: { [] },
        transcriptResult: { _ in throw StubError(description: "transcript endpoint down") }
    )

    await store.activate()
    await store.focusRun(runRow(runID: "run-9", transcriptPath: "t/run-9.jsonl"))

    #expect(store.focusedRunID == "run-9")
    #expect(store.transcript.isEmpty)
}

// MARK: - (d) overlapping focusRun calls — only the most recent one applies

@MainActor @Test func overlappingFocusRunCallsOnlyTheMostRecentOneAppliesItsResult() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: { [] },
        transcriptResult: { path in
            if path == "t/a.jsonl" {
                // Deliberately slow: still in flight when "b"'s call starts.
                try? await Task.sleep(for: .milliseconds(80))
                return APITranscriptPage(events: [.assistantMessage(content: "A snapshot", thinking: nil)], summary: nil)
            }
            return APITranscriptPage(events: [.assistantMessage(content: "B snapshot", thinking: nil)], summary: nil)
        }
    )

    await store.activate() // no runs -> no auto-focus, no overlap yet

    async let first: Void = store.focusRun(runRow(runID: "a", transcriptPath: "t/a.jsonl"))
    try? await Task.sleep(for: .milliseconds(10)) // let the first call actually begin its slow fetch
    async let second: Void = store.focusRun(runRow(runID: "b", transcriptPath: "t/b.jsonl"))
    _ = await (first, second)

    // Outlive "a"'s slow fetch, in case it's still resolving.
    try? await Task.sleep(for: .milliseconds(120))

    #expect(store.focusedRunID == "b")
    #expect(store.transcript == [.assistantMessage(content: "B snapshot", thinking: nil)])
}

// MARK: - (e) a runs row carrying an error shows up in the list model

@MainActor @Test func runsRowWithErrorSurfacesInTheListModel() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: {
            [
                runRow(runID: "run-1", status: "ok"),
                runRow(runID: "run-2", status: "error", error: "provider: API error 401"),
            ]
        }
    )

    await store.activate()

    let rows = store.runs.value ?? []
    let failed = rows.first { $0.runID == "run-2" }
    #expect(failed?.status == "error")
    #expect(failed?.error == "provider: API error 401")
}
