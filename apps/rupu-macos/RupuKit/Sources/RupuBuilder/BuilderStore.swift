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
/// `moveNode`/`deleteSelection`/`rename`/`updateStep`/`updateName`/
/// `updateDescription`) builds a candidate `WorkflowGraph` via one of
/// `RupuFlowKit`'s pure `applyAdd`/`applyConnect`/`applyDelete`/
/// `applyRename`/`applyUpdate` helpers and hands
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

    /// `Hashable` (not just `Equatable`) so `WorkflowBuilderScreen`'s header
    /// can bind it straight to a SwiftUI `Picker`'s `selection:`/`.tag(_:)`
    /// (Task 10) — a plain two-case enum, synthesis is trivial and adds no
    /// behavior beyond what `Equatable` already gave every existing call site.
    public enum Mode: Hashable {
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
    /// last commit). Set by `revalidate()`. **Stale-while-revalidating**:
    /// after a commit this keeps showing the PREVIOUS commit's answer for
    /// the length of the debounce window (see `kickDebouncedRevalidate`) —
    /// it does not reset to `nil` on every keystroke-level edit, so a caller
    /// rendering this must treat it as "as of the last check", not "as of
    /// right now".
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

    // MARK: - Run mode (Task 14)

    /// The run `enterRunMode(backend:)` resolved and is currently following
    /// — `nil` while Run mode has nothing to show (`WorkflowBuilderScreen`
    /// renders the "No runs yet" empty state whenever `mode == .run` and
    /// this is `nil`). Owns a `RunDetailStore`, mirroring
    /// `RunDetailScreen`'s own activate/deactivate lifecycle: `enterRunMode`
    /// activates a fresh one, `exitRunMode` deactivates and releases it.
    private var runFollowStore: RunDetailStore?
    public private(set) var followedRunID: String?

    /// Reentrancy guard (review fix, Finding 2): bumped at the START of
    /// every `enterRunMode(backend:)` call AND by `exitRunMode()` — see
    /// both methods' doc comments for how this closes the "two overlapping
    /// `enterRunMode` calls orphan a `RunDetailStore`" gap a rapid
    /// Design<->Run toggle could otherwise hit (this repo has SSE-leak
    /// history; a `RunDetailStore` left activated with nothing referencing
    /// it anymore is exactly that class of bug). `internal private(set)`
    /// (not `private`), same "test-only visibility via `@testable import`"
    /// carve-out `RunDetailStore.graphRecomputeCount` already establishes:
    /// `BuilderStoreTests`' overlapping-calls test polls this directly to
    /// deterministically know the second call has claimed the current
    /// generation, rather than guessing with a fixed delay.
    internal private(set) var runFollowGeneration = 0

    /// The overlay `NodeView`/`EdgeLayer` render against in Run mode — a
    /// COMPUTED property, not cached state: reads `runFollowStore.graph`/
    /// `liveStates` fresh on every access, so SwiftUI's Observation
    /// tracking (a view's body touching `store.runOverlay` walks straight
    /// through into the nested `@Observable` `RunDetailStore`'s own tracked
    /// properties) picks up every live update the same way it would if the
    /// view read `runFollowStore` directly. `nil` whenever there's no run
    /// being followed, or its `graph` block hasn't loaded (yet, or failed)
    /// — `NodeView`/`EdgeLayer` render their plain design-mode look in
    /// either case, same as "nothing to show" everywhere else in this app.
    public var runOverlay: RunOverlay? {
        guard let runFollowStore, case .content(let g) = runFollowStore.graph else { return nil }
        return RupuBuilder.runOverlay(results: g.stepResults, units: g.units, liveStates: runFollowStore.liveStates)
    }

    /// The followed run's current status string (`detail.run.status`,
    /// server-raw — `BuilderHeader` normalizes it via `ActivityStatus`), for
    /// the header's "running" `StatusPill`. Same computed-property /
    /// observation-passthrough rationale as `runOverlay` above.
    public var followedRunStatus: String? {
        guard let runFollowStore, case .content(let d) = runFollowStore.detail else { return nil }
        return d.run.status
    }

    /// The workflow name `activate()` fetches — the identity this store was
    /// constructed for AND the route identity `save()` always PUTs to (final
    /// review fix, Important 2 — see `save()`'s doc comment for why this
    /// stopped being `graph.meta.name`: the server enforces the parsed
    /// document's own `name:` key against the `:name` route segment, so a
    /// PUT built off a since-renamed `graph.meta.name` either 404s against
    /// the OLD route (with an explicit scope) or silently shadow-writes a
    /// second file (without one) — neither ever actually renames anything).
    /// `internal`, not `private` (final review fix, Important 4): read by
    /// `WorkflowBuilderScreen.activateStore()`'s reuse guard, so a client
    /// swap that also changes which workflow this screen names never
    /// silently keeps serving the OLD store.
    let name: String
    private let scopeKind: String?
    private let scopeID: String?
    private let client: CPClient
    private let pendingActions: PendingActions

    /// The in-flight debounce timer kicked by a successful `commit(_:)` —
    /// cancelled and replaced on every subsequent commit, per the Task 9
    /// brief's "cancel the prior" instruction.
    private var revalidateTask: Task<Void, Never>?
    /// Length of `kickDebouncedRevalidate`'s debounce window — 400ms in
    /// production (see the public `init` below). `internal`, not `public`:
    /// this is a test seam only. `BuilderStoreTests` shrinks it via the
    /// designated `init(name:scopeKind:scopeID:client:pendingActions:
    /// debounceInterval:)` below to keep its debounce coverage (Task 9
    /// review, Finding 2) fast and non-flaky — real call sites always go
    /// through the public convenience `init`, which hard-codes 400ms.
    private let debounceInterval: Duration

    /// The production entry point — always a 400ms debounce. See the
    /// designated `init(name:scopeKind:scopeID:client:pendingActions:
    /// debounceInterval:)` below for the test seam.
    public convenience init(name: String, scopeKind: String?, scopeID: String?, client: CPClient, pendingActions: PendingActions) {
        self.init(
            name: name, scopeKind: scopeKind, scopeID: scopeID, client: client, pendingActions: pendingActions,
            debounceInterval: .milliseconds(400)
        )
    }

    init(
        name: String, scopeKind: String?, scopeID: String?, client: CPClient, pendingActions: PendingActions,
        debounceInterval: Duration
    ) {
        self.name = name
        self.scopeKind = scopeKind
        self.scopeID = scopeID
        self.client = client
        self.pendingActions = pendingActions
        self.debounceInterval = debounceInterval
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

    /// Clears a rejected edit's reason once the user has read it
    /// (`WorkflowBuilderScreen`'s commit-error banner's dismiss control) —
    /// see `commitError`'s own doc comment for why this is a caller-driven
    /// clear rather than something a timer or the next render does on its
    /// own.
    public func dismissCommitError() {
        commitError = nil
    }

    // MARK: - Commit (WorkflowEditor.tsx `commit` port)

    /// Serialize-first, all-or-nothing. `graphToWorkflowObject(next)` runs
    /// BEFORE anything on this store changes: a `.failure` (the edit closed
    /// a cycle) sets `commitError` and returns with `graph`/`canonicalYAML`/
    /// `problems`/`dirty` completely untouched — the canvas never shows a
    /// graph state the underlying YAML disagrees with. A `.object` success
    /// applies the new graph, recomputes `canonicalYAML`/`problems`, marks
    /// `dirty`, clears any previous `commitError`, and kicks a debounced
    /// `revalidate()` (`debounceInterval`, 400ms in production — cancelling
    /// whatever debounce was already in flight).
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
        let interval = debounceInterval
        revalidateTask = Task { [weak self] in
            try? await Task.sleep(for: interval)
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

    /// A single-field-safe alternative to `updateStep(_:)` for the Step
    /// form's per-field debounced commits (macOS Workflow Builder, Task 13
    /// review Finding 1). `updateStep(_:)` takes a WHOLE `StepNodeData` the
    /// caller must have built ahead of time — fine for a synchronous edit,
    /// but wrong for a debounce/blur-timer closure: a `Task` scheduled 300ms
    /// in the future (or a SwiftUI view's `onChange`/`onSubmit` closure)
    /// necessarily captured its `StepNodeData` snapshot back when the
    /// keystroke fired, and by the time it actually runs, ANOTHER field's
    /// debounce may already have committed — applying the stale snapshot
    /// wholesale would silently revert that other field's edit. Worse, a
    /// rename firing mid-debounce changes the node's id out from under a
    /// frozen snapshot, so `updateStep`'s `applyUpdate(graph, id: data.id,
    /// ...)` would look for an id that no longer exists and silently no-op.
    ///
    /// This method closes both gaps by resolving `data` fresh at CALL time —
    /// straight off `graph.nodes`, never off any value the caller captured
    /// earlier — and by requiring the caller to name which node it OWNS.
    ///
    /// **`ownerID` (final review fix, Critical 1)**: `StepFormTab.swift`
    /// keys `StepFormBody` on `.id(node.id)`, so selecting a DIFFERENT node
    /// tears the old form down and mounts a fresh one — but a debounce
    /// `Task` already in flight on the torn-down form is a plain `Task { }`,
    /// not something SwiftUI cancels on teardown, so it still fires 300ms
    /// later and still calls this method. The OLD signature resolved the
    /// target purely from `selectedID` at that point, which by then names
    /// the NEWLY selected node — so a stale debounce from node A's PROMPT
    /// field would silently write node A's leftover text into node B, and
    /// that write would then get saved. `ownerID` is the node id the FORM
    /// was built for (captured once, at that form's `init`, via `node.id` —
    /// see `StepFormTab.commit(_:)`), and the guard below requires it to
    /// still match the CURRENT `selectedID` before anything is looked up or
    /// applied — a stale debounce whose owner is no longer selected is a
    /// silent no-op, same contract `deleteSelection()` already gives a
    /// `nil`-selection or since-deleted-node call. `mutate` should
    /// therefore touch ONLY the one field it owns.
    public func updateSelectedStep(ownerID: String, _ mutate: (inout StepNodeData) -> Void) {
        guard selectedID == ownerID, let node = graph.nodes.first(where: { $0.id == ownerID }) else { return }
        var data = node.data
        mutate(&data)
        updateStep(data)
    }

    /// Sets the workflow's top-level `name`. Split out from the description
    /// setter (Task 9 review, Finding 1) because a single combined
    /// `updateMeta(name:description:)` had asymmetric nil semantics — `name
    /// == nil` meant "leave unchanged" but `description == nil` meant
    /// "clear it" — so a caller renaming via `updateMeta(name: x,
    /// description: nil)` intending only a rename silently wiped the
    /// description. Two single-purpose methods have no such ambiguity: this
    /// one always sets `name` (required — there is no "clear the name"
    /// gesture, hence non-optional here), `updateDescription(_:)` below
    /// always sets `description` (`nil` clearing it).
    public func updateName(_ name: String) {
        var meta = graph.meta
        meta.name = name
        commit(WorkflowGraph(nodes: graph.nodes, edges: graph.edges, meta: meta, loops: graph.loops))
    }

    /// Sets the workflow's top-level `description`; `nil` clears it. See
    /// `updateName(_:)`'s doc comment for why this is a separate method
    /// rather than a combined `updateMeta`.
    public func updateDescription(_ description: String?) {
        var meta = graph.meta
        meta.description = description
        commit(WorkflowGraph(nodes: graph.nodes, edges: graph.edges, meta: meta, loops: graph.loops))
    }

    // MARK: - Save

    /// `PUT /api/workflows/:name` where `:name` is ALWAYS this store's own
    /// `name` — the route identity it was constructed for — never
    /// `graph.meta.name` (final review fix, Important 2; the prior
    /// implementation PUT to `graph.meta.name`, which sounds right for "save
    /// under the renamed name" but isn't: the server parses the written YAML
    /// and enforces its `name:` key equals the `:name` route segment, so a
    /// PUT to the OLD route carrying NEW `meta.name` content either 404s
    /// (with an explicit scope, since that route requires an exact
    /// existing-file match) or, worse, silently creates a SECOND file at
    /// the new name while leaving the original untouched (without a scope).
    /// Neither ever actually renames the workflow — the write either fails
    /// outright or succeeds at writing the wrong thing).
    ///
    /// Since this store has no real rename route to fall back on either, a
    /// `graph.meta.name` that has drifted from `name` (via `rename`/
    /// `updateName`, which only ever touch the in-memory document) BLOCKS
    /// the save entirely with an explanatory `saveError` instead of
    /// attempting a PUT that would misbehave one of the two ways above —
    /// `SettingsTab`'s NAME field stays editable but shows an inline warning
    /// once it disagrees with the route name, so the user has to revert it
    /// (or a future task adds real rename support) before Save works again.
    ///
    /// Synchronous server-side (200 = written to disk — see
    /// `CPClient.writeWorkflow`'s doc comment), so confirming directly off
    /// the response is honest here, same as `ConfigStore`'s raw-TOML saves.
    /// On success clears `dirty`; on failure (including the name-mismatch
    /// guard above) `dirty` stays `true` and `saveError` carries the
    /// unwrapped server message (or the transport error's description, or
    /// the guard's own message).
    public func save() async -> Bool {
        guard graph.meta.name == name else {
            saveError = "workflow name differs from the file — rename isn't supported from the builder yet; revert the NAME field to save"
            return false
        }

        let key = ActionKey(name, .save)
        pendingActions.begin(key)
        saveError = nil
        do {
            try await client.writeWorkflow(
                name: name,
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

    /// Clears a rejected save's reason once the user has read it
    /// (`WorkflowBuilderScreen`'s save-error banner's dismiss control, final
    /// review fix Important 1) — mirrors `dismissCommitError()`'s own
    /// caller-driven clear contract. Also cleared implicitly at the START of
    /// every `save()` attempt (see above), so a retry that succeeds never
    /// leaves a stale error banner up either.
    public func dismissSaveError() {
        saveError = nil
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

    // MARK: - Run mode (Task 14)

    /// Resolves which run to follow and activates a `RunDetailStore` for it
    /// — called whenever `mode` becomes `.run` (`WorkflowBuilderScreen`'s
    /// `onChange(of: store.mode)`, which covers both the segmented control
    /// and the Launch button's own `mode = .run` flip). Resolution: the
    /// newest run for `graph.meta.name` from one page of
    /// `client.workflowRuns(offset: 0, limit: 50)` (`latestRunID(rows:
    /// workflowName:)`), or `nil` — Run mode renders the "No runs yet" empty
    /// state.
    ///
    /// **No launched-run short-circuit today** (final review fix, Minor d —
    /// a `launchedRunID` property tracking "the run I just launched from
    /// this screen" existed here but was dead: nothing ever assigned it,
    /// since `WorkflowBuilderScreen`'s Launch button opens the Launcher
    /// sheet via `AppModel.presentLauncher(...)`, a fire-and-forget
    /// navigation call with no completion callback threaded back to this
    /// store — see `WorkflowBuilderScreen.handleLaunch`'s own doc comment.
    /// Most of that gap is already closed cheaply anyway, NOT through a
    /// property here: `WorkflowBuilderScreen`'s `onChange(of: model.
    /// showLauncher)` re-runs `enterRunMode(backend:)` when the Launcher
    /// sheet closes while still in Run mode, and by then the just-launched
    /// run (if any) IS the newest run for this workflow, so the fallback
    /// below picks it up without any extra bookkeeping. The REMAINING gap —
    /// resolving the followed run instantly, before the sheet closes,
    /// rather than only once it does — is the seam a future task closes by
    /// threading an actual completion callback through `presentLauncher`
    /// and resolving directly to that run id here, ahead of the fallback.)
    ///
    /// Idempotent for the same already-followed run (a redundant
    /// `enterRunMode` call — e.g. the segmented control round-tripping
    /// Design -> Run without the followed run changing — leaves the
    /// existing `RunDetailStore` running rather than tearing it down and
    /// losing its live stream mid-flight); otherwise tears down whatever was
    /// being followed before starting the new one, same "rebuild, don't
    /// reuse" contract `RunDetailScreen.activate()` follows for a `runID`
    /// change.
    ///
    /// **Reentrancy (review fix, Finding 2)**: this function has TWO
    /// suspension points (the `workflowRuns` fetch, and `newStore.
    /// activate()`), and a rapid Design<->Run toggle can start a second
    /// `enterRunMode` — or call `exitRunMode()` directly — while an earlier
    /// call is still suspended at either one. `runFollowGeneration` (bumped
    /// here at entry, and by `exitRunMode()`) is what makes "only the most
    /// recent call ever mutates `runFollowStore`/`followedRunID`" true by
    /// construction rather than a timing accident: a call whose captured
    /// generation no longer matches `runFollowGeneration` by the time either
    /// `await` resumes discards its own results — the FIRST checkpoint bails
    /// out before touching any shared state at all; the SECOND deactivates
    /// the `RunDetailStore` this call itself just built and assigned, since
    /// a newer call already overwrote `runFollowStore` out from under it
    /// while `activate()` was in flight. Without this, a stale call
    /// resuming last would silently leave an activated `RunDetailStore` (SSE
    /// stream and all) running with nothing referencing it anymore — this
    /// repo has SSE-leak history (see `project_sse_starvation_arc`).
    public func enterRunMode(backend: BackendController) async {
        runFollowGeneration += 1
        let generation = runFollowGeneration

        let workflowName = graph.meta.name
        // Review fix, Finding 3: `host: "local"` is required, not
        // optional — `CPClient.workflowRuns(offset:limit:host:since:
        // until:)`'s own doc comment warns an OMITTED `host` fans the
        // call out to every registered host sequentially server-side,
        // turning a ~60ms call into several seconds against a fleet with
        // one offline node. The Builder only ever edits LOCAL workflow
        // definitions, so "local" is always the right scope for this
        // lookup — the followed RUN's own host still comes from the
        // resolved row (`target.host` below) and threads into
        // `RunDetailStore`'s init unchanged, so a workflow that was last
        // run on a remote Fleet host is still followed correctly.
        let rows = (try? await client.workflowRuns(offset: 0, limit: 50, host: "local")) ?? []
        let target = RupuBuilder.latestRunID(rows: rows, workflowName: workflowName)

        // Checkpoint 1 — see the doc comment above.
        guard generation == runFollowGeneration else { return }

        guard let target else {
            exitRunMode()
            return
        }
        if followedRunID == target.id, runFollowStore != nil {
            return
        }

        runFollowStore?.deactivate()
        let newStore = RunDetailStore(runID: target.id, host: target.host, client: client, backend: backend)
        runFollowStore = newStore
        followedRunID = target.id
        await newStore.activate()

        // Checkpoint 2 — see the doc comment above. `newStore` (this call's
        // OWN local reference), not `runFollowStore` (which a newer call may
        // have already reassigned) is what gets torn down here.
        guard generation == runFollowGeneration else {
            newStore.deactivate()
            return
        }
    }

    /// Deactivates and releases the followed run's store — called whenever
    /// `mode` leaves `.run` (back to `.design`) and whenever this screen
    /// goes away entirely, mirroring `RunDetailScreen`'s own
    /// `onDisappear { store?.deactivate() }`. Idempotent (a `nil`
    /// `runFollowStore` is a harmless no-op), matching every other
    /// activate/deactivate pair in this codebase. Also bumps
    /// `runFollowGeneration` (review fix, Finding 2) — an `enterRunMode`
    /// call already in flight when this runs must never resurrect a store
    /// after the user explicitly asked to leave Run mode; see that
    /// function's doc comment.
    public func exitRunMode() {
        runFollowGeneration += 1
        runFollowStore?.deactivate()
        runFollowStore = nil
        followedRunID = nil
    }
}
