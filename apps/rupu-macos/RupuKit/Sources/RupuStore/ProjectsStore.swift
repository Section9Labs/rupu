import Foundation
import Observation
import RupuAPI

/// Owns the Projects list screen's (Phase 5A, Task 5) single fetch: every
/// registered workspace, via `GET /api/projects`. No streaming, no
/// mutations — a project's `runCount`/`lastRunAt`/`usage` are a REST
/// snapshot the operator refreshes by revisiting the screen, same as
/// `ActivityRow`'s underlying rows before any live patch lands; unlike
/// those rows this has no firehose consumer at all, since there is no
/// project-level live event to react to. Sorting (`name`/`runs`/`lastRun`/
/// `spend`) is view-local state (`ProjectsScreen`'s own `ListSort`), applied
/// over `rows` the same way `ActivityTable`'s view-local sort is applied
/// over `ActivityStore.rows` — not this store's concern.
///
/// **Audited, not converted, for local-first (perf & interaction arc, Plan
/// 5 Task 2)**: this store was on that task's list of screens suspected of
/// blocking first paint on a fleet-wide fan-out. Verified against
/// `list_projects` (`crates/rupu-cp/src/api/projects.rs`): no `Query`
/// extractor at all, reads straight from `s.run_store`/this CP's own
/// `WorkspaceStore` — a "project" is a workspace REGISTERED ON THIS CP
/// instance, not a per-Fleet-host concept the server ever fans out across
/// (unlike `runs`/`dashboard`/`usage`, which proxy to remote hosts). There
/// is no per-host progressive merge to build here; converting this store
/// would mean fabricating a `host` param the server ignores, which the task
/// brief explicitly forbids. Left unchanged.
@MainActor
@Observable
public final class ProjectsStore {
    public private(set) var rows: BlockState<[APIProjectRow]> = .loading

    private let fetchProjects: @Sendable () async throws -> [APIProjectRow]

    /// Production entry point — `ProjectsScreen` calls this.
    public convenience init(client: CPClient) {
        self.init(fetchProjects: { try await client.projects() })
    }

    /// Designated init — takes a plain fetch closure rather than a
    /// `CPClient` directly, the same "fake client closures" seam every
    /// other store in this module already established (`CPClient` itself
    /// has no protocol to mock). `internal`, not `public` — reached from
    /// tests via `@testable import RupuStore`.
    init(fetchProjects: @escaping @Sendable () async throws -> [APIProjectRow]) {
        self.fetchProjects = fetchProjects
    }

    /// One-shot REST load. Repeatable — safe to call again (e.g. the screen
    /// reappearing), same "reload from scratch" contract every other
    /// store's `activate()` follows.
    public func activate() async {
        do {
            let projects = try await fetchProjects()
            rows = projects.isEmpty ? .empty : .content(projects)
        } catch {
            // Cancellation (a superseded `.task`) is benign — see
            // `isCancellation`'s doc comment; every other store's load path
            // in this module follows the same guard.
            guard !isCancellation(error) else { return }
            rows = .failed(String(describing: error))
        }
    }
}
