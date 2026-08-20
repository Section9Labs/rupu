import Foundation
import Observation

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
@MainActor
@Observable
public final class PagedSnapshot<Row: Sendable & Identifiable> {
    public private(set) var rows: [Row] = []
    public private(set) var state: BlockState<Void> = .loading
    public private(set) var exhausted: Bool = false

    private let pageSize: Int
    private let fetch: @Sendable (_ offset: Int, _ limit: Int) async throws -> [Row]

    public init(pageSize: Int = 50, fetch: @escaping @Sendable (_ offset: Int, _ limit: Int) async throws -> [Row]) {
        self.pageSize = pageSize
        self.fetch = fetch
    }

    public func refresh() async {
        state = .loading
        do {
            let page = try await fetch(0, pageSize)
            rows = page
            exhausted = page.count < pageSize
            state = page.isEmpty ? .empty : .content(())
        } catch {
            state = .failed(String(describing: error))
        }
    }

    public func loadMore() async {
        guard !exhausted else { return }
        do {
            let page = try await fetch(rows.count, pageSize)
            rows.append(contentsOf: page)
            exhausted = page.count < pageSize
            state = .content(())
        } catch {
            state = .failed(String(describing: error))
        }
    }
}
