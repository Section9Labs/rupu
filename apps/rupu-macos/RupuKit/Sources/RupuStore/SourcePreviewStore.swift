import Foundation
import Observation
import RupuAPI

/// Per-`(path, line[, col])` fetch cache backing the run-detail transcript's
/// inline "view source" / "view AST" disclosures (Phase 6B, Task 5) —
/// `GET /api/runs/:id/source` and `GET /api/runs/:id/ast`
/// (`CPClient.runSource`/`runAst`). Mirrors the web viewer's `SourcePreview`/
/// `AstTree` components (`crates/rupu-cp/web/src/components/transcript/
/// {SourcePreview,AstTree}.tsx`), which fetch lazily on mount and cache
/// nothing themselves — React unmounts a closed disclosure, discarding its
/// fetched state automatically, so a re-toggled row just re-fetches. A
/// persistent SwiftUI view hierarchy doesn't get that for free, so this
/// store adds the caching layer the web side doesn't need.
///
/// **Lazy, cache-once-per-key.** `loadSourceIfNeeded`/`loadAstIfNeeded` are
/// no-ops once a key already has ANY cached `BlockState` — loading,
/// content, OR a prior failure alike — same "second call after the first is
/// a no-op fetch-wise" contract `CodeStore.loadFilter()` establishes for its
/// one-shot file list. `reloadSource`/`reloadAst` are the unconditional
/// Retry path, bypassing that guard, mirroring `CodeStore.reloadFilter()`'s
/// own split.
///
/// **Generation-guarded on run identity.** `setRun(runID:host:)` is the
/// seam a long-lived owner (a screen that reuses this store across a
/// `runID` prop change, the same "same screen slot, different run" case
/// `RunDetailScreen`'s own doc comment describes for `RunDetailStore`) calls
/// whenever the focused run changes. A DIFFERENT `runID` flushes both
/// caches and bumps an internal generation counter, so a fetch already in
/// flight for the OLD run can never populate the NEW run's cache once it
/// eventually lands — same "capture the generation at dispatch, apply the
/// result only if it still matches" idiom `FleetStore`/`DashboardStore`/
/// `CodeStore` all use, applied here across a run-identity change rather
/// than a per-block reload. Calling `setRun(runID:host:)` with the SAME
/// `runID` is a no-op (including for `host`) — in this app one route owns
/// exactly one `(runID, host)` pair, so `host` never changes independently
/// of `runID` (see `RunDetailScreen.init`).
@MainActor
@Observable
public final class SourcePreviewStore {
    private struct SourceKey: Hashable {
        let path: String
        let line: Int
    }

    private struct AstKey: Hashable {
        let path: String
        let line: Int
        let col: Int
    }

    public private(set) var runID: String
    public private(set) var host: String?

    private let client: CPClient

    private var sourceCache: [SourceKey: BlockState<APISourceSlice>] = [:]
    private var astCache: [AstKey: BlockState<APIAstResponse>] = [:]

    /// Bumped only by `setRun(runID:host:)` on an actual `runID` change —
    /// see the type doc comment's "Generation-guarded on run identity"
    /// section.
    private var runGeneration = 0

    public init(runID: String, host: String?, client: CPClient) {
        self.runID = runID
        self.host = host
        self.client = client
    }

    /// See the type doc comment. Flushes both caches and bumps
    /// `runGeneration` only when `runID` actually differs from the one this
    /// store currently holds — a same-`runID` call (e.g. a redundant
    /// `activate()` re-entry) leaves every cached entry and any in-flight
    /// fetch's eventual result untouched.
    public func setRun(runID: String, host: String?) {
        guard runID != self.runID else { return }
        self.runID = runID
        self.host = host
        sourceCache = [:]
        astCache = [:]
        runGeneration += 1
    }

    /// `nil` means "never requested" — distinct from `.loading`, which
    /// means a fetch for this exact key is in flight.
    public func sourceState(path: String, line: Int) -> BlockState<APISourceSlice>? {
        sourceCache[SourceKey(path: path, line: line)]
    }

    public func astState(path: String, line: Int, col: Int) -> BlockState<APIAstResponse>? {
        astCache[AstKey(path: path, line: line, col: col)]
    }

    /// Lazy fetch — a no-op once `sourceState(path:line:)` is already
    /// non-`nil` for this key (loading, content, or a prior failure alike).
    /// The caller-controlled "only mount the preview view once the row is
    /// expanded" is what makes this actually lazy end-to-end; this guard
    /// just keeps a SECOND expand of the same row from re-fetching.
    public func loadSourceIfNeeded(path: String, line: Int, context: Int = 20) async {
        guard sourceCache[SourceKey(path: path, line: line)] == nil else { return }
        await reloadSource(path: path, line: line, context: context)
    }

    /// Unconditional (re)fetch — the Retry path for a `.failed` slice.
    public func reloadSource(path: String, line: Int, context: Int = 20) async {
        let key = SourceKey(path: path, line: line)
        let generation = runGeneration
        let runID = self.runID
        let host = self.host
        sourceCache[key] = .loading
        do {
            let slice = try await client.runSource(id: runID, path: path, line: line, context: context, host: host)
            guard generation == runGeneration else { return }
            sourceCache[key] = .content(slice)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == runGeneration else { return }
            sourceCache[key] = .failed(String(describing: error))
        }
    }

    /// Lazy fetch — same "second call is a no-op" contract as
    /// `loadSourceIfNeeded`, keyed by `(path, line, col)` since the AST
    /// endpoint targets a column too.
    public func loadAstIfNeeded(path: String, line: Int, col: Int) async {
        guard astCache[AstKey(path: path, line: line, col: col)] == nil else { return }
        await reloadAst(path: path, line: line, col: col)
    }

    /// Unconditional (re)fetch — the Retry path for a `.failed` AST response.
    public func reloadAst(path: String, line: Int, col: Int) async {
        let key = AstKey(path: path, line: line, col: col)
        let generation = runGeneration
        let runID = self.runID
        let host = self.host
        astCache[key] = .loading
        do {
            let response = try await client.runAst(id: runID, path: path, line: line, col: col, host: host)
            guard generation == runGeneration else { return }
            astCache[key] = .content(response)
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == runGeneration else { return }
            astCache[key] = .failed(String(describing: error))
        }
    }
}
