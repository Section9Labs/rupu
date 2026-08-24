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
/// `.skipped`, and — corrected after review; the original brief's four-case
/// list omitted this — `stepFailed`→`.done(success: false)` too.
/// `stepCompleted{success:false}` and `stepFailed{error}` are **not** the
/// same event: the Rust runner emits `stepCompleted{success:false}` when a
/// step actually ran and *reported* failure, and a separate `stepFailed`
/// for dispatch-level failure (connector error, timeout, panic — the step
/// never got to report anything). Without reducing `stepFailed` too, a
/// dispatch-failed step stayed visually `.running` in the graph forever —
/// `graph` is deliberately never refetched on the terminal event (see
/// below), so nothing else would ever correct it. Every other `CPEvent`
/// case is ignored here. A terminal run event
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
/// `stepID` is the run's current `activeStepID`.
///
/// **REST snapshot vs. tail — never both** (review fix): `/api/transcript/
/// stream` (`TranscriptTail` on the server) replays the *entire* transcript
/// JSONL from byte 0 on every connection, then tails new lines — so its
/// backlog is already a strict superset of any REST snapshot taken just
/// before it. When the step will be tailed (local run, and the step reads
/// as currently running), `focusStep` skips the REST `fetchTranscript` call
/// outright and lets `startTail` clear `transcript` and let the tail's own
/// byte-0 replay repopulate it from scratch — fetching the snapshot first
/// would just leave a duplicate prefix once the replay landed on top of it.
/// The REST snapshot path is still used for every non-tailing case: a
/// completed step, or any step on a remote run (remote is REST-only this
/// phase; see `isRemote` above). The same "clear, don't refetch" contract
/// covers `startTail`'s `resnapshot` closure — every tail *reconnect*
/// replays the whole transcript again, so clearing and letting the replay
/// repopulate is what avoids duplicating it a second time.
///
/// **Overlapping calls**: `focusStep` is Task 9's contractual interface for
/// repeated click-to-focus, so two calls can legitimately overlap (a click
/// while a slower prior fetch is still in flight). A monotonic
/// `focusGeneration` token, bumped on entry and captured locally, makes
/// "only the most recent call wins" true *by construction*: a call whose
/// captured generation no longer matches `focusGeneration` by the time its
/// `await fetchTranscript` resolves discards its results outright — it
/// never touches `transcript`/`focusedTranscriptPath` and never starts a
/// tail — rather than racing whichever `await` happens to resume first
/// (which, before this fix, could leave an earlier, now-stale step's tail
/// running forever, silently appending its events into the shared
/// `transcript` array). Only the winning call tears down the previous tail
/// and applies its own results; `startTail` also unconditionally stops any
/// existing `tailLifecycle` before assigning a new one, as a second,
/// construction-level guarantee against ever running two tails at once.
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

    /// Flows-composition Task 4: the step the tab panel currently follows —
    /// set by `select(step:)`, and by `activate()`'s own initial focus (via
    /// the same `select(step:)` call, so the two never drift). `nil` only
    /// when the workflow has no steps to focus at all (`initialFocusStepID()`
    /// returned `nil`).
    public private(set) var selectedStepID: String?

    /// Flows-composition Task 4: every `CPEvent` the run stream has
    /// delivered this activation, oldest first, capped at 500 (drop
    /// oldest — see `apply(_:)`) — the Events tab's raw feed. Cleared on
    /// `activate()`, same "repeatable" contract `didHandleTerminal`'s own
    /// reset already documents. Never populated for a remote run (no run
    /// stream at all — see `isRemote`).
    public private(set) var events: [CPEvent] = []

    private static let eventsCap = 500

    public let isRemote: Bool

    /// Phase 3, Task 5: the shared pending-mutation ledger — see
    /// `BackendController.pendingActions`'s doc comment for why this is
    /// shared rather than private. Defaults to a fresh private instance so
    /// every pre-Task-5 test (and any test that genuinely doesn't care
    /// about cross-screen sharing) is unaffected; `RunDetailScreen` passes
    /// `backend.pendingActions` explicitly.
    public let pendingActions: PendingActions

    /// Whether this run currently reads as archived. Phase 3, Task 5:
    /// `APIRunRecord` (and the server's `RunRecord` it decodes) carries no
    /// `archived` field at all — "archived" is purely which directory a
    /// run's `run.json` lives in, not a status on the record itself (see
    /// `RunStore::archive`/`list_archived` on the Rust side) — so there is
    /// no REST signal this store can read to learn a run's archived state.
    /// This is therefore local, optimistic state: `false` at construction
    /// (Task 8's `RunDetailScreen` never routes to an archived run from a
    /// context that already knows otherwise this phase) and flipped by a
    /// successful `archive()`/`restore()` call once its response proves the
    /// effect (`response.archived` matching). `availableVerbs` reads this
    /// to decide Archive vs. Restore.
    public private(set) var isArchived = false

    private let runID: String
    private let host: String?

    private let fetchDetail: @Sendable () async throws -> APIRunDetail
    private let fetchGraph: @Sendable () async throws -> APIRunGraph
    private let fetchNetflow: @Sendable () async throws -> APINetflow
    private let fetchFindings: @Sendable () async throws -> APIFindings
    private let fetchTranscript: @Sendable (String) async throws -> APITranscriptPage

    // MARK: - Mutations (Phase 3, Task 5)
    //
    // One closure per run-control route, mirroring the `fetchDetail`/
    // `fetchGraph`/... seam above rather than storing a raw `CPClient` —
    // same "fake client closures" testing seam this type's doc comment
    // already documents for the REST reads, extended to the write side.
    private let postApprove: @Sendable (_ gate: String, _ mode: String?) async throws -> RunControlResponse
    private let postReject: @Sendable (_ gate: String, _ reason: String?) async throws -> RunControlResponse
    private let postCancel: @Sendable (_ reason: String?) async throws -> RunControlResponse
    private let postPause: @Sendable () async throws -> RunControlResponse
    private let postResume: @Sendable () async throws -> RunControlResponse
    private let postArchive: @Sendable () async throws -> RunControlResponse
    private let postRestore: @Sendable () async throws -> RunControlResponse

    /// `nil` for a remote store (never called) or when `backend` has no live
    /// connection configured yet — both cases leave the run stream off.
    private let runSignalsFactory: (@Sendable () -> AsyncStream<StreamSignal<CPEvent>>)?
    private let transcriptTailFactory: (@Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>>)?

    private var runLifecycle: StreamLifecycle?
    private var tailLifecycle: StreamLifecycle?
    private var didHandleTerminal = false

    /// Monotonic token guarding `focusStep` against overlapping calls — see
    /// the type doc comment's "Overlapping calls" section.
    private var focusGeneration = 0

    /// Carry-over (Phase 3, Task 4): closes the "known parked residual" the
    /// final review flagged — `StreamLifecycle.stop()`'s cancellation is
    /// cooperative (see that type's doc comment), so a tail `apply` already
    /// mid-flight, or even one merely *queued* on the underlying
    /// `AsyncStream` when `stopTail()` runs, can still fire afterward and
    /// silently re-append into `transcript` after `handleTerminal`'s
    /// `reloadTranscriptSnapshot` has already replaced it wholesale with the
    /// run's final snapshot. Bumped in `stopTail()`; `startTail`'s `apply`
    /// closure captures the generation current at the moment its tail
    /// started and discards its own callback once that no longer matches,
    /// and `reloadTranscriptSnapshot` captures it before its `await` and
    /// discards a now-stale result the same way `focusStep`'s
    /// `focusGeneration` already does for overlapping `focusStep` calls —
    /// same recipe, different seam.
    private var tailTeardownGeneration = 0

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
            postApprove: { gate, mode in try await client.approveRun(id: runID, host: host, gate: gate, body: mode.map { ApproveBody(mode: $0) }) },
            postReject: { gate, reason in try await client.rejectRun(id: runID, host: host, gate: gate, body: RejectBody(reason: reason)) },
            postCancel: { reason in try await client.cancelRun(id: runID, host: host, body: reason.map { CancelBody(reason: $0) }) },
            postPause: { try await client.pauseRun(id: runID, host: host) },
            postResume: { try await client.resumeRun(id: runID, host: host) },
            postArchive: { try await client.archiveRun(id: runID, host: host) },
            postRestore: { try await client.restoreRun(id: runID, host: host) },
            runSignalsFactory: isRemote ? nil : RunDetailStore.makeRunSignalsFactory(backend: backend, runID: runID),
            transcriptTailFactory: isRemote ? nil : RunDetailStore.makeTranscriptTailFactory(backend: backend),
            pendingActions: backend.pendingActions
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
        postApprove: @escaping @Sendable (_ gate: String, _ mode: String?) async throws -> RunControlResponse = { _, _ in throw CPError.transport("postApprove not wired") },
        postReject: @escaping @Sendable (_ gate: String, _ reason: String?) async throws -> RunControlResponse = { _, _ in throw CPError.transport("postReject not wired") },
        postCancel: @escaping @Sendable (_ reason: String?) async throws -> RunControlResponse = { _ in throw CPError.transport("postCancel not wired") },
        postPause: @escaping @Sendable () async throws -> RunControlResponse = { throw CPError.transport("postPause not wired") },
        postResume: @escaping @Sendable () async throws -> RunControlResponse = { throw CPError.transport("postResume not wired") },
        postArchive: @escaping @Sendable () async throws -> RunControlResponse = { throw CPError.transport("postArchive not wired") },
        postRestore: @escaping @Sendable () async throws -> RunControlResponse = { throw CPError.transport("postRestore not wired") },
        runSignalsFactory: (@Sendable () -> AsyncStream<StreamSignal<CPEvent>>)?,
        transcriptTailFactory: (@Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>>)?,
        pendingActions: PendingActions = PendingActions()
    ) {
        self.runID = runID
        self.host = host
        self.isRemote = isRemote
        self.fetchDetail = fetchDetail
        self.fetchGraph = fetchGraph
        self.fetchNetflow = fetchNetflow
        self.fetchFindings = fetchFindings
        self.fetchTranscript = fetchTranscript
        self.postApprove = postApprove
        self.postReject = postReject
        self.postCancel = postCancel
        self.postPause = postPause
        self.postResume = postResume
        self.postArchive = postArchive
        self.postRestore = postRestore
        self.runSignalsFactory = runSignalsFactory
        self.transcriptTailFactory = transcriptTailFactory
        self.pendingActions = pendingActions
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
        events = []

        async let detailLoad: Void = loadDetail()
        async let graphLoad: Void = loadGraph()
        async let netflowLoad: Void = loadNetflow()
        async let findingsLoad: Void = loadFindings()
        _ = await (detailLoad, graphLoad, netflowLoad, findingsLoad)

        // Review fix: a remote run still gets its initial focus — `GET
        // /api/transcript?host=` works for remote same as local, and
        // `focusStep` itself already guards the *tail* on `isRemote` (see
        // its doc comment above). Only the run-scoped event stream is
        // local-only; skip that alone rather than the whole tail of
        // `activate()`.
        if !isRemote {
            startRunStream()
        }

        if let stepID = initialFocusStepID() {
            await select(step: stepID)
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

    // MARK: - Mutations (Phase 3, Task 5)

    /// The verbs the header/overflow menu may render as live buttons for
    /// the run's *current* status — never a fixed set. `.idle`/no `detail`
    /// yet yields no verbs at all, matching every other block's "nothing to
    /// show while loading" contract. Approve/reject are deliberately absent
    /// here: they render from `detail.run.awaiting` directly (the awaiting
    /// banner, one control pair per parked gate), not from this status-only
    /// derivation.
    public var availableVerbs: Set<ActionVerb> {
        guard case .content(let d) = detail else { return [] }
        let status = ActivityStatus.normalize(d.run.status)
        var verbs: Set<ActionVerb> = []
        switch status {
        case .running:
            verbs.insert(.cancel)
            verbs.insert(.pause)
        case .awaiting, .pending:
            verbs.insert(.cancel)
        case .paused:
            verbs.insert(.resume)
        case .completed, .failed, .rejected, .cancelled:
            verbs.insert(isArchived ? .restore : .archive)
        case .unknown:
            break
        }
        return verbs
    }

    /// **Marker-only** (approve/resume never confirm here): the POST's 200
    /// means "recorded", not "done" — the key stays `.pending` until an
    /// observed status transition proves the run actually left the gate.
    /// That observation arrives through `apply(_:)`'s live `runResumed`/
    /// `stepStarted`/... reductions (fast path, local runs only) or the
    /// `loadDetail()` refresh fired right after the POST below (slow path,
    /// every run including remote) — both call
    /// `pendingActions.resolve(runID:observedStatus:)`, which is what
    /// actually flips this key to `.confirmed`. That immediate refresh also
    /// re-checks `detail.run.awaiting` — a background worker on a very fast
    /// gate can already have cleared it by the time this call returns, so
    /// the banner has a chance to catch up right away rather than waiting on
    /// this run's next unrelated refresh.
    /// **Gate-scoped key** (fix round 1): `ActionKey.gate(runID:stepID:verb:)`,
    /// not a plain run-scoped key — a run showing more than one parked gate
    /// at once needs independent pending state per gate. See `ActionKey.gate`'s
    /// doc comment.
    public func approve(gate stepID: String, mode: String? = nil) async {
        let key = ActionKey.gate(runID: runID, stepID: stepID, verb: .approve)
        pendingActions.begin(key)
        do {
            _ = try await postApprove(stepID, mode)
            await loadDetail()
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// **Immediate**, **gate-scoped key** (same rationale as `approve`
    /// above): the response record already reflects the rejection —
    /// `handleImmediateResponse` resolves the key from it right away rather
    /// than waiting for the next observed event.
    public func reject(gate stepID: String, reason: String? = nil) async {
        let key = ActionKey.gate(runID: runID, stepID: stepID, verb: .reject)
        pendingActions.begin(key)
        do {
            let response = try await postReject(stepID, reason)
            await handleImmediateResponse(key, response)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// **Immediate**: status flips in the response (and a live runner pid is
    /// TERM'd server-side). 409 `AlreadyTerminal` surfaces as a normal
    /// `.failed` state via `mutationErrorMessage`.
    public func cancel(reason: String? = nil) async {
        let key = ActionKey(runID, .cancel)
        pendingActions.begin(key)
        do {
            let response = try await postCancel(reason)
            await handleImmediateResponse(key, response)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// **Immediate** plus a marker for detached runners. 409 when the run
    /// isn't currently running.
    public func pause() async {
        let key = ActionKey(runID, .pause)
        pendingActions.begin(key)
        do {
            let response = try await postPause()
            await handleImmediateResponse(key, response)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// **Marker-only**, same contract as `approve(gate:mode:)` above — stays
    /// `.pending` until an observed status transition (off `.paused`)
    /// resolves it. 501 without launcher runtime configured; 409 when the
    /// run isn't currently `paused`.
    public func resume() async {
        let key = ActionKey(runID, .resume)
        pendingActions.begin(key)
        do {
            _ = try await postResume()
            await loadDetail()
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// Immediate filesystem move. `PendingActions.resolve(runID:observedStatus:)`
    /// never confirms `.archive`/`.restore` (see that method's doc
    /// comment — they're not a run-status transition), so this confirms
    /// directly off `response.archived` rather than going through `resolve`.
    public func archive() async {
        let key = ActionKey(runID, .archive)
        pendingActions.begin(key)
        do {
            let response = try await postArchive()
            if response.archived == true {
                isArchived = true
                pendingActions.confirm(key)
            }
            await loadDetail()
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// Symmetric with `archive()` above.
    public func restore() async {
        let key = ActionKey(runID, .restore)
        pendingActions.begin(key)
        do {
            let response = try await postRestore()
            if response.archived == false {
                isArchived = false
                pendingActions.confirm(key)
            }
            await loadDetail()
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// Shared by every *immediate* mutation (reject/cancel/pause): resolves
    /// `key` off the response's own `confirmedStatus` when present (the
    /// local response shape, which already carries the post-mutation
    /// `RunRecord`) via the same `PendingActions.resolve` confirmation table
    /// every live-event reduction uses — so, e.g., a reject that actually
    /// routed to `on_reject` cancellation still confirms correctly.
    ///
    /// **The `ok == true` fallback branch** (fix round 1: previously
    /// untested): a remote-proxy response (`{ok, host_id}`, no `run` body —
    /// see `RunControlResponse`'s doc comment) carries no post-mutation
    /// status to check `resolve` against at all — the local host that
    /// actually ran the mutation never sent its record back across the
    /// proxy hop. This is accepted, not a gap to close: a 2xx from an
    /// *immediate* route (reject/cancel/pause, never approve/resume) is
    /// itself the server's proof the effect already happened, so confirming
    /// `key` directly off `ok == true` is the honest reading of that
    /// response shape, not a guess. Covered by
    /// `cancelConfirmsFromResponseOkAloneWhenNoRunBodyIsPresent`/
    /// `rejectConfirmsFromResponseOkAloneWhenNoRunBodyIsPresent` in
    /// `RunDetailStoreTests`.
    ///
    /// Refreshes `detail` either way, so the header/awaiting-banner catch up
    /// to the mutation's effect even on the remote-proxy path.
    private func handleImmediateResponse(_ key: ActionKey, _ response: RunControlResponse) async {
        if let confirmedStatus = response.confirmedStatus {
            pendingActions.resolve(runID: runID, observedStatus: .normalize(confirmedStatus))
        } else if response.ok == true {
            pendingActions.confirm(key)
        }
        await loadDetail()
    }

    /// Switches the transcript feed to `stepID`: resolves and loads a fresh
    /// snapshot, then — local run, and the step currently reads as running —
    /// starts tailing it. See the type doc comment's "Overlapping calls"
    /// section for why this only tears down / applies state once it's
    /// confirmed to still be the most recent call.
    public func focusStep(_ stepID: String) async {
        focusGeneration += 1
        let generation = focusGeneration

        guard let path = resolveTranscriptPath(stepID: stepID) else {
            // Nothing here has awaited yet — this call ran fully
            // synchronously up to this point, so it cannot have been
            // superseded by a later call. Always safe to tear down whatever
            // was focused before.
            stopTail()
            focusedTranscriptPath = nil
            transcript = []
            return
        }

        // Review fix: when this step is about to be tailed, skip the REST
        // snapshot entirely — `/api/transcript/stream`'s byte-0 replay is
        // already a superset of it (see the type doc comment's "REST
        // snapshot vs. tail" section). This branch never awaits, so — same
        // reasoning as the `guard let path` branch above — it cannot have
        // been superseded by a later call either; always safe to apply.
        let willTail = !isRemote && isStepRunning(stepID) && transcriptTailFactory != nil
        if willTail, let transcriptTailFactory {
            focusedTranscriptPath = path
            startTail(path: path, factory: transcriptTailFactory)
            return
        }

        let events: [TranscriptEvent]
        do {
            events = try await fetchTranscript(path).events
        } catch {
            // Cancellation (a rapid re-focus, or the whole screen tearing
            // down) is benign — leave `transcript`/`focusedTranscriptPath`
            // exactly as they were rather than blanking them. See
            // `isCancellation`'s doc comment. This is a plain early
            // `return`, not just a `continue` into the catch's fallback:
            // nothing below (including the generation check) should run
            // for a call that never really produced a result.
            guard !isCancellation(error) else { return }
            // No dedicated failure surface for the feed this phase — per
            // "per-block independence", a transcript-load failure for the
            // focused step must never blank the graph/rails, which don't
            // depend on it. Blanking `transcript` (rather than leaving a
            // stale prior step's events on screen under the new step's
            // label) is the honest failure state here — but only if this
            // call is still the current one; see the generation check below.
            events = []
        }

        // A newer `focusStep` call started while this one was suspended
        // awaiting `fetchTranscript` above — that later call owns the focus
        // now. Discard these now-stale results rather than applying them:
        // this is what guarantees only the most recent call ever mutates
        // `transcript`/`focusedTranscriptPath` or starts a tail.
        guard generation == focusGeneration else { return }

        stopTail()
        focusedTranscriptPath = path
        transcript = events
    }

    // MARK: - Selection (Task 4, flows-composition)

    /// Sets `selectedStepID` then awaits the existing `focusStep` machinery
    /// — the tab panel's Events/Transcript content both follow whatever this
    /// leaves in `selectedStepID`/`transcript`. `focusStep`'s own
    /// `focusGeneration` token already guards two overlapping calls (a rapid
    /// re-selection while a slower prior focus is still resolving), so this
    /// adds no guarding of its own — `selectedStepID` itself is a plain,
    /// synchronous assignment with nothing to race.
    public func select(step stepID: String) async {
        selectedStepID = stepID
        await focusStep(stepID)
    }

    /// The Events tab's filtered feed: with a step selected, only events
    /// whose `CPEvent.stepID` (see the static helper below) matches it — a
    /// run-level event (no step id at all) drops out, same as a step-scoped
    /// event for a *different* step. Web parity: `RunDetail.tsx:726-731`.
    /// With nothing selected, every accumulated event is returned as-is.
    public func eventsForSelection() -> [CPEvent] {
        guard let selectedStepID else { return events }
        return events.filter { Self.stepID(for: $0) == selectedStepID }
    }

    /// The step id a given `CPEvent` is scoped to, or `nil` for a run-level
    /// event (`runStarted`/`runCompleted`/.../`dispatchStarted`/
    /// `dispatchCompleted`/`.unknown`) that carries no step id at all.
    /// `public static` (not private) so `RupuRunDetail`'s Events tab content
    /// can reuse this exact mapping to render each row's step `Badge`
    /// instead of maintaining a second, driftable copy of this switch.
    public static func stepID(for event: CPEvent) -> String? {
        switch event {
        case .stepStarted(_, let stepID, _, _, _): return stepID
        case .stepWorking(_, let stepID, _, _): return stepID
        case .stepAwaitingApproval(_, let stepID, _): return stepID
        case .stepCompleted(_, let stepID, _, _, _): return stepID
        case .stepFailed(_, let stepID, _): return stepID
        case .stepSkipped(_, let stepID, _): return stepID
        case .unitStarted(_, let stepID, _, _, _, _, _): return stepID
        case .unitCompleted(_, let stepID, _, _, _, _, _, _): return stepID
        case .panelRound(_, let stepID, _, _, _): return stepID
        case .stepPaused(_, let stepID): return stepID
        case .stepResumed(_, let stepID): return stepID
        case .runStarted, .runCompleted, .runFailed, .runPaused, .runResumed,
             .dispatchStarted, .dispatchCompleted, .unknown:
            return nil
        }
    }

    // MARK: - REST loads

    private func loadDetail() async {
        do {
            let d = try await fetchDetail()
            detail = .content(d)
            // Phase 3, Task 5: the "slow path" half of the pending-mutation
            // contract — every successful detail refresh (initial
            // `activate()`, a terminal-event refresh, an immediate
            // mutation's own post-POST refresh, or just a screen
            // re-appearing) re-checks every currently-`.pending` key for
            // this run against the REST-observed status, so a marker verb
            // (approve/resume) still confirms even for a remote run (no
            // live stream at all) or if the live event that should have
            // confirmed it faster never arrived.
            pendingActions.resolve(runID: runID, observedStatus: .normalize(d.run.status))
        } catch {
            // Cancellation (e.g. `RunDetailScreen`'s `.task(id: runID)`
            // superseded by a `runID` change) is benign — leave `detail`
            // untouched rather than surfacing `.failed`. See
            // `isCancellation`'s doc comment.
            guard !isCancellation(error) else { return }
            detail = .failed(String(describing: error))
        }
    }

    private func loadGraph() async {
        do {
            graph = .content(try await fetchGraph())
        } catch {
            guard !isCancellation(error) else { return }
            graph = .failed(String(describing: error))
        }
    }

    private func loadNetflow() async {
        do {
            netflow = .content(try await fetchNetflow())
        } catch {
            guard !isCancellation(error) else { return }
            netflow = .failed(String(describing: error))
        }
    }

    private func loadFindings() async {
        do {
            findings = .content(try await fetchFindings())
        } catch {
            guard !isCancellation(error) else { return }
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
        // Flows-composition Task 4: every event the run stream delivers is
        // recorded for the Events tab, independent of whether the `switch`
        // below reduces it into `liveStates` — a `runResumed`/`stepPaused`/
        // ...event carries no `NodeState` transition but is still something
        // the operator should be able to see in the raw feed. Capped at 500,
        // dropping the OLDEST entry first so the feed always reflects the
        // most recent activity rather than truncating what just arrived.
        events.append(event)
        if events.count > Self.eventsCap {
            events.removeFirst(events.count - Self.eventsCap)
        }

        switch event {
        case .stepStarted(let runID, let stepID, _, _, _):
            liveStates[stepID] = .running
            // Phase 3, Task 5: the "fast path" half of the pending-mutation
            // contract. A step actually starting proves the run is
            // `.running` right now — in particular, that it left an
            // `.awaiting`/`.paused` gate — which is exactly what confirms a
            // pending `approve`/`resume`. See `PendingActions.resolve`'s
            // confirmation table.
            pendingActions.resolve(runID: runID, observedStatus: .running)
        case .stepCompleted(_, let stepID, let success, _, _):
            liveStates[stepID] = .done(success: success)
        case .stepAwaitingApproval(let runID, let stepID, _):
            liveStates[stepID] = .gatePending
            pendingActions.resolve(runID: runID, observedStatus: .awaiting)
        case .stepSkipped(_, let stepID, _):
            liveStates[stepID] = .skipped
        case .stepFailed(_, let stepID, _):
            // Dispatch-level failure (connector error, timeout, panic) —
            // distinct from `stepCompleted{success:false}`, which is only
            // emitted when the step actually ran and reported failure. See
            // the type doc comment's "Live semantics" section.
            liveStates[stepID] = .done(success: false)
        case .runResumed(let runID):
            // Same fast-path rationale as `stepStarted` above — the most
            // direct signal a run left a pause or an approval gate, per the
            // Rust runner's own doc comment ("resume emits RunResumed /
            // StepResumed").
            pendingActions.resolve(runID: runID, observedStatus: .running)
        case .runPaused(let runID):
            pendingActions.resolve(runID: runID, observedStatus: .paused)
        case .stepPaused(let runID, _):
            // Fix round 1 (folded minor): the step-scoped counterpart to
            // `runPaused` — a detached/step-level pause still implies the
            // run is now `.paused` for this store's confirmation purposes.
            pendingActions.resolve(runID: runID, observedStatus: .paused)
        case .stepResumed(let runID, _):
            // Step-scoped counterpart to `runResumed` — same fast-path
            // rationale.
            pendingActions.resolve(runID: runID, observedStatus: .running)
        case .runCompleted(let runID, let status, _):
            pendingActions.resolve(runID: runID, observedStatus: .normalize(status))
            handleTerminal()
        case .runFailed(let runID, _, _):
            pendingActions.resolve(runID: runID, observedStatus: .failed)
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
    /// Review fix: a terminal run event also stops any live transcript tail
    /// and reloads the focused step's transcript once via REST — the run is
    /// over, so there's nothing left to tail, and without this the feed
    /// could be left showing a truncated view (whatever arrived before the
    /// tail's underlying connection tore down) with `transcriptTailActive`
    /// stuck `true` forever.
    private func handleTerminal() {
        guard !didHandleTerminal else { return }
        didHandleTerminal = true
        runLifecycle?.stop()
        runLifecycle = nil
        stopTail()

        Task { [weak self] in
            guard let self else { return }
            async let detailLoad: Void = self.loadDetail()
            async let findingsLoad: Void = self.loadFindings()
            async let netflowLoad: Void = self.loadNetflow()
            _ = await (detailLoad, findingsLoad, netflowLoad)

            if let path = self.focusedTranscriptPath {
                await self.reloadTranscriptSnapshot(path: path)
            }
        }
    }

    // MARK: - Transcript tail

    private func startTail(path: String, factory: @escaping @Sendable (String) -> AsyncStream<StreamSignal<TranscriptEvent>>) {
        // Defense in depth alongside `focusStep`'s generation-token guard:
        // never let a new tail's assignment silently drop a still-running
        // previous one without stopping it first. Routing through
        // `stopTail()` (rather than `tailLifecycle?.stop()` alone) also
        // bumps `tailTeardownGeneration` here, so a straggling `apply` from
        // the tail just torn down is discarded the same way one from an
        // explicit `stopTail()` call (e.g. `handleTerminal`) is — see that
        // property's doc comment.
        stopTail()

        // Captured now, not read fresh inside `apply` below: this tail's own
        // callback must keep comparing against the generation *it* started
        // under, not whatever `tailTeardownGeneration` happens to be by the
        // time a given event fires.
        let generation = tailTeardownGeneration
        let lifecycle = StreamLifecycle()
        tailLifecycle = lifecycle
        transcriptTailActive = true
        // `/api/transcript/stream` replays the entire transcript JSONL from
        // byte 0 on every connection — including this very first one — so
        // starting from an empty `transcript` and letting that replay (plus
        // whatever's tailed live after it) repopulate it is what avoids a
        // duplicate on top of any REST snapshot `focusStep` might have left
        // behind for a *previous* step's focus.
        transcript = []
        lifecycle.start(
            signals: factory(path),
            resnapshot: { [weak self] in
                guard let self else { return }
                // A reconnect's new connection replays the whole transcript
                // from byte 0 again — the exact same contract as the initial
                // connection above, not a partial "missed events" catch-up.
                // Clearing (never refetching via REST) is what lets that
                // replay repopulate `transcript` from scratch instead of
                // duplicating whatever was already showing.
                await self.clearTranscriptForTailReconnect()
            },
            apply: { [weak self] event in
                self?.applyTailEvent(event, generation: generation)
            }
        )
    }

    /// `StreamLifecycle.stop()`'s cancellation is cooperative, not
    /// preemptive (see that type's doc comment) — a signal already queued
    /// on the underlying `AsyncStream`, or an `apply` call already
    /// dispatched, can still land after `stopTail()` runs. Comparing the
    /// generation captured when this tail started against the current
    /// `tailTeardownGeneration` is what makes discarding that straggler true
    /// *by construction* rather than a timing accident: `stopTail()` always
    /// runs (directly, or via `startTail`'s own teardown-then-restart) at
    /// every point this tail could become stale, and always bumps it first.
    private func applyTailEvent(_ event: TranscriptEvent, generation: Int) {
        guard generation == tailTeardownGeneration else { return }
        transcript.append(event)
    }

    private func clearTranscriptForTailReconnect() async {
        transcript = []
    }

    /// REST reload, replacing `transcript` wholesale (never appended) —
    /// used by `handleTerminal` to leave the feed showing a complete final
    /// snapshot once the run (and any tail of it) is over.
    ///
    /// Captures `tailTeardownGeneration` before the `await` and discards its
    /// own result if it no longer matches by the time the fetch resolves —
    /// same "most recent teardown wins" guarantee `applyTailEvent` gives the
    /// tail side of this race, applied to the reload itself: a `focusStep`
    /// call (or another `stopTail()`) that lands while this fetch is still
    /// in flight owns the feed now, and this now-stale snapshot must not
    /// clobber it.
    private func reloadTranscriptSnapshot(path: String) async {
        let generation = tailTeardownGeneration
        guard let page = try? await fetchTranscript(path) else { return }
        guard generation == tailTeardownGeneration else { return }
        transcript = page.events
    }

    private func stopTail() {
        tailTeardownGeneration += 1
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
