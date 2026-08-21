import Foundation
import Observation
import RupuAPI

/// Owns everything the Run Detail screen (Task 8) shows: four independent
/// REST-backed `BlockState`s (`detail`/`graph`/`netflow`/`findings` — one
/// failing never blanks the others, same contract as every other block on
/// this screen), the step graph's live `liveStates` (driven by a run-scoped
/// `CPEvent` stream for local runs only), and the focused step's transcript
/// feed (`transcript`, snapshot-then-tail).
///
/// **Local vs remote**: `isRemote` (`host` present and not `"local"`) turns
/// off both streams entirely — remote runs are REST-only this phase (Phase 5
/// brings Fleet streaming); `transcriptTailActive` stays `false` and
/// `activate()` never opens the run-scoped stream.
///
/// **Live semantics**: `stepStarted`→`.running`, `stepCompleted`→
/// `.done(success:)`, `stepAwaitingApproval`→`.gatePending`, `stepSkipped`→
/// `.skipped` (four `CPEvent` cases per the brief; every other case is
/// ignored here — including `stepFailed`, which the brief doesn't ask this
/// store to reduce, since `stepCompleted(success: false)` is the terminal
/// signal for a failed step in this event stream). A terminal run event
/// (`runCompleted`/`runFailed`) stops the run stream and refreshes
/// `detail`/`findings`/`netflow` exactly once — `didHandleTerminal` guards
/// against a duplicate terminal event (or an `.unknown` fallback racing a
/// real one) firing the refresh twice; `graph` is deliberately not
/// refreshed here (the brief's terminal-refresh list is `detail`/`findings`/
/// `netflow` only — the step graph's terminal state already arrived via the
/// `stepCompleted`/`stepSkipped` deltas that got it there).
///
/// **`focusStep`**: resolves the target step's transcript path — a finished
/// `APIStepResult` first, else a matching `APIUnitRow` (first match; a
/// fan-out step's units each have their own path, and this phase has no
/// per-unit focus UI yet), else `RunRecord.active_step_transcript_path` when
/// `stepID` is the run's current `activeStepID` — loads a snapshot via
/// `fetchTranscript`, then (local, and the step reads as currently running)
/// tails `/api/transcript/stream` for that path, appending events in
/// arrival order. Switching focus always tears the previous tail down
/// first, whether or not the new step gets one of its own.
///
/// **Deviation from the brief's literal
/// `init(runID:host:client:backend:)`-only surface**: that convenience init
/// is exactly what `RunDetailScreen` calls and is the only entry point
/// documented publicly, but the *designated* init (below it) takes plain
/// fetch closures plus optional stream factories — same "fake client
/// closures + FakeStream" seam `ActivityStore`'s tests already established
/// for this codebase (`CPClient` itself has no protocol to mock; a
/// real-but-stub-backed `CPClient` handles the REST side in tests, and a
/// scripted `AsyncStream<StreamSignal<T>>` factory stands in for
/// `BackendController`'s live stream without spinning up a real
/// `EmbeddedServer`/network connection). `internal`, not `public` — reached
/// from tests via `@testable import RupuStore`, invisible outside this
/// module.
@MainActor
@Observable
public final class RunDetailStore {
    public private(set) var detail: BlockState<APIRunDetail> = .loading
    public private(set) var graph: BlockState<APIRunGraph> = .loading
    public private(set) var netflow: BlockState<APINetflow> = .loading
    public private(set) var findings: BlockState<APIFindings> = .loading

    public private(set) var liveStates: [String: NodeState] = [:]

    public private(set) var transcript: [TranscriptEvent] = []
    public private(set) var transcriptTailActive: Bool = false
    public private(set) var focusedTranscriptPath: String?

    public let isRemote: Bool

    private let runID: String
    private let host: String?

    private let fetchDetail: @Sendable () async throws -> APIRunDetail
    private let fetchGraph: @Sendable () async throws -> APIRunGraph
    private let fetchNetflow: @Sendable () async throws -> APINetflow
    private let fetchFindings: @Sendable () async throws -> APIFindings
    private let fetchTranscript: @Sendable (String) async throws -> APITranscriptPage

    /// `nil` for a remote store (never called) or when `backend` has no live
    /// connection configured yet — both cases leave the run stream off.
    private let runSignalsFactory: (@Sendable () -> AsyncStream<StreamSignal<CPEvent>>)?
    private let transcriptTailFactory: (@Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>>)?

    private var runLifecycle: StreamLifecycle?
    private var tailLifecycle: StreamLifecycle?
    private var didHandleTerminal = false

    /// Production entry point — `RunDetailScreen` calls this. `isRemote` is
    /// derived once, here, from `host` (`nil` or `"local"` means the
    /// embedded/attached local backend; anything else is a Fleet node, REST
    /// only this phase — see api-facts.md's `host_id` convention, the same
    /// one `ActivityRow.localHost` already codifies).
    public convenience init(runID: String, host: String?, client: CPClient, backend: BackendController) {
        let isRemote = host != nil && host != "local"
        self.init(
            runID: runID,
            host: host,
            isRemote: isRemote,
            fetchDetail: { try await client.runDetail(id: runID, host: host) },
            fetchGraph: { try await client.runGraph(id: runID, host: host) },
            // `CPClient.runNetflow`/`runFindings` take no `host` parameter —
            // that's the current client surface (api-facts.md notes neither
            // endpoint is host-scoped server-side either), not an omission
            // here.
            fetchNetflow: { try await client.runNetflow(id: runID) },
            fetchFindings: { try await client.runFindings(id: runID) },
            fetchTranscript: { path in try await client.transcript(path: path, host: host) },
            runSignalsFactory: isRemote ? nil : RunDetailStore.makeRunSignalsFactory(backend: backend, runID: runID),
            transcriptTailFactory: isRemote ? nil : RunDetailStore.makeTranscriptTailFactory(backend: backend)
        )
    }

    init(
        runID: String,
        host: String?,
        isRemote: Bool,
        fetchDetail: @escaping @Sendable () async throws -> APIRunDetail,
        fetchGraph: @escaping @Sendable () async throws -> APIRunGraph,
        fetchNetflow: @escaping @Sendable () async throws -> APINetflow,
        fetchFindings: @escaping @Sendable () async throws -> APIFindings,
        fetchTranscript: @escaping @Sendable (String) async throws -> APITranscriptPage,
        runSignalsFactory: (@Sendable () -> AsyncStream<StreamSignal<CPEvent>>)?,
        transcriptTailFactory: (@Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>>)?
    ) {
        self.runID = runID
        self.host = host
        self.isRemote = isRemote
        self.fetchDetail = fetchDetail
        self.fetchGraph = fetchGraph
        self.fetchNetflow = fetchNetflow
        self.fetchFindings = fetchFindings
        self.fetchTranscript = fetchTranscript
        self.runSignalsFactory = runSignalsFactory
        self.transcriptTailFactory = transcriptTailFactory
    }

    /// Fires all four REST loads concurrently (independent `BlockState`s —
    /// one failing leaves the others exactly as they resolved), then, for a
    /// local run, starts the run-scoped event stream and focuses whichever
    /// step should be showing initially (`activeStepID`, else the last step
    /// with a result). Repeatable, like `ActivityStore.activate`: resets
    /// `didHandleTerminal` so a screen that deactivates and reactivates
    /// (e.g. navigating away and back) can observe a fresh terminal event
    /// again rather than staying silently latched from a previous visit.
    public func activate() async {
        didHandleTerminal = false

        async let detailLoad: Void = loadDetail()
        async let graphLoad: Void = loadGraph()
        async let netflowLoad: Void = loadNetflow()
        async let findingsLoad: Void = loadFindings()
        _ = await (detailLoad, graphLoad, netflowLoad, findingsLoad)

        guard !isRemote else { return }
        startRunStream()

        if let stepID = initialFocusStepID() {
            await focusStep(stepID)
        }
    }

    /// Stops the run stream and any active transcript tail. Idempotent and
    /// symmetric with `activate()`, matching every other store's
    /// activate/deactivate contract in this codebase.
    public func deactivate() {
        runLifecycle?.stop()
        runLifecycle = nil
        stopTail()
    }

    /// Switches the transcript feed to `stepID`: tears down any prior tail
    /// unconditionally (even if the new step never gets one of its own),
    /// resolves and loads a fresh snapshot, then — local run, and the step
    /// currently reads as running — starts tailing it.
    public func focusStep(_ stepID: String) async {
        stopTail()

        guard let path = resolveTranscriptPath(stepID: stepID) else {
            focusedTranscriptPath = nil
            transcript = []
            return
        }
        focusedTranscriptPath = path

        do {
            transcript = try await fetchTranscript(path).events
        } catch {
            // No dedicated failure surface for the feed this phase — per
            // "per-block independence", a transcript-load failure for the
            // focused step must never blank the graph/rails, which don't
            // depend on it. Blanking `transcript` (rather than leaving a
            // stale prior step's events on screen under the new step's
            // label) is the honest failure state here.
            transcript = []
        }

        guard !isRemote, isStepRunning(stepID), let transcriptTailFactory else { return }
        startTail(path: path, factory: transcriptTailFactory)
    }

    // MARK: - REST loads

    private func loadDetail() async {
        do {
            detail = .content(try await fetchDetail())
        } catch {
            detail = .failed(String(describing: error))
        }
    }

    private func loadGraph() async {
        do {
            graph = .content(try await fetchGraph())
        } catch {
            graph = .failed(String(describing: error))
        }
    }

    private func loadNetflow() async {
        do {
            netflow = .content(try await fetchNetflow())
        } catch {
            netflow = .failed(String(describing: error))
        }
    }

    private func loadFindings() async {
        do {
            findings = .content(try await fetchFindings())
        } catch {
            findings = .failed(String(describing: error))
        }
    }

    // MARK: - Run event stream

    private func startRunStream() {
        guard let runSignalsFactory else { return }
        runLifecycle?.stop()

        let lifecycle = StreamLifecycle()
        runLifecycle = lifecycle
        lifecycle.start(
            signals: runSignalsFactory(),
            resnapshot: { [weak self] in
                guard let self else { return }
                async let detailLoad: Void = self.loadDetail()
                async let graphLoad: Void = self.loadGraph()
                _ = await (detailLoad, graphLoad)
            },
            apply: { [weak self] event in
                self?.apply(event)
            }
        )
    }

    private func apply(_ event: CPEvent) {
        switch event {
        case .stepStarted(_, let stepID, _, _, _):
            liveStates[stepID] = .running
        case .stepCompleted(_, let stepID, let success, _, _):
            liveStates[stepID] = .done(success: success)
        case .stepAwaitingApproval(_, let stepID, _):
            liveStates[stepID] = .gatePending
        case .stepSkipped(_, let stepID, _):
            liveStates[stepID] = .skipped
        case .runCompleted, .runFailed:
            handleTerminal()
        default:
            break
        }
    }

    /// Guarded by `didHandleTerminal` so a duplicate terminal event never
    /// double-fires the refresh. Stops the run stream synchronously (no
    /// more deltas make sense once the run is over) and refreshes
    /// `detail`/`findings`/`netflow` — not `graph`, see the type doc
    /// comment — in one fire-and-forget `Task` (an event's `apply` callback
    /// itself is synchronous, so the refresh can't be `await`ed inline
    /// here).
    private func handleTerminal() {
        guard !didHandleTerminal else { return }
        didHandleTerminal = true
        runLifecycle?.stop()
        runLifecycle = nil

        Task { [weak self] in
            guard let self else { return }
            async let detailLoad: Void = self.loadDetail()
            async let findingsLoad: Void = self.loadFindings()
            async let netflowLoad: Void = self.loadNetflow()
            _ = await (detailLoad, findingsLoad, netflowLoad)
        }
    }

    // MARK: - Transcript tail

    private func startTail(path: String, factory: @escaping @Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>>) {
        let lifecycle = StreamLifecycle()
        tailLifecycle = lifecycle
        transcriptTailActive = true
        lifecycle.start(
            signals: factory(path),
            resnapshot: { [weak self] in
                guard let self else { return }
                // A reconnect may have missed events — reload the whole
                // snapshot and replace `transcript` wholesale (never
                // append), the same "resnapshot before any further delta"
                // contract `StreamLifecycle` gives every other consumer.
                await self.reloadTranscriptSnapshot(path: path)
            },
            apply: { [weak self] event in
                self?.transcript.append(event)
            }
        )
    }

    private func reloadTranscriptSnapshot(path: String) async {
        if let page = try? await fetchTranscript(path) {
            transcript = page.events
        }
    }

    private func stopTail() {
        tailLifecycle?.stop()
        tailLifecycle = nil
        transcriptTailActive = false
    }

    // MARK: - Helpers

    private func resolveTranscriptPath(stepID: String) -> String? {
        if case .content(let detail) = detail, let result = detail.steps.first(where: { $0.stepID == stepID }) {
            return result.transcriptPath
        }
        if case .content(let graph) = graph, let unit = graph.units.first(where: { $0.stepID == stepID }) {
            return unit.transcriptPath
        }
        if case .content(let detail) = detail, detail.run.activeStepID == stepID {
            return detail.run.activeStepTranscriptPath
        }
        return nil
    }

    /// "Running" for tail-start purposes: a live `.running` state wins
    /// outright; any other live state (`.done`/`.gatePending`/`.skipped`)
    /// says no just as outright. Only with no live state at all — e.g. the
    /// very first `focusStep` call, before any event has arrived for this
    /// step — does this fall back to "is this the run's current active step,
    /// and does it not have a finished result yet", the same inference
    /// `layoutGraph` avoids making on its own but that's reasonable here as
    /// a one-time bootstrap: initial focus, per the brief, is exactly
    /// `activeStepID`, and a step that's active with no result yet is
    /// running by definition.
    private func isStepRunning(_ stepID: String) -> Bool {
        if let live = liveStates[stepID] {
            return live == .running
        }
        guard case .content(let detail) = detail, detail.run.activeStepID == stepID else { return false }
        return !detail.steps.contains { $0.stepID == stepID }
    }

    private func initialFocusStepID() -> String? {
        guard case .content(let detail) = detail else { return nil }
        return detail.run.activeStepID ?? detail.steps.last?.stepID
    }

    // MARK: - Stream factories (backend bridging)

    /// Bridges `BackendController.makeRunEventStream(runID:)` into the
    /// `signalsFactory` shape `StreamLifecycle.start` needs — same two-phase
    /// `makeSignalBridge` recipe `RupuActivity.ActivityScreen` already
    /// documents for `makeFirehoseStream`: `onChange` is threaded into the
    /// stream's own `init` so a connection signal for a given attempt is
    /// always yielded before any frame of that same attempt, and a separate
    /// pump `Task` forwards decoded frames into the same ordered
    /// continuation. Called fresh on every `activate()` (via
    /// `startRunStream`), matching `ActivityStore`'s "rebuild, don't reuse"
    /// contract for a stream tied to a screen's visible lifetime.
    private static func makeRunSignalsFactory(
        backend: BackendController,
        runID: String
    ) -> @Sendable () -> AsyncStream<StreamSignal<CPEvent>> {
        {
            let (onChange, continuation, signals) = MainActor.assumeIsolated {
                StreamLifecycle.makeSignalBridge(CPEvent.self)
            }
            guard let stream = MainActor.assumeIsolated({
                backend.makeRunEventStream(runID: runID, onConnectionChange: onChange)
            }) else {
                continuation.finish()
                return signals
            }
            let pump = Task {
                for await event in stream.events() {
                    continuation.yield(.event(event))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
            return signals
        }
    }

    /// Same bridging shape as `makeRunSignalsFactory`, parameterized by
    /// `path` at call time (rather than baked in at store-construction time)
    /// since `focusStep` rebuilds the tail against a new path every time
    /// focus moves to a different step.
    private static func makeTranscriptTailFactory(
        backend: BackendController
    ) -> @Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>> {
        { path in
            let (onChange, continuation, signals) = MainActor.assumeIsolated {
                StreamLifecycle.makeSignalBridge(TranscriptEvent.self)
            }
            guard let stream = MainActor.assumeIsolated({
                backend.makeTranscriptStream(path: path, onConnectionChange: onChange)
            }) else {
                continuation.finish()
                return signals
            }
            let pump = Task {
                for await event in stream.events() {
                    continuation.yield(.event(event))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
            return signals
        }
    }
}
