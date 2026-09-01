import Testing
import Foundation
@testable import RupuStore
import RupuAPI

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

/// Path-routing HTTP stub, single-purpose for test (j) below. Duplicated
/// from `LauncherStubURLProtocol`/`ActivityStubURLProtocol` rather than
/// reused — both those types' doc comments establish this codebase's "fresh
/// copy per file" convention: their `handler`/`pathHits` are
/// `nonisolated(unsafe) static var`s, so sharing one `URLProtocol` subclass
/// across test files would race whenever Swift Testing schedules two
/// different files' suites concurrently (their own internal `.serialized`
/// traits only protect against races *within* a suite, not across files).
final class AgentRunDetailStubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?

    static func session() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [AgentRunDetailStubURLProtocol.self]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        self.handler = handler
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let handler = AgentRunDetailStubURLProtocol.handler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let (status, body) = handler(request)
        let response = HTTPURLResponse(
            url: request.url!, statusCode: status, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// Thread-safe mutable flag — same rationale as every other test file in
/// this target: a plain captured `var` can't cross into a `@Sendable` fetch
/// closure under Swift 6 strict concurrency.
private final class FlagBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v = false
    var value: Bool {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

/// Thread-safe call counter — same Sendable rationale as `FlagBox`, for
/// closures whose behavior differs between the first and second call
/// (tests (k)/(l)'s "retry while the backend is still erroring" shape).
private final class CounterBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    func next() -> Int {
        lock.withLock {
            v += 1
            return v
        }
    }
}

@MainActor
private func makeStore(
    transcriptPath: String?,
    fetchTranscript: @escaping @Sendable (String) async throws -> APITranscriptPage = { _ in
        APITranscriptPage(events: [], summary: nil)
    }
) -> AgentRunDetailStore {
    AgentRunDetailStore(transcriptPath: transcriptPath, fetchTranscript: fetchTranscript)
}

// MARK: - (a) a nil transcriptPath never touches the network — honest empty
// state, not a spinner that would never resolve.

@MainActor @Test func nilTranscriptPathGoesStraightToEmptyWithoutFetching() async {
    let fetched = FlagBox()
    let store = makeStore(transcriptPath: nil, fetchTranscript: { _ in
        fetched.value = true
        return APITranscriptPage(events: [], summary: nil)
    })

    await store.activate()

    guard case .empty = store.transcript else {
        Issue.record("expected .empty for a nil transcriptPath, got \(store.transcript)")
        return
    }
    #expect(fetched.value == false)
}

// MARK: - (b) a real transcriptPath fetches and populates content

@MainActor @Test func realTranscriptPathFetchesAndPopulatesContent() async {
    let store = makeStore(
        transcriptPath: "t/run-99.jsonl",
        fetchTranscript: { path in
            #expect(path == "t/run-99.jsonl")
            return APITranscriptPage(events: [.assistantMessage(content: "hello", thinking: nil)], summary: nil)
        }
    )

    await store.activate()

    guard case .content(let events) = store.transcript else {
        Issue.record("expected .content, got \(store.transcript)")
        return
    }
    #expect(events == [.assistantMessage(content: "hello", thinking: nil)])
}

/// Plan 3, Task 2: `transcriptUnparsedCount` surfaces the fetched page's
/// `unparsed` field, the same signal `RunDetailStore.transcriptUnparsedCount`
/// exposes for the orchestrator-run transcript tab.
@MainActor @Test func activatePopulatesTranscriptUnparsedCountFromThePage() async {
    let store = makeStore(
        transcriptPath: "t/run-99.jsonl",
        fetchTranscript: { _ in
            APITranscriptPage(events: [.assistantMessage(content: "hello", thinking: nil)], summary: nil, unparsed: 2)
        }
    )

    await store.activate()

    #expect(store.transcriptUnparsedCount == 2)
}

// MARK: - (c) a real transcriptPath resolving to zero events is .empty, not .failed

@MainActor @Test func transcriptPathResolvingToZeroEventsIsEmptyNotFailed() async {
    let store = makeStore(transcriptPath: "t/empty.jsonl", fetchTranscript: { _ in
        APITranscriptPage(events: [], summary: nil)
    })

    await store.activate()

    guard case .empty = store.transcript else {
        Issue.record("expected .empty, got \(store.transcript)")
        return
    }
}

// MARK: - (d) a real failure surfaces as .failed

@MainActor @Test func realFetchFailureSurfacesAsFailed() async {
    let store = makeStore(transcriptPath: "t/down.jsonl", fetchTranscript: { _ in
        throw StubError(description: "transcript endpoint down")
    })

    await store.activate()

    guard case .failed(let message) = store.transcript else {
        Issue.record("expected .failed, got \(store.transcript)")
        return
    }
    #expect(message.contains("transcript endpoint down"))
}

// MARK: - (e) cancellation is benign (hotfix root cause B)

@MainActor @Test func cancelledFetchLeavesTranscriptAsLoadingNeverFailed() async {
    let store = makeStore(transcriptPath: "t/run-1.jsonl", fetchTranscript: { _ in
        throw CancellationError()
    })

    await store.activate()

    guard case .loading = store.transcript else {
        Issue.record("expected cancellation to leave transcript .loading, got \(store.transcript)")
        return
    }
}

// MARK: - (f)-(i) Final-review fix (Important 3): resolving a just-launched
// run's transcript path — `LauncherStore.performLaunch`'s `.agentRun` route
// only ever returns a `run_id`, never a transcript path, so
// `AgentRunDetailStore` makes one best-effort resolution attempt when
// `transcriptPath` starts out `nil`. See the type doc comment's
// "`transcriptPath == nil`" section.

@MainActor
private func makeResolvingStore(
    transcriptPath: String?,
    resolveTranscriptPath: (@Sendable () async throws -> String?)?,
    fetchTranscript: @escaping @Sendable (String) async throws -> APITranscriptPage = { _ in
        APITranscriptPage(events: [], summary: nil)
    }
) -> AgentRunDetailStore {
    AgentRunDetailStore(
        transcriptPath: transcriptPath,
        fetchTranscript: fetchTranscript,
        resolveTranscriptPath: resolveTranscriptPath
    )
}

// (f) A successful resolution fetches and populates content, and
// `resolvedPath` reflects the resolved path — not the constructor's
// (nil) `transcriptPath`.
@MainActor @Test func nilTranscriptPathResolvedSuccessfullyFetchesAndPopulatesContent() async {
    let store = makeResolvingStore(
        transcriptPath: nil,
        resolveTranscriptPath: { "t/resolved.jsonl" },
        fetchTranscript: { path in
            #expect(path == "t/resolved.jsonl")
            return APITranscriptPage(events: [.assistantMessage(content: "hi", thinking: nil)], summary: nil)
        }
    )

    await store.activate()

    #expect(store.resolvedPath == "t/resolved.jsonl")
    guard case .content(let events) = store.transcript else {
        Issue.record("expected .content, got \(store.transcript)")
        return
    }
    #expect(events == [.assistantMessage(content: "hi", thinking: nil)])
}

// (g) Resolution finding nothing (a genuinely path-less run) falls back to
// the same honest `.empty` state as a constructor-supplied `nil` — no
// network fetch fires, and `resolvedPath` stays `nil` so
// `AgentRunDetailScreen` still renders "NO TRANSCRIPT RECORDED".
@MainActor @Test func nilTranscriptPathResolvingToNilFallsBackToEmptyWithoutFetching() async {
    let fetched = FlagBox()
    let store = makeResolvingStore(
        transcriptPath: nil,
        resolveTranscriptPath: { nil },
        fetchTranscript: { _ in
            fetched.value = true
            return APITranscriptPage(events: [], summary: nil)
        }
    )

    await store.activate()

    #expect(store.resolvedPath == nil)
    #expect(fetched.value == false)
    guard case .empty = store.transcript else {
        Issue.record("expected .empty, got \(store.transcript)")
        return
    }
}

// (h) Resolution itself failing (e.g. the agent-runs list fetch errors, or
// is cancelled) is swallowed the same "best-effort" way — never surfaces
// as `.failed`, since this is a bonus lookup, not the screen's primary
// contract.
@MainActor @Test func resolutionFailureIsSwallowedAndFallsBackToEmpty() async {
    let store = makeResolvingStore(
        transcriptPath: nil,
        resolveTranscriptPath: { throw StubError(description: "agent-runs list unreachable") }
    )

    await store.activate()

    #expect(store.resolvedPath == nil)
    guard case .empty = store.transcript else {
        Issue.record("expected .empty, got \(store.transcript)")
        return
    }
}

// (i) A constructor-supplied `transcriptPath` never invokes the resolver at
// all — resolution is only ever a fallback for the genuinely-unknown case,
// not a second opinion on a path the caller already had.
@MainActor @Test func knownTranscriptPathNeverInvokesResolver() async {
    let resolverCalled = FlagBox()
    let store = makeResolvingStore(
        transcriptPath: "t/known.jsonl",
        resolveTranscriptPath: {
            resolverCalled.value = true
            return "t/should-not-be-used.jsonl"
        },
        fetchTranscript: { path in
            #expect(path == "t/known.jsonl")
            return APITranscriptPage(events: [], summary: nil)
        }
    )

    await store.activate()

    #expect(resolverCalled.value == false)
    #expect(store.resolvedPath == "t/known.jsonl")
}

// (k) A retry while the backend is still erroring never rewrites a real
// failure as "no transcript recorded": first activate resolved a path but
// the transcript fetch failed (`.failed` banner shown); on re-activate the
// resolver itself now throws too. The prior resolved path must survive —
// a thrown resolution is "couldn't check", not "resolved to nothing" — so
// the screen keeps its `.failed` state (and Retry affordance) instead of
// flipping to the false `.empty`/`resolvedPath == nil` combination.
@MainActor @Test func failedReactivateWithErroringResolverNeverConvertsFailedToEmpty() async {
    let resolverCalls = CounterBox()
    let store = makeResolvingStore(
        transcriptPath: nil,
        resolveTranscriptPath: {
            if resolverCalls.next() == 1 { return "t/flaky.jsonl" }
            throw StubError(description: "agent-runs list unreachable")
        },
        fetchTranscript: { _ in
            throw StubError(description: "transcript endpoint down")
        }
    )

    await store.activate()

    #expect(store.resolvedPath == "t/flaky.jsonl")
    guard case .failed = store.transcript else {
        Issue.record("expected .failed after the first activate, got \(store.transcript)")
        return
    }

    await store.activate()

    #expect(store.resolvedPath == "t/flaky.jsonl")
    guard case .failed(let message) = store.transcript else {
        Issue.record("expected the retry to stay .failed, got \(store.transcript)")
        return
    }
    #expect(message.contains("transcript endpoint down"))
}

// (l) The retained path is genuinely reused, not just displayed: with the
// resolver still erroring on the retry, a now-healthy transcript endpoint
// recovers the screen to `.content` through the previously resolved path.
@MainActor @Test func reactivateWithErroringResolverReusesPriorPathAndRecovers() async {
    let resolverCalls = CounterBox()
    let fetchCalls = CounterBox()
    let store = makeResolvingStore(
        transcriptPath: nil,
        resolveTranscriptPath: {
            if resolverCalls.next() == 1 { return "t/flaky.jsonl" }
            throw StubError(description: "agent-runs list unreachable")
        },
        fetchTranscript: { path in
            #expect(path == "t/flaky.jsonl")
            if fetchCalls.next() == 1 { throw StubError(description: "transcript endpoint down") }
            return APITranscriptPage(events: [.assistantMessage(content: "recovered", thinking: nil)], summary: nil)
        }
    )

    await store.activate()
    await store.activate()

    #expect(store.resolvedPath == "t/flaky.jsonl")
    guard case .content(let events) = store.transcript else {
        Issue.record("expected the retry to recover to .content, got \(store.transcript)")
        return
    }
    #expect(events == [.assistantMessage(content: "recovered", thinking: nil)])
}

// (j) End-to-end wiring: the production `init(runID:transcriptPath:host:
// client:)` resolves through the real agent-runs list endpoint
// (`GET /api/runs/agents`), scanning for the matching `run_id` — the
// "cheapest honest variant" `lookUpTranscriptPath` documents.
@MainActor @Test func productionInitResolvesTranscriptPathFromAgentRunsList() async {
    AgentRunDetailStubURLProtocol.reset { req in
        if req.url?.path == "/api/transcript" {
            return (200, Data(#"{"events":[],"summary":null}"#.utf8))
        }
        guard req.url?.path == "/api/runs/agents" else { return (200, Data("[]".utf8)) }
        let usage = #"{"cached_tokens":0,"cost_usd":null,"input_tokens":0,"output_tokens":0,"priced":false,"runs":0,"total_tokens":0}"#
        let row = #"""
        {"run_id":"run-just-launched","source":"agent","agent":"rupuso","session_id":null,"trigger_source":null,"status":"running","started_at":"2026-08-24T00:00:00Z","transcript_path":"t/run-just-launched.jsonl","usage":\#(usage),"turns":0,"duration_ms":null,"host_id":"local"}
        """#
        return (200, Data("[\(row)]".utf8))
    }
    let client = CPClient(
        config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
        session: AgentRunDetailStubURLProtocol.session()
    )
    let store = AgentRunDetailStore(runID: "run-just-launched", transcriptPath: nil, host: nil, client: client)

    await store.activate()

    #expect(store.resolvedPath == "t/run-just-launched.jsonl")
}
