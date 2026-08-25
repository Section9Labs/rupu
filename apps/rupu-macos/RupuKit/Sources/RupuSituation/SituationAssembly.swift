import Foundation
import RupuAPI
import RupuStore

// Situation Room — the glue between `SituationStore`'s raw snapshot
// (`RupuStore`, wire types only) and this module's own pure card/roster/
// vitals derivations (`StreamCards.swift`/`Roster.swift`/`Vitals.swift`).
// See `SituationStore`'s doc comment for why this assembly lives HERE
// rather than inside the store itself: `RupuStore` cannot import
// `RupuSituation` (this module already depends on `RupuStore`, so the
// reverse edge would cycle the package graph), so the fold from raw
// `CPEventRow`/`APIFinding`/`APIProjectRow` into `StreamCard`/`RosterEntry`/
// `Vitals` happens on this side of the boundary instead.
//
// A free, non-isolated function (not a method on a View, not `@MainActor`)
// — its inputs are plain `Sendable` value types read off `SituationStore`'s
// `@MainActor` properties, so the fold itself needs no actor affinity and
// stays directly testable (`SituationAssemblyTests`), matching this
// module's existing pure-function convention (`cardForEvent`/`mergeStream`/
// `foldRoster`/...).

/// The three view models `SituationRoomScreen` renders, assembled together
/// so a single call site produces everything the layout needs.
public struct SituationSnapshot: Equatable, Sendable {
    public let cards: [StreamCard]
    public let roster: [RosterEntry]
    public let vitals: Vitals
    /// `StreamCard.key`s currently inside the fresh-arrival highlight
    /// window — the `EventStreamColumn`-facing translation of
    /// `SituationStore.freshEvents: Set<CPEvent>` (see that property's doc
    /// comment for why the store tracks the `CPEvent` identity rather than
    /// this string one). Computed alongside `cards` below so it always
    /// reflects exactly the keys actually present in THIS snapshot's
    /// `cards` — never a stale key for a card that got capped out of the
    /// stream. Findings can never appear here — `Events.tsx`'s own
    /// `freshKeys` is populated only from the live event stream
    /// (`subscribeEvents`'s callback), never from a finding.
    public let freshKeys: Set<String>

    public init(cards: [StreamCard], roster: [RosterEntry], vitals: Vitals, freshKeys: Set<String> = []) {
        self.cards = cards
        self.roster = roster
        self.vitals = vitals
        self.freshKeys = freshKeys
    }
}

/// Folds `SituationStore`'s raw snapshot into a `SituationSnapshot`. Mirrors
/// `Events.tsx`'s own `useMemo` derivations (lines 251-318) end to end:
/// - `eventCards`/`findingCards` (lines 251-269): every event row becomes a
///   card (`cardForEvent` only returns `nil` for a note-less
///   `step_working` heartbeat), findings are sorted newest-`declared_at`-
///   first and capped at `findingsStreamCap` (`STREAM_FINDINGS_CAP` = 60)
///   before becoming cards — the roster/vitals below still see the FULL
///   `findings` array, only the stream itself is capped.
/// - `cards` (lines 271-274): `mergeStream` do the dedup+sort+cap
///   `[...eventCards, ...findingCards].sort((a,b) => b.ts - a.ts)` was doing
///   on the web, capped at `cardCap` (`MAX_EVENTS` = 5000, matching
///   `SituationStore.eventRows`'s own cap — see that type's doc comment).
/// - `freshKeys` (redesign pass, Task 4 — no `Events.tsx` line range of its
///   own, since the web computes `freshKeys` directly as `StreamCard`-shaped
///   string keys already; this port's `SituationStore` tracks the same
///   concept keyed on `CPEvent` instead, so this function is where that
///   translates to the `StreamCard.key` strings `EventStreamColumn` actually
///   renders against — see `SituationSnapshot.freshKeys`'s own doc comment).
/// - `activity`/`roster` (lines 276-283): `deriveActivity` needs its input
///   newest-first, which `eventRows` already is (see `SituationStore`'s doc
///   comment on that invariant).
/// - `errors`/`vitals` (lines 304-318): `errors` counts the MERGED `cards`
///   list's `.error`-group members (not `shown`, the filter-narrowed list —
///   filtering is a display-only concern, handled inside
///   `EventStreamColumn`).
public func assembleSituation(
    eventRows: [CPEventRow],
    findings: [APIFinding],
    findingsSummary: APIFindingsSummary?,
    projects: [APIProjectRow],
    runToWorkspace: [String: String],
    runTerminalStatus: [String: String],
    dashboard: APIDashboardResponse?,
    eventsPerMin: Int,
    freshEvents: Set<CPEvent> = [],
    cardCap: Int = 5_000,
    findingsStreamCap: Int = 60
) -> SituationSnapshot {
    var freshKeys: Set<String> = []
    let eventCards: [StreamCard] = eventRows.compactMap { row in
        guard let card = cardForEvent(row.event, ts: Date(timeIntervalSince1970: Double(row.ts) / 1000)) else {
            return nil
        }
        if freshEvents.contains(row.event) { freshKeys.insert(card.key) }
        return card
    }
    let findingCards: [StreamCard] = findings
        .sorted { rfc3339ToMS($0.declaredAt) > rfc3339ToMS($1.declaredAt) }
        .prefix(findingsStreamCap)
        .map(cardForFinding)
    let cards = mergeStream(eventCards + findingCards, max: cardCap)
    // `mergeStream` can cap/dedup cards out of the final list — never claim
    // a key is fresh if its card didn't survive into `cards`.
    let survivingKeys = Set(cards.map(\.key))
    freshKeys.formIntersection(survivingKeys)

    let activity = reconcileActivity(deriveActivity(eventRows), persistedStatus: runTerminalStatus)
    let roster = foldRoster(projects: projects, runToWs: runToWorkspace, activity: activity, findings: findings)

    let errors = cards.filter { $0.group == .error }.count
    let vitals = buildVitals(
        activeRuns: dashboard?.active.running,
        awaiting: dashboard?.active.awaitingApproval,
        findings: findingsSummary,
        projectsLive: projectsLive(roster),
        projectsTotal: projects.count,
        errors: errors,
        eventsPerMin: eventsPerMin
    )

    return SituationSnapshot(cards: cards, roster: roster, vitals: vitals, freshKeys: freshKeys)
}

/// Port of `Events.tsx`'s `resolveProject` (lines 291-302): a card's own
/// `projectName` (findings already carry one) wins outright; otherwise
/// resolve through the card's `runID` → `SituationStore.runToWorkspace` →
/// the matching `APIProjectRow`. `projectsByWorkspace` is the caller's own
/// `[wsID: APIProjectRow]` index (built once per render from
/// `store.projects`, same as the web's `wsById` memo) rather than a second
/// parameter list this function would have to re-derive on every call.
public func resolveCardProject(
    _ card: StreamCard,
    runToWorkspace: [String: String],
    projectsByWorkspace: [String: APIProjectRow]
) -> (label: String?, branch: String?) {
    if let name = card.projectName {
        return (name, nil)
    }
    if let runID = card.runID,
       let wsID = runToWorkspace[runID],
       let project = projectsByWorkspace[wsID] {
        return (project.name, project.branch)
    }
    return (nil, nil)
}
