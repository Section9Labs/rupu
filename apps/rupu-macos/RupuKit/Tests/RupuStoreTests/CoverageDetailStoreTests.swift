import Testing
import Foundation
@testable import RupuStore
import RupuAPI

// MARK: - Test doubles

/// Thread-safe call counter — same rationale as every other store test's
/// `Counter` in this module (`SecurityStoreTests`/`ProjectDetailStoreTests`):
/// a plain captured `var` can't cross into a `@Sendable` fetch closure under
/// Swift 6 strict concurrency.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var v = 0
    @discardableResult
    func increment() -> Int { lock.withLock { v += 1; return v } }
    var value: Int { lock.withLock { v } }
}

private struct StubError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - Fixture builders

private func attribution(runID: String = "run-1", surface: String = "workflow") -> APICoverageAttribution {
    APICoverageAttribution(runID: runID, model: "claude-sonnet-4-6", surface: surface)
}

private func assertion(concernID: String = "sql-injection", filePath: String = "src/auth/login.rs") -> APICoverageAssertion {
    APICoverageAssertion(
        concernID: concernID, filePath: filePath, status: "clean",
        evidence: APICoverageEvidence(summary: "parameterized", lineRanges: [[1, 10]], findingIDs: []),
        declaredBy: attribution(), declaredAt: "2026-08-20T11:00:00Z"
    )
}

private func coverageFinding(id: String = "fnd-1", concernID: String? = "timing-attack") -> APICoverageFinding {
    APICoverageFinding(
        id: id, filePath: "src/auth/session.rs", lineRange: [42, 58], scope: "line",
        summary: "timing side-channel", severity: "critical", concernID: concernID,
        evidence: APICoverageFindingEvidence(codeExcerpt: nil, rationale: "non-constant-time", references: []),
        declaredBy: attribution(), declaredAt: "2026-08-20T12:00:00Z"
    )
}

private func detailPayload(
    hasCatalog: Bool = true,
    assertions: [APICoverageAssertion] = [assertion()],
    findings: [APICoverageFinding] = [coverageFinding()]
) -> APICoverageDetail {
    APICoverageDetail(
        wsID: "ws-1", project: "rupu", targetID: "auth-core", assertionLines: 128,
        hasCatalog: hasCatalog, assertions: assertions, findings: findings, files: []
    )
}

private func concern(id: String = "timing-attack") -> APICoverageConcern {
    APICoverageConcern(
        id: id, name: "Timing side-channel", description: "Secret comparisons must be constant-time.",
        severity: "critical", applicableGlobs: ["**/auth/**"], minStrength: "read", references: [], tags: []
    )
}

private func catalogPayload(concerns: [APICoverageConcern] = [concern()]) -> APICoverageCatalog {
    APICoverageCatalog(concerns: concerns, sources: ["timing-attack": "stride"], renderModes: ["timing-attack": "full"])
}

// MARK: - makeStore

@MainActor
private func makeStore(
    fetchDetail: @escaping @Sendable () async throws -> APICoverageDetail = { detailPayload() },
    fetchCatalog: @escaping @Sendable () async throws -> APICoverageCatalog = { catalogPayload() }
) -> CoverageDetailStore {
    CoverageDetailStore(fetchDetail: fetchDetail, fetchCatalog: fetchCatalog)
}

// MARK: - Initial state

@MainActor @Test func bothCoverageDetailBlocksStartLoadingBeforeAnyFetch() {
    let store = makeStore()
    if case .loading = store.detail {} else { Issue.record("expected detail .loading") }
    if case .loading = store.catalog {} else { Issue.record("expected catalog .loading") }
}

// MARK: - activate() loads detail eagerly, catalog stays lazy

@MainActor @Test func activateLoadsDetailButNeverTouchesCatalog() async {
    let detailCounter = Counter()
    let catalogCounter = Counter()
    let store = makeStore(
        fetchDetail: { detailCounter.increment(); return detailPayload() },
        fetchCatalog: { catalogCounter.increment(); return catalogPayload() }
    )

    await store.activate()

    #expect(detailCounter.value == 1)
    #expect(catalogCounter.value == 0)
    #expect(store.detail.value != nil)
    if case .loading = store.catalog {} else { Issue.record("expected catalog to still be .loading") }
}

// MARK: - Catalog gating on `hasCatalog`

/// A target with `hasCatalog == false` never fires a catalog fetch — not on
/// the first `loadCatalogIfNeeded()` call, and not on a forced `loadCatalog()`
/// "retry" either. Per `CoverageDetailStore`'s own doc comment and the
/// brief's "lazy-fetch ONLY when hasCatalog — when false, NO fetch" contract.
@MainActor @Test func loadCatalogIfNeededNeverFetchesWhenTargetHasNoCatalog() async {
    let catalogCounter = Counter()
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: false) },
        fetchCatalog: { catalogCounter.increment(); return catalogPayload() }
    )
    await store.activate()

    await store.loadCatalogIfNeeded()

    #expect(catalogCounter.value == 0)
    if case .loading = store.catalog {} else { Issue.record("expected catalog to still be .loading (never fetched)") }
}

@MainActor @Test func loadCatalogForcedRetryAlsoNeverFetchesWhenTargetHasNoCatalog() async {
    let catalogCounter = Counter()
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: false) },
        fetchCatalog: { catalogCounter.increment(); return catalogPayload() }
    )
    await store.activate()

    await store.loadCatalog()

    #expect(catalogCounter.value == 0)
}

/// A call before `detail` has resolved at all (still `.loading`, e.g.
/// `activate()` never ran) is also a no-op — `hasCatalog` is unknown, so
/// this must never guess.
@MainActor @Test func loadCatalogIfNeededIsANoOpBeforeDetailHasResolved() async {
    let catalogCounter = Counter()
    let store = makeStore(fetchCatalog: { catalogCounter.increment(); return catalogPayload() })

    await store.loadCatalogIfNeeded()

    #expect(catalogCounter.value == 0)
}

@MainActor @Test func loadCatalogIfNeededFetchesOnceWhenTargetHasCatalogAndIsANoOpOnASecondCall() async {
    let catalogCounter = Counter()
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: true) },
        fetchCatalog: { catalogCounter.increment(); return catalogPayload() }
    )
    await store.activate()

    await store.loadCatalogIfNeeded()
    await store.loadCatalogIfNeeded()

    #expect(catalogCounter.value == 1, "a second loadCatalogIfNeeded() must not refetch")
    #expect(store.catalog.value != nil)
}

/// `loadCatalog()` forces a refetch regardless of the lazy flag — the
/// screen's own retry affordance for a `.failed` Catalog tab.
@MainActor @Test func loadCatalogForcesARefetchEvenWhenAlreadyRequested() async {
    let catalogCounter = Counter()
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: true) },
        fetchCatalog: { catalogCounter.increment(); return catalogPayload() }
    )
    await store.activate()

    await store.loadCatalogIfNeeded()
    await store.loadCatalog()

    #expect(catalogCounter.value == 2)
}

/// `activate()` resets the catalog lazy-flag so a screen re-appearance (same
/// store instance) lets the Catalog tab refetch fresh on next selection —
/// same contract `ProjectDetailStore.activate()`/`SecurityStore.activate()`
/// document.
@MainActor @Test func activateResetsCatalogLazyFlagSoItRefetchesOnNextSelection() async {
    let catalogCounter = Counter()
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: true) },
        fetchCatalog: { catalogCounter.increment(); return catalogPayload() }
    )
    await store.activate()
    await store.loadCatalogIfNeeded()
    #expect(catalogCounter.value == 1)

    await store.activate()
    await store.loadCatalogIfNeeded()
    #expect(catalogCounter.value == 2)
}

// MARK: - Per-block independence

@MainActor @Test func detailFailureLeavesCatalogUnaffected() async {
    let store = makeStore(fetchDetail: { throw StubError(description: "detail down") })

    await store.activate()

    guard case .failed(let message) = store.detail else {
        Issue.record("expected detail .failed")
        return
    }
    #expect(message.contains("detail down"))
    if case .loading = store.catalog {} else { Issue.record("expected catalog to still be .loading") }
}

@MainActor @Test func catalogFailureLeavesDetailUnaffected() async {
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: true) },
        fetchCatalog: { throw StubError(description: "catalog down") }
    )
    await store.activate()

    await store.loadCatalogIfNeeded()

    guard case .failed(let message) = store.catalog else {
        Issue.record("expected catalog .failed")
        return
    }
    #expect(message.contains("catalog down"))
    #expect(store.detail.value != nil)
}

// MARK: - Empty-state semantics

/// `catalog` follows the ordinary list-block rule: an empty `concerns` array
/// is `.empty`, not `.content([])`.
@MainActor @Test func catalogSetsEmptyStateForAZeroConcernResult() async {
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: true) },
        fetchCatalog: { catalogPayload(concerns: []) }
    )
    await store.activate()

    await store.loadCatalogIfNeeded()

    if case .empty = store.catalog {} else { Issue.record("expected catalog .empty, got \(store.catalog)") }
}

// MARK: - Content decoding sanity

@MainActor @Test func loadDetailDecodesAssertionsAndFindings() async {
    let assertions = [assertion(concernID: "sql-injection"), assertion(concernID: "timing-attack", filePath: "src/auth/session.rs")]
    let findings = [coverageFinding(id: "fnd-1"), coverageFinding(id: "fnd-2")]
    let store = makeStore(fetchDetail: { detailPayload(assertions: assertions, findings: findings) })

    await store.activate()

    #expect(store.detail.value?.assertions.map(\.concernID) == ["sql-injection", "timing-attack"])
    #expect(store.detail.value?.findings.map(\.id) == ["fnd-1", "fnd-2"])
}

@MainActor @Test func loadCatalogDecodesConcernsSourcesAndRenderModes() async {
    let store = makeStore(
        fetchDetail: { detailPayload(hasCatalog: true) },
        fetchCatalog: { catalogPayload(concerns: [concern(id: "timing-attack"), concern(id: "sql-injection")]) }
    )
    await store.activate()

    await store.loadCatalogIfNeeded()

    #expect(store.catalog.value?.concerns.map(\.id) == ["timing-attack", "sql-injection"])
    #expect(store.catalog.value?.sources["timing-attack"] == "stride")
    #expect(store.catalog.value?.renderModes["timing-attack"] == "full")
}
