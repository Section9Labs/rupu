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

    /// Routes through the internal `debounceInterval`-taking designated
    /// `init` (the Task 9 review Finding 2 test seam — see that
    /// initializer's doc comment in `BuilderStore.swift`) rather than the
    /// public 400ms convenience one, so the debounce tests below can use a
    /// short interval and stay fast/non-flaky.
    private func makeStore(client: CPClient, name: String = "nightly", debounceInterval: Duration) -> BuilderStore {
        BuilderStore(
            name: name, scopeKind: nil, scopeID: nil, client: client, pendingActions: PendingActions(),
            debounceInterval: debounceInterval
        )
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

    // MARK: - updateSelectedStep (Task 13 review, Finding 1)
    //
    // `updateSelectedStep` exists specifically so the Step form's debounced
    // field commits never read a FROZEN `StepNodeData` snapshot: each call
    // resolves `selectedID` and that node's CURRENT `data` fresh, mutates
    // only the one field the caller's closure touches, and applies through
    // `updateStep`. These two tests are the store-level coverage the review
    // asked for — `StepFormTab.swift`'s own field-commit closures have no
    // dedicated tests (this package doesn't render-test SwiftUI bodies; see
    // every other `RupuBuilderTests` file's own "pure seam only" convention),
    // so the guarantee is proven once here, at the layer that actually owns
    // it.

    @Test func updateSelectedStepAppliesSequentialSingleFieldMutationsWithoutClobbering() async {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)
        await store.activate()
        store.select("a")

        // Two SEPARATE calls, each touching only its own field — mirrors
        // two different debounced fields on the Step form firing one after
        // another. If either call read a stale snapshot of the whole node
        // (the Finding 1 bug) the first field's edit would vanish once the
        // second one lands.
        store.updateSelectedStep { $0.prompt = "updated prompt" }
        store.updateSelectedStep { $0.when = "always" }

        let node = store.graph.nodes.first { $0.id == "a" }
        #expect(node?.data.prompt == "updated prompt")
        #expect(node?.data.when == "always")
    }

    @Test func updateSelectedStepAfterARenameLandsOnTheRenamedNode() async {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)
        await store.activate()
        store.select("a")

        // A rename mid-edit — e.g. the STEP ID field committing on blur
        // while a debounced PROMPT edit is still in flight for the SAME
        // node — must not strand the follow-up mutation on the OLD id.
        #expect(store.rename(id: "a", to: "renamed-a"))
        #expect(store.selectedID == "renamed-a")

        store.updateSelectedStep { $0.prompt = "after rename" }

        let renamedNode = store.graph.nodes.first { $0.id == "renamed-a" }
        #expect(renamedNode?.data.prompt == "after rename")
        #expect(!store.graph.nodes.contains { $0.id == "a" })
    }

    @Test func updateSelectedStepIsANoOpWithNoSelection() async {
        let client = makeClient(detailYAML: Self.twoStepYAML)
        let store = makeStore(client: client)
        await store.activate()
        #expect(store.selectedID == nil)
        let graphBefore = store.graph

        store.updateSelectedStep { $0.prompt = "should never land" }

        #expect(store.graph == graphBefore)
    }

    // MARK: - Debounced revalidate (Task 9 review, Finding 2)
    //
    // Both tests use the `debounceInterval`-taking test-seam `init` with a
    // SHORT interval (30ms) rather than real wall-clock waits against the
    // production 400ms — the review comment offered either a bounded
    // wall-clock wait against 400ms or an injectable interval "if wall-clock
    // waits prove flaky"; the injectable interval was chosen up front since
    // it removes the flakiness risk entirely rather than discovering it
    // later, and keeps the suite fast. Each test still bounded-waits
    // (`pollUntil`/`expectEventually`) rather than a fixed `Task.sleep`, so
    // there is no hardcoded "surely long enough by now" duration anywhere.

    @Test func revalidateAfterCommitSetsServerValidFromExactlyOnePost() async {
        let validateHits = Counter()
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            guard req.httpMethod == "POST", req.url?.path == "/api/workflows/validate" else { return nil }
            validateHits.increment()
            return (200, Data(#"{"ok":true}"#.utf8))
        }
        let store = makeStore(client: client, debounceInterval: .milliseconds(30))
        await store.activate()
        #expect(store.serverValid == nil, "nothing has been checked yet")

        store.addNode(kind: .step, at: .zero)

        await expectEventually("the debounced revalidate has landed") {
            store.serverValid != nil
        }
        #expect(store.serverValid == true)
        #expect(validateHits.value == 1)
    }

    @Test func twoRapidCommitsWithinTheDebounceWindowStillProduceExactlyOnePost() async {
        let validateHits = Counter()
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            guard req.httpMethod == "POST", req.url?.path == "/api/workflows/validate" else { return nil }
            validateHits.increment()
            return (200, Data(#"{"ok":true}"#.utf8))
        }
        // A wider interval than the "after one commit" test above — wide
        // enough that the two back-to-back commits below (synchronous,
        // no `await` between them) are both well inside the SAME debounce
        // window on any reasonably loaded CI box.
        let store = makeStore(client: client, debounceInterval: .milliseconds(80))
        await store.activate()

        store.addNode(kind: .step, at: .zero) // first commit — kicks the debounce
        store.addNode(kind: .step, at: .zero) // second commit — cancels + replaces it

        await expectEventually("the debounced revalidate has landed") {
            store.serverValid != nil
        }
        #expect(validateHits.value == 1, "the first commit's debounce must have been cancelled, not just superseded in effect")
    }

    // MARK: - Run mode (Task 14)
    //
    // `enterRunMode(backend:)` awaits a real `RunDetailStore.activate()`
    // internally, so these route the same `BuilderStubURLProtocol` harness
    // through `RunDetailStore`'s own four REST endpoints in addition to
    // `BuilderStore`'s usual ones — proving the store-level integration is
    // cheap enough here that skipping it would have been the exception, not
    // the rule the Task 14 brief's "only if... cheaply" carve-out expected.
    // `BackendController()` (no live connection configured) is enough:
    // `RunDetailStore`'s run-scoped event stream factory degrades to an
    // already-finished, empty `AsyncStream` when there's no active
    // connection (see that factory's own doc comment), which is exactly
    // "no live updates" — fine for these REST-snapshot-only assertions.

    nonisolated private static func usageJSON() -> String {
        #"{"input_tokens":0,"output_tokens":0,"cached_tokens":0,"total_tokens":0,"cost_usd":null,"priced":false,"runs":0}"#
    }

    /// `rows` is passed in the exact order the stub should return it —
    /// callers put whichever row `latestRunID` should pick FIRST, mirroring
    /// the server's own newest-first paging that function assumes.
    nonisolated private static func runListRowsJSON(_ rows: [(id: String, workflowName: String)]) -> Data {
        let items = rows.map { row in
            """
            {"id":"\(row.id)","workflow_name":"\(row.workflowName)","status":"running","started_at":"2026-08-28T00:00:00Z","finished_at":null,"trigger":"manual","usage":\(usageJSON()),"turns":0,"duration_ms":null,"host_id":null}
            """
        }.joined(separator: ",")
        return Data("[\(items)]".utf8)
    }

    nonisolated private static func runRecordJSON(id: String, workflowName: String, status: String) -> String {
        """
        {"id":"\(id)","workflow_name":"\(workflowName)","status":"\(status)","workspace_id":"ws-1","started_at":"2026-08-28T00:00:00Z","finished_at":null,"error_message":null,"awaiting":[],"active_step_id":null,"active_step_transcript_path":null,"parent_run_id":null,"permission_mode":"ask","final_output":null}
        """
    }

    nonisolated private static func runDetailJSON(id: String, workflowName: String, status: String) -> Data {
        Data("""
        {"run":\(runRecordJSON(id: id, workflowName: workflowName, status: status)),"steps":[],"usage":\(usageJSON())}
        """.utf8)
    }

    nonisolated private static func runGraphJSON(id: String, workflowName: String, status: String) -> Data {
        Data("""
        {"run":\(runRecordJSON(id: id, workflowName: workflowName, status: status)),"workflow":{"steps":[]},"step_results":[],"units":[],"usage":\(usageJSON())}
        """.utf8)
    }

    nonisolated private static let netflowJSON = Data(#"{"flows":[],"hosts":[],"dropped_total":0,"asn_loaded":false}"#.utf8)
    nonisolated private static let findingsJSON = Data(#"{"findings":[],"summary":{"total":0,"critical":0,"high":0,"medium":0,"low":0,"info":0}}"#.utf8)

    /// Routes `RunDetailStore`'s own four REST endpoints for `runID` — every
    /// `enterRunMode` test below layers this into `makeClient`'s `extra`
    /// closure so the `RunDetailStore.activate()` `enterRunMode` awaits has
    /// somewhere real to land.
    nonisolated private static func runDetailEndpoints(runID: String, workflowName: String, status: String = "running") -> @Sendable (URLRequest) -> (status: Int, body: Data)? {
        { req in
            guard req.httpMethod == "GET", let path = req.url?.path else { return nil }
            switch path {
            case "/api/runs/\(runID)": return (200, Self.runDetailJSON(id: runID, workflowName: workflowName, status: status))
            case "/api/runs/\(runID)/graph": return (200, Self.runGraphJSON(id: runID, workflowName: workflowName, status: status))
            case "/api/runs/\(runID)/netflow": return (200, Self.netflowJSON)
            case "/api/runs/\(runID)/findings": return (200, Self.findingsJSON)
            default: return nil
            }
        }
    }

    @Test func enterRunModeFollowsTheNewestRunForThisWorkflowWhenNoneWasLaunchedThisSession() async {
        let runDetailStub = Self.runDetailEndpoints(runID: "run-newest", workflowName: "nightly", status: "running")
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            if req.httpMethod == "GET", req.url?.path == "/api/runs/workflows" {
                // Newest first, per `latestRunID`'s own "server already
                // paginates newest-first" assumption.
                return (200, Self.runListRowsJSON([
                    (id: "run-newest", workflowName: "nightly"),
                    (id: "run-older", workflowName: "nightly"),
                ]))
            }
            return runDetailStub(req)
        }
        let store = makeStore(client: client)
        await store.activate()

        await store.enterRunMode(backend: BackendController())

        #expect(store.followedRunID == "run-newest")
        #expect(store.followedRunStatus == "running")
    }

    @Test func enterRunModeIgnoresRunsBelongingToOtherWorkflows() async {
        let runDetailStub = Self.runDetailEndpoints(runID: "run-nightly", workflowName: "nightly")
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            if req.httpMethod == "GET", req.url?.path == "/api/runs/workflows" {
                return (200, Self.runListRowsJSON([
                    (id: "run-other", workflowName: "unrelated-workflow"),
                    (id: "run-nightly", workflowName: "nightly"),
                ]))
            }
            return runDetailStub(req)
        }
        let store = makeStore(client: client)
        await store.activate()

        await store.enterRunMode(backend: BackendController())

        #expect(store.followedRunID == "run-nightly")
    }

    @Test func enterRunModeLeavesNoFollowedRunWhenTheWorkflowHasNeverRun() async {
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            if req.httpMethod == "GET", req.url?.path == "/api/runs/workflows" {
                return (200, Data("[]".utf8))
            }
            return nil
        }
        let store = makeStore(client: client)
        await store.activate()

        await store.enterRunMode(backend: BackendController())

        #expect(store.followedRunID == nil)
        #expect(store.runOverlay == nil)
    }

    @Test func exitRunModeClearsTheFollowedRun() async {
        let runDetailStub = Self.runDetailEndpoints(runID: "run-1", workflowName: "nightly")
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            if req.httpMethod == "GET", req.url?.path == "/api/runs/workflows" {
                return (200, Self.runListRowsJSON([(id: "run-1", workflowName: "nightly")]))
            }
            return runDetailStub(req)
        }
        let store = makeStore(client: client)
        await store.activate()
        await store.enterRunMode(backend: BackendController())
        #expect(store.followedRunID == "run-1")

        store.exitRunMode()

        #expect(store.followedRunID == nil)
        #expect(store.runOverlay == nil)
        #expect(store.followedRunStatus == nil)
    }

    /// Review fix, Finding 2: two overlapping `enterRunMode` calls must
    /// leave EXACTLY one active followed run, never an orphaned one. The
    /// FIRST call's own `/api/runs/workflows` fetch is gated on a semaphore
    /// (held by the test, same "deterministic mid-flight window" idiom
    /// `ActivityStoreTests.activateRendersLocalImmediatelyThenMergesSlow
    /// OnlineRemoteHostProgressively`'s `remoteGate` already establishes for
    /// this codebase) so the first call is still suspended when the second
    /// one starts.
    ///
    /// **Not gated on the second call's own network round-trip completing**
    /// (an earlier draft of this test was, and it reliably paid the FULL
    /// 10s `firstRequestGate` timeout in the process — this custom
    /// `URLProtocol`'s loading system does not run two in-flight requests on
    /// the same session freely concurrently, so a request created AFTER an
    /// already-blocked one on the same stub can itself stall behind it).
    /// What actually matters for the guard is `runFollowGeneration`, bumped
    /// SYNCHRONOUSLY at the very top of `enterRunMode`, before its own first
    /// `await` — so the second call only needs to have been SCHEDULED and
    /// run its synchronous prefix, not have its network fetch resolve, for
    /// the generation to already be current. Polling the test-only
    /// `runFollowGeneration` counter (see that property's doc comment)
    /// proves exactly that, fast and deterministically, without needing the
    /// second request to race the first one through the stub at all.
    @Test func overlappingEnterRunModeCallsLeaveExactlyOneActiveFollowedRun() async {
        let requestIndex = Counter()
        let firstRequestGate = DispatchSemaphore(value: 0)
        let runADetailStub = Self.runDetailEndpoints(runID: "run-A", workflowName: "nightly")
        let runBDetailStub = Self.runDetailEndpoints(runID: "run-B", workflowName: "nightly")
        let client = makeClient(detailYAML: Self.twoStepYAML) { req in
            if req.httpMethod == "GET", req.url?.path == "/api/runs/workflows" {
                let index = requestIndex.increment()
                if index == 1 {
                    // Held by the test until the second (overlapping) call
                    // has already claimed the current generation — the 10s
                    // cap only bounds a hung test, it is not a timing knob.
                    _ = firstRequestGate.wait(timeout: .now() + 10)
                    return (200, Self.runListRowsJSON([(id: "run-A", workflowName: "nightly")]))
                }
                return (200, Self.runListRowsJSON([(id: "run-B", workflowName: "nightly")]))
            }
            return runADetailStub(req) ?? runBDetailStub(req)
        }
        let store = makeStore(client: client)
        await store.activate()

        let backend = BackendController()
        let taskA = Task { await store.enterRunMode(backend: backend) }
        await expectEventually("the first (gated) workflowRuns request has been issued") {
            BuilderStubURLProtocol.hits("GET /api/runs/workflows") == 1
        }
        #expect(store.runFollowGeneration == 1)

        let taskB = Task { await store.enterRunMode(backend: backend) }
        await expectEventually("the second call has claimed the current generation") {
            store.runFollowGeneration == 2
        }

        // Release the stale first call now — it will eventually resolve to
        // "run-A", but its captured generation (1) no longer matches
        // (`runFollowGeneration` is 2), so it must bail out without ever
        // touching `followedRunID`/`runFollowStore`.
        firstRequestGate.signal()

        await taskA.value
        await taskB.value

        #expect(store.followedRunID == "run-B", "the call holding the current generation must win, and a stale one resuming later must never clobber it")
    }
}

/// Thread-safe call counter — same rationale as `ConfigStoreTests`'s own
/// private copy of this shape (`private` to its own file): a plain captured
/// `var` can't cross into a `@Sendable` stub-handler closure under Swift 6
/// strict concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

/// De-flakes "wait for an async effect to land" — same rationale/shape as
/// every other store-test file's own copy (e.g. `ConfigStoreTests`'s).
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
