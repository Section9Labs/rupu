import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test infra (generation-token-isolated stub — see
// `ConfigStoreTests.ConfigStubURLProtocol`'s doc comment for the full
// rationale; duplicated here because it's `internal`/file-scoped to that
// sibling test file, same as every other store-test file's own copy of
// this rig).

/// Path+query-routing HTTP stub for `CodeStore`'s three endpoints (`GET
/// .../tree`, `.../source`, `.../files`). Routes on `request.url?.path` +
/// the `path` query item (where relevant); a monotonically increasing
/// generation token (stamped into each session via `httpAdditionalHeaders`,
/// read back off the request itself) makes a straggling response from a
/// PRIOR test's session harmlessly fail as `.cancelled` instead of
/// corrupting the CURRENT test's `handler`/`pathHits` state — see
/// `DashboardStubURLProtocol`'s doc comment (`DashboardStoreTests.swift`)
/// for why this matters under full-suite parallel load.
final class CodeStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-Code-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var pathHits: [String: Int] = [:]
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [CodeStubURLProtocol.self]
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

    /// Hit count keyed by `path` alone (query ignored) — enough for "was
    /// this endpoint hit N times" assertions; tests that need to
    /// distinguish which `path=` query fired route inside their own
    /// `handler` closure instead.
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
struct CodeStoreTests {
    private func makeClient(respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) -> CPClient {
        CodeStubURLProtocol.reset(handler: respond)
        return CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: CodeStubURLProtocol.session()
        )
    }

    /// Reads the `path` query item off a request — every one of `CodeStore`'s
    /// three endpoints except `/files` carries one.
    nonisolated private static func pathQuery(_ req: URLRequest) -> String? {
        guard let url = req.url else { return nil }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?
            .queryItems?.first(where: { $0.name == "path" })?.value
    }

    /// Minimal single-entry tree JSON. `entries` defaults to one dir + one
    /// file, matching `code_tree.json`'s shape.
    nonisolated private static func treeJSON(path: String, parent: String?) -> String {
        let parentJSON = parent.map { "\"\($0)\"" } ?? "null"
        return """
        {"path": "\(path)", "parent": \(parentJSON), "entries": [
          {"name": "auth", "path": "\(path.isEmpty ? "" : path + "/")auth", "kind": "dir"},
          {"name": "main.rs", "path": "\(path.isEmpty ? "" : path + "/")main.rs", "kind": "file"}
        ]}
        """
    }

    nonisolated private static func fileJSON(path: String, available: Bool = true, reason: String? = nil) -> String {
        if available {
            return """
            {"available": true, "path": "\(path)", "language": "rust", "totalLines": 1,
             "lines": [{"n": 1, "text": "fn main() {}"}], "reason": null}
            """
        }
        let reasonJSON = reason.map { "\"\($0)\"" } ?? "null"
        return """
        {"available": false, "path": "\(path)", "language": null, "totalLines": null,
         "lines": null, "reason": \(reasonJSON)}
        """
    }

    // MARK: - Tree load

    @Test func navigateLoadsTheFixtureIntoTreeContent() async throws {
        let client = makeClient { req in
            guard req.url?.path == "/api/projects/ws1/tree" else { return (404, Data()) }
            return (200, try! Fixtures.data("code_tree.json"))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.navigate(path: "src")

        #expect(store.currentPath == "src")
        guard case .content(let tree) = store.tree else {
            Issue.record("expected .content, got \(store.tree)")
            return
        }
        #expect(tree.path == "src")
        #expect(tree.parent == "")
        #expect(tree.entries.count == 2)
        // A directory change clears any prior file selection.
        #expect(store.file == nil)
        #expect(store.selectedPath == nil)
    }

    @Test func navigateFailureSetsFailedState() async {
        let client = makeClient { _ in (500, Data()) }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.navigate(path: "src")

        guard case .failed = store.tree else {
            Issue.record("expected .failed, got \(store.tree)")
            return
        }
    }

    @Test func activateLoadsTheWorkspaceRoot() async {
        let seenPaths = LockedPathLog()
        let client = makeClient { req in
            if let path = Self.pathQuery(req) { seenPaths.append(path) }
            return (200, Data(Self.treeJSON(path: "", parent: nil).utf8))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.activate()

        #expect(seenPaths.values == [""])
        #expect(store.currentPath == "")
        #expect(store.tree.value?.parent == nil)
    }

    // MARK: - Open file

    @Test func openLoadsTheFixtureIntoFileContent() async throws {
        let client = makeClient { req in
            guard req.url?.path == "/api/projects/ws1/source" else { return (404, Data()) }
            return (200, try! Fixtures.data("code_file.json"))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.open(path: "src/main.rs")

        #expect(store.selectedPath == "src/main.rs")
        guard case .content(let file) = store.file else {
            Issue.record("expected .content, got \(String(describing: store.file))")
            return
        }
        #expect(file.available)
        #expect(file.path == "src/main.rs")
        #expect(file.language == "rust")
        #expect(file.lines?.count == 3)
    }

    @Test func openOfAnUnavailableFileRendersTheReasonHonestly() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/projects/ws1/source" else { return (404, Data()) }
            return (200, Data(Self.fileJSON(path: "bin/blob", available: false, reason: "binary file").utf8))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.open(path: "bin/blob")

        guard case .content(let file) = store.file else {
            Issue.record("expected .content (available:false is still a 200), got \(String(describing: store.file))")
            return
        }
        #expect(!file.available)
        #expect(file.reason == "binary file")
        #expect(file.lines == nil)
    }

    @Test func openFailureSetsFailedState() async {
        let client = makeClient { _ in (500, Data()) }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.open(path: "src/main.rs")

        guard case .failed = store.file else {
            Issue.record("expected .failed, got \(String(describing: store.file))")
            return
        }
    }

    // MARK: - Generation guard (Step 1: "navigate mid-flight drops stale")

    /// A `navigate("slow")` call whose response is artificially delayed,
    /// superseded by a fast `navigate("fresh")` call before the first
    /// resolves, must never let the slow call's late-arriving tree land —
    /// `tree` must reflect ONLY the fresher call once both have settled.
    /// Same "start the stale call, wait until it is genuinely in flight,
    /// start the fresh one, then let the stale one's slow fetch finish too"
    /// shape `PaletteStoreTests.overlappingOpenCallsOnlyApplyTheFresherResults`
    /// establishes.
    ///
    /// Timed-sleep sweep, classification (b): the stub's own delay is KEPT.
    /// `navigate` bumps `treeGeneration` synchronously on entry, so once the
    /// stale call is known to have started, its result is dropped whenever
    /// it lands — before or after the fresh one — and no assertion here
    /// reads a mid-flight state. The delay only picks which interleaving is
    /// exercised. What was NOT order-independent is the start ordering (a
    /// 20ms sleep that, if lost, made the "stale" call the newest one and
    /// failed the test on a correct store), so that half is now a signal.
    /// A gate can't replace the delay here: this stub runs handlers inline
    /// on `URLSession`'s single shared custom-`URLProtocol` queue, so
    /// holding this response would block the fresh call's own request from
    /// ever being sent (see `LauncherStubURLProtocol.startLoading`'s doc
    /// comment for that verified behaviour).
    @Test func navigateMidFlightDropsStaleResult() async {
        let client = makeClient { req in
            let path = Self.pathQuery(req) ?? ""
            if path == "slow" {
                Thread.sleep(forTimeInterval: 0.08)
                return (200, Data(Self.treeJSON(path: "slow", parent: "").utf8))
            }
            return (200, Data(Self.treeJSON(path: "fresh", parent: "").utf8))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        let staleTask = Task { await store.navigate(path: "slow") }
        await expectEventually("the stale navigate's tree request is genuinely in flight") {
            CodeStubURLProtocol.hits("/api/projects/ws1/tree") >= 1
        }
        await store.navigate(path: "fresh")

        #expect(store.currentPath == "fresh")
        #expect(store.tree.value?.path == "fresh")

        await staleTask.value // let the stale call's slow fetch land too

        // The stale call's late completion must not have clobbered the
        // fresher result already standing.
        #expect(store.currentPath == "fresh")
        #expect(store.tree.value?.path == "fresh")
    }

    /// **Review round 1 fix** (pinned bug): `open(path:)` starting while an
    /// earlier `navigate(path:)` is still in flight must NOT strand that
    /// navigate's tree at `.loading` forever. An earlier revision of
    /// `CodeStore` shared ONE generation counter across both operations, so
    /// `open`'s bump made the still-in-flight `navigate`'s own guard fail
    /// once its (perfectly current, not actually stale) tree result
    /// arrived — this exact test used to assert `tree` STAYS `.loading`,
    /// pinning the bug rather than catching it. Per-block generations
    /// (`treeGeneration`/`fileGeneration` — see `CodeStore`'s type doc
    /// comment) fix this: `open(path:)` only ever bumps `fileGeneration`,
    /// so `navigate`'s own `treeGeneration` guard is untouched and its tree
    /// result lands normally once the slow fetch resolves.
    @Test func openMidFlightNoLongerStrandsAnEarlierInFlightNavigatesTree() async {
        let client = makeClient { req in
            if req.url?.path == "/api/projects/ws1/tree" {
                Thread.sleep(forTimeInterval: 0.08)
                return (200, Data(Self.treeJSON(path: "slow-dir", parent: "").utf8))
            }
            return (200, Data(Self.fileJSON(path: "main.rs").utf8))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        // Timed-sleep sweep: the navigate MUST be in flight before `open`
        // is called — if it started after, its synchronous prologue (which
        // clears `file`/`selectedPath` and bumps `fileGeneration`) would
        // land during `open`'s await and legitimately drop `open`'s result,
        // failing the assertions on a correct store. A 20ms sleep only made
        // that likely; the request reaching the stub proves it. The stub's
        // own 80ms delay stays (classification (b) — see
        // `navigateMidFlightDropsStaleResult`'s doc comment: it only widens
        // the window, and this stub's inline handler can't be gated without
        // blocking `open`'s own request).
        let navigateTask = Task { await store.navigate(path: "slow-dir") }
        await expectEventually("the navigate's tree request is genuinely in flight") {
            CodeStubURLProtocol.hits("/api/projects/ws1/tree") >= 1
        }
        await store.open(path: "main.rs")

        #expect(store.selectedPath == "main.rs")
        #expect(store.file?.value?.path == "main.rs")

        await navigateTask.value // let the still-in-flight navigate's slow fetch land

        // The navigate's tree result must land — `open` must never strand it.
        #expect(store.currentPath == "slow-dir")
        #expect(store.tree.value?.path == "slow-dir")
    }

    /// The other direction of the same cross-kind contract: a
    /// `navigate(path:)` that starts while an earlier `open(path:)` is
    /// still in flight must invalidate that `open`'s eventual result — the
    /// stale file must never resurrect `file` after the operator has moved
    /// to a different directory. `navigate(path:)` clears `file`/
    /// `selectedPath` synchronously AND bumps `fileGeneration`, so even a
    /// late-arriving, otherwise-successful `open` response is dropped.
    @Test func navigateMidFlightDropsAnEarlierInFlightOpensStaleFileResult() async {
        let client = makeClient { req in
            if req.url?.path == "/api/projects/ws1/source" {
                Thread.sleep(forTimeInterval: 0.08)
                return (200, Data(Self.fileJSON(path: "slow-file.rs").utf8))
            }
            return (200, Data(Self.treeJSON(path: "dir-b", parent: "").utf8))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        // Same start-ordering signal as the two tests above (a 20ms sleep
        // that lost its race made the `open` the NEWER call, so its result
        // was no longer stale and the "must be dropped" assertion failed on
        // a correct store). The stub's own delay stays — classification (b).
        let openTask = Task { await store.open(path: "slow-file.rs") }
        await expectEventually("the open's source request is genuinely in flight") {
            CodeStubURLProtocol.hits("/api/projects/ws1/source") >= 1
        }
        await store.navigate(path: "dir-b")

        #expect(store.currentPath == "dir-b")
        #expect(store.tree.value?.path == "dir-b")
        // `navigate` synchronously clears the selection/file panel.
        #expect(store.selectedPath == nil)
        #expect(store.file == nil)

        await openTask.value // let the stale open's slow fetch land too

        // The stale open's late-arriving (otherwise perfectly valid) file
        // content must never resurrect `file` after `navigate` moved on.
        #expect(store.file == nil, "the stale open's late-arriving result must be dropped")
    }

    // MARK: - Filter (Step 1: "filter loads once and is reused")

    @Test func loadFilterFetchesOnceAndIsReused() async {
        let hits = LockedCounter()
        let client = makeClient { req in
            guard req.url?.path == "/api/projects/ws1/files" else { return (404, Data()) }
            hits.increment()
            return (200, try! Fixtures.data("code_files.json"))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.loadFilter()
        await store.loadFilter()
        await store.loadFilter()

        #expect(hits.value == 1, "loadFilter() must only ever fetch once")
        guard case .content(let files) = store.filter else {
            Issue.record("expected .content, got \(String(describing: store.filter))")
            return
        }
        #expect(files.truncated)
        #expect(files.files.count == 3)
    }

    @Test func loadFilterFailureIsNotRetriedByLoadFilterButIsByReloadFilter() async {
        let hits = LockedCounter()
        let client = makeClient { req in
            let n = hits.increment()
            let status = n == 1 ? 500 : 200
            return (status, n == 1 ? Data() : try! Fixtures.data("code_files.json"))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.loadFilter()
        guard case .failed = store.filter else {
            Issue.record("expected .failed, got \(String(describing: store.filter))")
            return
        }

        await store.loadFilter() // no-op: filter is already non-nil (.failed)
        #expect(hits.value == 1, "loadFilter() must never retry a .failed filter")

        await store.reloadFilter() // the Retry path
        #expect(hits.value == 2)
        guard case .content = store.filter else {
            Issue.record("expected .content after reloadFilter(), got \(String(describing: store.filter))")
            return
        }
    }

    @Test func loadFilterOfAnEmptyProjectYieldsEmptyNotContent() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/projects/ws1/files" else { return (404, Data()) }
            return (200, Data(#"{"files": [], "truncated": false}"#.utf8))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        await store.loadFilter()

        guard case .empty = store.filter else {
            Issue.record("expected .empty, got \(String(describing: store.filter))")
            return
        }
    }

    // MARK: - Final-review fix, item 2: a cancelled filter fetch leaves the
    // store re-dispatchable (the file header's own claim, now true here too).

    /// `filter` is presence-gated, so a cancelled `reloadFilter()` that left
    /// it latched at `.loading` made `loadFilter()`'s `filter == nil` guard
    /// reject every later keystroke — a permanently "loading" filter field
    /// with no list and no Retry (`CodeTab`'s filter Retry only renders for
    /// `.failed`).
    ///
    /// De-flake (timed-stub sweep): the cancel must land while the fetch is
    /// genuinely unresolved — that is the whole scenario. An 80ms
    /// `Thread.sleep` made that probable; under parallel-suite load it can
    /// elapse first, the fetch completes, `filter` is `.content`, and the
    /// `== nil` assertion fails on a correct store. The response is now
    /// held on a gate opened immediately AFTER `cancel()` — so the
    /// cancellation is always in before the response can be delivered —
    /// and released promptly so the re-dispatch below isn't queued behind
    /// it (this stub runs handlers inline on `URLSession`'s single shared
    /// custom-`URLProtocol` queue).
    @Test func aCancelledFilterFetchLeavesTheStoreReDispatchable() async {
        let hits = LockedCounter()
        let filterGate = DispatchSemaphore(value: 0)
        let client = makeClient { req in
            guard req.url?.path == "/api/projects/ws1/files" else { return (404, Data()) }
            if hits.increment() == 1 {
                _ = filterGate.wait(timeout: .now() + 10) // 10s caps a hung test only
            }
            return (200, try! Fixtures.data("code_files.json"))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        let typing = Task { await store.loadFilter() }
        await expectEventually("the filter request is genuinely in flight") {
            CodeStubURLProtocol.hits("/api/projects/ws1/files") >= 1
        }
        typing.cancel()
        filterGate.signal() // the response can only ever arrive post-cancel
        await typing.value

        #expect(
            store.filter == nil,
            "a cancelled filter fetch must reset `filter` to nil, not latch it at .loading"
        )

        await store.loadFilter() // a later keystroke must actually fetch

        #expect(hits.value == 2, "the re-request after a cancel must reach the stub a second time")
        guard case .content = store.filter else {
            Issue.record("expected .content after the re-request, got \(String(describing: store.filter))")
            return
        }
    }

    /// The reset is conditional on `filter` still being `.loading`, so a
    /// cancelled call can never wipe out a CONCURRENT live fetch's result —
    /// either interleaving must end on `.content`.
    ///
    /// Timed-sleep sweep, classification (b) — the sleeps are KEPT because
    /// this test is explicitly order-INDEPENDENT, and that is its whole
    /// point: whichever of the two calls resolves first, and whether the
    /// cancel lands before or after the doomed call's own response, the
    /// surviving call always writes `.content` and the cancelled call's
    /// `.loading`-guarded reset can never clear it. Nothing here asserts a
    /// mid-flight state, so there is no bracket for a gate to hold open.
    @Test func aCancelledFilterFetchNeverWipesOutAConcurrentFetchsResult() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/projects/ws1/files" else { return (404, Data()) }
            Thread.sleep(forTimeInterval: 0.05)
            return (200, try! Fixtures.data("code_files.json"))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        let doomed = Task { await store.reloadFilter() }
        let survivor = Task { await store.reloadFilter() }
        try? await Task.sleep(for: .milliseconds(15))
        doomed.cancel()
        await doomed.value
        await survivor.value

        guard case .content = store.filter else {
            Issue.record("expected .content, got \(String(describing: store.filter))")
            return
        }
    }
}

/// Thread-safe call counter — same rationale as every other store test's
/// own copy of this pattern (`ConfigStoreTests.Counter`, `ClaimsStoreTests.
/// LockedCounter`). Named distinctly to avoid any ambiguity within this
/// file/target.
private final class LockedCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

/// Thread-safe append log — same rationale as `LockedCounter` above, for a
/// test that needs to assert WHICH query values were seen, not just how
/// many times.
private final class LockedPathLog: @unchecked Sendable {
    private let lock = NSLock()
    private var log: [String] = []
    func append(_ value: String) { lock.withLock { log.append(value) } }
    var values: [String] { lock.withLock { log } }
}
