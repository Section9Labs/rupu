import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test infra (generation-token-isolated stub — see
// `ConfigStoreTests.ConfigStubURLProtocol`'s doc comment for the full
// rationale; duplicated here because it's `internal`/file-scoped to that
// sibling test file, same as every other store-test file's own copy of
// this rig).

/// Path-routing HTTP stub for `ClaimsStore`'s three endpoints (`GET
/// /api/autoflows/claims`, `POST /api/autoflows/claims/release`, `POST
/// /api/autoflows/claims/requeue`). Routes on `request.url?.path`; a
/// monotonically increasing generation token (stamped into each session via
/// `httpAdditionalHeaders`, read back off the request itself) makes a
/// straggling response from a PRIOR test's session harmlessly fail as
/// `.cancelled` instead of corrupting the CURRENT test's `handler`/
/// `pathHits` state — see `DashboardStubURLProtocol`'s doc comment
/// (`DashboardStoreTests.swift`) for why this matters under full-suite
/// parallel load.
final class ClaimsStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-Claims-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var pathHits: [String: Int] = [:]
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ClaimsStubURLProtocol.self]
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

@Suite(.serialized)
@MainActor
struct ClaimsStoreTests {
    private func makeClient(respond: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)) -> CPClient {
        ClaimsStubURLProtocol.reset(handler: respond)
        return CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: ClaimsStubURLProtocol.session()
        )
    }

    /// Decodes a POST body as `{"issue_ref": "..."}` — the shape both
    /// `ReleaseClaimBody`/`RequeueClaimBody` share.
    nonisolated private static func issueRef(fromBody req: URLRequest) -> String? {
        guard let body = req.httpBodyStreamedOrDirect() else { return nil }
        guard let obj = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else { return nil }
        return obj["issue_ref"] as? String
    }

    /// Minimal single-row claim JSON — every field explicit so a test that
    /// wants a specific shape (nil-heavy vs fully populated) can override
    /// just what it cares about. Mirrors `APIClaimRow.CodingKeys` 1:1.
    nonisolated private static func claimJSON(
        issueRef: String = "github:Section9Labs/rupu/issues/101",
        repoRef: String = "github:Section9Labs/rupu",
        workflow: String = "issue-supervisor-dispatch",
        status: String = "await_human",
        updatedAt: String = "2026-08-25T12:00:00Z"
    ) -> String {
        """
        {"issue_ref": \(quoted(issueRef)), "issue_display_ref": null, "repo_ref": \(quoted(repoRef)),
         "issue_title": null, "issue_url": null, "workflow": \(quoted(workflow)), "status": \(quoted(status)),
         "last_run_id": null, "last_error": null, "last_summary": null, "pr_url": null,
         "claim_owner": null, "lease_expires_at": null, "updated_at": \(quoted(updatedAt))}
        """
    }

    nonisolated private static func quoted(_ s: String) -> String {
        "\"\(s)\""
    }

    // MARK: - Load

    @Test func loadDecodesTheFixtureIntoContent() async throws {
        let client = makeClient { req in
            guard req.url?.path == "/api/autoflows/claims" else { return (404, Data()) }
            return (200, try! Fixtures.data("autoflow_claims.json"))
        }
        let store = ClaimsStore(client: client)

        await store.load()

        guard case .content(let rows) = store.claims else {
            Issue.record("expected .content, got \(store.claims)")
            return
        }
        #expect(rows.count == 3)
        #expect(rows[0].issueRef == "github:Section9Labs/rupu/issues/101")
        #expect(rows[0].status == "await_human")
        #expect(rows[0].lastError == "panel review exceeded max_iterations")
        #expect(rows[0].prURL == "https://github.com/Section9Labs/rupu/pull/220")
        #expect(rows[1].claimOwner == "host:kuki:5521")
        #expect(rows[1].leaseExpiresAt == "2026-08-25T13:30:00Z")
        #expect(rows[2].issueDisplayRef == nil)
        #expect(rows[2].status == "eligible")
    }

    @Test func loadOfEmptyListYieldsEmptyNotContent() async {
        let client = makeClient { req in
            guard req.url?.path == "/api/autoflows/claims" else { return (404, Data()) }
            return (200, Data("[]".utf8))
        }
        let store = ClaimsStore(client: client)

        await store.load()

        guard case .empty = store.claims else {
            Issue.record("expected .empty, got \(store.claims)")
            return
        }
    }

    @Test func loadFailureSetsFailedState() async {
        let client = makeClient { _ in (500, Data()) }
        let store = ClaimsStore(client: client)

        await store.load()

        guard case .failed = store.claims else {
            Issue.record("expected .failed, got \(store.claims)")
            return
        }
    }

    // MARK: - Release (Step 1: success re-loads — stub asserts second GET)

    @Test func releaseSuccessConfirmsAndReloads() async {
        let issueRef = "github:Section9Labs/rupu/issues/101"
        let getHits = LockedCounter()
        let client = makeClient { req in
            if req.url?.path == "/api/autoflows/claims" {
                let n = getHits.increment()
                // After release, the claim is gone — the second GET reflects that.
                let body = n == 1 ? "[\(Self.claimJSON(issueRef: issueRef))]" : "[]"
                return (200, Data(body.utf8))
            }
            if req.url?.path == "/api/autoflows/claims/release" {
                #expect(Self.issueRef(fromBody: req) == issueRef)
                return (200, Data(#"{"released": true}"#.utf8))
            }
            return (404, Data())
        }
        let pendingActions = PendingActions()
        let store = ClaimsStore(client: client, pendingActions: pendingActions)
        await store.load()
        guard case .content(let initial) = store.claims else {
            Issue.record("expected initial .content")
            return
        }
        #expect(initial.count == 1)

        await store.release(issueRef: issueRef)

        #expect(ClaimsStubURLProtocol.hits("/api/autoflows/claims") == 2, "a successful release must re-load")
        guard case .empty = store.claims else {
            Issue.record("expected .empty after the reload reflects the release, got \(store.claims)")
            return
        }
        #expect(pendingActions.state(ActionKey(issueRef, .release)) == .confirmed)
    }

    /// Idempotent contract (`ClaimRow.from`/`release_claim` on the Rust
    /// side): releasing an already-untracked issue still returns `200` with
    /// `released: false`, not a 404 — this must still confirm the pending key
    /// and still trigger the reload, exactly like `released: true` does.
    @Test func releaseOfUntrackedStillConfirmsAndReloads() async {
        let issueRef = "github:Section9Labs/rupu/issues/999"
        let getHits = LockedCounter()
        let client = makeClient { req in
            if req.url?.path == "/api/autoflows/claims" {
                getHits.increment()
                return (200, Data("[]".utf8))
            }
            if req.url?.path == "/api/autoflows/claims/release" {
                #expect(Self.issueRef(fromBody: req) == issueRef)
                return (200, Data(#"{"released": false}"#.utf8))
            }
            return (404, Data())
        }
        let pendingActions = PendingActions()
        let store = ClaimsStore(client: client, pendingActions: pendingActions)
        await store.load()

        await store.release(issueRef: issueRef)

        #expect(ClaimsStubURLProtocol.hits("/api/autoflows/claims") == 2, "an idempotent released:false must still re-load")
        #expect(pendingActions.state(ActionKey(issueRef, .release)) == .confirmed, "released:false is still a successful 200 — still confirmed")
    }

    @Test func releaseFailureFailsTheKeyAndLeavesRowsUntouched() async {
        let issueRef = "github:Section9Labs/rupu/issues/101"
        let client = makeClient { req in
            if req.url?.path == "/api/autoflows/claims" {
                return (200, Data("[\(Self.claimJSON(issueRef: issueRef))]".utf8))
            }
            if req.url?.path == "/api/autoflows/claims/release" {
                return (500, Data())
            }
            return (404, Data())
        }
        let pendingActions = PendingActions()
        let store = ClaimsStore(client: client, pendingActions: pendingActions)
        await store.load()

        await store.release(issueRef: issueRef)

        #expect(ClaimsStubURLProtocol.hits("/api/autoflows/claims") == 1, "a failed release must never re-load")
        guard case .content(let rows) = store.claims else {
            Issue.record("expected rows to stay .content, got \(store.claims)")
            return
        }
        #expect(rows.count == 1)
        guard case .failed(let message) = pendingActions.state(ActionKey(issueRef, .release)) else {
            Issue.record("expected .failed")
            return
        }
        #expect(!message.isEmpty)
    }

    // MARK: - Requeue

    @Test func requeueSuccessConfirmsAndReloads() async {
        let issueRef = "github:Section9Labs/rupu/issues/102"
        let getHits = LockedCounter()
        let client = makeClient { req in
            if req.url?.path == "/api/autoflows/claims" {
                let n = getHits.increment()
                let status = n == 1 ? "retry_backoff" : "eligible"
                return (200, Data("[\(Self.claimJSON(issueRef: issueRef, status: status))]".utf8))
            }
            if req.url?.path == "/api/autoflows/claims/requeue" {
                #expect(Self.issueRef(fromBody: req) == issueRef)
                return (200, Data(#"{"wake_id": "wake_1"}"#.utf8))
            }
            return (404, Data())
        }
        let pendingActions = PendingActions()
        let store = ClaimsStore(client: client, pendingActions: pendingActions)
        await store.load()

        await store.requeue(issueRef: issueRef)

        #expect(ClaimsStubURLProtocol.hits("/api/autoflows/claims") == 2, "a successful requeue must re-load")
        #expect(pendingActions.state(ActionKey(issueRef, .requeue)) == .confirmed)
        #expect(store.claims.value?.first?.status == "eligible")
    }

    /// Step 1: "requeue error surfaces without dropping rows" — a failed
    /// requeue must fail the key AND leave `claims` exactly as it was
    /// (no reload attempted at all).
    @Test func requeueErrorSurfacesWithoutDroppingRows() async {
        let issueRef = "github:Section9Labs/rupu/issues/102"
        let client = makeClient { req in
            if req.url?.path == "/api/autoflows/claims" {
                return (200, Data("[\(Self.claimJSON(issueRef: issueRef))]".utf8))
            }
            if req.url?.path == "/api/autoflows/claims/requeue" {
                return (404, Data(#"{"error":"no claim tracked for that issue"}"#.utf8))
            }
            return (404, Data())
        }
        let pendingActions = PendingActions()
        let store = ClaimsStore(client: client, pendingActions: pendingActions)
        await store.load()

        await store.requeue(issueRef: issueRef)

        #expect(ClaimsStubURLProtocol.hits("/api/autoflows/claims") == 1, "a failed requeue must never re-load")
        guard case .content(let rows) = store.claims else {
            Issue.record("expected rows to stay .content, got \(store.claims)")
            return
        }
        #expect(rows.count == 1, "the failed requeue must never drop the already-visible row")
        guard case .failed(let message) = pendingActions.state(ActionKey(issueRef, .requeue)) else {
            Issue.record("expected .failed")
            return
        }
        #expect(!message.isEmpty)
    }

    // MARK: - Pending state, mid-flight

    @Test func releaseMarksTheKeyPendingBeforeTheRequestResolves() async {
        let issueRef = "github:Section9Labs/rupu/issues/101"
        let client = makeClient { req in
            if req.url?.path == "/api/autoflows/claims" {
                return (200, Data("[\(Self.claimJSON(issueRef: issueRef))]".utf8))
            }
            if req.url?.path == "/api/autoflows/claims/release" {
                Thread.sleep(forTimeInterval: 0.15)
                return (200, Data(#"{"released": true}"#.utf8))
            }
            return (404, Data())
        }
        let pendingActions = PendingActions()
        let store = ClaimsStore(client: client, pendingActions: pendingActions)
        await store.load()

        let key = ActionKey(issueRef, .release)
        #expect(pendingActions.state(key) == .idle)
        let task = Task { await store.release(issueRef: issueRef) }
        await expectEventually("release begins pending") {
            if case .pending = pendingActions.state(key) { return true }
            return false
        }
        _ = await task.value
        #expect(pendingActions.state(key) == .confirmed)
    }
}

/// Thread-safe call counter — same rationale as every other store test's
/// own copy of this pattern (`ConfigStoreTests.Counter`, `FleetStoreTests.
/// Counter`): a plain captured `var` can't cross into a `@Sendable` fetch
/// closure under Swift 6 strict concurrency. Named distinctly from those
/// files' own `private` (file-scoped) `Counter` to avoid any ambiguity
/// within this file.
private final class LockedCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
}

/// De-flakes "wait for an async effect to land" — same rationale/shape as
/// every other store-test file's own copy (`ConfigStoreTests.pollUntil`/
/// `expectEventually`, etc).
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

private extension URLRequest {
    /// `URLSession` converts a request's `httpBody` into an `httpBodyStream`
    /// internally before handing the request to a custom `URLProtocol` —
    /// `.httpBody` reads back `nil` at that point even though `CPClient.post`
    /// set it directly. Read whichever one is populated — same helper
    /// `CPClientWriteTests`/`ConfigModelsTests` each keep their own copy of
    /// (file-scoped `private`, so not shared across test targets/files).
    func httpBodyStreamedOrDirect() -> Data? {
        if let httpBody { return httpBody }
        guard let stream = httpBodyStream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        let bufferSize = 4096
        var buffer = [UInt8](repeating: 0, count: bufferSize)
        while stream.hasBytesAvailable {
            let read = stream.read(&buffer, maxLength: bufferSize)
            guard read > 0 else { break }
            data.append(buffer, count: read)
        }
        return data
    }
}
