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

    private let pageSize: Int
    private let fetch: @Sendable (_ offset: Int, _ limit: Int) async throws -> [Row]
    private var inFlight = false

    public init(pageSize: Int = 50, fetch: @escaping @Sendable (_ offset: Int, _ limit: Int) async throws -> [Row]) {
        self.pageSize = pageSize
        self.fetch = fetch
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
        inFlight = true
        defer { inFlight = false }

        state = .loading
        do {
            let page = try await fetch(0, pageSize)
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
        inFlight = true
        defer { inFlight = false }

        do {
            let page = try await fetch(rows.count, pageSize)
            rows.append(contentsOf: page)
            exhausted = page.count < pageSize
            state = .content(())
        } catch {
            // See `refresh()`'s matching comment — a cancelled fetch leaves
            // `state`/`rows` exactly as they already were.
            guard !isCancellation(error) else { return true }
            state = .failed(String(describing: error))
        }
        return true
    }
}
