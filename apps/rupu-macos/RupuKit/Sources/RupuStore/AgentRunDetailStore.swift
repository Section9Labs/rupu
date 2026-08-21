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
/// at all (see `agent_run_rows.json`'s `run-11` fixture). `activate()`
/// never calls the network in that case — `transcript` goes straight to
/// `.empty`, and `AgentRunDetailScreen` renders an honest "no transcript
/// recorded" label rather than a load spinner that would never resolve.
///
/// **Cancellation-safe** (hotfix root cause B): a `CancellationError`/
/// `CPError.cancelled` from `fetchTranscript` (e.g. this screen's
/// `.task(id: runID)` superseded by navigating to a different run before
/// the fetch lands) leaves `transcript` exactly as it was — never `.failed`.
/// See `isCancellation`'s doc comment.
@MainActor
@Observable
public final class AgentRunDetailStore {
    public private(set) var transcript: BlockState<[TranscriptEvent]> = .loading

    private let transcriptPath: String?
    private let fetchTranscript: @Sendable (String) async throws -> APITranscriptPage

    /// Production entry point — `AgentRunDetailScreen` calls this.
    /// `host` follows the same convention `RunDetailStore`/
    /// `SessionDetailStore` already use: `nil`/`"local"` means the
    /// embedded/attached local backend, anything else a Fleet node.
    public convenience init(transcriptPath: String?, host: String?, client: CPClient) {
        self.init(
            transcriptPath: transcriptPath,
            fetchTranscript: { path in try await client.transcript(path: path, host: host) }
        )
    }

    /// Designated init — takes a plain fetch closure rather than a
    /// `CPClient` directly, the same "fake client closures" seam
    /// `RunDetailStore`/`SessionDetailStore`/`ActivityStore` already
    /// established for this codebase. `internal`, not `public` — reached
    /// from tests via `@testable import RupuStore`, invisible outside this
    /// module.
    init(
        transcriptPath: String?,
        fetchTranscript: @escaping @Sendable (String) async throws -> APITranscriptPage
    ) {
        self.transcriptPath = transcriptPath
        self.fetchTranscript = fetchTranscript
    }

    /// A one-shot REST fetch — no live tail, no stream to start or stop
    /// (this row has no run-scoped event stream to speak of; even if it
    /// did, it isn't an orchestrator run so nothing would ever fire on it).
    /// Repeatable, like every other store's `activate()` in this codebase —
    /// safe to call again (e.g. the screen reappearing).
    public func activate() async {
        guard let transcriptPath else {
            transcript = .empty
            return
        }
        transcript = .loading
        do {
            let page = try await fetchTranscript(transcriptPath)
            transcript = page.events.isEmpty ? .empty : .content(page.events)
        } catch {
            guard !isCancellation(error) else { return }
            transcript = .failed(String(describing: error))
        }
    }
}
