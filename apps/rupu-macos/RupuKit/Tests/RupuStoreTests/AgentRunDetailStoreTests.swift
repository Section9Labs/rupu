import Testing
import Foundation
@testable import RupuStore
import RupuAPI

private struct StubError: Error, CustomStringConvertible {
    let description: String
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
