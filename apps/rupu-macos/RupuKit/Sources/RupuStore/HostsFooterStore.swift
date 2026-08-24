import Foundation
import Observation
import RupuAPI

/// Drives the v2 sidebar rail's host-fleet line (`"N hosts · M down"`),
/// mirroring `Shell.tsx`'s `HostFooter` in the web CP: polls
/// `GET /api/hosts` every 60s and reduces the response down to a count pair.
///
/// `apply(_:)` is the pure seam — computing `(total, down)` from a batch of
/// `APIHostRow`s never touches the network, so it's the only part of this
/// store worth unit-testing directly (Task 1 brief: the poll loop itself is
/// deliberately not timing-tested). `activate(client:)`/`deactivate()`
/// mirror `RupuBackend.HealthMonitor`'s `start()`/`stop()` idiom: a
/// `Task`-based loop guarded by `task == nil` for idempotence, cancelled and
/// nilled out on `deactivate()`. A failed poll is silently swallowed —
/// `summary` just keeps whatever it last held, so a momentary `/api/hosts`
/// hiccup doesn't blank the footer.
@MainActor
@Observable
public final class HostsFooterStore {
    public private(set) var summary: (total: Int, down: Int)?

    private var client: CPClient?
    private var task: Task<Void, Never>?

    public init() {}

    /// Reduces a batch of host rows to `(total, down)` — `down` is every row
    /// whose `status` isn't the literal `"online"` (matches the existing
    /// `== "online"` comparisons `ActivityStore`/`LauncherStore` already use
    /// against this same untyped string field).
    public func apply(_ rows: [APIHostRow]) {
        let down = rows.filter { $0.status != "online" }.count
        summary = (total: rows.count, down: down)
    }

    /// Idempotent: a second `activate(client:)` call while already polling
    /// (e.g. a re-healthy transition after a degraded blip) just updates
    /// `client` for the next tick rather than spawning a second loop.
    public func activate(client: CPClient) {
        self.client = client
        guard task == nil else { return }
        task = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.pollOnce()
                try? await Task.sleep(for: .seconds(60))
            }
        }
    }

    public func deactivate() {
        task?.cancel()
        task = nil
        client = nil
    }

    private func pollOnce() async {
        guard let client else { return }
        guard let rows = try? await client.hosts() else { return }
        apply(rows)
    }
}
