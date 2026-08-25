import Foundation
import Observation
import RupuAPI

/// Owns everything the Project Detail screen (Phase 5A, Task 5) shows: the
/// project rollup (`detail` — identity, run/session/coverage counts, the 10
/// most recent runs, a usage summary) plus five independent, **lazily**
/// loaded tab blocks (`runs`/`sessions`/`agents`/`workflows`/`autoflows`/
/// `findings`) — each its own `BlockState`, one failing never blanks the
/// others, same per-block-independence contract every other detail store in
/// this module (`RunDetailStore`/`SessionDetailStore`) already follows.
///
/// **Read-only, no streaming**: unlike `RunDetailStore`, there is no live
/// event stream here — a project has no run-scoped stream of its own to
/// subscribe to, and the tab content this phase covers (Overview/Runs/
/// Sessions/Findings/Definitions) is all REST snapshots. `activate()`/the
/// `loadXIfNeeded()` family are all plain one-shot fetches; there is nothing
/// to start or stop, and no `deactivate()` to speak of — mirrors
/// `SessionDetailStore`'s own "Snapshot only, no tail" note.
///
/// **Lazy per-tab loading**: `activate()` only loads `detail` (the Overview
/// tab's own content, and the header every tab renders above its content) —
/// every other tab's block starts `.loading` and is fetched the first time
/// `ProjectDetailScreen` actually shows that tab, via the matching
/// `loadXIfNeeded()` call. Each `loadXIfNeeded()` is idempotent past its
/// first successful dispatch (tracked by a private `Bool`, not by the
/// `BlockState` itself — `BlockState`'s own `.loading` can't distinguish
/// "never requested" from "request in flight", so a separate flag is what
/// makes re-selecting an already-loaded tab a no-op rather than a silent
/// refetch on every switch). A `loadXIfNeeded()` call that fails still
/// leaves the flag set — a `.failed` tab is not retried automatically; the
/// screen's own retry affordance (same "every mutating/loading control's
/// tap is its own retry" convention `RunDetailScreen` documents) calls the
/// unconditional `loadX()` counterpart instead. A call that is CANCELLED
/// mid-flight (SwiftUI tearing down the tab's `.task` because the user
/// switched away before the fetch landed) clears the flag instead — the
/// block is still `.loading` and no retry affordance will ever render for
/// it, so leaving the latch set would freeze the tab as a permanent
/// spinner; clearing it lets the tab's next `.task` re-dispatch fresh
/// (same "cancellation always leaves the store re-dispatchable" contract
/// `CodeStore` documents).
///
/// **Windowing (Runs/Sessions only)**: `GET /api/projects/:ws_id/runs` and
/// `.../sessions` have no pagination envelope, same as every other list
/// endpoint in this codebase (bare array out) — this store fetches the
/// first `Self.windowSize` (50) rows on first load, and exposes
/// `runsShowingAll`/`sessionsShowingAll` plus `showAllRuns()`/
/// `showAllSessions()` for the screen's "show all" affordance to refetch
/// with `Self.showAllLimit` (1000) instead. This is a deliberately simpler
/// contract than `PagedSnapshot`'s incremental `loadMore()`: the brief
/// calls for "windowed — first 50 + show all (honest, simple; no fake
/// infinite scroll)", not a second paging mechanism to maintain alongside
/// `PagedSnapshot`.
///
/// **`runsShowingAll`/`sessionsShowingAll` are an honesty gate, not a
/// "did the show-all fetch run" flag** (review fix): they read `true` only
/// once the loaded row count is *provably* every row the project has —
/// `loaded >= total`, `total` being `detail`'s own server-reported
/// `runs.total`/`sessions.total`, never inferred from which fetch (window
/// vs. `showAllLimit`) ran or from a bare "the request succeeded" signal.
/// A project with more rows than `showAllLimit` therefore NEVER reaches
/// `showingAll == true` — `showAllRuns()`/`showAllSessions()` cap out at
/// exactly `showAllLimit` rows and stay honestly incomplete forever (this
/// phase has no server-side cursor past that cap to keep going with). The
/// screen's own `ShowAllFooter.resolve(...)` (`RupuProjects/
/// ProjectDetailScreen.swift`) reads `loaded == showAllLimit` alongside
/// `showingAll == false` as the distinct "hit the cap, still short" state
/// and renders a quiet persistent note instead of a button that would just
/// refetch the identical capped page again.
@MainActor
@Observable
public final class ProjectDetailStore {
    public private(set) var detail: BlockState<APIProjectDetail> = .loading

    public private(set) var runs: BlockState<[APIRunListRow]> = .loading
    public private(set) var runsShowingAll = false

    public private(set) var sessions: BlockState<[APISessionRow]> = .loading
    public private(set) var sessionsShowingAll = false

    public private(set) var agents: BlockState<[AgentDefinition]> = .loading
    public private(set) var workflows: BlockState<[WorkflowDefinition]> = .loading
    public private(set) var autoflows: BlockState<[AutoflowDefinition]> = .loading
    public private(set) var findings: BlockState<APIFindings> = .loading

    /// The first-page row count every windowed tab loads before "show all"
    /// is tapped.
    public static let windowSize = 50
    /// "Show all"'s fetch limit — effectively unbounded for this phase's
    /// project sizes rather than a true second page of pagination; see the
    /// type doc comment's "Windowing" section.
    public static let showAllLimit = 1000

    private let wsID: String
    private let fetchDetail: @Sendable () async throws -> APIProjectDetail
    private let fetchRuns: @Sendable (_ offset: Int, _ limit: Int) async throws -> [APIRunListRow]
    private let fetchSessions: @Sendable (_ offset: Int, _ limit: Int) async throws -> [APISessionRow]
    private let fetchAgents: @Sendable () async throws -> [AgentDefinition]
    private let fetchWorkflows: @Sendable () async throws -> [WorkflowDefinition]
    private let fetchAutoflows: @Sendable () async throws -> [AutoflowDefinition]
    private let fetchFindings: @Sendable () async throws -> APIFindings

    /// See the type doc comment's "Lazy per-tab loading" section — each
    /// tracks whether its `loadXIfNeeded()` has ever dispatched a fetch for
    /// the current `activate()` cycle.
    private var runsRequested = false
    private var sessionsRequested = false
    private var agentsRequested = false
    private var workflowsRequested = false
    private var autoflowsRequested = false
    private var findingsRequested = false

    /// Production entry point — `ProjectDetailScreen` calls this.
    public convenience init(wsID: String, client: CPClient) {
        self.init(
            wsID: wsID,
            fetchDetail: { try await client.projectDetail(wsID: wsID) },
            fetchRuns: { offset, limit in try await client.projectRuns(wsID: wsID, offset: offset, limit: limit) },
            fetchSessions: { offset, limit in try await client.projectSessions(wsID: wsID, offset: offset, limit: limit) },
            fetchAgents: { try await client.projectAgents(wsID: wsID) },
            fetchWorkflows: { try await client.projectWorkflows(wsID: wsID) },
            fetchAutoflows: { try await client.projectAutoflows(wsID: wsID) },
            fetchFindings: { try await client.findings(wsID: wsID) }
        )
    }

    /// Designated init — plain fetch closures, same "fake client closures"
    /// seam every other store in this module already established.
    /// `internal`, not `public` — reached from tests via `@testable import
    /// RupuStore`.
    init(
        wsID: String,
        fetchDetail: @escaping @Sendable () async throws -> APIProjectDetail,
        fetchRuns: @escaping @Sendable (_ offset: Int, _ limit: Int) async throws -> [APIRunListRow],
        fetchSessions: @escaping @Sendable (_ offset: Int, _ limit: Int) async throws -> [APISessionRow],
        fetchAgents: @escaping @Sendable () async throws -> [AgentDefinition],
        fetchWorkflows: @escaping @Sendable () async throws -> [WorkflowDefinition],
        fetchAutoflows: @escaping @Sendable () async throws -> [AutoflowDefinition],
        fetchFindings: @escaping @Sendable () async throws -> APIFindings
    ) {
        self.wsID = wsID
        self.fetchDetail = fetchDetail
        self.fetchRuns = fetchRuns
        self.fetchSessions = fetchSessions
        self.fetchAgents = fetchAgents
        self.fetchWorkflows = fetchWorkflows
        self.fetchAutoflows = fetchAutoflows
        self.fetchFindings = fetchFindings
    }

    /// Loads `detail` alone (the Overview tab + the header every tab
    /// shares) and resets every lazy-tab flag, so a screen re-appearance
    /// (same store instance, `activate()` called again) lets every tab
    /// refetch fresh rather than staying latched on stale content from a
    /// previous visit. Repeatable, like every other store's `activate()` in
    /// this codebase.
    public func activate() async {
        runsRequested = false
        sessionsRequested = false
        agentsRequested = false
        workflowsRequested = false
        autoflowsRequested = false
        findingsRequested = false
        await loadDetail()
    }

    // MARK: - Lazy per-tab loads

    public func loadRunsIfNeeded() async {
        guard !runsRequested else { return }
        runsRequested = true
        await loadRuns(limit: Self.windowSize)
    }

    /// Forces a reload regardless of `runsRequested` — the screen's own
    /// retry affordance for a `.failed` Runs tab.
    public func loadRuns() async {
        runsRequested = true
        await loadRuns(limit: Self.windowSize)
    }

    public func showAllRuns() async {
        runsRequested = true
        await loadRuns(limit: Self.showAllLimit)
    }

    public func loadSessionsIfNeeded() async {
        guard !sessionsRequested else { return }
        sessionsRequested = true
        await loadSessions(limit: Self.windowSize)
    }

    public func loadSessions() async {
        sessionsRequested = true
        await loadSessions(limit: Self.windowSize)
    }

    public func showAllSessions() async {
        sessionsRequested = true
        await loadSessions(limit: Self.showAllLimit)
    }

    public func loadAgentsIfNeeded() async {
        guard !agentsRequested else { return }
        agentsRequested = true
        await loadAgents()
    }

    public func loadAgents() async {
        agentsRequested = true
        do {
            let rows = try await fetchAgents()
            agents = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else {
                agentsRequested = false
                return
            }
            agents = .failed(String(describing: error))
        }
    }

    public func loadWorkflowsIfNeeded() async {
        guard !workflowsRequested else { return }
        workflowsRequested = true
        await loadWorkflows()
    }

    public func loadWorkflows() async {
        workflowsRequested = true
        do {
            let rows = try await fetchWorkflows()
            workflows = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else {
                workflowsRequested = false
                return
            }
            workflows = .failed(String(describing: error))
        }
    }

    public func loadAutoflowsIfNeeded() async {
        guard !autoflowsRequested else { return }
        autoflowsRequested = true
        await loadAutoflows()
    }

    public func loadAutoflows() async {
        autoflowsRequested = true
        do {
            let rows = try await fetchAutoflows()
            autoflows = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else {
                autoflowsRequested = false
                return
            }
            autoflows = .failed(String(describing: error))
        }
    }

    public func loadFindingsIfNeeded() async {
        guard !findingsRequested else { return }
        findingsRequested = true
        await loadFindings()
    }

    public func loadFindings() async {
        findingsRequested = true
        do {
            findings = .content(try await fetchFindings())
        } catch {
            guard !isCancellation(error) else {
                findingsRequested = false
                return
            }
            findings = .failed(String(describing: error))
        }
    }

    // MARK: - REST loads

    private func loadDetail() async {
        do {
            detail = .content(try await fetchDetail())
        } catch {
            guard !isCancellation(error) else { return }
            detail = .failed(String(describing: error))
        }
    }

    private func loadRuns(limit: Int) async {
        do {
            let rows = try await fetchRuns(0, limit)
            runs = rows.isEmpty ? .empty : .content(rows)
            runsShowingAll = Self.isComplete(loaded: rows.count, total: detail.value?.runs.total)
        } catch {
            guard !isCancellation(error) else {
                runsRequested = false
                return
            }
            runs = .failed(String(describing: error))
        }
    }

    private func loadSessions(limit: Int) async {
        do {
            let rows = try await fetchSessions(0, limit)
            sessions = rows.isEmpty ? .empty : .content(rows)
            sessionsShowingAll = Self.isComplete(loaded: rows.count, total: detail.value?.sessions.total)
        } catch {
            guard !isCancellation(error) else {
                sessionsRequested = false
                return
            }
            sessions = .failed(String(describing: error))
        }
    }

    /// `true` only when `loaded` demonstrably contains every row the
    /// project has. `total == nil` (`detail` hasn't loaded, or failed)
    /// never counts as complete — an unknown total must never be read as
    /// "nothing more to show".
    private static func isComplete(loaded: Int, total: Int?) -> Bool {
        guard let total else { return false }
        return loaded >= total
    }
}
