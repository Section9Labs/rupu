import Foundation
import Observation
import RupuAPI

/// Generic loading/content/empty/failure state for a block of UI, keyed on
/// its payload type `T`. `value` unwraps `.content` for callers that just
/// want the payload when present, nil otherwise. `PagedSnapshot` keeps its
/// `rows` independent of `state` deliberately — a failed refresh leaves
/// `state` as `.failed(...)` while `rows` stays whatever it already was,
/// so stale content stays visible through a transient error rather than
/// being blanked by it.
public enum BlockState<T: Sendable>: Sendable {
    case loading
    case content(T)
    case empty
    case failed(String)

    public var value: T? {
        if case .content(let value) = self { return value }
        return nil
    }
}

/// Offset/limit pager over a `fetch` closure. `rupu-cp`'s list endpoints
/// (`/api/runs`, `/api/runs/agents`, `/api/runs/autoflows/events`,
/// `/api/sessions`, ...) have no pagination envelope — bare array out, "end"
/// signaled only by a page shorter than what was asked for (see
/// api-facts.md) — so that's the only exhaustion signal this type looks
/// for too.
///
/// `refresh()` reloads page 0 and replaces `rows` wholesale (so a row
/// inserted upstream since the last load reappears at the top, rather than
/// being missed by a `loadMore`-only path). `loadMore()` appends the next
/// page after the current `rows.count` offset and is a no-op once
/// `exhausted`. A `fetch` failure on either call sets `state = .failed(...)`
/// and leaves `rows` untouched — the recovery path is calling `refresh()`
/// again.
///
/// **Reentrancy:** `refresh()`/`loadMore()` are `async` and this class is
/// `@MainActor`, not actor-isolated-per-call — a second call can start
/// while the first is still suspended inside `await fetch(...)` (e.g. two
/// views both triggering a load on appear). Without a guard, both calls
/// would read the same `rows.count` before either finishes and each append
/// its own page, duplicating rows. `inFlight` closes that window: whichever
/// call sets it first proceeds: any other `refresh()`/`loadMore()` call
/// that arrives before it clears is a **no-op** (returns immediately,
/// `state`/`rows` untouched) rather than queueing or cancelling the
/// in-progress one. A caller that truly needs "refresh after this load
/// finishes" should await the in-flight call itself rather than fire a
/// second one — `PagedSnapshot` doesn't coalesce for you.
@MainActor
@Observable
public final class PagedSnapshot<Row: Sendable & Identifiable> {
    public private(set) var rows: [Row] = []
    public private(set) var state: BlockState<Void> = .loading
    public private(set) var exhausted: Bool = false

    /// True only while a `loadMore()` fetch is actually in flight — deliberately
    /// separate from the `inFlight` reentrancy guard below (which also covers
    /// `refresh()`/`resetAndRefresh()`): the infinite-scroll sentinel (perf &
    /// interaction arc, Plan 5 Task 5) needs to distinguish "still loading the
    /// next page" (show "loading more…") from any other in-flight fetch.
    public private(set) var isLoadingMore: Bool = false

    /// Mutable (perf & interaction arc, Plan 5 Task 5 — `ActivityStore`
    /// switches this between a kind page's 20 and `.all`'s 50 via
    /// `setPageSize(_:)` on every `activate(kind:)`, since all four kinds'
    /// sources are shared instances rather than per-mode ones — see
    /// `ActivityStore.sources`'s doc comment). Only takes effect on the NEXT
    /// `refresh()`/`loadMore()`/`resetAndRefresh()` call — changing it mid-page
    /// does not retroactively resize `rows`.
    private var pageSize: Int
    private let fetch: @Sendable (_ offset: Int, _ limit: Int) async throws -> [Row]
    private var inFlight = false

    /// Bumped by `resetAndRefresh()` — every in-flight `refresh()`/`loadMore()`/
    /// `resetAndRefresh()` call captures the generation it started with and
    /// re-checks it after its `await fetch(...)` before mutating `rows`/`state`/
    /// `exhausted`: a call whose generation no longer matches (a
    /// `resetAndRefresh()` landed while it was still in flight — e.g. an
    /// operator changed the date-range filter mid-load) is a stale result and
    /// is discarded rather than applied on top of newer state. This is what
    /// makes a filter change reset paging safely even when a `loadMore()` was
    /// already in flight for the OLD filter: `PagedSnapshot`'s single `inFlight`
    /// boolean is not enough on its own (a plain `refresh()` would just no-op
    /// if `inFlight` were already true), so `resetAndRefresh()` deliberately
    /// bypasses that guard and lets generation checking discard the stale
    /// caller instead.
    private var generation = 0

    public init(pageSize: Int = 50, fetch: @escaping @Sendable (_ offset: Int, _ limit: Int) async throws -> [Row]) {
        self.pageSize = pageSize
        self.fetch = fetch
    }

    /// Changes the page size used by every subsequent `refresh()`/`loadMore()`/
    /// `resetAndRefresh()` call. See `pageSize`'s own doc comment.
    public func setPageSize(_ newPageSize: Int) {
        pageSize = newPageSize
    }

    /// Returns whether the fetch actually ran (`true`) or was skipped
    /// because a call was already in flight (`false`) — both a successful
    /// fetch and one that threw (surfaced via `state = .failed`) count as
    /// "ran". `@discardableResult` so existing call sites that don't care
    /// keep compiling unchanged; `ActivityStore`'s debounced live-tail
    /// refresh reads it to detect a collision and retry once (see that
    /// type's `scheduleDebouncedRefresh`).
    @discardableResult
    public func refresh() async -> Bool {
        guard !inFlight else { return false }
        let gen = generation
        inFlight = true
        defer { if generation == gen { inFlight = false } }

        state = .loading
        do {
            let page = try await fetch(0, pageSize)
            guard gen == generation else { return true }
            rows = page
            exhausted = page.count < pageSize
            state = page.isEmpty ? .empty : .content(())
        } catch {
            // Cancellation (the fetch's owning `Task` was cancelled — most
            // commonly a SwiftUI `.task(id:)` whose id changed mid-fetch)
            // is benign: it ran, it just didn't get to finish. `state`
            // stays whatever it already was (`.loading`, set just above)
            // rather than flipping to `.failed` for something the user
            // didn't cause. See `isCancellation`'s doc comment.
            guard !isCancellation(error) else { return true }
            guard gen == generation else { return true }
            state = .failed(String(describing: error))
        }
        return true
    }

    /// Refetches page 0 and SPLICES it onto the front of `rows`, rather than
    /// replacing `rows` wholesale like `refresh()` does (perf & interaction
    /// arc, Plan 5 Task 5). This is the live-tail-safe refresh: once
    /// `loadMore()` is wired to a scroll sentinel, an operator can have
    /// several pages loaded (well past `pageSize` rows) — a background
    /// refresh triggered by an SSE event or a reconnect resnapshot must pick
    /// up whatever changed at the head WITHOUT discarding everything the
    /// operator already scrolled into. `ActivityStore.refreshActiveSources()`
    /// (shared by `activate(kind:)`, the "N new runs" pill, the debounced
    /// live-tail refresh, and `StreamLifecycle`'s reconnect resnapshot) uses
    /// this instead of `refresh()` for exactly that reason.
    ///
    /// Any row already present further down `rows` that ALSO appears in the
    /// fresh head is deduped in favor of the fresh copy (it may have moved
    /// up, or its fields may have changed since); everything else already
    /// loaded stays exactly where it was, in its existing relative order.
    ///
    /// Does NOT set `state = .loading` (unlike `refresh()`) — this is a
    /// background refresh; swapping already-visible content for a loading
    /// spinner mid-view would itself be a scroll-disrupting regression, just
    /// via a different mechanism than replacing `rows` outright.
    ///
    /// `exhausted` is only touched when there was NO existing tail beyond
    /// the head (a plain page-0-only view, e.g. right after `activate(kind:)`
    /// — behaves identically to `refresh()` in that case): a head-only
    /// refresh reveals nothing about whatever boundary a previous
    /// `loadMore()` already established further down, so that boundary is
    /// left untouched when a tail exists.
    @discardableResult
    public func refreshHead() async -> Bool {
        guard !inFlight else { return false }
        let gen = generation
        inFlight = true
        defer { if generation == gen { inFlight = false } }

        do {
            let head = try await fetch(0, pageSize)
            guard gen == generation else { return true }
            let headIDs = Set(head.map(\.id))
            let preservedTail = rows.filter { !headIDs.contains($0.id) }
            rows = head + preservedTail
            if preservedTail.isEmpty {
                exhausted = head.count < pageSize
            }
            state = rows.isEmpty ? .empty : .content(())
        } catch {
            guard !isCancellation(error) else { return true }
            guard gen == generation else { return true }
            state = .failed(String(describing: error))
        }
        return true
    }

    /// Same "ran vs skipped" contract as `refresh()` above — `false` covers
    /// both reasons a call can be skipped (already in flight, or already
    /// `exhausted`).
    @discardableResult
    public func loadMore() async -> Bool {
        guard !inFlight, !exhausted else { return false }
        let gen = generation
        inFlight = true
        isLoadingMore = true
        defer {
            if generation == gen {
                inFlight = false
                isLoadingMore = false
            }
        }

        do {
            let page = try await fetch(rows.count, pageSize)
            guard gen == generation else { return true }
            rows.append(contentsOf: page)
            exhausted = page.count < pageSize
            state = .content(())
        } catch {
            // See `refresh()`'s matching comment — a cancelled fetch leaves
            // `state`/`rows` exactly as they already were.
            guard !isCancellation(error) else { return true }
            guard gen == generation else { return true }
            state = .failed(String(describing: error))
        }
        return true
    }

    /// Bumps the generation and force-refreshes page 0, DISCARDING whatever
    /// `refresh()`/`loadMore()` call may already be in flight for the old
    /// generation (its eventual completion will see `gen != generation` and
    /// no-op — see `generation`'s doc comment) rather than skipping like a
    /// plain `refresh()` would when `inFlight` is already `true`.
    ///
    /// The one entry point for a filter change (perf & interaction arc, Plan
    /// 5 Task 5 — a date-range or `kind` change that must genuinely restart
    /// paging from page 0): `ActivityStore`'s `since`/`until` setters call
    /// this instead of `refresh()` specifically so an in-flight `loadMore()`
    /// from the OLD filter can never silently win the race and append a
    /// stale page onto the freshly-reset `rows`.
    @discardableResult
    public func resetAndRefresh() async -> Bool {
        generation += 1
        let gen = generation
        inFlight = true
        isLoadingMore = false
        defer { if generation == gen { inFlight = false } }

        state = .loading
        do {
            let page = try await fetch(0, pageSize)
            guard gen == generation else { return true }
            rows = page
            exhausted = page.count < pageSize
            state = page.isEmpty ? .empty : .content(())
        } catch {
            guard !isCancellation(error) else { return true }
            guard gen == generation else { return true }
            state = .failed(String(describing: error))
        }
        return true
    }
}
