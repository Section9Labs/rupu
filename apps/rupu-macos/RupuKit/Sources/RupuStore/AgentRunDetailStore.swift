import Foundation
import Observation
import RupuAPI

/// Owns the read-only transcript feed for a standalone agent run (hotfix
/// root cause C): a row from `GET /api/runs/agents` with `source != "session"`
/// (or `"session"` with no `session_id`) is **not** an orchestrator run —
/// `GET /api/runs/:id` 404s for it every time, verified live against a real
/// `rupu cp serve` for exactly this row shape. There is no run-detail
/// record, no step graph, no netflow/findings for a row like this — the
/// only endpoint honestly addressable for it is `GET /api/transcript?path=`,
/// the same one `RunDetailStore`/`SessionDetailStore` already use for their
/// own transcript feeds. This store fetches nothing else.
///
/// **`transcriptPath == nil`**: some agent runs never recorded a transcript
/// at all (see `agent_run_rows.json`'s `run-11` fixture) — `resolvedPath`
/// stays `nil` and `activate()` goes straight to `.empty`, no network call,
/// and `AgentRunDetailScreen` renders an honest "no transcript recorded"
/// label rather than a load spinner that would never resolve. A *fresh*
/// launch (Phase 3 final-review fix, Important 3) is the one other case a
/// `nil` `transcriptPath` covers: `LauncherStore.performLaunch`'s `.agentRun`
/// route only ever gets a `run_id`/`host_id` back, never a transcript path,
/// yet the run it just started may well already have one. `resolveTranscriptPath`
/// — wired only when the constructor's own `transcriptPath` is `nil` — makes
/// one best-effort attempt to find it before falling back to the same empty
/// state: `GET /api/runs/:id` 404s for a standalone agent run (hotfix root
/// cause C, see this type's own doc comment above), so that's not tried;
/// the only other honest read is the agent-runs list itself
/// (`GET /api/runs/agents`), scanned for the matching `run_id` — see
/// `lookUpTranscriptPath`'s doc comment for why one bounded page is the
/// "cheapest honest variant" rather than paging further. `resolvedPath`
/// exposes whichever path (constructor-supplied or resolved) `activate()`
/// ended up using, so `AgentRunDetailScreen` can tell "genuinely no
/// transcript" apart from "resolved a path, but it has zero events so far".
///
/// **Cancellation-safe** (hotfix root cause B): a `CancellationError`/
/// `CPError.cancelled` from `fetchTranscript` (e.g. this screen's
/// `.task(id: runID)` superseded by navigating to a different run before
/// the fetch lands) leaves `transcript` exactly as it was — never `.failed`.
/// See `isCancellation`'s doc comment. `resolveTranscriptPath` itself is
/// best-effort in the same spirit: a failure (cancellation included) is
/// swallowed and treated as "couldn't resolve" — falling through to `.empty`
/// on a first activate rather than surfacing `.failed` for what is,
/// honestly, a bonus lookup. But "couldn't resolve" is not "resolved to
/// nothing": on a re-activate, a throwing resolver falls back to the
/// previously resolved path (see `activate()`), so a retry against a
/// still-erroring backend keeps its `.failed` banner instead of silently
/// rewriting the failure as "no transcript recorded".
@MainActor
@Observable
public final class AgentRunDetailStore {
    public private(set) var transcript: BlockState<[TranscriptEvent]> = .loading

    /// Whichever transcript path `activate()` actually used — the
    /// constructor's own `transcriptPath` when it was non-nil, else
    /// whatever `resolveTranscriptPath` returned (`nil` if resolution was
    /// never attempted, failed, or found nothing). See the type doc
    /// comment's "`transcriptPath == nil`" section for why `AgentRunDetailScreen`
    /// reads this instead of its own constructor argument to decide between
    /// "no transcript recorded" and a plain empty transcript panel.
    public private(set) var resolvedPath: String?

    private let transcriptPath: String?
    private let fetchTranscript: @Sendable (String) async throws -> APITranscriptPage
    private let resolveTranscriptPath: (@Sendable () async throws -> String?)?

    /// Production entry point — `AgentRunDetailScreen` calls this.
    /// `host` follows the same convention `RunDetailStore`/
    /// `SessionDetailStore` already use: `nil`/`"local"` means the
    /// embedded/attached local backend, anything else a Fleet node.
    /// `runID` is only ever used to build `resolveTranscriptPath` (and only
    /// when `transcriptPath` is `nil` to begin with) — see the type doc
    /// comment.
    public convenience init(runID: String, transcriptPath: String?, host: String?, client: CPClient) {
        var resolve: (@Sendable () async throws -> String?)?
        if transcriptPath == nil {
            resolve = { try await AgentRunDetailStore.lookUpTranscriptPath(runID: runID, host: host, client: client) }
        }
        self.init(
            transcriptPath: transcriptPath,
            fetchTranscript: { path in try await client.transcript(path: path, host: host) },
            resolveTranscriptPath: resolve
        )
    }

    /// Designated init — takes plain fetch/resolve closures rather than a
    /// `CPClient` directly, the same "fake client closures" seam
    /// `RunDetailStore`/`SessionDetailStore`/`ActivityStore` already
    /// established for this codebase. `internal`, not `public` — reached
    /// from tests via `@testable import RupuStore`, invisible outside this
    /// module. `resolveTranscriptPath` defaults to `nil` so every
    /// pre-existing call site (production `transcriptPath != nil`, and
    /// every test that doesn't care about resolution) is unaffected.
    init(
        transcriptPath: String?,
        fetchTranscript: @escaping @Sendable (String) async throws -> APITranscriptPage,
        resolveTranscriptPath: (@Sendable () async throws -> String?)? = nil
    ) {
        self.transcriptPath = transcriptPath
        self.fetchTranscript = fetchTranscript
        self.resolveTranscriptPath = resolveTranscriptPath
    }

    /// A one-shot REST fetch — no live tail, no stream to start or stop
    /// (this row has no run-scoped event stream to speak of; even if it
    /// did, it isn't an orchestrator run so nothing would ever fire on it).
    /// Repeatable, like every other store's `activate()` in this codebase —
    /// safe to call again (e.g. the screen reappearing).
    public func activate() async {
        var path = transcriptPath
        if path == nil, let resolveTranscriptPath {
            do {
                path = try await resolveTranscriptPath()
            } catch {
                // A *thrown* resolution is "couldn't check", not "resolved
                // to nothing" — keep whatever a prior activate resolved, so
                // a retry while the backend is erroring can never rewrite a
                // `.failed` transcript into the false "no transcript
                // recorded" `.empty` (`resolvedPath == nil`) state. First
                // activate (`resolvedPath` still nil) keeps the established
                // best-effort fallback to `.empty`.
                path = resolvedPath
            }
        }
        resolvedPath = path

        guard let path else {
            transcript = .empty
            return
        }
        transcript = .loading
        do {
            let page = try await fetchTranscript(path)
            transcript = page.events.isEmpty ? .empty : .content(page.events)
        } catch {
            guard !isCancellation(error) else { return }
            transcript = .failed(String(describing: error))
        }
    }

    /// Cheapest honest resolution for "just launched, no known path yet" —
    /// see the type doc comment. Scans one bounded page of the agent-runs
    /// list (`limit: 50`) for the matching `run_id` rather than paging
    /// further: a run resolved through here was *just* launched, by
    /// definition the newest thing that could appear, so it is essentially
    /// always on that endpoint's first page. A miss (not found within the
    /// page, or the request itself fails) is treated the same as "nothing
    /// to resolve" by `activate()`'s `try?` — this never throws a
    /// distinguishable error of its own.
    private static func lookUpTranscriptPath(runID: String, host: String?, client: CPClient) async throws -> String? {
        let rows = try await client.agentRuns(offset: 0, limit: 50, host: host)
        return rows.first(where: { $0.runID == runID })?.transcriptPath
    }
}
