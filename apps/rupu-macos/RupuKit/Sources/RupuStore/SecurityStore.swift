import Foundation
import Observation
import RupuAPI

/// Owns the Security screen's (Phase 5B, Task 3) two independent, **lazily**
/// loaded tab blocks — global `findings` (every finding across every
/// registered workspace, unscoped — `CPClient.findings(wsID:)` called with
/// no `wsID`, per that method's doc comment) and `coverage` (every coverage
/// target's rollup across every registered workspace, `CPClient.coverage()`).
/// Same per-block-independence contract every other multi-block store in
/// this module (`ProjectDetailStore`/`FleetStore`/`LibraryStore`) already
/// follows — one tab failing never blanks the other.
///
/// **Lazy per-tab loading, same recipe as `ProjectDetailStore`**: `activate()`
/// itself fetches nothing — there is no shared "header" fetch here (unlike
/// `ProjectDetailStore`'s `detail`; the Security screen's summary strip is
/// entirely inside `findings`' own `APIFindingsSummary` payload) — it only
/// resets both lazy-tab flags, so a screen re-appearance (same store
/// instance) lets each tab refetch fresh on next selection rather than
/// staying latched on stale content. `loadFindingsIfNeeded()`/
/// `loadCoverageIfNeeded()` are what `SecurityScreen` calls the first time it
/// actually shows each tab; `loadFindings()`/`loadCoverage()` force an
/// unconditional reload for the screen's own retry affordance on a `.failed`
/// tab, same "every `IfNeeded` sibling has an unconditional force-reload
/// twin" convention `ProjectDetailStore` establishes.
///
/// **`findings` is never `.empty`, even when the payload's `findings` array
/// is** — same reasoning `ProjectDetailStore.loadFindings()` already
/// documents: `APIFindings` carries `summary` (the per-severity counts the
/// screen's summary strip renders) alongside the row array, so a
/// zero-findings response still has real content to show; the "No findings"
/// empty state is a render-time decision on `value.findings.isEmpty`, not a
/// `BlockState.empty` one. `coverage` is a bare `[APICoverageSummary]` with
/// no such envelope, so it follows the ordinary `isEmpty ? .empty :
/// .content` rule every other list-only block in this module uses.
///
/// **Read-only, no streaming** — same "Snapshot only, no tail" rationale
/// `ProjectDetailStore`/`SessionDetailStore` document: there is no
/// fleet-wide findings/coverage event stream to subscribe to, only REST
/// snapshots.
@MainActor
@Observable
public final class SecurityStore {
    public private(set) var findings: BlockState<APIFindings> = .loading
    public private(set) var coverage: BlockState<[APICoverageSummary]> = .loading

    private let fetchFindings: @Sendable () async throws -> APIFindings
    private let fetchCoverage: @Sendable () async throws -> [APICoverageSummary]

    /// See the type doc comment's "Lazy per-tab loading" section — tracks
    /// whether the matching `loadXIfNeeded()` has ever dispatched a fetch
    /// for the current `activate()` cycle.
    private var findingsRequested = false
    private var coverageRequested = false

    /// Production entry point — `SecurityScreen` calls this.
    public convenience init(client: CPClient) {
        self.init(
            fetchFindings: { try await client.findings() },
            fetchCoverage: { try await client.coverage() }
        )
    }

    /// Designated init — plain fetch closures, the same "fake client
    /// closures" seam every other store in this module already established.
    /// `internal`, not `public` — reached from tests via `@testable import
    /// RupuStore`.
    init(
        fetchFindings: @escaping @Sendable () async throws -> APIFindings,
        fetchCoverage: @escaping @Sendable () async throws -> [APICoverageSummary]
    ) {
        self.fetchFindings = fetchFindings
        self.fetchCoverage = fetchCoverage
    }

    /// Resets both lazy-tab flags. Fetches nothing itself — see the type
    /// doc comment. Repeatable, like every other store's `activate()` in
    /// this codebase.
    public func activate() async {
        findingsRequested = false
        coverageRequested = false
    }

    // MARK: - Lazy per-tab loads

    public func loadFindingsIfNeeded() async {
        guard !findingsRequested else { return }
        findingsRequested = true
        await loadFindings()
    }

    /// Forces a reload regardless of `findingsRequested` — the screen's own
    /// retry affordance for a `.failed` Findings tab.
    public func loadFindings() async {
        findingsRequested = true
        do {
            // Always `.content`, never `.empty` — see the type doc comment.
            findings = .content(try await fetchFindings())
        } catch {
            guard !isCancellation(error) else { return }
            findings = .failed(String(describing: error))
        }
    }

    public func loadCoverageIfNeeded() async {
        guard !coverageRequested else { return }
        coverageRequested = true
        await loadCoverage()
    }

    /// Forces a reload regardless of `coverageRequested` — the screen's own
    /// retry affordance for a `.failed` Coverage tab.
    public func loadCoverage() async {
        coverageRequested = true
        do {
            let rows = try await fetchCoverage()
            coverage = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            coverage = .failed(String(describing: error))
        }
    }
}
