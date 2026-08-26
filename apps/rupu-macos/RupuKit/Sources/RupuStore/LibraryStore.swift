import Foundation
import Observation
import RupuAPI
import RupuDesign

/// Maps an agent definition's frontmatter `permissionMode` (`AgentDefinition.
/// mode` / `AgentDetail.mode` — `"ask"` / `"bypass"` / `"readonly"`, verbatim
/// off the DTO) to the umbrella tone rule (`docs/macOS_design/HANDOFF.md`'s
/// Library line): read-only = done, ask = await, bypass = fail (always
/// loud). Pure, and the sole place this mapping is defined — the Library
/// screen's row badge and the Agent detail screen's header badge both call
/// this rather than re-deriving it.
///
/// **`nil` (or any string `parse_mode` on the Rust side wouldn't recognize —
/// same defensive tolerance) returns `nil`, never a guessed tone.** The
/// DTO's absence means "this agent's frontmatter has no `permissionMode:`
/// override" — the actual EFFECTIVE mode at run time still depends on
/// project/global config layers this per-agent row has no visibility into
/// (see `rupu_agent::permission::resolve_mode`'s precedence chain on the
/// Rust side). Asserting a tone here — e.g. defaulting absence to "ask" —
/// would claim knowledge this DTO field doesn't carry; the caller renders no
/// badge at all for a `nil` result instead.
public func agentPermissionTone(mode: String?) -> StatusTone? {
    switch mode {
    case "readonly": .done
    case "ask": .awaiting
    case "bypass": .failed
    default: nil
    }
}

/// Owns the Library screen's (Phase 5A, Task 7) three definition lists —
/// agents, workflows, autoflows — plus the one mutation this phase's Library
/// offers: enabling/disabling an autoflow definition. Also reused by
/// `WorkflowDetailScreen` (its own lazily-built instance) so the detail
/// page's autoflow toggle shares the exact same mutation/confirmation logic
/// the list screen's row toggle uses, rather than a second parallel
/// implementation.
///
/// **Three independent blocks**: `agents`/`workflows`/`autoflows` are
/// fetched concurrently by `activate()`, and — the same per-block-
/// independence contract every other multi-block store in this module
/// (`FleetStore`/`ProjectDetailStore`) already follows — one block failing
/// never blanks the others.
///
/// **Audited, not converted, for local-first (perf & interaction arc, Plan
/// 5 Task 2)**: this store was on that task's list of screens suspected of
/// blocking first paint on a fleet-wide fan-out. Verified against
/// `list_agents`/`list_workflows`/`list_autoflows` (`crates/rupu-cp/src/
/// api/{agents,workflows,autoflows}.rs`): none has a `host` query param or
/// any host-fan-out mechanism — all three read `.md`/YAML definitions
/// straight off `s.global_dir`/this CP's own `WorkspaceStore`, and
/// `list_autoflows` is explicitly documented Rust-side as "Local-only, no
/// `?host=`". There is no per-host progressive merge to build here;
/// converting this store would mean fabricating a `host` param the server
/// ignores, which the task brief explicitly forbids. Left unchanged.
///
/// **`setAutoflowEnabled` is IMMEDIATE, not confirm-on-refetch** (contrast
/// `FleetStore.removeHost`'s "row disappearing IS the confirmation"):
/// `POST /api/autoflows/:name/enable|disable`'s response body IS the
/// on-disk file's actual new state — see `CPClient.setAutoflowEnabled`'s and
/// `ActionVerb.setEnabled`'s doc comments. This confirms the pending key
/// straight off that response and patches `autoflows` in place with the new
/// value, with no follow-up fetch needed.
@MainActor
@Observable
public final class LibraryStore {
    public private(set) var agents: BlockState<[AgentDefinition]> = .loading
    public private(set) var workflows: BlockState<[WorkflowDefinition]> = .loading
    public private(set) var autoflows: BlockState<[AutoflowDefinition]> = .loading

    /// Shared with the rest of the app the same way `FleetStore`/
    /// `RunDetailStore` share `BackendController.pendingActions` — the
    /// screen passes that instance explicitly; the default here only exists
    /// so every test in this file can build a store without threading one
    /// through.
    public let pendingActions: PendingActions

    private let fetchAgents: @Sendable () async throws -> [AgentDefinition]
    private let fetchWorkflows: @Sendable () async throws -> [WorkflowDefinition]
    private let fetchAutoflows: @Sendable () async throws -> [AutoflowDefinition]
    private let postSetAutoflowEnabled:
        @Sendable (_ name: String, _ scopeKind: String?, _ scopeID: String?, _ enabled: Bool) async throws
            -> AutoflowSetEnabledResponse

    /// Production entry point — `LibraryScreen`/`WorkflowDetailScreen` call
    /// this.
    public convenience init(client: CPClient, pendingActions: PendingActions) {
        self.init(
            fetchAgents: { try await client.agentDefinitions() },
            fetchWorkflows: { try await client.workflowDefinitions() },
            fetchAutoflows: { try await client.autoflowDefinitions() },
            postSetAutoflowEnabled: { name, scopeKind, scopeID, enabled in
                try await client.setAutoflowEnabled(name: name, scopeKind: scopeKind, scopeID: scopeID, enabled: enabled)
            },
            pendingActions: pendingActions
        )
    }

    /// Designated init — plain fetch/mutate closures, the same "fake client
    /// closures" seam every other store in this module already established.
    /// `internal`, not `public` — reached from tests via `@testable import
    /// RupuStore`.
    init(
        fetchAgents: @escaping @Sendable () async throws -> [AgentDefinition],
        fetchWorkflows: @escaping @Sendable () async throws -> [WorkflowDefinition],
        fetchAutoflows: @escaping @Sendable () async throws -> [AutoflowDefinition],
        postSetAutoflowEnabled: @escaping @Sendable (
            _ name: String, _ scopeKind: String?, _ scopeID: String?, _ enabled: Bool
        ) async throws -> AutoflowSetEnabledResponse,
        pendingActions: PendingActions = PendingActions()
    ) {
        self.fetchAgents = fetchAgents
        self.fetchWorkflows = fetchWorkflows
        self.fetchAutoflows = fetchAutoflows
        self.postSetAutoflowEnabled = postSetAutoflowEnabled
        self.pendingActions = pendingActions
    }

    /// Fans the three fetches out concurrently. Repeatable — safe to call
    /// again (e.g. the screen reappearing), same "reload from scratch"
    /// contract every other store's `activate()` follows.
    public func activate() async {
        async let agentsLoad: Void = loadAgents()
        async let workflowsLoad: Void = loadWorkflows()
        async let autoflowsLoad: Void = loadAutoflows()
        _ = await (agentsLoad, workflowsLoad, autoflowsLoad)
    }

    /// Public for the same reason as `loadWorkflows` — the Agents tab's
    /// failed-block Retry button reloads just this block rather than
    /// re-fanning all three through `activate()`.
    public func loadAgents() async {
        do {
            let rows = try await fetchAgents()
            agents = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            agents = .failed(String(describing: error))
        }
    }

    /// The narrower single-block reload `WorkflowDetailScreen` calls instead
    /// of a full `activate()` (that screen only ever needs `workflows`, not
    /// `agents`/`autoflows` too); also the Workflows tab's failed-block
    /// Retry target.
    public func loadWorkflows() async {
        do {
            let rows = try await fetchWorkflows()
            workflows = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            workflows = .failed(String(describing: error))
        }
    }

    /// Public for the same reason as `loadWorkflows` — the Autoflows tab's
    /// failed-block Retry button reloads just this block.
    public func loadAutoflows() async {
        do {
            let rows = try await fetchAutoflows()
            autoflows = rows.isEmpty ? .empty : .content(rows)
        } catch {
            guard !isCancellation(error) else { return }
            autoflows = .failed(String(describing: error))
        }
    }

    /// Enable/disable one autoflow definition, keyed by `ActionKey.
    /// autoflow(name:scopeKind:scopeID:verb:)` — a plain
    /// `ActionKey(name, .setEnabled)` collides across two repos defining the
    /// same autoflow name (review fix, round 1: see that helper's doc
    /// comment for the exact cross-toggle bug this closes). See
    /// `ActionVerb.setEnabled`'s doc comment for why this confirms off the
    /// response directly rather than waiting on a refetch. `scopeKind`/
    /// `scopeID` should be the exact values of the row being toggled (an
    /// `AutoflowDefinition`'s own `scopeKind`/`scopeID`, or a
    /// `WorkflowDefinition`'s) — passed straight through to both the
    /// `?scope_kind=&scope_id=` pinning pair (so the server-side mutation
    /// itself never cross-toggles) and this key (so the UI ledger tracking
    /// it doesn't either).
    ///
    /// On success, patches whichever of `autoflows`/`workflows` currently
    /// holds a row matching `name` **and** `scopeKind`/`scopeID` exactly (the
    /// same three-field match the key above uses, and the server itself pins
    /// on) — never a bare name match, which could silently update the wrong
    /// same-named row from a different repo. A row that doesn't match (list
    /// not loaded yet, or genuinely absent) is left untouched; the mutation
    /// itself still succeeded and is still reported `.confirmed`.
    public func setAutoflowEnabled(name: String, scopeKind: String?, scopeID: String?, enabled: Bool) async {
        let key = ActionKey.autoflow(name: name, scopeKind: scopeKind, scopeID: scopeID, verb: .setEnabled)
        pendingActions.begin(key)
        do {
            _ = try await postSetAutoflowEnabled(name, scopeKind, scopeID, enabled)
            pendingActions.confirm(key)
            applyAutoflowEnabled(name: name, scopeKind: scopeKind, scopeID: scopeID, enabled: enabled)
        } catch {
            pendingActions.fail(key, mutationErrorMessage(error))
        }
    }

    private func applyAutoflowEnabled(name: String, scopeKind: String?, scopeID: String?, enabled: Bool) {
        if case .content(var rows) = autoflows,
            let idx = rows.firstIndex(where: { $0.name == name && $0.scopeKind == scopeKind && $0.scopeID == scopeID })
        {
            let row = rows[idx]
            rows[idx] = AutoflowDefinition(
                name: row.name, slug: row.slug, trigger: row.trigger, scope: row.scope,
                scopeKind: row.scopeKind, scopeID: row.scopeID, enabled: enabled
            )
            autoflows = .content(rows)
        }
        if case .content(var rows) = workflows,
            let idx = rows.firstIndex(where: { $0.name == name && $0.scopeKind == scopeKind && $0.scopeID == scopeID })
        {
            let row = rows[idx]
            rows[idx] = WorkflowDefinition(
                name: row.name, scope: row.scope, scopeKind: row.scopeKind, scopeID: row.scopeID,
                runCount: row.runCount, lastRun: row.lastRun, autoflowEnabled: enabled
            )
            workflows = .content(rows)
        }
    }
}
