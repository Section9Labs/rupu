import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test doubles

/// Thread-safe call counter — same rationale as `ActivityStoreTests.Counter`
/// (a plain captured `var` can't cross into a `@Sendable` fetch closure
/// under Swift 6 strict concurrency); a fresh local copy per the codebase's
/// established "each test area gets its own" convention.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    func increment() { lock.withLock { v += 1 } }
    var value: Int { lock.withLock { v } }
}

private final class FlagBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v = false
    var value: Bool {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

private final class PathBox: @unchecked Sendable {
    private let lock = NSLock()
    private var v: String?
    var value: String? {
        get { lock.withLock { v } }
        set { lock.withLock { v = newValue } }
    }
}

/// Fakes the run-scoped `runSignalsFactory` seam: each call records a fresh
/// `AsyncStream<StreamSignal<CPEvent>>` + continuation pair so a test can
/// reach into the specific cycle `RunDetailStore.activate()`/`startRunStream`
/// actually built and drive it directly — the "FakeStream" the brief's Step
/// 1 calls for, standing in for `BackendController.makeRunEventStream`.
private final class RunStreamBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [AsyncStream<StreamSignal<CPEvent>>.Continuation] = []

    func factory() -> AsyncStream<StreamSignal<CPEvent>> {
        let (stream, continuation) = AsyncStream<StreamSignal<CPEvent>>.makeStream()
        lock.withLock { continuations.append(continuation) }
        return stream
    }

    var callCount: Int { lock.withLock { continuations.count } }
    var latest: AsyncStream<StreamSignal<CPEvent>>.Continuation { lock.withLock { continuations[continuations.count - 1] } }
}

/// Same idea as `RunStreamBox`, for the path-parameterized transcript tail —
/// `focusStep` rebuilds this against a new path every time focus moves.
private final class TailStreamBox: @unchecked Sendable {
    private let lock = NSLock()
    private var continuations: [(path: String, continuation: AsyncStream<StreamSignal<TranscriptEvent>>.Continuation)] = []

    func factory(path: String) -> AsyncStream<StreamSignal<TranscriptEvent>> {
        let (stream, continuation) = AsyncStream<StreamSignal<TranscriptEvent>>.makeStream()
        lock.withLock { continuations.append((path, continuation)) }
        return stream
    }

    var callCount: Int { lock.withLock { continuations.count } }
    var latestPath: String { lock.withLock { continuations[continuations.count - 1].path } }
    var latest: AsyncStream<StreamSignal<TranscriptEvent>>.Continuation { lock.withLock { continuations[continuations.count - 1].continuation } }
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - Fixture builders (hand-rolled, not golden-JSON: each test needs
// precise control over stepIDs / activeStepID / awaiting to exercise one
// specific code path in `RunDetailStore`).

private func usage() -> APIUsageSummary {
    APIUsageSummary(inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0, costUSD: nil, priced: false, runs: 0)
}

private func runRecord(
    id: String = "run-1",
    status: String = "running",
    awaiting: [APIAwaitingGate] = [],
    activeStepID: String? = nil,
    activeStepTranscriptPath: String? = nil
) -> APIRunRecord {
    APIRunRecord(
        id: id, workflowName: "nightly-health", status: status, workspaceID: "ws-1",
        startedAt: "2026-08-20T12:00:00Z", finishedAt: nil, errorMessage: nil,
        awaiting: awaiting, activeStepID: activeStepID, activeStepTranscriptPath: activeStepTranscriptPath,
        parentRunID: nil, permissionMode: "ask", finalOutput: nil
    )
}

private func stepResult(stepID: String, transcriptPath: String, success: Bool = true) -> APIStepResult {
    APIStepResult(stepID: stepID, runID: "run-1", transcriptPath: transcriptPath, output: "", success: success, skipped: false, kind: "linear", iterations: 0)
}

private func detail(run: APIRunRecord, steps: [APIStepResult] = []) -> APIRunDetail {
    APIRunDetail(run: run, steps: steps, usage: usage())
}

private func graph(run: APIRunRecord, units: [APIUnitRow] = []) -> APIRunGraph {
    APIRunGraph(run: run, workflow: APIStepDag(steps: []), stepResults: [], units: units, usage: usage())
}

private func netflow() -> APINetflow {
    APINetflow(flows: [], hosts: [], droppedTotal: 0, asnLoaded: false)
}

private func findings() -> APIFindings {
    APIFindings(findings: [], summary: APIFindingsSummary(total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0))
}

@MainActor
private func makeStore(
    isRemote: Bool = false,
    detailResult: @escaping @Sendable () async throws -> APIRunDetail,
    graphResult: @escaping @Sendable () async throws -> APIRunGraph = { graph(run: runRecord()) },
    netflowResult: @escaping @Sendable () async throws -> APINetflow = { netflow() },
    findingsResult: @escaping @Sendable () async throws -> APIFindings = { findings() },
    transcriptResult: @escaping @Sendable (String) async throws -> APITranscriptPage = { _ in APITranscriptPage(events: [], summary: nil) },
    runStreamBox: RunStreamBox? = nil,
    tailStreamBox: TailStreamBox? = nil
) -> RunDetailStore {
    var runSignalsFactory: (@Sendable () -> AsyncStream<StreamSignal<CPEvent>>)?
    if let runStreamBox {
        runSignalsFactory = { runStreamBox.factory() }
    }
    var transcriptTailFactory: (@Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>>)?
    if let tailStreamBox {
        transcriptTailFactory = { path in tailStreamBox.factory(path: path) }
    }
    return RunDetailStore(
        runID: "run-1",
        host: isRemote ? "remote-1" : nil,
        isRemote: isRemote,
        fetchDetail: detailResult,
        fetchGraph: graphResult,
        fetchNetflow: netflowResult,
        fetchFindings: findingsResult,
        fetchTranscript: transcriptResult,
        runSignalsFactory: runSignalsFactory,
        transcriptTailFactory: transcriptTailFactory
    )
}

// MARK: - (a) activate populates all four blocks independently

@MainActor @Test func activatePopulatesAllFourBlocksAndOneFailureLeavesOthersContent() async {
    let store = makeStore(
        detailResult: { detail(run: runRecord()) },
        graphResult: { throw StubError(description: "graph endpoint down") },
        netflowResult: { netflow() },
        findingsResult: { findings() }
    )

    await store.activate()

    #expect(store.detail.value != nil)
    #expect(store.netflow.value != nil)
    #expect(store.findings.value != nil)
    guard case .failed(let message) = store.graph else {
        Issue.record("expected graph to be .failed, got \(store.graph)")
        return
    }
    #expect(message.contains("graph endpoint down"))

    store.deactivate()
}

// MARK: - (b) stepAwaitingApproval updates liveStates

@MainActor @Test func stepAwaitingApprovalEventSetsGatePendingLiveState() async {
    let runBox = RunStreamBox()
    let store = makeStore(
        detailResult: { detail(run: runRecord()) }, // activeStepID nil -> no auto focusStep
        runStreamBox: runBox
    )

    await store.activate()
    #expect(runBox.callCount == 1)

    runBox.latest.yield(.connection(true))
    runBox.latest.yield(.event(.stepAwaitingApproval(runID: "run-1", stepID: "gate", reason: "deploy?")))
    try? await Task.sleep(for: .milliseconds(30))

    #expect(store.liveStates["gate"] == .gatePending)

    store.deactivate()
    runBox.latest.finish()
}

@Test @MainActor func stepLifecycleEventsMapToExpectedNodeStates() async {
    let runBox = RunStreamBox()
    let store = makeStore(
        detailResult: { detail(run: runRecord()) },
        runStreamBox: runBox
    )

    await store.activate()
    runBox.latest.yield(.connection(true))
    runBox.latest.yield(.event(.stepStarted(runID: "run-1", stepID: "plan", kind: "step", agent: "rupuso", host: nil)))
    runBox.latest.yield(.event(.stepCompleted(runID: "run-1", stepID: "review", success: false, durationMS: 10, host: nil)))
    runBox.latest.yield(.event(.stepSkipped(runID: "run-1", stepID: "skip-me", reason: "condition false")))
    try? await Task.sleep(for: .milliseconds(30))

    #expect(store.liveStates["plan"] == .running)
    #expect(store.liveStates["review"] == .done(success: false))
    #expect(store.liveStates["skip-me"] == .skipped)

    store.deactivate()
    runBox.latest.finish()
}

// Review fix: `stepFailed` (dispatch-level failure — connector error,
// timeout, panic; the step never got to report anything) is a distinct
// `CPEvent` case from `stepCompleted{success:false}` (the step ran and
// reported failure). Without reducing it, a dispatch-failed step stayed
// visibly `.running` forever, since `graph` is never refetched on the
// terminal event.
@MainActor @Test func stepFailedEventMapsToDoneFailureNodeState() async {
    let runBox = RunStreamBox()
    let store = makeStore(
        detailResult: { detail(run: runRecord()) },
        runStreamBox: runBox
    )

    await store.activate()
    runBox.latest.yield(.connection(true))
    runBox.latest.yield(.event(.stepFailed(runID: "run-1", stepID: "build", error: "connector timeout")))
    try? await Task.sleep(for: .milliseconds(30))

    #expect(store.liveStates["build"] == .done(success: false))

    store.deactivate()
    runBox.latest.finish()
}

// MARK: - (c) terminal event stops the stream and refreshes once

@MainActor @Test func terminalEventStopsStreamAndRefreshesDetailFindingsNetflowExactlyOnce() async {
    let runBox = RunStreamBox()
    let detailCalls = Counter()
    let findingsCalls = Counter()
    let netflowCalls = Counter()
    let terminated = FlagBox()

    let store = makeStore(
        detailResult: {
            detailCalls.increment()
            return detail(run: runRecord())
        },
        netflowResult: {
            netflowCalls.increment()
            return netflow()
        },
        findingsResult: {
            findingsCalls.increment()
            return findings()
        },
        runStreamBox: runBox
    )

    await store.activate()
    #expect(detailCalls.value == 1)
    #expect(findingsCalls.value == 1)
    #expect(netflowCalls.value == 1)

    runBox.latest.onTermination = { _ in terminated.value = true }
    runBox.latest.yield(.connection(true))
    runBox.latest.yield(.event(.runCompleted(runID: "run-1", status: "completed", finishedAt: "2026-08-20T13:00:00Z")))
    try? await Task.sleep(for: .milliseconds(60))

    #expect(detailCalls.value == 2)
    #expect(findingsCalls.value == 2)
    #expect(netflowCalls.value == 2)
    #expect(terminated.value == true)

    // A second terminal event (e.g. a stray `.unknown` racing a real one)
    // never double-fires the refresh.
    runBox.latest.yield(.event(.runFailed(runID: "run-1", error: "x", finishedAt: "y")))
    try? await Task.sleep(for: .milliseconds(30))
    #expect(detailCalls.value == 2)

    store.deactivate()
    runBox.latest.finish()
}

// Review fix: a terminal run event also stops any live transcript tail and
// reloads the focused step's transcript once via REST, so the feed ends up
// showing a complete final snapshot rather than whatever partial state the
// tail's connection happened to be in when the run ended.
@MainActor @Test func terminalEventStopsTailAndReloadsFocusedTranscriptSnapshotOnce() async {
    let runBox = RunStreamBox()
    let tailBox = TailStreamBox()
    let tailTerminated = FlagBox()
    let transcriptCalls = Counter()

    let store = makeStore(
        detailResult: {
            detail(run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"), steps: [])
        },
        transcriptResult: { _ in
            transcriptCalls.increment()
            return APITranscriptPage(events: [.assistantMessage(content: "final snapshot", thinking: nil)], summary: nil)
        },
        runStreamBox: runBox,
        tailStreamBox: tailBox
    )

    // "build" is active with no result yet -> activate()'s initial focus
    // starts a tail for it, with no REST fetch (per the tail-skip fix).
    await store.activate()
    #expect(transcriptCalls.value == 0)
    #expect(store.transcriptTailActive == true)

    tailBox.latest.onTermination = { _ in tailTerminated.value = true }
    runBox.latest.yield(.connection(true))
    tailBox.latest.yield(.connection(true))
    runBox.latest.yield(.event(.runCompleted(runID: "run-1", status: "completed", finishedAt: "2026-08-20T13:00:00Z")))
    try? await Task.sleep(for: .milliseconds(60))

    #expect(tailTerminated.value == true)
    #expect(store.transcriptTailActive == false)
    #expect(transcriptCalls.value == 1)
    #expect(store.transcript == [.assistantMessage(content: "final snapshot", thinking: nil)])

    store.deactivate()
    runBox.latest.finish()
    tailBox.latest.finish()
}

// MARK: - (d) focusStep on a tailing step skips the REST snapshot and lets
// the tail's byte-0 replay populate `transcript`
//
// Review fix: `/api/transcript/stream` (`TranscriptTail` on the server)
// replays the ENTIRE transcript JSONL from byte 0 on every connection, then
// tails — a strict superset of any REST snapshot taken just before it. The
// old test here asserted a REST fetch *and* a tail both landing in
// `transcript`, which is exactly the duplication bug: a real connection
// would have delivered the snapshot's own events again as part of the
// tail's replay, on top of what REST already put there. The fixed contract
// is: no REST fetch at all for a step about to be tailed; `transcript`
// starts empty and is populated purely from whatever the tail delivers.

@MainActor @Test func focusStepOnATailingStepSkipsRESTAndPopulatesTranscriptFromTheTailAlone() async {
    let runBox = RunStreamBox()
    let tailBox = TailStreamBox()
    let requestedPaths = Counter() // used purely to record how many transcript() calls fired

    let store = makeStore(
        detailResult: {
            detail(
                run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"),
                steps: [] // "build" has no finished result yet -> reads as running
            )
        },
        transcriptResult: { path in
            requestedPaths.increment()
            return APITranscriptPage(events: [.assistantMessage(content: "REST snapshot — must never appear", thinking: nil)], summary: nil)
        },
        runStreamBox: runBox,
        tailStreamBox: tailBox
    )

    // activate() itself resolves initial focus (activeStepID) and starts the tail.
    await store.activate()

    #expect(requestedPaths.value == 0)
    #expect(store.focusedTranscriptPath == "t/build.jsonl")
    #expect(store.transcript == [])
    #expect(store.transcriptTailActive == true)
    #expect(tailBox.callCount == 1)
    #expect(tailBox.latestPath == "t/build.jsonl")

    // Simulate the real contract: the tail's first connection replays the
    // whole transcript from byte 0 (here: one line that predates focus),
    // then tails new lines as they arrive — all as plain `.event`s, since
    // `TranscriptTail` doesn't distinguish "replay" from "live" at the
    // protocol level.
    tailBox.latest.yield(.connection(true))
    tailBox.latest.yield(.event(.assistantMessage(content: "replayed from byte 0", thinking: nil)))
    tailBox.latest.yield(.event(.assistantDelta(content: "first")))
    tailBox.latest.yield(.event(.assistantDelta(content: "second")))
    try? await Task.sleep(for: .milliseconds(30))

    #expect(store.transcript == [
        .assistantMessage(content: "replayed from byte 0", thinking: nil),
        .assistantDelta(content: "first"),
        .assistantDelta(content: "second"),
    ])
    // Still never fetched via REST at any point.
    #expect(requestedPaths.value == 0)

    store.deactivate()
    runBox.latest.finish()
    tailBox.latest.finish()
}

// Review fix: a tail *reconnect* replays the whole transcript from byte 0
// again (the identical contract as the first connection) — so the
// `resnapshot` closure `startTail` wires up must CLEAR `transcript`, never
// refetch it via REST, or the replay lands on top of what's already there
// and duplicates every line.
@MainActor @Test func tailReconnectReplayClearsTranscriptRatherThanDuplicatingIt() async {
    let runBox = RunStreamBox()
    let tailBox = TailStreamBox()

    let store = makeStore(
        detailResult: {
            detail(run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"), steps: [])
        },
        runStreamBox: runBox,
        tailStreamBox: tailBox
    )

    await store.activate()
    tailBox.latest.yield(.connection(true)) // pristine first connect -> no resnapshot
    tailBox.latest.yield(.event(.assistantMessage(content: "line 1", thinking: nil)))
    tailBox.latest.yield(.event(.assistantDelta(content: "line 2")))
    try? await Task.sleep(for: .milliseconds(20))
    #expect(store.transcript.count == 2)

    // Disconnect then reconnect: `StreamLifecycle` awaits `resnapshot()` to
    // completion before applying any further event on this same
    // continuation.
    tailBox.latest.yield(.connection(false))
    tailBox.latest.yield(.connection(true))
    try? await Task.sleep(for: .milliseconds(20))
    #expect(store.transcript.isEmpty) // cleared by the resnapshot closure, replay not yet arrived

    // The new connection's own replay, from byte 0, same content as before.
    tailBox.latest.yield(.event(.assistantMessage(content: "line 1", thinking: nil)))
    tailBox.latest.yield(.event(.assistantDelta(content: "line 2")))
    try? await Task.sleep(for: .milliseconds(20))

    // Landed exactly once — not appended on top of a stale prior copy.
    #expect(store.transcript == [.assistantMessage(content: "line 1", thinking: nil), .assistantDelta(content: "line 2")])

    store.deactivate()
    runBox.latest.finish()
    tailBox.latest.finish()
}

@MainActor @Test func focusStepSwitchingTearsDownThePriorTailBeforeStartingANewOne() async {
    let runBox = RunStreamBox()
    let tailBox = TailStreamBox()
    let terminatedFirst = FlagBox()

    let store = makeStore(
        detailResult: {
            detail(
                run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"),
                steps: [stepResult(stepID: "other", transcriptPath: "t/other.jsonl")]
            )
        },
        runStreamBox: runBox,
        tailStreamBox: tailBox
    )

    await store.activate() // focuses "build" (active, no result) -> tail #1
    #expect(tailBox.callCount == 1)
    tailBox.latest.onTermination = { _ in terminatedFirst.value = true }
    tailBox.latest.yield(.connection(true))
    try? await Task.sleep(for: .milliseconds(20))

    // Switch focus to a finished (non-running) step: the prior tail must be
    // torn down even though the new step gets no tail of its own.
    await store.focusStep("other")
    try? await Task.sleep(for: .milliseconds(20))

    #expect(terminatedFirst.value == true)
    #expect(store.transcriptTailActive == false)
    #expect(store.focusedTranscriptPath == "t/other.jsonl")
    // No second tail was ever opened for a non-running step.
    #expect(tailBox.callCount == 1)

    store.deactivate()
    runBox.latest.finish()
}

// Review fix: two overlapping `focusStep` calls — "stepA" stays a finished,
// non-tailing step (so its call awaits the REST snapshot, deliberately
// slow, and is still in flight when the second call starts), while "stepB"
// is marked running (tail-eligible) before either call fires. Since a
// tailing `focusStep` call never awaits (it skips the REST fetch entirely —
// see the "REST snapshot vs. tail" fix above), "stepB"'s call runs fully to
// completion, synchronously, before "stepA"'s slow fetch ever resolves.
// Before the generation-token fix, "stepA"'s eventually-resolving fetch
// would still have gone on to overwrite `transcript`/`focusedTranscriptPath`
// out from under "stepB" once it finally completed — this proves the fix
// discards it instead, and that only "stepB" (the tailing step) ever starts
// a tail.
@MainActor @Test func overlappingFocusStepCallsOnlyTheMostRecentOneAppliesItsResults() async {
    let runBox = RunStreamBox()
    let tailBox = TailStreamBox()

    let store = makeStore(
        detailResult: {
            detail(
                run: runRecord(),
                steps: [
                    stepResult(stepID: "stepA", transcriptPath: "t/a.jsonl"),
                    stepResult(stepID: "stepB", transcriptPath: "t/b.jsonl"),
                ]
            )
        },
        transcriptResult: { path in
            if path == "t/a.jsonl" {
                // Deliberately slow: still in flight when "stepB"'s call starts.
                try? await Task.sleep(for: .milliseconds(80))
                return APITranscriptPage(events: [.assistantMessage(content: "A snapshot — must never win", thinking: nil)], summary: nil)
            }
            return APITranscriptPage(events: [], summary: nil)
        },
        runStreamBox: runBox,
        tailStreamBox: tailBox
    )

    // `runRecord()`'s default `activeStepID` is nil, so `activate()`'s own
    // auto-focus (steps.last -> "stepB") runs once, fully, before it
    // returns — no overlap yet, and `liveStates` is still empty at that
    // point, so this first focus takes the REST path too (0 tails so far).
    await store.activate()
    #expect(tailBox.callCount == 0)

    // Mark only "stepB" running: "stepA" stays a finished step, so its
    // `focusStep` call below awaits the REST snapshot (the race this test
    // exercises); "stepB" is tail-eligible, so its call skips REST
    // entirely and starts a tail with no await to race at all.
    runBox.latest.yield(.connection(true))
    runBox.latest.yield(.event(.stepStarted(runID: "run-1", stepID: "stepB", kind: "step", agent: nil, host: nil)))
    try? await Task.sleep(for: .milliseconds(20))

    async let first: Void = store.focusStep("stepA")
    try? await Task.sleep(for: .milliseconds(10)) // let stepA's slow fetch actually begin
    async let second: Void = store.focusStep("stepB")
    _ = await (first, second)

    // Outlive stepA's slow fetch, in case it's still resolving.
    try? await Task.sleep(for: .milliseconds(120))

    #expect(store.focusedTranscriptPath == "t/b.jsonl")
    // stepB's tail owns `transcript` (cleared, then never fed any events in
    // this test); stepA's now-stale REST result — which would otherwise
    // have overwritten it 80ms later — was discarded by the generation
    // check before it ever touched `transcript`.
    #expect(store.transcript == [])
    #expect(store.transcriptTailActive == true)
    // Only "stepB"'s tail was ever started — "stepA" never reaches
    // `startTail` at all (non-tailing steps never do).
    #expect(tailBox.callCount == 1)
    #expect(tailBox.latestPath == "t/b.jsonl")

    store.deactivate()
    runBox.latest.finish()
    tailBox.latest.finish()
}

// MARK: - (e) deactivate cancels everything

@MainActor @Test func deactivateCancelsBothTheRunStreamAndTheTranscriptTail() async {
    let runBox = RunStreamBox()
    let tailBox = TailStreamBox()
    let runTerminated = FlagBox()
    let tailTerminated = FlagBox()

    let store = makeStore(
        detailResult: {
            detail(run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"), steps: [])
        },
        runStreamBox: runBox,
        tailStreamBox: tailBox
    )

    await store.activate()
    runBox.latest.onTermination = { _ in runTerminated.value = true }
    tailBox.latest.onTermination = { _ in tailTerminated.value = true }
    runBox.latest.yield(.connection(true))
    tailBox.latest.yield(.connection(true))
    try? await Task.sleep(for: .milliseconds(20))
    #expect(runTerminated.value == false)
    #expect(tailTerminated.value == false)

    store.deactivate()
    try? await Task.sleep(for: .milliseconds(30))

    #expect(runTerminated.value == true)
    #expect(tailTerminated.value == true)
    #expect(store.transcriptTailActive == false)

    // Events yielded after deactivate() are never applied.
    let before = store.liveStates
    runBox.latest.yield(.event(.stepStarted(runID: "run-1", stepID: "plan", kind: "step", agent: nil, host: nil)))
    try? await Task.sleep(for: .milliseconds(20))
    #expect(store.liveStates == before)

    runBox.latest.finish()
    tailBox.latest.finish()
}

// MARK: - (f) remote store never opens a stream

@MainActor @Test func remoteStoreNeverInvokesStreamFactoriesEvenWhenSupplied() async {
    let invoked = FlagBox()
    let store = RunDetailStore(
        runID: "run-1",
        host: "remote-1",
        isRemote: true,
        fetchDetail: { detail(run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl")) },
        fetchGraph: { graph(run: runRecord()) },
        fetchNetflow: { netflow() },
        fetchFindings: { findings() },
        fetchTranscript: { _ in APITranscriptPage(events: [], summary: nil) },
        runSignalsFactory: {
            invoked.value = true
            return AsyncStream { _ in }
        },
        transcriptTailFactory: { _ in
            invoked.value = true
            return AsyncStream { _ in }
        }
    )

    await store.activate()

    #expect(invoked.value == false)
    #expect(store.transcriptTailActive == false)
    #expect(store.isRemote == true)
    // REST blocks still populate normally for a remote run.
    #expect(store.detail.value != nil)

    store.deactivate()
}

// Review fix: the old `guard !isRemote else { return }` in `activate()`
// skipped BOTH the run stream AND the initial `focusStep` call for a remote
// store — but `GET /api/transcript?host=` works fine for a remote run (only
// the *tail* is local-only, and `focusStep` already guards that separately
// via `isRemote`). A remote run detail screen should still show its active
// step's transcript, fetched via REST, on first load.
@MainActor @Test func remoteStoreStillRunsInitialFocusStepViaRESTOnly() async {
    let transcriptCalls = Counter()
    let store = makeStore(
        isRemote: true,
        detailResult: {
            detail(run: runRecord(activeStepID: "build", activeStepTranscriptPath: "t/build.jsonl"), steps: [])
        },
        transcriptResult: { _ in
            transcriptCalls.increment()
            return APITranscriptPage(events: [.assistantMessage(content: "remote snapshot", thinking: nil)], summary: nil)
        }
        // No runStreamBox/tailStreamBox supplied -> `makeStore` passes `nil`
        // factories, matching production's `isRemote ? nil : ...`.
    )

    await store.activate()

    #expect(transcriptCalls.value == 1)
    #expect(store.focusedTranscriptPath == "t/build.jsonl")
    #expect(store.transcript == [.assistantMessage(content: "remote snapshot", thinking: nil)])
    #expect(store.transcriptTailActive == false)
    #expect(store.isRemote == true)

    store.deactivate()
}

// MARK: - Cancellation is benign (hotfix root cause B)

/// `detail`/`graph`/`netflow`/`findings` each independently leave their
/// `BlockState` untouched (still `.loading`, never `.failed`) when their
/// fetch closure is cancelled — the shape a superseded `RunDetailScreen
/// .task(id: runID)` produces mid-load.
@MainActor @Test func activateWithCancelledFetchesLeavesAllFourBlocksAsLoadingNeverFailed() async {
    let store = makeStore(
        detailResult: { throw CancellationError() },
        graphResult: { throw CancellationError() },
        netflowResult: { throw CancellationError() },
        findingsResult: { throw CancellationError() }
    )

    await store.activate()

    guard case .loading = store.detail else {
        Issue.record("expected detail to stay .loading through cancellation, got \(store.detail)")
        return
    }
    guard case .loading = store.graph else {
        Issue.record("expected graph to stay .loading through cancellation, got \(store.graph)")
        return
    }
    guard case .loading = store.netflow else {
        Issue.record("expected netflow to stay .loading through cancellation, got \(store.netflow)")
        return
    }
    guard case .loading = store.findings else {
        Issue.record("expected findings to stay .loading through cancellation, got \(store.findings)")
        return
    }

    store.deactivate()
}

/// `focusStep`'s transcript fetch, cancelled, must leave
/// `transcript`/`focusedTranscriptPath` exactly as they were — not blanked
/// (the fallback for a *real* failure) and not advanced to the new step's
/// (never-received) content.
@MainActor @Test func focusStepWithCancelledTranscriptFetchLeavesTranscriptAndFocusedPathUntouched() async {
    let store = makeStore(
        detailResult: {
            detail(
                run: runRecord(activeStepID: "plan"),
                steps: [stepResult(stepID: "plan", transcriptPath: "t/plan.jsonl", success: true)]
            )
        },
        transcriptResult: { _ in APITranscriptPage(events: [.assistantMessage(content: "plan output", thinking: nil)], summary: nil) }
    )
    await store.activate()
    #expect(store.focusedTranscriptPath == "t/plan.jsonl")
    #expect(store.transcript == [.assistantMessage(content: "plan output", thinking: nil)])

    let cancellingStore = makeStore(
        detailResult: {
            detail(
                run: runRecord(activeStepID: "plan"),
                steps: [
                    stepResult(stepID: "plan", transcriptPath: "t/plan.jsonl", success: true),
                    stepResult(stepID: "review", transcriptPath: "t/review.jsonl", success: true),
                ]
            )
        },
        transcriptResult: { path in
            if path == "t/review.jsonl" { throw CancellationError() }
            return APITranscriptPage(events: [.assistantMessage(content: "plan output", thinking: nil)], summary: nil)
        }
    )
    await cancellingStore.activate()
    #expect(cancellingStore.focusedTranscriptPath == "t/plan.jsonl")

    await cancellingStore.focusStep("review")
    // Cancellation left it exactly where it was before this call — still
    // "plan", never blanked to nil and never advanced to "review".
    #expect(cancellingStore.focusedTranscriptPath == "t/plan.jsonl")
    #expect(cancellingStore.transcript == [.assistantMessage(content: "plan output", thinking: nil)])

    store.deactivate()
    cancellingStore.deactivate()
}
