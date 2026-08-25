import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test infra (generation-token-isolated stub — see
// `DashboardStoreTests.DashboardStubURLProtocol`'s doc comment for the full
// rationale; duplicated here because it's `internal`/file-scoped to that
// sibling test file, same as every other store-test file's own copy of
// this rig).

/// Path-routing HTTP stub for `ConfigStore`'s four endpoints (`GET
/// /api/config`, `PUT /api/config/global`, `PUT /api/config/project/:id`,
/// `PUT /api/config/policy`). Routes on `request.url?.path`; a
/// monotonically increasing generation token (stamped into each session via
/// `httpAdditionalHeaders`, read back off the request itself) makes a
/// straggling response from a PRIOR test's session harmlessly fail as
/// `.cancelled` instead of corrupting the CURRENT test's `handler`/
/// `pathHits` state — see `DashboardStubURLProtocol`'s doc comment for why
/// this matters under full-suite parallel load.
final class ConfigStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-Config-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var pathHits: [String: Int] = [:]
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ConfigStubURLProtocol.self]
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

/// Thread-safe call counter — same rationale as
/// `LauncherStoreTests.Counter`/`DashboardStoreTests.HitCounter` (both
/// `private` to their own files): a plain captured `var` can't cross into a
/// `@Sendable` fetch-handler closure under Swift 6 strict concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

/// Thread-safe mutable string box — used where a handler needs to echo back
/// the most recently "persisted" raw text (see
/// `lastSaveRestartKeysClearsAtStartOfNextSaveAttempt`'s use). Same
/// rationale as `Counter` above.
private final class RawBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: String
    init(_ initial: String) { value = initial }
    func get() -> String { lock.withLock { value } }
    func set(_ newValue: String) { lock.withLock { value = newValue } }
}

/// De-flakes "wait for an async effect to land" — same rationale/shape as
/// every other store-test file's own copy.
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
struct ConfigStoreTests {
    /// `nonisolated` (not just `private`) — these three helpers are called
    /// from inside the `@Sendable` stub-handler closures passed to
    /// `makeClient`, which run on a background `URLProtocol` thread, never
    /// on `MainActor`; without `nonisolated` here, that call would be an
    /// implicit cross-actor `await` a synchronous closure can't make.
    nonisolated private static func queryValue(_ name: String, in req: URLRequest) -> String? {
        guard let url = req.url else { return nil }
        return URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == name })?.value
    }

    /// Builds a minimal, decodable `APIConfigView` JSON body. `rawGlobal` is
    /// the one field every test varies to make a specific fetch's response
    /// distinguishable from another's.
    nonisolated private static func configJSON(
        rawGlobal: String = "default_model = \"claude-sonnet-4-6\"\n",
        rawProject: String? = nil,
        restartRequiredKeys: [String] = ["bind", "token"]
    ) -> Data {
        let rawProjectJSON = rawProject.map { Self.jsonString($0) } ?? "null"
        let json = """
        {
          "effective": {"default_model": "claude-sonnet-4-6"},
          "provenance": {
            "default_model": {"source": "global", "locked": false}
          },
          "raw_global": \(Self.jsonString(rawGlobal)),
          "raw_project": \(rawProjectJSON),
          "status": {
            "bind": "127.0.0.1:7420",
            "token_set": false,
            "restart_required_keys": \(Self.jsonStringArray(restartRequiredKeys))
          }
        }
        """
        return Data(json.utf8)
    }

    /// Full JSON string-literal escaping (backslash, quote, newline, tab) —
    /// the raw TOML text these tests embed (`bind = "127.0.0.1:7420"`)
    /// contains literal double quotes that a bare newline substitution
    /// would leave unescaped, corrupting the surrounding JSON.
    nonisolated private static func jsonString(_ value: String) -> String {
        var escaped = value
        escaped = escaped.replacingOccurrences(of: "\\", with: "\\\\")
        escaped = escaped.replacingOccurrences(of: "\"", with: "\\\"")
        escaped = escaped.replacingOccurrences(of: "\n", with: "\\n")
        escaped = escaped.replacingOccurrences(of: "\t", with: "\\t")
        return "\"\(escaped)\""
    }

    nonisolated private static func jsonStringArray(_ values: [String]) -> String {
        "[" + values.map { Self.jsonString($0) }.joined(separator: ",") + "]"
    }

    private func makeClient(respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) -> CPClient {
        ConfigStubURLProtocol.reset(handler: respond)
        return CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: ConfigStubURLProtocol.session()
        )
    }

    // MARK: - Load happy path

    @Test func loadHappyPathDecodesFixtureIntoView() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/config" else { return (404, Data()) }
            return (200, Self.configJSON(rawGlobal: "default_model = \"claude-sonnet-4-6\"\n"))
        }
        let store = ConfigStore()

        await store.load(client: client, project: nil)

        #expect(store.selectedProject == nil)
        guard case .content(let view) = store.view else {
            Issue.record("expected .content, got \(store.view)")
            return
        }
        #expect(view.rawGlobal == "default_model = \"claude-sonnet-4-6\"\n")
        #expect(view.status.restartRequiredKeys == ["bind", "token"])
        #expect(view.provenance["default_model"]?.source == .global)
    }

    @Test func loadWithProjectSendsProjectQueryParam() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/config" else { return (404, Data()) }
            #expect(Self.queryValue("project", in: req) == "ws-1")
            return (200, Self.configJSON())
        }
        let store = ConfigStore()

        await store.load(client: client, project: "ws-1")

        #expect(store.selectedProject == "ws-1")
        #expect(store.view.value != nil)
    }

    // MARK: - Generation guard

    /// Starts a slow `load(project: "a")`, then fires a fast
    /// `load(project: "b")` before the first resolves — the first's
    /// eventual (STALE) response must never overwrite the second's.
    @Test func generationGuardDropsStaleResponseWhenProjectSwitchedMidFlight() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/config" else { return (404, Data()) }
            let project = Self.queryValue("project", in: req) ?? ""
            if project == "a" {
                Thread.sleep(forTimeInterval: 0.3) // artificially slow, superseded fetch
                return (200, Self.configJSON(rawGlobal: "STALE\n"))
            }
            return (200, Self.configJSON(rawGlobal: "FRESH\n"))
        }
        let store = ConfigStore()

        Task { await store.load(client: client, project: "a") }
        await expectEventually("the slow 'a' fetch has dispatched") {
            ConfigStubURLProtocol.hits("/api/config") >= 1
        }

        await store.load(client: client, project: "b")
        #expect(store.selectedProject == "b")
        #expect(store.view.value?.rawGlobal == "FRESH\n")

        // Give the stale 'a' fetch's artificial 0.3s delay generous room to
        // actually resolve well past this point, and confirm it never
        // clobbered the now-settled 'b' state.
        try? await Task.sleep(for: .milliseconds(600))
        #expect(store.view.value?.rawGlobal == "FRESH\n", "the stale generation's response must never land")
        #expect(store.selectedProject == "b")
    }

    // MARK: - 501 (read-only)

    @Test func fiveOhOneWriteSetsReadOnlyAndFixedMessageAndLeavesViewUntouched() async {
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, Self.configJSON(rawGlobal: "original\n"))
            }
            if req.url?.path == "/api/config/global" {
                return (501, Data(#"{"error":"editing config requires `rupu cp serve`"}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)

        let result = await store.saveGlobalRaw("original\nextra = 1\n", client: client)

        #expect(result == false)
        #expect(store.readOnly == true)
        #expect(store.saveError == "editing config requires `rupu cp serve`")
        #expect(store.saving == false)
        #expect(ConfigStubURLProtocol.hits("/api/config") == 1, "a failed write must never trigger a re-load")
        #expect(store.view.value?.rawGlobal == "original\n", "a failed write must never patch `view` locally")
    }

    // MARK: - 400 (validation failure)

    @Test func fourHundredWriteSurfacesBodyVerbatimAndReadOnlyStaysFalse() async {
        let errorBody = #"{"error":"invalid TOML at line 3: unexpected character"}"#
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, Self.configJSON(rawGlobal: "original\n"))
            }
            if req.url?.path == "/api/config/global" {
                return (400, Data(errorBody.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)

        let result = await store.saveGlobalRaw("not valid toml {{{", client: client)

        #expect(result == false)
        #expect(store.saveError == errorBody)
        #expect(store.readOnly == false)
        #expect(store.saving == false)
    }

    /// A second save attempt must clear the prior attempt's `saveError`
    /// before dispatching, per its "cleared on next attempt" contract —
    /// even one that itself fails again clears the OLD message first.
    @Test func saveErrorClearsAtStartOfNextAttempt() async {
        let globalCallCount = Counter()
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, Self.configJSON(rawGlobal: "original\n"))
            }
            if req.url?.path == "/api/config/global" {
                if globalCallCount.increment() == 1 {
                    return (400, Data(#"{"error":"first failure"}"#.utf8))
                }
                return (200, Data(#"{"ok":true,"restart_required":[]}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)

        _ = await store.saveGlobalRaw("bad", client: client)
        #expect(store.saveError == #"{"error":"first failure"}"#)

        let result = await store.saveGlobalRaw("original\n", client: client)
        #expect(result == true)
        #expect(store.saveError == nil)
    }

    // MARK: - Successful save re-loads

    @Test func successfulGlobalSaveTriggersReload() async {
        let getHits = Counter()
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                let n = getHits.increment()
                return (200, Self.configJSON(rawGlobal: n == 1 ? "original\n" : "original\nbind = \"x\"\n"))
            }
            if req.url?.path == "/api/config/global" {
                return (200, Data(#"{"ok":true,"restart_required":[]}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)
        #expect(ConfigStubURLProtocol.hits("/api/config") == 1)

        let result = await store.saveGlobalRaw("original\nbind = \"x\"\n", client: client)

        #expect(result == true)
        #expect(ConfigStubURLProtocol.hits("/api/config") == 2, "a successful save must re-load")
        #expect(store.view.value?.rawGlobal == "original\nbind = \"x\"\n")
        #expect(store.saving == false)
    }

    @Test func successfulProjectSaveRequiresSelectedProjectAndReloads() async {
        let projectPutHits = Counter()
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                #expect(Self.queryValue("project", in: req) == "ws-1")
                return (200, Self.configJSON(rawProject: "default_model = \"claude-opus-4-8\"\n"))
            }
            if req.url?.path == "/api/config/project/ws-1" {
                projectPutHits.increment()
                return (200, Data(#"{"ok":true}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: "ws-1")

        let result = await store.saveProjectRaw("default_model = \"claude-opus-4-8\"\n", client: client)

        #expect(result == true)
        #expect(projectPutHits.value == 1)
        #expect(ConfigStubURLProtocol.hits("/api/config") == 2)
    }

    @Test func saveProjectRawWithNoSelectedProjectIsANoOp() async {
        let client = makeClient { _ in (404, Data()) }
        let store = ConfigStore()
        // No `load` call — `selectedProject` stays nil.

        let result = await store.saveProjectRaw("x = 1\n", client: client)

        #expect(result == false)
        #expect(ConfigStubURLProtocol.hits("/api/config/project/ws-1") == 0)
    }

    @Test func successfulPolicySaveReloads() async {
        let policyPutHits = Counter()
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, Self.configJSON())
            }
            if req.url?.path == "/api/config/policy" {
                policyPutHits.increment()
                return (200, Data(#"{"ok":true}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)

        let result = await store.savePolicy(lock: ["default_model"], client: client)

        #expect(result == true)
        #expect(policyPutHits.value == 1)
        #expect(ConfigStubURLProtocol.hits("/api/config") == 2)
    }

    // MARK: - Restart-keys heuristic (Step 2 ruling)

    @Test func globalSaveTouchingARestartKeyLineSetsLastSaveRestartKeys() async {
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, Self.configJSON(rawGlobal: "bind = \"127.0.0.1:7420\"\n", restartRequiredKeys: ["bind", "token"]))
            }
            if req.url?.path == "/api/config/global" {
                return (200, Data(#"{"ok":true,"restart_required":[]}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)

        let result = await store.saveGlobalRaw("bind = \"0.0.0.0:9000\"\n", client: client)

        #expect(result == true)
        #expect(store.lastSaveRestartKeys == ["bind", "token"])
    }

    @Test func globalSaveTouchingAnUnrelatedLineLeavesLastSaveRestartKeysEmpty() async {
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, Self.configJSON(rawGlobal: "default_model = \"claude-sonnet-4-6\"\n", restartRequiredKeys: ["bind", "token"]))
            }
            if req.url?.path == "/api/config/global" {
                return (200, Data(#"{"ok":true,"restart_required":[]}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)

        let result = await store.saveGlobalRaw("default_model = \"claude-opus-4-8\"\n", client: client)

        #expect(result == true)
        #expect(store.lastSaveRestartKeys == [])
    }

    @Test func lastSaveRestartKeysClearsAtStartOfNextSaveAttempt() async {
        let currentRaw = RawBox("bind = \"127.0.0.1:7420\"\n")
        let client = makeClient { req in
            if req.url?.path == "/api/config" {
                return (200, Self.configJSON(rawGlobal: currentRaw.get(), restartRequiredKeys: ["bind", "token"]))
            }
            if req.url?.path == "/api/config/global" {
                return (200, Data(#"{"ok":true,"restart_required":[]}"#.utf8))
            }
            return (404, Data())
        }
        let store = ConfigStore()
        await store.load(client: client, project: nil)

        currentRaw.set("bind = \"0.0.0.0:9000\"\n")
        _ = await store.saveGlobalRaw("bind = \"0.0.0.0:9000\"\n", client: client)
        #expect(store.lastSaveRestartKeys == ["bind", "token"])

        // Second save touches an unrelated line only (relative to what the
        // FIRST save just persisted) — the FIRST save's restart-keys
        // banner must not linger.
        currentRaw.set("bind = \"0.0.0.0:9000\"\ndefault_model = \"x\"\n")
        _ = await store.saveGlobalRaw("bind = \"0.0.0.0:9000\"\ndefault_model = \"x\"\n", client: client)
        #expect(store.lastSaveRestartKeys == [])
    }
}
