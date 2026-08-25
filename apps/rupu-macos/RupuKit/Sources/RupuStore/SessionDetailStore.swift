import Foundation
import Observation
import RupuAPI

/// Owns everything the Session Detail screen (Task 9) shows: the session's
/// own metadata (`session`), its ordered turns/runs (`runs`), and the
/// transcript feed for whichever run is currently focused (`transcript` —
/// newest run by default).
///
/// **Snapshot only, no tail**: unlike `RunDetailStore`, there is no live
/// stream here. A session's own "live" story — new turns arriving as the
/// user sends messages, tailing the active run's transcript — is Phase 3
/// work, gated on `send` actually existing. This phase is read-only end to
/// end, so `activate()`/`focusRun()` are both plain one-shot REST fetches;
/// there is nothing to start or stop, and no `deactivate()` to speak of.
///
/// **Local by construction**: `CPClient.sessionDetail`/`sessionRuns` take no
/// `host` parameter — there is no host-scoped session API yet — so every
/// session in this phase is local by construction, and `focusRun`'s
/// transcript fetch always passes `host: nil`, the same convention
/// `RunDetailStore` uses for its own local-run fetches. Child-run rows are
/// session-turn agent runs, not orchestrator runs — `GET /api/runs/:id`
/// 404s for them (hotfix root cause C, verified live) — so
/// `SessionDetailScreen` navigates them to `.agentRunDetail(id:
/// transcriptPath:host:)` with `host: nil`, never `.runDetail` (that's
/// `SessionDetailScreen`'s job, not this store's).
///
/// **Default focus**: `GET /api/sessions/:id/runs` returns a session's
/// turns in chronological (recorded) order, oldest first — mirrored by
/// `session_runs_from_json`'s "order preserved" contract on the Rust side —
/// so the newest run is simply `runs.last`, the same "last is newest"
/// convention `RunDetailStore.initialFocusStepID` relies on for
/// `detail.steps.last`.
///
/// **Overlapping `focusRun` calls — judgment call**: `RunDetailStore.
/// focusStep` guards overlapping calls with a generation token *because* it
/// also has a tail to tear down — an orphaned tail left running by a stale
/// call would silently keep appending a no-longer-focused step's events
/// forever. There is no tail here, so the only hazard two overlapping
/// `focusRun` calls pose is a plain "last write wins" race on
/// `transcript`/`focusedRunID` decided by whichever `await fetchTranscript`
/// happens to resume first — not by call order. That's still worth fixing
/// (a slow fetch for a run the user already navigated away from could
/// clobber a faster, more recent focus), so this keeps the same
/// `focusGeneration` token, just without any stream to stop alongside it:
/// a call whose captured generation no longer matches by the time its fetch
/// resolves discards its result outright rather than applying it, making
/// "only the most recent call wins" true by construction instead of by
/// scheduling luck.
///
/// **Mutations (Phase 3, Task 6)**: `send`/`archive`/`restore`, same
/// `PendingActions`-backed pending-state contract `RunDetailStore`'s
/// mutations already established (see that type's doc comment's
/// "Mutations" section) — one shared ledger (`BackendController.
/// pendingActions`), `begin()` right after firing the POST, `fail()` on a
/// thrown error, `confirm()` once the effect is provably visible.
///
/// - `send` is **never** resolved by a status transition (there's no
///   status feed here at all) — always an explicit `confirm(_:)` off the
///   response's own 2xx, mirroring `PendingActions`' own confirmation-table
///   note that `send` is one of the verbs `resolve(runID:observedStatus:)`
///   never touches.
/// - `archive`/`restore` mirror `RunDetailStore.archive`/`.restore`, but
///   with one real difference verified against the Rust source
///   (`crates/rupu-cp/src/api/sessions.rs`'s `mutate_session`): the local
///   session-archive/restore response is `{ok: true, id}` — **no**
///   `archived` field at all, unlike run archive/restore's `RunRecord`-
///   carrying response. Confirmation here is therefore off `response.ok`,
///   not `response.archived` (see `CPClient.archiveSession`'s doc comment).
///   `isArchived` is local, optimistic state for the same reason
///   `RunDetailStore.isArchived` is: there's no REST field to read it back
///   from, so it starts `false` and flips only once a successful
///   archive/restore call proves the effect.
///
/// **`isStopped` — reactive, not proactive** (judgment call): the server
/// checks a session's own `status` field for `"stopped"` before allowing a
/// send (`send_session`'s "best-effort pre-check", 409 `"session <id> is
/// stopped"`), but `APISessionRow.status` is deliberately *not* decoded on
/// the client (see that type's doc comment — it's a raw, varying-shape JSON
/// value server-side) — there is no REST signal this store can read
/// proactively to know a session is stopped before the operator tries to
/// send. `isStopped` is therefore local, optimistic state exactly like
/// `isArchived` above, except it flips the *other* direction: `false` at
/// construction, and set `true` only once a `send` attempt actually comes
/// back with that specific 409. A session stopped since the last visit but
/// never yet sent-to this session detail visit still shows the send box
/// until the first attempt reveals it — an accepted gap given the client
/// genuinely has no earlier signal, not a bug to silently paper over.
@MainActor
@Observable
public final class SessionDetailStore {
    public private(set) var session: BlockState<APISessionRow> = .loading
    public private(set) var runs: BlockState<[APISessionRunRow]> = .loading
    public private(set) var transcript: [TranscriptEvent] = []
    public private(set) var focusedRunID: String?

    /// Phase 3, Task 6: the shared pending-mutation ledger — see
    /// `BackendController.pendingActions`'s doc comment for why this is
    /// shared rather than private. Defaults to a fresh private instance so
    /// every pre-Task-6 test is unaffected; `SessionDetailScreen` passes
    /// `backend.pendingActions` explicitly.
    public let pendingActions: PendingActions

    /// See the type doc comment's "Mutations" section — local, optimistic,
    /// flipped only by a confirmed `archive()`/`restore()` call.
    public private(set) var isArchived = false

    /// See the type doc comment's "`isStopped` — reactive, not proactive"
    /// section — local, optimistic, flipped only by a `send()` call that
    /// comes back with the server's stopped-session 409.
    public private(set) var isStopped = false

    private let sessionID: String
    private let fetchSession: @Sendable () async throws -> APISessionRow
    private let fetchRuns: @Sendable () async throws -> [APISessionRunRow]
    private let fetchTranscript: @Sendable (String) async throws -> APITranscriptPage

    // MARK: - Mutations (Phase 3, Task 6)
    //
    // One closure per write route, same "fake client closures" seam every
    // fetch closure above (and `RunDetailStore`'s own mutation closures)
    // already use rather than storing a raw `CPClient`.
    private let postSend: @Sendable (_ prompt: String) async throws -> LaunchResponse
    private let postArchive: @Sendable () async throws -> RunControlResponse
    private let postRestore: @Sendable () async throws -> RunControlResponse

    /// Monotonic token guarding `focusRun` against overlapping calls — see
    /// the type doc comment's "Overlapping `focusRun` calls" section.
    private var focusGeneration = 0

    /// Production entry point — `SessionDetailScreen` calls this. `host:
    /// nil` throughout (send, archive, restore, transcript) — see the type
    /// doc comment's "Local by construction" note above; sessions have no
    /// host-scoped API surface yet this phase.
    public convenience init(sessionID: String, client: CPClient, backend: BackendController) {
        self.init(
            sessionID: sessionID,
            fetchSession: { try await client.sessionDetail(id: sessionID) },
            fetchRuns: { try await client.sessionRuns(id: sessionID) },
            fetchTranscript: { path in try await client.transcript(path: path, host: nil) },
            postSend: { prompt in try await client.sendToSession(id: sessionID, body: SendBody(prompt: prompt)) },
            postArchive: { try await client.archiveSession(id: sessionID) },
            postRestore: { try await client.restoreSession(id: sessionID) },
            pendingActions: backend.pendingActions
        )
    }

    /// Designated init — takes plain fetch/mutation closures rather than a
    /// `CPClient` directly, the same "fake client closures" seam
    /// `RunDetailStore` and `ActivityStore` already established for this
    /// codebase (`CPClient` itself has no protocol to mock). `internal`,
    /// not `public` — reached from tests via `@testable import RupuStore`,
    /// invisible outside this module.
    init(
        sessionID: String,
        fetchSession: @escaping @Sendable () async throws -> APISessionRow,
        fetchRuns: @escaping @Sendable () async throws -> [APISessionRunRow],
        fetchTranscript: @escaping @Sendable (String) async throws -> APITranscriptPage,
        postSend: @escaping @Sendable (_ prompt: String) async throws -> LaunchResponse = { _ in throw CPError.transport("postSend not wired") },
        postArchive: @escaping @Sendable () async throws -> RunControlResponse = { throw CPError.transport("postArchive not wired") },
        postRestore: @escaping @Sendable () async throws -> RunControlResponse = { throw CPError.transport("postRestore not wired") },
        pendingActions: PendingActions = PendingActions()
    ) {
        self.sessionID = sessionID
        self.fetchSession = fetchSession
        self.fetchRuns = fetchRuns
        self.fetchTranscript = fetchTranscript
        self.postSend = postSend
        self.postArchive = postArchive
        self.postRestore = postRestore
        self.pendingActions = pendingActions
    }

    /// Fires both REST loads concurrently (independent `BlockState`s — one
    /// failing leaves the other exactly as it resolved), then, if `runs`
    /// loaded with at least one row, focuses the newest one
    /// (`runs.last` — see the type doc comment's "Default focus" section).
    /// Repeatable, like every other store's `activate()` in this codebase —
    /// safe to call again (e.g. the screen reappearing) and it simply
    /// reloads everything and re-focuses.
    public func activate() async {
        async let sessionLoad: Void = loadSession()
        async let runsLoad: Void = loadRuns()
        _ = await (sessionLoad, runsLoad)

        if case .content(let rows) = runs, let newest = rows.last {
            await focusRun(newest)
        }
    }

    /// Switches the transcript feed to `run`'s snapshot. See the type doc
    /// comment's "Overlapping `focusRun` calls" section for why only the
    /// most recent call's result is ever applied.
    public func focusRun(_ run: APISessionRunRow) async {
        focusGeneration += 1
        let generation = focusGeneration

        let events: [TranscriptEvent]
        do {
            events = try await fetchTranscript(run.transcriptPath).events
        } catch {
            // Cancellation is benign — see `isCancellation`'s doc comment
            // and `RunDetailStore.focusStep`'s matching guard. A plain
            // early `return`: nothing below, including the generation
            // check, should run for a call that never produced a result.
            guard !isCancellation(error) else { return }
            // No dedicated failure surface for the feed this phase — same
            // "per-block independence" call `RunDetailStore.focusStep` makes:
            // a transcript-load failure for the focused run must never
            // blank `session`/`runs`, which don't depend on it. Blanking
            // `transcript` (rather than leaving a stale prior run's events
            // on screen under the new run's label) is the honest failure
            // state here — but only if this call is still the current one;
            // see the generation check below.
            events = []
        }

        // A newer `focusRun` call started while this one was suspended
        // awaiting `fetchTranscript` above — that later call owns the focus
        // now. Discard these now-stale results rather than applying them.
        guard generation == focusGeneration else { return }

        focusedRunID = run.runID
        transcript = events
    }

    // MARK: - Mutations (Phase 3, Task 6)

    /// Trims; an empty (or whitespace-only) prompt is a no-op — the Send
    /// button is disabled for this case too, but `send` itself stays safe
    /// to call directly (e.g. a text field's submit action) without
    /// duplicating the emptiness check at every call site. No key is ever
    /// `begin()`-ed for a no-op call.
    ///
    /// On success: `confirm`s immediately (there is no status feed to wait
    /// on — see the type doc comment), refreshes `runs`, then focuses the
    /// newly-created run so its transcript becomes the visible reply
    /// surface. If the freshly-reloaded `runs` doesn't yet contain the new
    /// run (a race against the row actually landing server-side) focus is
    /// simply left where it was — the next `runs` refresh will carry it.
    ///
    /// On failure: `fail`s the key with the server's message
    /// (`mutationErrorMessage`, same helper every other mutation in this
    /// codebase uses — verbatim except for the one 501 special case). A
    /// 409 stopped-session failure additionally flips `isStopped`, per the
    /// type doc comment's "`isStopped`" section.
    public func send(_ prompt: String) async {
        let trimmed = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        let key = ActionKey(sessionID, .send)
        pendingActions.begin(key)
        do {
            let response = try await postSend(trimmed)
            pendingActions.confirm(key)
            await loadRuns()
            if let runID = response.runID,
               case .content(let rows) = runs,
               let newRun = rows.first(where: { $0.runID == runID }) {
                await focusRun(newRun)
            }
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
            if isStoppedSessionError(error) {
                isStopped = true
            }
        }
    }

    /// Mirrors `RunDetailStore.archive()`, but confirms off `response.ok`
    /// rather than `response.archived` — see the type doc comment's
    /// "Mutations" section for why session archive/restore's real response
    /// shape has no `archived` field to read.
    public func archive() async {
        let key = ActionKey(sessionID, .archive)
        pendingActions.begin(key)
        do {
            let response = try await postArchive()
            if response.ok == true {
                isArchived = true
                pendingActions.confirm(key)
            }
            await loadSession()
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// Symmetric with `archive()` above.
    public func restore() async {
        let key = ActionKey(sessionID, .restore)
        pendingActions.begin(key)
        do {
            let response = try await postRestore()
            if response.ok == true {
                isArchived = false
                pendingActions.confirm(key)
            }
            await loadSession()
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// `true` only for a 409 whose body carries the server's exact
    /// stopped-session phrasing (`send_session`'s `"session <id> is
    /// stopped"`, `crates/rupu-cp/src/api/sessions.rs`) — a substring check
    /// on the raw HTTP body, not a decoded field (the body is a plain
    /// `{"error": "..."}` JSON string this client has no dedicated model
    /// for, same as every other `CPError.http` case in this codebase).
    private func isStoppedSessionError(_ error: Error) -> Bool {
        guard let cpError = error as? CPError, case .http(let status, let body) = cpError, status == 409 else {
            return false
        }
        return body.contains("is stopped")
    }

    /// Public (unlike `loadRuns`, which stays private behind `activate()`):
    /// the session header's failed-block Retry button reloads just this
    /// block — a full `activate()` would also refetch `runs` and refocus
    /// the newest run, discarding whichever run the user had focused.
    public func loadSession() async {
        do {
            session = .content(try await fetchSession())
        } catch {
            guard !isCancellation(error) else { return }
            session = .failed(String(describing: error))
        }
    }

    private func loadRuns() async {
        do {
            let rows = try await fetchRuns()
            runs = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            runs = .failed(String(describing: error))
        }
    }
}
