import Foundation
import Observation
import RupuAPI

/// Owns the Coverage Detail screen's (Phase 5B, Task 4) two independent
/// blocks, scoped to one specific `target` × `wsID` pair (`Route.
/// coverageDetail`'s own doc comment explains why both are required to
/// disambiguate a `targetID` that collides across workspaces): `detail` —
/// one coverage target's full record (assertions, findings, header figures —
/// the header every tab renders above it, plus the Overview tab's own
/// content) — and `catalog` — the flattened concern catalog effective for
/// this target (the Catalog tab's content). Same per-entity store precedent
/// `RunDetailStore`/`ProjectDetailStore`/`AgentRunDetailStore` already
/// establish for a screen scoped to one id rather than a fleet-wide list.
///
/// **`detail` loads eagerly, `catalog` loads lazily** — same split
/// `ProjectDetailStore` uses between its own `detail` (eager, feeds the
/// header + Overview tab) and every other tab's block (lazy, `loadXIfNeeded()`
/// on first tab visit).
///
/// **`catalog` is additionally gated on `detail.hasCatalog`** — a target
/// with no catalog (`hasCatalog == false`, `CoverageModels.swift`'s doc
/// comment on `APICoverageSummary`) has no `GET /api/coverage/:target/
/// catalog` content to fetch; `loadCatalogIfNeeded()`/`loadCatalog()` are
/// both no-ops in that case, per the brief's "lazy-fetch ONLY when
/// hasCatalog — when false, NO fetch" contract. See those methods' own doc
/// comments for the exact gating. `CoverageDetailScreen`'s Catalog tab reads
/// `detail.value?.hasCatalog` directly to decide between its own honest
/// quiet "no catalog" note and rendering `catalog`'s `BlockState` — it never
/// has to guess this from `catalog` staying `.loading` forever.
///
/// **Read-only, no streaming** — same "Snapshot only, no tail" rationale
/// `ProjectDetailStore`/`SessionDetailStore` document: a coverage target has
/// no event stream of its own to subscribe to, only the two REST snapshots
/// this store fetches.
@MainActor
@Observable
public final class CoverageDetailStore {
    public private(set) var detail: BlockState<APICoverageDetail> = .loading
    public private(set) var catalog: BlockState<APICoverageCatalog> = .loading

    private let fetchDetail: @Sendable () async throws -> APICoverageDetail
    private let fetchCatalog: @Sendable () async throws -> APICoverageCatalog

    /// See `loadCatalogIfNeeded()`'s doc comment — tracks whether the
    /// catalog fetch has ever dispatched for the current `activate()` cycle.
    private var catalogRequested = false

    /// Production entry point — `CoverageDetailScreen` calls this. `wsID` is
    /// threaded straight through to both `CPClient` calls' disambiguation
    /// query param — see `Route.coverageDetail`'s doc comment on why it's
    /// never optional here even though `CPClient.coverageDetail(target:
    /// wsID:)` itself accepts a `nil`.
    public convenience init(target: String, wsID: String, client: CPClient) {
        self.init(
            fetchDetail: { try await client.coverageDetail(target: target, wsID: wsID) },
            fetchCatalog: { try await client.coverageCatalog(target: target, wsID: wsID) }
        )
    }

    /// Designated init — plain fetch closures, same "fake client closures"
    /// seam every other store in this module already established. `internal`,
    /// not `public` — reached from tests via `@testable import RupuStore`.
    init(
        fetchDetail: @escaping @Sendable () async throws -> APICoverageDetail,
        fetchCatalog: @escaping @Sendable () async throws -> APICoverageCatalog
    ) {
        self.fetchDetail = fetchDetail
        self.fetchCatalog = fetchCatalog
    }

    /// Loads `detail` alone (the header + Overview tab's own content) and
    /// resets the catalog lazy-flag, so a screen re-appearance (same store
    /// instance) lets the Catalog tab refetch fresh rather than staying
    /// latched on stale content from a previous visit. Repeatable, like
    /// every other store's `activate()` in this codebase.
    public func activate() async {
        catalogRequested = false
        await loadDetail()
    }

    private func loadDetail() async {
        detail = .loading
        do {
            detail = .content(try await fetchDetail())
        } catch {
            guard !isCancellation(error) else { return }
            detail = .failed(String(describing: error))
        }
    }

    // MARK: - Catalog (lazy, gated on `hasCatalog`)

    /// Dispatches the catalog fetch at most once per `activate()` cycle, and
    /// ONLY once `detail` has resolved with `hasCatalog == true`. Two
    /// distinct "do nothing" cases:
    /// - `detail` hasn't resolved yet (`.loading`/`.failed`/`.empty`, so
    ///   `hasCatalog` is unknown) — `catalogRequested` stays `false`, so a
    ///   later call (once `detail` lands) can still dispatch.
    /// - `detail` resolved with `hasCatalog == false` — this target
    ///   genuinely has no catalog; no request is ever sent for it, this
    ///   call, or any later one this activation (nothing about a fixed
    ///   `hasCatalog` value can change without a fresh `activate()`).
    ///
    /// This method alone can't strand the Catalog tab — a no-op call while
    /// `detail` is unresolved is always safe to retry later, per the first
    /// bullet. The caller is what has to actually retry it:
    /// `CoverageDetailScreen.tabPanel`'s `.task(id:)` folds `store.detail.
    /// value != nil` into its id specifically so selecting Catalog before
    /// `detail` lands gets a second, successful call once it does — see
    /// that task's own doc comment.
    public func loadCatalogIfNeeded() async {
        guard detail.value?.hasCatalog == true else { return }
        guard !catalogRequested else { return }
        catalogRequested = true
        await loadCatalog()
    }

    /// Forces a reload regardless of `catalogRequested` — the screen's own
    /// retry affordance for a `.failed` Catalog tab. Still gated on
    /// `hasCatalog == true`, same as `loadCatalogIfNeeded()` — a target
    /// without a catalog has nothing here to retry.
    public func loadCatalog() async {
        guard detail.value?.hasCatalog == true else { return }
        catalogRequested = true
        do {
            let value = try await fetchCatalog()
            catalog = value.concerns.isEmpty ? .empty : .content(value)
        } catch {
            guard !isCancellation(error) else { return }
            catalog = .failed(String(describing: error))
        }
    }
}
