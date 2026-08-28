import Testing
import Foundation
import CoreGraphics
@testable import RupuBuilder
import RupuFlowKit
import RupuAPI
import RupuStore

// MARK: - Test infra (path/method-routing stub — see
// `ConfigStoreTests.ConfigStubURLProtocol`'s doc comment for the full
// generation-token rationale; duplicated here because it's `internal`/
// file-scoped to that sibling test target, same as every other store-test
// file's own copy of this rig).

/// Path+method-routing HTTP stub for `BuilderStore`'s four endpoints (`GET
/// /api/workflows/:name`, `GET /api/agents`, `GET /api/tools`, `PUT
/// /api/workflows/:name`, `POST /api/workflows/validate`). A monotonically
/// increasing generation token (stamped into each session via
/// `httpAdditionalHeaders`, read back off the request itself) makes a
/// straggling response from a PRIOR test's session harmlessly fail as
/// `.cancelled` instead of corrupting the CURRENT test's `handler`/
/// `pathHits` state under full-suite parallel load.
final class BuilderStubURLProtocol: URLProtocol {
    private static let generationHeader = "X-Builder-Stub-Generation"
    private static let lock = NSLock()
    nonisolated(unsafe) private static var handler: (@Sendable (URLRequest) -> (status: Int, body: Data))?
    nonisolated(unsafe) private static var pathHits: [String: Int] = [:]
    nonisolated(unsafe) private static var currentGeneration = 0

    static func session() -> URLSession {
        let generation = lock.withLock { currentGeneration }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [BuilderStubURLProtocol.self]
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

    static func hits(_ key: String) -> Int {
        lock.withLock { pathHits[key, default: 0] }
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
            let key = "\(request.httpMethod ?? "GET") \(path)"
            Self.lock.withLock { Self.pathHits[key, default: 0] += 1 }
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
struct BuilderStoreTests {
    private static let twoStepYAML = """
    name: nightly
    steps:
      - id: a
        agent: reviewer
        prompt: hi
      - id: b
        agent: reviewer
        prompt: bye
    """

    private static let anchorYAML = "name: nightly\nsteps: &s []\n"

    nonisolated private static func jsonString(_ value: String) -> String {
        var escaped = value
        escaped = escaped.replacingOccurrences(of: "\\", with: "\\\\")
        escaped = escaped.replacingOccurrences(of: "\"", with: "\\\"")
        escaped = escaped.replacingOccurrences(of: "\n", with: "\\n")
        escaped = escaped.replacingOccurrences(of: "\t", with: "\\t")
        return "\"\(escaped)\""
    }

    nonisolated private static func workflowDetailJSON(name: String = "nightly", yaml: String) -> Data {
        Data("""
        {"workflow": {"name": \(jsonString(name)), "inputs": {}}, "yaml": \(jsonString(yaml))}
        """.utf8)
    }

    /// Routes `GET /api/workflows/:name` to `yaml`, `GET /api/agents` to an
    /// empty catalog, `GET /api/tools` to an empty catalog, and lets `extra`
    /// handle everything else (PUT save / POST validate) — every test below
    /// that only cares about `activate()` can omit `extra` entirely.
    private func makeClient(
        detailYAML: String,
        detailName: String = "nightly",
        extra: @escaping @Sendable (URLRequest) -> (status: Int, body: Data)? = { _ in nil }
    ) -> CPClient {
        BuilderStubURLProtocol.reset { req in
            if let handled = extra(req) { return handled }
            guard let path = req.url?.path else { return (404, Data()) }
            if req.httpMethod == "GET", path == "/api/workflows/\(detailName)" {
                return (200, Self.workflowDetailJSON(name: detailName, yaml: detailYAML))
            }
            if req.httpMethod == "GET", path == "/api/agents" {
                return (200, Data("[]".utf8))
            }
            if req.httpMethod == "GET", path == "/api/tools" {
                return (200, Data(#"{"tools":[]}"#.utf8))
            }
            return (404, Data())
        }
        return CPClient(
            config: CPConfig(baseURL: URL(string: "https://cp.example.com")!),
            session: BuilderStubURLProtocol.session()
        )
    }

    private func makeStore(client: CPClient, name: String = "nightly") -> BuilderStore {
        BuilderStore(name: name, scopeKind: nil, scopeID: nil, client: client, pendingActions: PendingActions())
    }

    // MARK: - activate

    @Test func activateParsesAndIsNotDirty() async throws {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)

        await store.activate()

        #expect(store.phase == .ready)
        #expect(store.graph.nodes.count == 2)
        #expect(store.dirty == false)

        // Fixed point: re-parsing/re-serializing `canonicalYAML` must
        // reproduce itself exactly — the canonical form is stable.
        let reparsed = try YAMLParser.parse(store.canonicalYAML)
        let regraph = yamlToGraph(reparsed)
        guard case .object(let value) = graphToWorkflowObject(regraph) else {
            Issue.record("expected a valid re-serialize of the canonical YAML")
            return
        }
        #expect(YAMLEmitter.dump(value) == store.canonicalYAML)
    }

    @Test func unsupportedYAMLSurfacesHonestly() async {
        let client = makeClient(detailYAML: Self.anchorYAML)
        let store = makeStore(client: client)

        await store.activate()

        guard case .unsupported = store.phase else {
            Issue.record("expected .unsupported, got \(store.phase)")
            return
        }
        #expect(store.graph.nodes.isEmpty, "an unsupported load must never mutate the graph")
    }

    // MARK: - commit

    @Test func commitRejectsCycleWithoutMutating() async {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)
        // Deliberately never `activate()`s — `commit(_:)` is pure over
        // whatever `graph`/`dirty` the store already holds, so this exercises
        // it against the store's untouched initial state.
        let graphBefore = store.graph
        let dirtyBefore = store.dirty

        var aData = StepNodeData(id: "a", kind: .step)
        aData.agent = "x"
        aData.prompt = "hi"
        aData.next = ["b"]
        var bData = StepNodeData(id: "b", kind: .step)
        bData.agent = "x"
        bData.prompt = "hi"
        bData.next = ["a"]
        let cyclic = withDerivedEdges(
            meta: WorkflowMeta(name: "nightly"),
            nodes: [
                GraphNode(id: "a", data: aData, position: .zero),
                GraphNode(id: "b", data: bData, position: CGPoint(x: 1, y: 1)),
            ],
            loops: []
        )

        store.commit(cyclic)

        #expect(store.commitError != nil)
        #expect(store.graph == graphBefore, "a rejected commit must never mutate `graph`")
        #expect(store.dirty == dirtyBefore, "a rejected commit must never mark `dirty`")
    }

    // MARK: - Edit flow

    @Test func editFlowMarksDirtyAndRegeneratesYAML() async {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)
        await store.activate()
        #expect(store.dirty == false)

        store.addNode(kind: .step, at: .zero)

        #expect(store.dirty == true)
        #expect(store.canonicalYAML.contains("step-1"))
        #expect(store.selectedID == "step-1", "a freshly added node is selected, mirroring the web editor")
    }

    // MARK: - Save

    @Test func saveClearsDirtyOnSuccess() async {
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            guard req.httpMethod == "PUT", req.url?.path == "/api/workflows/nightly" else { return nil }
            return (200, Data(#"{"ok":true}"#.utf8))
        }
        let store = makeStore(client: client)
        await store.activate()
        store.addNode(kind: .step, at: .zero)
        #expect(store.dirty == true)

        let result = await store.save()

        #expect(result == true)
        #expect(store.dirty == false)
        #expect(store.saveError == nil)
    }

    @Test func saveFailureKeepsDirtyAndSurfacesError() async {
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            guard req.httpMethod == "PUT", req.url?.path == "/api/workflows/nightly" else { return nil }
            return (400, Data(#"{"error":"invalid workflow: missing required field"}"#.utf8))
        }
        let store = makeStore(client: client)
        await store.activate()
        store.addNode(kind: .step, at: .zero)
        #expect(store.dirty == true)

        let result = await store.save()

        #expect(result == false)
        #expect(store.dirty == true)
        #expect(store.saveError != nil)
    }

    // MARK: - Rename / Delete

    @Test func renameKeepsSelectionOnNewID() async {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)
        await store.activate()
        store.select("b")

        let renamed = store.rename(id: "b", to: "my-step")

        #expect(renamed == true)
        #expect(store.selectedID == "my-step")
        #expect(store.graph.nodes.contains { $0.id == "my-step" })
    }

    @Test func deleteClearsSelection() async {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)
        await store.activate()
        store.select("a")

        store.deleteSelection()

        #expect(store.selectedID == nil)
        #expect(!store.graph.nodes.contains { $0.id == "a" })
    }
}
