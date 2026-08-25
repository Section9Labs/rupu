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

    public init(cards: [StreamCard], roster: [RosterEntry], vitals: Vitals) {
        self.cards = cards
        self.roster = roster
        self.vitals = vitals
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
    cardCap: Int = 5_000,
    findingsStreamCap: Int = 60
) -> SituationSnapshot {
    let eventCards: [StreamCard] = eventRows.compactMap { row in
        cardForEvent(row.event, ts: Date(timeIntervalSince1970: Double(row.ts) / 1000))
    }
    let findingCards: [StreamCard] = findings
        .sorted { rfc3339ToMS($0.declaredAt) > rfc3339ToMS($1.declaredAt) }
        .prefix(findingsStreamCap)
        .map(cardForFinding)
    let cards = mergeStream(eventCards + findingCards, max: cardCap)

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

    return SituationSnapshot(cards: cards, roster: roster, vitals: vitals)
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
