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
    /// Same "start the stale call, give it time to actually fire, start the
    /// fresh one, then let the stale one's slow fetch finish too" shape
    /// `PaletteStoreTests.overlappingOpenCallsOnlyApplyTheFresherResults`
    /// establishes.
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
        try? await Task.sleep(for: .milliseconds(20)) // let the slow request actually fire
        await store.navigate(path: "fresh")

        #expect(store.currentPath == "fresh")
        #expect(store.tree.value?.path == "fresh")

        await staleTask.value // let the stale call's slow fetch land too

        // The stale call's late completion must not have clobbered the
        // fresher result already standing.
        #expect(store.currentPath == "fresh")
        #expect(store.tree.value?.path == "fresh")
    }

    /// The shared-`generation` contract cuts BOTH ways (see `CodeStore`'s
    /// type doc comment): an `open(path:)` that starts while an earlier
    /// `navigate(path:)` is still in flight must invalidate that
    /// `navigate`'s eventual result too, not just a same-kind race.
    @Test func openMidFlightInvalidatesAnEarlierInFlightNavigate() async {
        let client = makeClient { req in
            if req.url?.path == "/api/projects/ws1/tree" {
                Thread.sleep(forTimeInterval: 0.08)
                return (200, Data(Self.treeJSON(path: "slow-dir", parent: "").utf8))
            }
            return (200, Data(Self.fileJSON(path: "main.rs").utf8))
        }
        let store = CodeStore(wsID: "ws1", client: client)

        let staleNavigate = Task { await store.navigate(path: "slow-dir") }
        try? await Task.sleep(for: .milliseconds(20))
        await store.open(path: "main.rs")

        #expect(store.selectedPath == "main.rs")
        #expect(store.file?.value?.path == "main.rs")

        await staleNavigate.value

        // The stale navigate's late tree result must not have landed —
        // `tree` stays whatever it was before (`.loading`, since this store
        // never had a successful `navigate` before the stale one).
        guard case .loading = store.tree else {
            Issue.record("expected tree to stay .loading (the stale navigate's result must be dropped), got \(store.tree)")
            return
        }
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
