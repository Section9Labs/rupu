import Testing
import RupuStore
@testable import RupuSecurity

/// `findingNavigationRoute(surface:runID:)` — the pure mapping
/// `FindingsTable.swift`'s rows delegate to for their honest, per-surface
/// navigation decision (see `FindingsTabView`'s doc comment for the full
/// rationale this table pins down):
/// - `workflow`/`autoflow` → `.runDetail(id:host:)`, local (`host: nil`) —
///   the findings registry is local-only.
/// - `agent`/`session` (and anything unrecognized) → `nil`, never a guessed
///   route — those run ids structurally 404 against `GET /api/runs/:id`
///   (the Phase 2 lesson), and `APIFinding` carries neither a
///   `transcriptPath` nor a `sessionID` to route the alternates with.
/// - An empty `runID` never routes, regardless of `surface` — the
///   all-empty `APICoverageAttribution` default `APIFinding`'s memberwise
///   init falls back to for pre-existing call sites is deliberately
///   unroutable.
@Test func workflowSurfaceRoutesToLocalRunDetail() {
    #expect(findingNavigationRoute(surface: "workflow", runID: "run_9k2f") == .runDetail(id: "run_9k2f", host: nil))
}

@Test func autoflowSurfaceRoutesToLocalRunDetail() {
    #expect(findingNavigationRoute(surface: "autoflow", runID: "run_am4d") == .runDetail(id: "run_am4d", host: nil))
}

@Test func agentSurfaceDoesNotRoute() {
    #expect(findingNavigationRoute(surface: "agent", runID: "run_agent_1") == nil)
}

@Test func sessionSurfaceDoesNotRoute() {
    #expect(findingNavigationRoute(surface: "session", runID: "sess_1") == nil)
}

@Test func unrecognizedSurfaceDoesNotRoute() {
    #expect(findingNavigationRoute(surface: "some_future_surface", runID: "run_1") == nil)
}

/// Even a `workflow`/`autoflow` surface never routes with an empty
/// `runID` — the placeholder `APICoverageAttribution` default, not a real
/// run to navigate to.
@Test func emptyRunIDNeverRoutesRegardlessOfSurface() {
    #expect(findingNavigationRoute(surface: "workflow", runID: "") == nil)
    #expect(findingNavigationRoute(surface: "autoflow", runID: "") == nil)
}
