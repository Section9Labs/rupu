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

/// Fixture builder for `LaunchResponse` — `sendToSession`'s success shape.
private func launchResponse(runID: String? = "run-new", sessionID: String? = nil, ok: Bool? = nil, hostID: String = "local") -> LaunchResponse {
    LaunchResponse(runID: runID, sessionID: sessionID, ok: ok, hostID: hostID)
}

/// Fixture builder for `RunControlResponse` — reused by `archiveSession`/
/// `restoreSession`'s response shape too (see `CPClient.archiveSession`'s
/// doc comment: the real local session-archive response is `{ok, id}`, no
/// `archived` field at all, unlike run archive/restore's `RunRecord`-backed
/// one — so tests here only ever set `ok`, never `archived`).
private func runControlResponse(ok: Bool = true) -> RunControlResponse {
    RunControlResponse(run: nil, ok: ok, hostID: "local", archived: nil)
}

@MainActor
private func makeStore(
    sessionResult: @escaping @Sendable () async throws -> APISessionRow,
    runsResult: @escaping @Sendable () async throws -> [APISessionRunRow] = { [] },
    transcriptResult: @escaping @Sendable (String) async throws -> APITranscriptPage = { _ in APITranscriptPage(events: [], summary: nil) },
    sendResult: @escaping @Sendable (String) async throws -> LaunchResponse = { _ in launchResponse() },
    archiveResult: @escaping @Sendable () async throws -> RunControlResponse = { runControlResponse() },
    restoreResult: @escaping @Sendable () async throws -> RunControlResponse = { runControlResponse() },
    pendingActions: PendingActions = PendingActions()
) -> SessionDetailStore {
    SessionDetailStore(
        sessionID: "sess-1",
        fetchSession: sessionResult,
        fetchRuns: runsResult,
        fetchTranscript: transcriptResult,
        postSend: sendResult,
        postArchive: archiveResult,
        postRestore: restoreResult,
        pendingActions: pendingActions
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

/// `loadSession()` — the single-block reload the session header's
/// failed-block Retry button calls — retries only the session from a
/// `.failed` state: no runs refetch, and the user's focused run stays put
/// (contrast `activate()`, which refetches runs and refocuses the newest).
@MainActor @Test func loadSessionAloneRetriesAFailedSessionWithoutRefetchingRunsOrMovingFocus() async {
    let sessionCounter = Counter()
    let runsCounter = Counter()
    let store = makeStore(
        sessionResult: {
            sessionCounter.increment()
            if sessionCounter.value == 1 { throw StubError(description: "session down") }
            return sessionRow()
        },
        runsResult: {
            runsCounter.increment()
            return [runRow(runID: "run-1"), runRow(runID: "run-2")]
        }
    )
    await store.activate()
    guard case .failed = store.session else {
        Issue.record("expected session to be .failed after activate, got \(store.session)")
        return
    }
    #expect(store.focusedRunID == "run-2")
    await store.focusRun(runRow(runID: "run-1"))
    #expect(store.focusedRunID == "run-1")

    await store.loadSession()

    #expect(store.session.value != nil)
    #expect(store.focusedRunID == "run-1") // never re-focused
    #expect(runsCounter.value == 1) // activate's one fetch — the retry never refetched runs
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

// MARK: - (f) cancellation is benign (hotfix root cause B)

/// `session`/`runs` each independently leave their `BlockState` untouched
/// (still `.loading`, never `.failed`) when their fetch closure is
/// cancelled — the shape a superseded `SessionDetailScreen.task(id:
/// sessionID)` produces mid-load.
@MainActor @Test func activateWithCancelledFetchesLeavesSessionAndRunsAsLoadingNeverFailed() async {
    let store = makeStore(
        sessionResult: { throw CancellationError() },
        runsResult: { throw CancellationError() }
    )

    await store.activate()

    guard case .loading = store.session else {
        Issue.record("expected session to stay .loading through cancellation, got \(store.session)")
        return
    }
    guard case .loading = store.runs else {
        Issue.record("expected runs to stay .loading through cancellation, got \(store.runs)")
        return
    }
}

/// `focusRun`'s transcript fetch, cancelled, must leave
/// `transcript`/`focusedRunID` exactly as they were.
@MainActor @Test func focusRunWithCancelledTranscriptFetchLeavesTranscriptAndFocusedRunIDUntouched() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: { [] },
        transcriptResult: { path in
            if path == "t/a.jsonl" {
                return APITranscriptPage(events: [.assistantMessage(content: "A snapshot", thinking: nil)], summary: nil)
            }
            throw CancellationError()
        }
    )
    await store.activate()
    await store.focusRun(runRow(runID: "a", transcriptPath: "t/a.jsonl"))
    #expect(store.focusedRunID == "a")
    #expect(store.transcript == [.assistantMessage(content: "A snapshot", thinking: nil)])

    await store.focusRun(runRow(runID: "b", transcriptPath: "t/b.jsonl"))

    // Cancellation left it exactly where it was — still "a".
    #expect(store.focusedRunID == "a")
    #expect(store.transcript == [.assistantMessage(content: "A snapshot", thinking: nil)])
}

// MARK: - (g) send — Phase 3, Task 6

/// Happy path: `begin` → POST succeeds with `{run_id}` → `confirm`, `runs`
/// refreshed, and the store focuses the newly-sent run — its transcript is
/// the reply surface, per the store's doc comment.
@MainActor @Test func sendHappyPathConfirmsRefreshesRunsAndFocusesTheNewRun() async {
    let runsBox = FlagBox()
    let store = makeStore(
        sessionResult: { sessionRow() },
        runsResult: {
            if runsBox.value {
                return [runRow(runID: "run-1", transcriptPath: "t/run-1.jsonl"), runRow(runID: "run-new", transcriptPath: "t/run-new.jsonl")]
            }
            return [runRow(runID: "run-1", transcriptPath: "t/run-1.jsonl")]
        },
        transcriptResult: { path in APITranscriptPage(events: [.assistantMessage(content: "content at \(path)", thinking: nil)], summary: nil) },
        sendResult: { prompt in
            #expect(prompt == "hello there")
            runsBox.value = true
            return launchResponse(runID: "run-new")
        }
    )
    await store.activate()
    #expect(store.focusedRunID == "run-1")

    await store.send("  hello there  ")

    let key = ActionKey("sess-1", .send)
    #expect(store.pendingActions.state(key) == .confirmed)
    #expect(store.runs.value?.map(\.runID) == ["run-1", "run-new"])
    #expect(store.focusedRunID == "run-new")
    #expect(store.transcript == [.assistantMessage(content: "content at t/run-new.jsonl", thinking: nil)])
}

private final class FlagBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v = false
    var value: Bool {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

/// Empty (or whitespace-only) prompt is a no-op — no POST fires, no key is
/// ever `begin()`-ed.
@MainActor @Test func sendWithEmptyOrWhitespaceOnlyPromptIsANoOp() async {
    let sendCalls = Counter()
    let store = makeStore(
        sessionResult: { sessionRow() },
        sendResult: { _ in
            sendCalls.increment()
            return launchResponse()
        }
    )
    await store.activate()

    await store.send("")
    await store.send("   \n\t  ")

    #expect(sendCalls.value == 0)
    let key = ActionKey("sess-1", .send)
    #expect(store.pendingActions.state(key) == .idle)
}

/// A 409 "session ... is stopped" failure fails the pending key with the
/// server's message surfaced verbatim (not paraphrased into a canned
/// string — that's the 501 launch-runtime case only, see
/// `mutationErrorMessage`).
@MainActor @Test func sendFailureFromAStoppedSessionFailsTheKeyWithTheServerMessage() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        sendResult: { _ in throw CPError.http(status: 409, body: #"{"error":"session sess-1 is stopped"}"#) }
    )
    await store.activate()

    await store.send("hello")

    let key = ActionKey("sess-1", .send)
    guard case .failed(let message) = store.pendingActions.state(key) else {
        Issue.record("expected .failed, got \(store.pendingActions.state(key))")
        return
    }
    #expect(message.contains("is stopped"))
}

// MARK: - (h) archive/restore — Phase 3, Task 6, mirrors `RunDetailStore`

/// The real local session-archive response carries no `archived` field
/// (`{ok: true, id}` — see `CPClient.archiveSession`'s doc comment), so
/// confirmation here is off `response.ok`, not `response.archived`.
@MainActor @Test func archiveConfirmsFromResponseOkAndFlipsIsArchived() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        archiveResult: { runControlResponse(ok: true) }
    )
    await store.activate()
    #expect(!store.isArchived)

    await store.archive()

    let key = ActionKey("sess-1", .archive)
    #expect(store.pendingActions.state(key) == .confirmed)
    #expect(store.isArchived)
}

@MainActor @Test func restoreConfirmsFromResponseOkAndFlipsIsArchivedBack() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        archiveResult: { runControlResponse(ok: true) },
        restoreResult: { runControlResponse(ok: true) }
    )
    await store.activate()
    await store.archive()
    #expect(store.isArchived)

    await store.restore()

    let key = ActionKey("sess-1", .restore)
    #expect(store.pendingActions.state(key) == .confirmed)
    #expect(!store.isArchived)
}

@MainActor @Test func archiveFailureFailsTheKeyAndLeavesIsArchivedFalse() async {
    let store = makeStore(
        sessionResult: { sessionRow() },
        archiveResult: { throw StubError(description: "archive endpoint down") }
    )
    await store.activate()

    await store.archive()

    let key = ActionKey("sess-1", .archive)
    guard case .failed(let message) = store.pendingActions.state(key) else {
        Issue.record("expected .failed, got \(store.pendingActions.state(key))")
        return
    }
    #expect(message.contains("archive endpoint down"))
    #expect(!store.isArchived)
}
