import Testing
import Foundation
import RupuAPI
@testable import RupuStore
@testable import RupuRunDetail

// MARK: - Test infra (generation-token-isolated stub — see
// `CodeStoreTests.CodeStubURLProtocol`'s doc comment for the full
// rationale; duplicated here because it's `internal`/file-scoped to that
// sibling test file in a different target, same as every other store-test
// file's own copy of this rig).

/// Path-routing HTTP stub for `SourcePreviewStore`'s two endpoints (`GET
/// .../source`, `GET .../ast`). A monotonically increasing generation token
/// (stamped into each session via `httpAdditionalHeaders`, read back off the
/// request itself) makes a straggling response from a PRIOR test's session
/// harmlessly fail as `.cancelled` instead of corrupting the CURRENT test's
/// `handler`/`hits` state under full-suite parallel load.
final class SourcePreviewStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-SourcePreview-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var pathHits: [String: Int] = [:]
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [SourcePreviewStubURLProtocol.self]
        config.httpAdditionalHeaders = [generationHeader: String(generation)]
        return URLSession(configuration: config)
    }

    static func reset(handler: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) {
        lock.withLock {
            currentGeneration += 1
            pathHits = [:]
            self.handler = handler
        }
    }

    static func hits(_ path: String) -> Int {
        lock.withLock { pathHits[path, default: 0] }
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let requestGeneration = request.value(forHTTPHeaderField: Self.generationHeader).flatMap(Int.init) ?? -1
        let (isCurrent, activeHandler): (Bool, (@Sendable (URLRequest) -> (status: Int, body: Data))?) = Self.lock.withLock {
            (requestGeneration == Self.currentGeneration, Self.handler)
        }
        guard isCurrent else {
            client?.urlProtocol(self, didFailWithError: URLError(.cancelled))
            return
        }
        if let path = request.url?.path {
            Self.lock.withLock { Self.pathHits[path, default: 0] += 1 }
        }
        guard let handler = activeHandler else {
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

/// De-flakes "wait for an async effect to land" — a private helper per file,
/// this codebase's established convention (see `ActivityStoreTests.
/// pollUntil`/`expectEventually` for the identical shape).
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

@Suite(.serialized)
@MainActor
struct SourcePreviewStoreTests {
    private func makeClient(respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) -> CPClient {
        SourcePreviewStubURLProtocol.reset(handler: respond)
        return CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: SourcePreviewStubURLProtocol.session()
        )
    }

    nonisolated private static func sourceJSON(available: Bool, line: Int = 10, reason: String? = nil) -> String {
        guard available else {
            let reasonJSON = reason.map { "\"\($0)\"" } ?? "null"
            return """
            {"available": false, "path": null, "language": null, "startLine": null,
             "endLine": null, "targetLine": null, "totalLines": null, "lines": null,
             "reason": \(reasonJSON)}
            """
        }
        return """
        {"available": true, "path": "src/a.rs", "language": "rust", "startLine": \(line - 1),
         "endLine": \(line + 1), "targetLine": \(line), "totalLines": 100,
         "lines": [{"n": \(line), "text": "let x = 1;"}], "reason": null}
        """
    }

    nonisolated private static func astJSON(available: Bool, reason: String? = nil) -> String {
        guard available else {
            let reasonJSON = reason.map { "\"\($0)\"" } ?? "null"
            return #"{"available": false, "language": null, "root": null, "truncated": null, "reason": \#(reasonJSON)}"#
        }
        return """
        {"available": true, "language": "rust",
         "root": {"kind": "source_file", "named": true, "field": null, "startLine": 1,
                   "startCol": 1, "endLine": 1, "endCol": 1, "matched": true, "children": []},
         "truncated": false}
        """
    }

    // MARK: - "second expand = no second hit"

    @Test func loadSourceIfNeededCachesPerKeySoASecondExpandDoesNotRefetch() async {
        let hits = LockedCounter()
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/source" else { return (404, Data()) }
            hits.increment()
            return (200, Data(Self.sourceJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)
        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)
        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)

        #expect(hits.value == 1, "a second/third expand of the same (path,line) must not re-fetch")
        guard case .content(let slice) = store.sourceState(path: "src/a.rs", line: 10) else {
            Issue.record("expected .content, got \(String(describing: store.sourceState(path: "src/a.rs", line: 10)))")
            return
        }
        #expect(slice.available)
        #expect(slice.targetLine == 10)
    }

    @Test func loadAstIfNeededCachesPerKeySoASecondExpandDoesNotRefetch() async {
        let hits = LockedCounter()
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/ast" else { return (404, Data()) }
            hits.increment()
            return (200, Data(Self.astJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        await store.loadAstIfNeeded(path: "src/a.rs", line: 2, col: 8)
        await store.loadAstIfNeeded(path: "src/a.rs", line: 2, col: 8)

        #expect(hits.value == 1, "a second expand of the same (path,line,col) must not re-fetch")
        guard case .content(let response) = store.astState(path: "src/a.rs", line: 2, col: 8) else {
            Issue.record("expected .content, got \(String(describing: store.astState(path: "src/a.rs", line: 2, col: 8)))")
            return
        }
        #expect(response.available)
    }

    /// Two different (path,line) keys are independent — a second cache slot
    /// must actually fetch (this is the flip side of the "no second hit"
    /// test above: proving the guard is keyed correctly, not just present).
    @Test func differentKeysEachFetchIndependently() async {
        let hits = LockedCounter()
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/source" else { return (404, Data()) }
            hits.increment()
            return (200, Data(Self.sourceJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)
        await store.loadSourceIfNeeded(path: "src/b.rs", line: 10)
        await store.loadSourceIfNeeded(path: "src/a.rs", line: 20)

        #expect(hits.value == 3)
    }

    // MARK: - Unavailable reason surfaces verbatim

    @Test func unavailableSourceReasonSurfacesVerbatim() async {
        let reason = "Source preview is not available for remote-host runs yet."
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/source" else { return (404, Data()) }
            return (200, Data(Self.sourceJSON(available: false, reason: reason).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: "mini", client: client)

        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)

        guard case .content(let slice) = store.sourceState(path: "src/a.rs", line: 10) else {
            Issue.record("expected .content (available:false is still a 200), got \(String(describing: store.sourceState(path: "src/a.rs", line: 10)))")
            return
        }
        #expect(!slice.available)
        #expect(slice.reason == reason, "the server's reason must render verbatim, never a synthesized enum")
    }

    @Test func unavailableAstReasonSurfacesVerbatim() async {
        let reason = "No grammar registered for this file type."
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/ast" else { return (404, Data()) }
            return (200, Data(Self.astJSON(available: false, reason: reason).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        await store.loadAstIfNeeded(path: "src/a.rs", line: 1, col: 1)

        guard case .content(let response) = store.astState(path: "src/a.rs", line: 1, col: 1) else {
            Issue.record("expected .content, got \(String(describing: store.astState(path: "src/a.rs", line: 1, col: 1)))")
            return
        }
        #expect(!response.available)
        #expect(response.reason == reason)
    }

    // MARK: - Run-identity change flushes cache

    @Test func setRunWithADifferentRunIDFlushesBothCaches() async {
        let client = makeClient { req in
            if req.url?.path == "/api/runs/run-a/source" {
                return (200, Data(Self.sourceJSON(available: true).utf8))
            }
            return (404, Data())
        }
        let store = SourcePreviewStore(runID: "run-a", host: nil, client: client)
        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)
        #expect(store.sourceState(path: "src/a.rs", line: 10) != nil)

        store.setRun(runID: "run-b", host: nil)

        #expect(store.runID == "run-b")
        #expect(store.sourceState(path: "src/a.rs", line: 10) == nil, "a run-identity change must flush the cache, even for a key the new run hasn't fetched yet")
    }

    /// Same-`runID` call is a no-op — an idempotent re-`activate()` (or a
    /// redundant `setRun` from a screen that doesn't itself track whether
    /// the run actually changed) must never discard a cache the operator is
    /// still looking at.
    @Test func setRunWithTheSameRunIDLeavesTheCacheIntact() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-a/source" else { return (404, Data()) }
            return (200, Data(Self.sourceJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-a", host: nil, client: client)
        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)

        store.setRun(runID: "run-a", host: nil)

        #expect(store.sourceState(path: "src/a.rs", line: 10) != nil, "a same-runID setRun must not flush the cache")
    }

    /// A fetch still in flight when `setRun` switches to a different run
    /// must never populate the NEW run's cache once it eventually lands —
    /// same "captured generation must still match" shape `CodeStoreTests.
    /// navigateMidFlightDropsStaleResult` pins for `CodeStore`.
    ///
    /// Timed-sleep sweep, classification (b): the stub's delay is KEPT (the
    /// guard drops the run-a result whenever it lands, and no assertion
    /// here reads a mid-flight state), but the START ordering was NOT
    /// order-independent — if the fetch hadn't begun before `setRun`, it
    /// would run under run-b's own identity and legitimately populate the
    /// cache, failing the test on a correct store. That half is now a
    /// signal. See `CodeStoreTests.navigateMidFlightDropsStaleResult` for
    /// why the delay itself can't become a gate in an inline stub.
    @Test func aFetchInFlightWhenTheRunChangesIsDroppedOnArrival() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-a/source" else { return (404, Data()) }
            Thread.sleep(forTimeInterval: 0.08)
            return (200, Data(Self.sourceJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-a", host: nil, client: client)

        let staleTask = Task { await store.loadSourceIfNeeded(path: "src/a.rs", line: 10) }
        await expectEventually("the run-a source request is genuinely in flight") {
            SourcePreviewStubURLProtocol.hits("/api/runs/run-a/source") >= 1
        }
        store.setRun(runID: "run-b", host: nil)

        #expect(store.sourceState(path: "src/a.rs", line: 10) == nil)

        await staleTask.value // let the stale call's slow fetch land too

        #expect(store.sourceState(path: "src/a.rs", line: 10) == nil, "the stale run-a fetch's late-arriving result must never populate run-b's cache")
        #expect(store.runID == "run-b")
    }

    // MARK: - Retry (the "compact Retry" affordance's underlying contract)

    @Test func loadSourceIfNeededDoesNotRetryAFailureButReloadSourceDoes() async {
        let hits = LockedCounter()
        let client = makeClient { req in
            let n = hits.increment()
            let status = n == 1 ? 500 : 200
            return (status, n == 1 ? Data() : Data(Self.sourceJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)
        guard case .failed = store.sourceState(path: "src/a.rs", line: 10) else {
            Issue.record("expected .failed, got \(String(describing: store.sourceState(path: "src/a.rs", line: 10)))")
            return
        }

        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10) // no-op: already cached as .failed
        #expect(hits.value == 1, "loadSourceIfNeeded must never retry a .failed slice")

        await store.reloadSource(path: "src/a.rs", line: 10) // the Retry path
        #expect(hits.value == 2)
        guard case .content = store.sourceState(path: "src/a.rs", line: 10) else {
            Issue.record("expected .content after reloadSource(), got \(String(describing: store.sourceState(path: "src/a.rs", line: 10)))")
            return
        }
    }

    // MARK: - Final-review fix, item 1: a cancelled fetch leaves the key
    // re-dispatchable (collapse-while-loading must not strand a re-expand).

    /// The exact operator sequence: expand a preview on a slow fetch,
    /// collapse before it lands (SwiftUI cancels the `.task`), re-expand.
    /// Before the fix the cancelled call left `.loading` latched, so
    /// `loadSourceIfNeeded`'s `!= nil` presence guard swallowed the
    /// re-expand and the row sat on "Loading source…" forever with no
    /// Retry affordance (Retry only renders for `.failed`).
    ///
    /// De-flake (timed-stub sweep): the cancel must land while the fetch is
    /// genuinely unresolved — that IS the scenario. An 80ms `Thread.sleep`
    /// made it probable; under parallel-suite load it can elapse first, the
    /// fetch lands, the key is `.content`, and the `== nil` assertion fails
    /// on a correct store. The response is now held on a gate opened
    /// immediately AFTER `cancel()`, so cancellation always precedes
    /// delivery (verified: a response delivered right after a cancel still
    /// surfaces as `NSURLErrorCancelled`), and released at once so the
    /// re-dispatch below isn't queued behind it on this stub's shared
    /// inline queue.
    @Test func aCancelledSourceFetchLeavesTheKeyReDispatchable() async {
        let hits = LockedCounter()
        let sourceGate = DispatchSemaphore(value: 0)
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/source" else { return (404, Data()) }
            if hits.increment() == 1 {
                _ = sourceGate.wait(timeout: .now() + 10) // 10s caps a hung test only
            }
            return (200, Data(Self.sourceJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        // Expand → fetch in flight.
        let expand = Task { await store.loadSourceIfNeeded(path: "src/a.rs", line: 10) }
        await expectEventually("the source request is genuinely in flight") {
            SourcePreviewStubURLProtocol.hits("/api/runs/run-1/source") >= 1
        }
        // Collapse → SwiftUI cancels the mounted `.task`.
        expand.cancel()
        sourceGate.signal() // the response can only ever arrive post-cancel
        await expand.value

        #expect(
            store.sourceState(path: "src/a.rs", line: 10) == nil,
            "a cancelled fetch must remove its .loading entry, not latch it — otherwise the presence guard blocks every later expand"
        )

        // Re-expand → must actually re-fetch and land content.
        await store.loadSourceIfNeeded(path: "src/a.rs", line: 10)

        #expect(hits.value == 2, "the re-expand after a cancel must reach the stub a second time")
        guard case .content(let slice) = store.sourceState(path: "src/a.rs", line: 10) else {
            Issue.record("expected .content after the re-expand, got \(String(describing: store.sourceState(path: "src/a.rs", line: 10)))")
            return
        }
        #expect(slice.available)
    }

    /// Same contract on the AST path — `AstTreeView` mounts its fetch from
    /// the same kind of `.task(id:)` and gets cancelled the same way.
    @Test func aCancelledAstFetchLeavesTheKeyReDispatchable() async {
        // Same gate-instead-of-sleep de-flake as the source-path test above.
        let hits = LockedCounter()
        let astGate = DispatchSemaphore(value: 0)
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/ast" else { return (404, Data()) }
            if hits.increment() == 1 {
                _ = astGate.wait(timeout: .now() + 10) // 10s caps a hung test only
            }
            return (200, Data(Self.astJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        let expand = Task { await store.loadAstIfNeeded(path: "src/a.rs", line: 2, col: 8) }
        await expectEventually("the ast request is genuinely in flight") {
            SourcePreviewStubURLProtocol.hits("/api/runs/run-1/ast") >= 1
        }
        expand.cancel()
        astGate.signal() // the response can only ever arrive post-cancel
        await expand.value

        #expect(store.astState(path: "src/a.rs", line: 2, col: 8) == nil)

        await store.loadAstIfNeeded(path: "src/a.rs", line: 2, col: 8)

        #expect(hits.value == 2, "the re-expand after a cancel must reach the stub a second time")
        guard case .content(let response) = store.astState(path: "src/a.rs", line: 2, col: 8) else {
            Issue.record("expected .content after the re-expand, got \(String(describing: store.astState(path: "src/a.rs", line: 2, col: 8)))")
            return
        }
        #expect(response.available)
    }

    /// The removal is conditional on the entry still being `.loading`, so a
    /// cancelled call can never wipe out a CONCURRENT live fetch's result
    /// for the same key. Whichever way the two interleave — the cancelled
    /// call's `catch` first (entry is still `.loading`, removed; the live
    /// call then writes its content) or the live call's success first
    /// (entry is `.content`, so the `catch` leaves it alone) — the key must
    /// end on `.content`, never blanked back to "Loading…".
    ///
    /// Timed-sleep sweep, classification (b) — the sleeps are KEPT: the
    /// paragraph above IS the order-independence proof, and that
    /// independence is itself the contract under test. Nothing here asserts
    /// a mid-flight state, so there is no bracket for a gate to hold open.
    @Test func aCancelledFetchNeverWipesOutAConcurrentFetchsResultForTheSameKey() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/runs/run-1/source" else { return (404, Data()) }
            Thread.sleep(forTimeInterval: 0.05)
            return (200, Data(Self.sourceJSON(available: true).utf8))
        }
        let store = SourcePreviewStore(runID: "run-1", host: nil, client: client)

        let doomed = Task { await store.reloadSource(path: "src/a.rs", line: 10) }
        let survivor = Task { await store.reloadSource(path: "src/a.rs", line: 10) }
        try? await Task.sleep(for: .milliseconds(15)) // let both requests actually fire
        doomed.cancel()
        await doomed.value
        await survivor.value

        guard case .content(let slice) = store.sourceState(path: "src/a.rs", line: 10) else {
            Issue.record("expected .content, got \(String(describing: store.sourceState(path: "src/a.rs", line: 10)))")
            return
        }
        #expect(slice.available)
    }
}

// MARK: - View-member pure seams (no SwiftUI rendering — same idiom
// `RunDetailScreenStatusTests` establishes for `RunDetailScreen.
// unrecognizedStatusRaw`)

@Test @MainActor func gutterWidthGrowsWithDigitCountAndFloorsAtASmallMinimum() {
    // `nil`/small totals both fall back to the 1-digit case (`"1".count` /
    // `"5".count`), which is below the `max(28, ...)` floor — both must
    // land on the SAME floored value, not two different tiny widths.
    #expect(SourcePreview.gutterWidth(totalLines: nil) == SourcePreview.gutterWidth(totalLines: 5))
    #expect(SourcePreview.gutterWidth(totalLines: 5) == 28)
    // A 5-digit total (≥10,000 lines) must widen past the floor, not clip.
    #expect(SourcePreview.gutterWidth(totalLines: 12345) == CGFloat(5) * 7 + 12)
}

// MARK: - Finding 2 (review fix): the `.task(id:)` key folds in run identity
// so a same-slot run switch (surviving `@State`) re-fires the fetch against
// the freshly-flushed store instead of stranding on "Loading…" forever.

@Test @MainActor func sourcePreviewTaskIDChangesWhenRunIDChangesEvenForTheSamePathAndLine() {
    let idForRunA = SourcePreview.taskID(runID: "run-a", path: "src/a.rs", line: 10)
    let idForRunB = SourcePreview.taskID(runID: "run-b", path: "src/a.rs", line: 10)
    #expect(idForRunA != idForRunB)
}

@Test @MainActor func sourcePreviewTaskIDIsStableForTheSameInputs() {
    #expect(
        SourcePreview.taskID(runID: "run-a", path: "src/a.rs", line: 10)
            == SourcePreview.taskID(runID: "run-a", path: "src/a.rs", line: 10)
    )
}

@Test @MainActor func astTreeViewTaskIDChangesWhenRunIDChangesEvenForTheSamePathLineAndCol() {
    let idForRunA = AstTreeView.taskID(runID: "run-a", path: "src/a.rs", line: 2, col: 8)
    let idForRunB = AstTreeView.taskID(runID: "run-b", path: "src/a.rs", line: 2, col: 8)
    #expect(idForRunA != idForRunB)
}

@Test @MainActor func astTreeViewTaskIDIsStableForTheSameInputs() {
    #expect(
        AstTreeView.taskID(runID: "run-a", path: "src/a.rs", line: 2, col: 8)
            == AstTreeView.taskID(runID: "run-a", path: "src/a.rs", line: 2, col: 8)
    )
}

@Test @MainActor func matchedAncestorPathsFindsTheChainDownToTheMatchedNode() {
    // Mirrors `run_ast.json`'s 3-level shape: source_file -> function_item ->
    // identifier (matched).
    let matchedLeaf = APIAstNode(
        kind: "identifier", named: true, field: "name",
        startLine: 2, startCol: 8, endLine: 2, endCol: 12,
        matched: true, children: []
    )
    let functionItem = APIAstNode(
        kind: "function_item", named: true, field: nil,
        startLine: 1, startCol: 1, endLine: 3, endCol: 2,
        matched: false, children: [matchedLeaf]
    )
    let root = APIAstNode(
        kind: "source_file", named: true, field: nil,
        startLine: 1, startCol: 1, endLine: 3, endCol: 2,
        matched: false, children: [functionItem]
    )

    let ancestors = AstTreeView.matchedAncestorPaths(root: root, path: "0")

    #expect(ancestors == ["0", "0.0"], "must return every ancestor's path down to (but not including) the matched node itself")
}

@Test @MainActor func matchedAncestorPathsReturnsNilWhenNothingIsMatched() {
    let leaf = APIAstNode(kind: "identifier", named: true, field: nil, startLine: 1, startCol: 1, endLine: 1, endCol: 1, matched: false, children: [])
    let root = APIAstNode(kind: "source_file", named: true, field: nil, startLine: 1, startCol: 1, endLine: 1, endCol: 1, matched: false, children: [leaf])

    #expect(AstTreeView.matchedAncestorPaths(root: root, path: "0") == nil)
}

@Test @MainActor func matchedAncestorPathsHandlesTheRootItselfBeingMatched() {
    let root = APIAstNode(kind: "source_file", named: true, field: nil, startLine: 1, startCol: 1, endLine: 1, endCol: 1, matched: true, children: [])
    #expect(AstTreeView.matchedAncestorPaths(root: root, path: "0") == [])
}

@Test @MainActor func fromStructuredParsesWellFormedMatchesAndSkipsMalformedOnes() {
    let structured = JSONValue.object([
        "matchCount": .number(1),
        "truncated": .bool(false),
        "matches": .array([
            .object([
                "file": .string("src/a.rs"),
                "range": .object([
                    "startLine": .number(12), "startCol": .number(3),
                    "endLine": .number(12), "endCol": .number(9),
                ]),
                "text": .string("fn foo()"),
            ]),
            // Missing `range` — must be skipped, not crash or fabricate a location.
            .object(["file": .string("src/b.rs")]),
            // Missing `file` — must be skipped.
            .object(["range": .object(["startLine": .number(1), "startCol": .number(1)])]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult for a present matches array")
        return
    }

    #expect(result.matches == [
        AstGrepTranscriptParsing.Match(file: "src/a.rs", startLine: 12, startCol: 3, text: "fn foo()"),
    ])
    #expect(result.matchCount == 1)
    #expect(!result.truncated)
}

@Test @MainActor func fromStructuredReturnsNilForAbsentOrShapelessInput() {
    #expect(AstGrepTranscriptParsing.fromStructured(nil) == nil)
    #expect(AstGrepTranscriptParsing.fromStructured(.object([:])) == nil, "no `matches` key at all must fall back, same as the web's own trigger")
    #expect(AstGrepTranscriptParsing.fromStructured(.string("not an object")) == nil)
}

// MARK: - Finding 1 (review fix): truthful truncation — `matchCount`/
// `truncated` are read off the wire, not discarded, and the label renders
// the web's own "showing first N of M" shape under truncation.

@Test @MainActor func fromStructuredReadsMatchCountAndTruncatedFromTheWire() {
    let structured = JSONValue.object([
        "matchCount": .number(250),
        "truncated": .bool(true),
        "matches": .array([
            .object([
                "file": .string("src/a.rs"),
                "range": .object(["startLine": .number(1), "startCol": .number(1), "endLine": .number(1), "endCol": .number(1)]),
            ]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult")
        return
    }

    #expect(result.matchCount == 250, "the server's real total, not matches.count (which is the capped prefix)")
    #expect(result.truncated)
}

@Test @MainActor func fromStructuredFallsBackToMatchesCountWhenMatchCountFieldIsMissing() {
    let structured = JSONValue.object([
        "matches": .array([
            .object([
                "file": .string("src/a.rs"),
                "range": .object(["startLine": .number(1), "startCol": .number(1), "endLine": .number(1), "endCol": .number(1)]),
            ]),
        ]),
    ])

    guard let result = AstGrepTranscriptParsing.fromStructured(structured) else {
        Issue.record("expected a non-nil StructuredResult")
        return
    }

    #expect(result.matchCount == 1, "an honest read of \"assume the total is exactly what we parsed,\" not a fabricated 0")
    #expect(!result.truncated, "an absent `truncated` field must never be read as truncated")
}

@Test @MainActor func matchCountLabelRendersTheWebsShowingFirstNOfMShapeWhenTruncated() {
    let structured = AstGrepTranscriptParsing.StructuredResult(
        matches: [
            AstGrepTranscriptParsing.Match(file: "a.rs", startLine: 1, startCol: 1, text: nil),
            AstGrepTranscriptParsing.Match(file: "b.rs", startLine: 2, startCol: 1, text: nil),
        ],
        matchCount: 250,
        truncated: true
    )

    let label = AstGrepTranscriptParsing.matchCountLabel(structured: structured, matches: structured.matches)

    #expect(label == "showing first 2 of 250 matches")
}

@Test @MainActor func matchCountLabelIsAPlainCountWhenNotTruncated() {
    let structured = AstGrepTranscriptParsing.StructuredResult(
        matches: [AstGrepTranscriptParsing.Match(file: "a.rs", startLine: 1, startCol: 1, text: nil)],
        matchCount: 1,
        truncated: false
    )

    #expect(AstGrepTranscriptParsing.matchCountLabel(structured: structured, matches: structured.matches) == "1 match")
    #expect(AstGrepTranscriptParsing.matchCountLabel(structured: nil, matches: structured.matches) == "1 match")

    let two = [
        AstGrepTranscriptParsing.Match(file: "a.rs", startLine: 1, startCol: 1, text: nil),
        AstGrepTranscriptParsing.Match(file: "b.rs", startLine: 2, startCol: 1, text: nil),
    ]
    #expect(AstGrepTranscriptParsing.matchCountLabel(structured: nil, matches: two) == "2 matches", "the text-parsed fallback (structured == nil) carries no truncation signal — always a plain count")
}

@Test @MainActor func fromTextParsesTheCompactPathLineColFormatAndSkipsUnparseableLines() {
    let output = """
    src/a.rs:12:3: fn foo() {
    this line has no match at all
    src/b.rs:1:1: use bar;
    """

    let matches = AstGrepTranscriptParsing.fromText(output)

    #expect(matches == [
        AstGrepTranscriptParsing.Match(file: "src/a.rs", startLine: 12, startCol: 3, text: "fn foo() {"),
        AstGrepTranscriptParsing.Match(file: "src/b.rs", startLine: 1, startCol: 1, text: "use bar;"),
    ])
}

@Test @MainActor func fromTextReturnsEmptyForBlankOutput() {
    #expect(AstGrepTranscriptParsing.fromText("").isEmpty)
}

/// Thread-safe call counter — same rationale as every other store test's own
/// copy of this pattern (`CodeStoreTests.LockedCounter`). Named distinctly
/// to avoid ambiguity within this file/target.
private final class LockedCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}
