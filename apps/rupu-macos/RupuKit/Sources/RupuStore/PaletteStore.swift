import Foundation
import Observation
import RupuAPI

/// One row the command palette (⌘K, flows-composition Task 3) can show —
/// either a static destination (`.page`) or something fetched live
/// (`.run`/`.agent`/`.workflow`/`.approve`). `id` is namespaced per kind
/// (`"page:overview"`, `"run:run-1"`, ...) so a run and an agent definition
/// that happen to share a raw identifier never collide in a `ForEach`.
public struct PaletteItem: Identifiable, Equatable, Sendable {
    public enum Kind: String, Equatable, Sendable {
        case page, run, agent, workflow, approve
    }

    public let id: String
    public let kind: Kind
    public let title: String
    public let subtitle: String?
    public let action: PaletteAction

    public init(id: String, kind: Kind, title: String, subtitle: String?, action: PaletteAction) {
        self.id = id
        self.kind = kind
        self.title = title
        self.subtitle = subtitle
        self.action = action
    }
}

/// What executing a `PaletteItem` does. `.navigate` covers every static
/// page and every fetched run/agent/workflow item; `.approveGate` is the
/// one item kind (`.approve`) whose execution is a mutation, not a plain
/// route push — see `PaletteStore.execute(_:)`.
public enum PaletteAction: Equatable, Sendable {
    case navigate(Route)
    case approveGate(runID: String, host: String?)
}

/// Pure ranking function over an already-fetched item list — no I/O, no
/// `PaletteStore` state, fully unit-testable on its own (Step 1 of this
/// task's brief).
///
/// **Empty query**: every `.page` item first (in their original relative
/// order), then everything else (also in original relative order) — the
/// palette's "nothing typed yet" view is the 7 static destinations up top,
/// followed by whatever else got fetched. Capped to 30 like every other
/// path through this function.
///
/// **Non-empty query**: case-insensitive match against `title` only
/// (`subtitle` never participates in scoring — it's display-only context).
/// Three match tiers, computed once per item and never combined:
/// - **3** — `title` itself starts with the (trimmed, lowercased) query.
/// - **2** — some word inside `title` (split on any non-alphanumeric
///   boundary) starts with the query, but the title as a whole doesn't
///   (already caught by the tier-3 check, which runs first).
/// - **1** — the query's characters appear, in order, somewhere in `title`
///   (not necessarily contiguous) — a plain subsequence match, the loosest
///   tier that still counts as a match.
/// - anything else scores **0**, meaning "not a match at all" — the item is
///   dropped from the result, not merely ranked last.
///
/// Sorting is by score descending; **stable** within a score — two items
/// with the same score keep their relative order from the input `items`
/// array (implemented via each item's original index as an explicit sort
/// tiebreaker, since `Array.sorted(by:)` itself makes no stability
/// guarantee).
public func paletteRank(query: String, items: [PaletteItem]) -> [PaletteItem] {
    let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else {
        let pages = items.filter { $0.kind == .page }
        let rest = items.filter { $0.kind != .page }
        return Array((pages + rest).prefix(30))
    }

    let lowerQuery = trimmed.lowercased()
    let scored: [(item: PaletteItem, score: Int, index: Int)] = items.enumerated().compactMap { index, item in
        guard let score = paletteMatchScore(query: lowerQuery, title: item.title) else { return nil }
        return (item, score, index)
    }
    let ranked = scored.sorted { lhs, rhs in
        lhs.score != rhs.score ? lhs.score > rhs.score : lhs.index < rhs.index
    }
    return Array(ranked.prefix(30).map(\.item))
}

/// `nil` means "not a match" (tier 0, dropped by `paletteRank` entirely).
/// `query` must already be trimmed + lowercased; `title` is lowercased
/// here since callers pass the item's raw display casing.
private func paletteMatchScore(query: String, title: String) -> Int? {
    let lowerTitle = title.lowercased()
    if lowerTitle.hasPrefix(query) { return 3 }

    let words = lowerTitle.split(whereSeparator: { !$0.isLetter && !$0.isNumber })
    if words.contains(where: { $0.hasPrefix(query) }) { return 2 }

    if isSubsequence(query, of: lowerTitle) { return 1 }
    return nil
}

/// True if every character of `needle` appears in `haystack`, in the same
/// relative order, not necessarily contiguously (a classic subsequence
/// check — single forward pass over `haystack` per `needle` character).
private func isSubsequence(_ needle: String, of haystack: String) -> Bool {
    var haystackIndex = haystack.startIndex
    for character in needle {
        guard let found = haystack[haystackIndex...].firstIndex(of: character) else { return false }
        haystackIndex = haystack.index(after: found)
    }
    return true
}

/// The ⌘K command palette's store: fetches a live catalog of pages/runs/
/// definitions/gate-approvals on `open()`, and `paletteRank`s it against
/// whatever the operator types into `query`.
///
/// **Fail-open contract**: the 7 static page items are seeded into `items`
/// *before* any network call, and each of the three fetched sources
/// (`runs`/`agentDefinitions`/`workflowDefinitions`) is wrapped in its own
/// `try?` — one source erroring (or all three) never removes the page
/// items, and never fails `open()` as a whole. This mirrors `ActivityStore`'s
/// per-source independence (`loadRemoteHost`'s doc comment: "you should not
/// fail all for a single [source]").
///
/// **Approve resolution** mirrors `ActivityTable.resolveSoleAwaitingGate`'s
/// contract (fetch `client.runDetail`, read `run.awaiting`) with one
/// deliberate difference, ruled for this task: a run with more than one
/// awaiting gate is ambiguous for a single palette item to resolve, so
/// `execute(_:)` just navigates to the run (letting the run-detail screen's
/// own per-gate controls handle it) rather than guessing which gate — see
/// that method's doc comment.
@MainActor
@Observable
public final class PaletteStore {
    public var isOpen: Bool = false
    public var query: String = "" {
        didSet {
            guard query != oldValue else { return }
            activeIndex = 0
        }
    }
    public var activeIndex: Int = 0
    public private(set) var items: [PaletteItem] = []

    public var results: [PaletteItem] {
        paletteRank(query: query, items: items)
    }

    private let client: CPClient
    private let pendingActions: PendingActions
    private let onNavigate: (Route) -> Void

    /// Bumped on entry to `open()` and captured locally, same precedent as
    /// `ActivityStore.remoteGeneration`/`RunDetailStore.focusGeneration`:
    /// an overlapping second `open()` (e.g. a fast re-press of ⌘K while the
    /// first call's fetch is still in flight) makes the first call's
    /// captured generation stale, so its *later-arriving* fetch result is
    /// discarded rather than clobbering the second, fresher call's
    /// already-applied `items` — without this, whichever call's network
    /// round trip happened to finish last would win, regardless of which
    /// was triggered last. The early synchronous resets below (`isOpen`/
    /// `query`/`activeIndex`/the page-only `items` seed) apply
    /// unconditionally on every call, same reasoning
    /// `RunDetailStore.focusStep`'s doc comment gives for its own
    /// never-awaited branches: nothing has suspended yet at that point, so
    /// there's no later call that could have superseded them.
    private var openGeneration = 0

    /// Guards `execute(_:)` against a double-fire on an `.approveGate` item
    /// (e.g. a double-tap Return) — same rationale as
    /// `ActivityTable.isBusy`, adapted from a per-row `@State` flag to one
    /// store-wide flag: the palette only ever executes one selected item at
    /// a time. `.navigate` items are left unguarded — pushing the same
    /// route twice, or closing an already-closed palette, is idempotent, so
    /// there is nothing here for a guard to protect.
    private var isApprovingGate = false

    private static let localHost = "local"
    private static let runsFetchLimit = 50

    public init(client: CPClient, pendingActions: PendingActions, onNavigate: @escaping (Route) -> Void) {
        self.client = client
        self.pendingActions = pendingActions
        self.onNavigate = onNavigate
    }

    /// Opens the palette (`isOpen = true`) with just the 7 static pages
    /// showing immediately, resets `query`/`activeIndex` for a fresh
    /// session, then fetches the three live sources concurrently
    /// (`async let`) and folds in whatever came back. Safe to call again
    /// while already open (e.g. a second ⌘K while a slow fetch from the
    /// first is still in flight) — it just restarts the same fetch; see
    /// `openGeneration`'s doc comment for why the first call's fetch can
    /// never clobber the second's.
    public func open() async {
        openGeneration += 1
        let generation = openGeneration

        isOpen = true
        query = ""
        activeIndex = 0
        items = Self.pageItems

        async let runsResult: [APIRunListRow] = (try? await client.runs(offset: 0, limit: Self.runsFetchLimit, host: Self.localHost)) ?? []
        async let agentsResult: [AgentDefinition] = (try? await client.agentDefinitions()) ?? []
        async let workflowsResult: [WorkflowDefinition] = (try? await client.workflowDefinitions()) ?? []

        let (runs, agents, workflows) = await (runsResult, agentsResult, workflowsResult)

        // A newer `open()` call started while this one was suspended
        // awaiting the three sources above — that later call already owns
        // `items` (its own fetch may still be in flight, or may have
        // already landed). Discard this now-stale result rather than
        // applying it.
        guard generation == openGeneration else { return }

        items = Self.pageItems + Self.runItems(from: runs) + Self.agentItems(from: agents) + Self.workflowItems(from: workflows)
    }

    public func close() {
        isOpen = false
        query = ""
        activeIndex = 0
        // Invalidate any in-flight open() fetch so it can't write items after
        // the palette closed — same teardown-bump `ActivityStore.deactivate()`
        // does with `remoteGeneration`.
        openGeneration += 1
    }

    /// Runs `item`'s action. `.navigate` items push the route and close the
    /// palette immediately; `.approveGate` items resolve the gate first
    /// (see this type's doc comment), then always navigate to the run —
    /// whether or not the approve POST itself fired — so the operator lands
    /// somewhere that shows the pending/awaiting state either way. A second
    /// `.approveGate` execution while one is already in flight is a no-op —
    /// see `isApprovingGate`'s doc comment.
    public func execute(_ item: PaletteItem) async {
        switch item.action {
        case .navigate(let route):
            onNavigate(route)
            close()
        case .approveGate(let runID, let host):
            guard !isApprovingGate else { return }
            isApprovingGate = true
            defer { isApprovingGate = false }

            await approveGate(runID: runID, host: host)
            onNavigate(.runDetail(id: runID, host: host))
            close()
        }
    }

    /// Fetches the run's current detail and, only when it is parked on
    /// exactly one awaiting gate, begins + posts the approve for that gate
    /// (gate-scoped `ActionKey`, same composite convention
    /// `ActivityStore.approve`/`RunDetailStore.approve` use — see
    /// `ActionKey.gate(runID:stepID:verb:)`'s doc comment). Zero or
    /// multiple awaiting gates, or the detail fetch itself failing, is a
    /// no-op here — `execute(_:)` still navigates to the run afterward
    /// either way.
    private func approveGate(runID: String, host: String?) async {
        guard let detail = try? await client.runDetail(id: runID, host: host),
              detail.run.awaiting.count == 1,
              let gate = detail.run.awaiting.first
        else { return }

        let key = ActionKey.gate(runID: runID, stepID: gate.stepID, verb: .approve)
        pendingActions.begin(key)
        do {
            _ = try await client.approveRun(id: runID, host: host, gate: gate.stepID)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    /// The 7 sidebar destinations (`Route`'s non-detail cases), always
    /// present — `.activity` opens the unfiltered `.all` kind, matching the
    /// sidebar's own single collapsed "Activity" row (flows-composition
    /// Task 1).
    private static var pageItems: [PaletteItem] {
        [
            PaletteItem(id: "page:overview", kind: .page, title: "Overview", subtitle: nil, action: .navigate(.overview)),
            PaletteItem(id: "page:activity", kind: .page, title: "Activity", subtitle: nil, action: .navigate(.activity(.all))),
            PaletteItem(id: "page:projects", kind: .page, title: "Projects", subtitle: nil, action: .navigate(.projects)),
            PaletteItem(id: "page:security", kind: .page, title: "Security", subtitle: nil, action: .navigate(.security(.findings))),
            PaletteItem(id: "page:library", kind: .page, title: "Library", subtitle: nil, action: .navigate(.library(.agents))),
            PaletteItem(id: "page:fleet", kind: .page, title: "Fleet", subtitle: nil, action: .navigate(.fleet(.hosts))),
            PaletteItem(id: "page:usage", kind: .page, title: "Usage", subtitle: nil, action: .navigate(.usage)),
        ]
    }

    /// Maps each fetched run row through `ActivityRow.init(_:APIRunListRow)`
    /// — the exact same row shape `ActivityStore`'s workflow source
    /// produces — and then through `ActivityRow.Navigation.route` (the
    /// mapping lifted out of `ActivityScreen.handleSelect` for this reuse)
    /// so the palette's run items navigate identically to an Activity-table
    /// row click. A row whose navigation has no route (`.none`) is skipped
    /// — nothing to search for that this palette could act on.
    ///
    /// `.awaiting` rows additionally get a paired `.approve` item — see
    /// `approveGate(runID:host:)` for what executing it does.
    private static func runItems(from rows: [APIRunListRow]) -> [PaletteItem] {
        rows.flatMap { row -> [PaletteItem] in
            let activityRow = ActivityRow(row)
            guard let route = activityRow.navigation.route else { return [] }

            var result = [
                PaletteItem(
                    id: "run:\(activityRow.id)", kind: .run, title: activityRow.subject,
                    subtitle: "\(activityRow.status.displayLabel) · \(activityRow.host)", action: .navigate(route)
                ),
            ]
            if activityRow.status == .awaiting, case .run(let runID, let host) = activityRow.navigation {
                result.append(
                    PaletteItem(
                        id: "approve:\(activityRow.id)", kind: .approve, title: "Approve: \(activityRow.subject)",
                        subtitle: activityRow.host, action: .approveGate(runID: runID, host: host)
                    )
                )
            }
            return result
        }
    }

    /// No per-agent route exists yet (`Route` has no detail case for a
    /// definition) — every agent/workflow item navigates to `.library`,
    /// the definitions catalog page, same as the sidebar's own Library row.
    private static func agentItems(from definitions: [AgentDefinition]) -> [PaletteItem] {
        definitions.map { definition in
            PaletteItem(
                id: "agent:\(definition.slug)", kind: .agent, title: definition.name,
                subtitle: definition.description ?? "Agent", action: .navigate(.library(.agents))
            )
        }
    }

    private static func workflowItems(from definitions: [WorkflowDefinition]) -> [PaletteItem] {
        definitions.map { definition in
            PaletteItem(
                id: "workflow:\(definition.name)", kind: .workflow, title: definition.name,
                subtitle: "Workflow", action: .navigate(.library(.workflows))
            )
        }
    }
}
