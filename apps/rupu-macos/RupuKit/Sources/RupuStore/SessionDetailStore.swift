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
/// `RunDetailStore` uses for its own local-run fetches. Child-run rows
/// still navigate to `.runDetail(id:host:)` with `host: nil` for the same
/// reason (`SessionDetailScreen`'s job, not this store's).
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
@MainActor
@Observable
public final class SessionDetailStore {
    public private(set) var session: BlockState<APISessionRow> = .loading
    public private(set) var runs: BlockState<[APISessionRunRow]> = .loading
    public private(set) var transcript: [TranscriptEvent] = []
    public private(set) var focusedRunID: String?

    private let sessionID: String
    private let fetchSession: @Sendable () async throws -> APISessionRow
    private let fetchRuns: @Sendable () async throws -> [APISessionRunRow]
    private let fetchTranscript: @Sendable (String) async throws -> APITranscriptPage

    /// Monotonic token guarding `focusRun` against overlapping calls — see
    /// the type doc comment's "Overlapping `focusRun` calls" section.
    private var focusGeneration = 0

    /// Production entry point — `SessionDetailScreen` calls this.
    public convenience init(sessionID: String, client: CPClient) {
        self.init(
            sessionID: sessionID,
            fetchSession: { try await client.sessionDetail(id: sessionID) },
            fetchRuns: { try await client.sessionRuns(id: sessionID) },
            fetchTranscript: { path in try await client.transcript(path: path, host: nil) }
        )
    }

    /// Designated init — takes plain fetch closures rather than a `CPClient`
    /// directly, the same "fake client closures" seam `RunDetailStore` and
    /// `ActivityStore` already established for this codebase (`CPClient`
    /// itself has no protocol to mock). `internal`, not `public` — reached
    /// from tests via `@testable import RupuStore`, invisible outside this
    /// module.
    init(
        sessionID: String,
        fetchSession: @escaping @Sendable () async throws -> APISessionRow,
        fetchRuns: @escaping @Sendable () async throws -> [APISessionRunRow],
        fetchTranscript: @escaping @Sendable (String) async throws -> APITranscriptPage
    ) {
        self.sessionID = sessionID
        self.fetchSession = fetchSession
        self.fetchRuns = fetchRuns
        self.fetchTranscript = fetchTranscript
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

    private func loadSession() async {
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
