import CoreGraphics
import Foundation
import Observation
import RupuAPI
import RupuFlowKit
import RupuStore

/// The Workflow Builder's round-trip core (macOS design plan, Task 9):
/// fetch → parse → graph, every canvas edit re-serialized through
/// `RupuFlowKit`'s pure `WorkflowGraph -> YAMLValue` pipeline, and a save
/// path back to `PUT /api/workflows/:name`. This is the macOS port of the
/// web editor's `WorkflowEditor.tsx` `commit` callback — "serialize FIRST,
/// all-or-nothing": every mutating verb below (`addNode`/`connect`/
/// `deleteSelection`/`rename`/`updateStep`/`updateMeta`/`moveNode`) builds a
/// candidate `WorkflowGraph` via one of `RupuFlowKit`'s pure `applyAdd`/
/// `applyConnect`/`applyDelete`/`applyRename`/`applyUpdate` helpers and hands
/// it to `commit(_:)`, which is the ONLY place `graph`/`canonicalYAML`/
/// `problems`/`dirty` actually change. A commit whose `graphToWorkflowObject`
/// closes a cycle is REJECTED wholesale — `commitError` is set, and neither
/// `graph` nor `canonicalYAML` nor `dirty` moves at all, so a bad edit can
/// never leave the canvas showing a graph the YAML underneath disagrees
/// with.
///
/// **`phase` is the honest-degrade contract** (matt's no-mock-features
/// rule): a `YAMLError` from `YAMLParser.parse` on `activate()` — the
/// workflow's on-disk YAML uses a feature outside this repo's supported
/// subset (anchors, aliases, tags, …) — surfaces as `.unsupported(message)`,
/// a real error state the canvas must show, never a silent empty-graph
/// degrade.
@MainActor
@Observable
public final class BuilderStore {
    public enum Phase: Equatable {
        case loading
        case failed(String)
        case unsupported(String)
        case ready
    }

    public enum Mode: Equatable {
        case design
        case run
    }

    public private(set) var phase: Phase = .loading
    public private(set) var graph: WorkflowGraph = WorkflowGraph(nodes: [], edges: [], meta: WorkflowMeta(name: ""))
    public private(set) var canonicalYAML: String = ""
    public private(set) var problems: [String: [String]] = [:]
    public private(set) var selectedID: String?
    public private(set) var dirty: Bool = false
    /// `nil` = not yet checked (no `revalidate()` has completed since the
    /// last commit). Set by `revalidate()`.
    public private(set) var serverValid: Bool?
    /// The last rejected edit's reason (a cycle `commit(_:)` refused, or a
    /// rejection from `applyConnect`/`applyRename`) — transient, meant for a
    /// one-shot toast/banner; callers clear it however they see fit (e.g.
    /// overwriting it on the next attempt, which every mutating verb below
    /// already does on success).
    public private(set) var commitError: String?
    public private(set) var saveError: String?
    public var mode: Mode = .design
    /// Agent picker catalog (`client.agentDefinitions()`) — fetched
    /// best-effort in `activate()`; a failure leaves this empty rather than
    /// failing the whole activation (the canvas is still usable without the
    /// picker catalog, unlike a YAML parse failure).
    public private(set) var agents: [AgentDefinition] = []
    /// Action `with:` editor's tool catalog (`client.tools()`) — same
    /// best-effort fetch contract as `agents`.
    public private(set) var tools: [ToolSpec] = []

    /// The workflow name `activate()` fetches — the identity this store was
    /// constructed for, distinct from `graph.meta.name` (which `rename`/
    /// `updateMeta` can change mid-session; `save()` PUTs to
    /// `graph.meta.name`, not this field, so a renamed workflow writes to
    /// its NEW name — see `save()`'s doc comment).
    private let name: String
    private let scopeKind: String?
    private let scopeID: String?
    private let client: CPClient
    private let pendingActions: PendingActions

    /// The in-flight 400ms debounce timer kicked by a successful `commit(_:)`
    /// — cancelled and replaced on every subsequent commit, per the Task 9
    /// brief's "cancel the prior" instruction.
    private var revalidateTask: Task<Void, Never>?

    public init(name: String, scopeKind: String?, scopeID: String?, client: CPClient, pendingActions: PendingActions) {
        self.name = name
        self.scopeKind = scopeKind
        self.scopeID = scopeID
        self.client = client
        self.pendingActions = pendingActions
    }

    // MARK: - Activate

    /// `GET /api/workflows/:name` → `YAMLParser.parse` → `yamlToGraph` →
    /// `autoLayout` → a serialize-only pass to compute `canonicalYAML`
    /// (never marks `dirty` — reformatting an untouched load isn't a user
    /// edit). Agents/tools are fetched afterward, best-effort: neither
    /// failure blocks `phase` from reaching `.ready`.
    public func activate() async {
        phase = .loading

        let detail: WorkflowDetail
        do {
            detail = try await client.workflowDetail(name: name)
        } catch {
            phase = .failed(String(describing: error))
            return
        }

        let parsed: YAMLValue
        do {
            parsed = try YAMLParser.parse(detail.yaml)
        } catch let yamlError as YAMLError {
            phase = .unsupported(Self.message(for: yamlError))
            return
        } catch {
            phase = .failed(String(describing: error))
            return
        }

        var built = yamlToGraph(parsed)
        built.nodes = autoLayout(nodes: built.nodes, edges: built.edges)

        switch graphToWorkflowObject(built) {
        case .object(let value):
            graph = built
            canonicalYAML = YAMLEmitter.dump(value)
            problems = validateGraph(built)
            dirty = false
            serverValid = nil
            commitError = nil
            phase = .ready
        case .failure(let message):
            phase = .failed(message)
            return
        }

        agents = (try? await client.agentDefinitions()) ?? []
        tools = (try? await client.tools()) ?? []
    }

    private static func message(for error: YAMLError) -> String {
        switch error {
        case .unsupported(let message, let line):
            return "\(message) (line \(line))"
        case .malformed(let message, let line):
            return "\(message) (line \(line))"
        }
    }

    // MARK: - Selection

    public func select(_ id: String?) {
        selectedID = id
    }

    // MARK: - Commit (WorkflowEditor.tsx `commit` port)

    /// Serialize-first, all-or-nothing. `graphToWorkflowObject(next)` runs
    /// BEFORE anything on this store changes: a `.failure` (the edit closed
    /// a cycle) sets `commitError` and returns with `graph`/`canonicalYAML`/
    /// `problems`/`dirty` completely untouched — the canvas never shows a
    /// graph state the underlying YAML disagrees with. A `.object` success
    /// applies the new graph, recomputes `canonicalYAML`/`problems`, marks
    /// `dirty`, clears any previous `commitError`, and kicks a 400ms
    /// debounced `revalidate()` (cancelling whatever debounce was already
    /// in flight).
    public func commit(_ next: WorkflowGraph) {
        switch graphToWorkflowObject(next) {
        case .failure(let message):
            commitError = message
        case .object(let value):
            graph = next
            canonicalYAML = YAMLEmitter.dump(value)
            problems = validateGraph(next)
            dirty = true
            commitError = nil
            kickDebouncedRevalidate()
        }
    }

    private func kickDebouncedRevalidate() {
        revalidateTask?.cancel()
        revalidateTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(400))
            guard !Task.isCancelled else { return }
            await self?.revalidate()
        }
    }

    // MARK: - Mutating verbs (every one routes through `commit(_:)`)

    /// Appends a new node of `kind` at `at`, then selects it — mirrors the
    /// web editor's own post-add `onSelect(id)` (`WorkflowEditorGraph.tsx`'s
    /// `applyAddNode`/`applyAddNodeAt` call sites).
    /// `RupuFlowKit.StepKind`, spelled out — `RupuStore` also exports a
    /// (presentation-only, dashboard-glyph) type of the same bare name, and
    /// this store imports both, so the unqualified name is ambiguous.
    public func addNode(kind: RupuFlowKit.StepKind, at: CGPoint) {
        let (next, newID) = applyAdd(graph, kind: kind, at: at)
        commit(next)
        select(newID)
    }

    public func connect(source: String, target: String, arm: String?) {
        switch applyConnect(graph, source: source, target: target, arm: arm) {
        case .connected(let next):
            commit(next)
        case .rejected(let reason):
            commitError = reason
        }
    }

    /// Position-only change — but positions feed `topoSort`'s tiebreak (see
    /// `GraphSerialize.swift`'s `topoSort` doc comment), so a move CAN
    /// reorder the emitted step sequence. This DOES re-emit `canonicalYAML`
    /// and mark `dirty`, same as any other mutating verb — an honest
    /// round-trip, not a silent no-op edit that leaves the YAML stale.
    public func moveNode(id: String, to: CGPoint) {
        let nodes = graph.nodes.map { n -> GraphNode in
            guard n.id == id else { return n }
            var copy = n
            copy.position = to
            return copy
        }
        commit(withDerivedEdges(meta: graph.meta, nodes: nodes, loops: graph.loops))
    }

    /// Clears the selection unconditionally (mirrors the web editor's own
    /// `if (selectedId !== null && removed.includes(selectedId)) onSelect(null)`
    /// — since this only ever deletes the CURRENTLY selected node, the
    /// removed set always contains it).
    public func deleteSelection() {
        guard let id = selectedID else { return }
        let next = applyDelete(graph, id: id)
        selectedID = nil
        commit(next)
    }

    /// Renames step `id` to `to` (slugified by `applyRename`). Returns
    /// `false` on rejection (unknown id, empty-after-slug, or a collision
    /// with another step's id) — `graph` stays untouched and `commitError`
    /// carries the reason. On success the selection follows the renamed id
    /// if `id` was selected.
    @discardableResult
    public func rename(id: String, to: String) -> Bool {
        switch applyRename(graph, from: id, to: to) {
        case .renamed(let next, let newID):
            let wasSelected = selectedID == id
            commit(next)
            if wasSelected { selectedID = newID }
            return true
        case .rejected(let reason):
            commitError = reason
            return false
        }
    }

    /// Replaces node `data.id`'s data wholesale (form edits). Never renames
    /// — `data.id` must already match an existing node's id; use `rename`
    /// for an id change (see `applyUpdate`'s own doc comment).
    public func updateStep(_ data: StepNodeData) {
        commit(applyUpdate(graph, id: data.id, data: data))
    }

    /// Patches the workflow's top-level `name`/`description`. `name == nil`
    /// leaves the current name unchanged (the field is required — there is
    /// no "clear the name" gesture); `description` is a full replace,
    /// `nil` clearing it outright.
    public func updateMeta(name: String?, description: String?) {
        var meta = graph.meta
        if let name { meta.name = name }
        meta.description = description
        commit(WorkflowGraph(nodes: graph.nodes, edges: graph.edges, meta: meta, loops: graph.loops))
    }

    // MARK: - Save

    /// `PUT /api/workflows/:name` where `:name` is `graph.meta.name` — NOT
    /// the `name` this store was constructed with. A workflow renamed via
    /// `rename`/`updateMeta` mid-session therefore writes to its NEW name on
    /// save (the server has no separate "rename" route; a write to a
    /// different name simply creates/overwrites that file). Synchronous
    /// server-side (200 = written to disk — see `CPClient.writeWorkflow`'s
    /// doc comment), so confirming directly off the response is honest here,
    /// same as `ConfigStore`'s raw-TOML saves. On success clears `dirty`; on
    /// failure `dirty` stays `true` and `saveError` carries the unwrapped
    /// server message (or the transport error's description).
    public func save() async -> Bool {
        let key = ActionKey(graph.meta.name, .save)
        pendingActions.begin(key)
        saveError = nil
        do {
            try await client.writeWorkflow(
                name: graph.meta.name,
                body: WorkflowWriteBody(raw: canonicalYAML),
                scopeKind: scopeKind,
                scopeID: scopeID
            )
            dirty = false
            pendingActions.confirm(key)
            return true
        } catch {
            let message = Self.displayMessage(forError: error)
            saveError = message
            pendingActions.fail(key, message)
            return false
        }
    }

    /// Unwraps a `CPError.http` body's `{"error": "..."}` envelope, same
    /// convention as `ConfigStore.displayMessage(forBody:)` — falls back to
    /// the body verbatim when it isn't that shape, and to
    /// `String(describing:)` for any non-HTTP error (transport/decoding/
    /// unauthorized).
    private static func displayMessage(forError error: Error) -> String {
        guard case CPError.http(_, let body) = error else {
            return String(describing: error)
        }
        guard
            let data = body.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let message = object["error"] as? String
        else {
            return body
        }
        return message
    }

    // MARK: - Server-side validate

    /// `POST /api/workflows/validate {raw: canonicalYAML}` → `serverValid`.
    /// Called directly by callers that want an immediate check, and
    /// indirectly by `commit(_:)`'s 400ms debounce. A transport failure
    /// (including cancellation, when a newer commit's debounce superseded
    /// this call) leaves `serverValid` at its previous value rather than
    /// flapping it to `false` on a network hiccup.
    public func revalidate() async {
        do {
            serverValid = try await client.validateWorkflow(body: ValidateBody(raw: canonicalYAML))
        } catch {
            // Leave `serverValid` untouched — see doc comment above.
        }
    }
}
