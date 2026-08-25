import Foundation
import Observation
import RupuAPI

/// Which write route the Launcher sheet fires — a standalone agent run
/// (`POST /api/agents/:name/run`), an interactive agent session
/// (`POST /api/agents/:name/session`), or a workflow run
/// (`POST /api/workflows/:name/run`). Drives both `selectedDefinition`'s
/// source list (`agents` vs `workflows`) and `canLaunch`'s validation shape
/// (declared workflow inputs vs a free-text prompt).
public enum LaunchKind: String, CaseIterable, Sendable {
    case agentRun, session, workflow
}

/// `LaunchOutcome.result`'s failure payload — a bare message, not a typed
/// taxonomy of failure modes (every mutation surface in this module already
/// collapses failures down to one string via `mutationErrorMessage(_:)`).
/// Review fix (fold 1): originally a plain `String` via a module-wide
/// `extension String: Error`, which silently made *every* string in
/// `RupuStore` throwable (`throw "typo"` would have type-checked anywhere in
/// the module) — a foot-gun with no offsetting benefit. This one-case enum
/// gets the same ergonomics (`Result<Route, LaunchError>`) without that
/// blast radius.
public enum LaunchError: Error, Equatable, Sendable {
    case message(String)

    public var text: String {
        switch self {
        case .message(let value): return value
        }
    }
}

/// One target host's launch attempt, recorded after `launch()`'s POST for
/// it resolves — `host` is the target's id (`"local"` for the embedded
/// backend, a Fleet node id otherwise), never the request's `nil`-for-local
/// body encoding. `result` carries either the destination `Route` a
/// successful launch reached, or a `LaunchError` wrapping the same
/// `mutationErrorMessage(_:)`-mapped string every other mutation in this
/// module surfaces on failure (a 501 reads as "server lacks launch runtime —
/// start with `rupu cp serve`").
public struct LaunchOutcome: Equatable, Sendable {
    public let host: String
    public let result: Result<Route, LaunchError>

    public init(host: String, result: Result<Route, LaunchError>) {
        self.host = host
        self.result = result
    }

    /// `Result` has no stdlib `Equatable` conformance even when both its
    /// payload types are `Equatable` — this is a hand-written substitute,
    /// not a derived one.
    public static func == (lhs: LaunchOutcome, rhs: LaunchOutcome) -> Bool {
        guard lhs.host == rhs.host else { return false }
        switch (lhs.result, rhs.result) {
        case (.success(let l), .success(let r)):
            return l == r
        case (.failure(let l), .failure(let r)):
            return l == r
        default:
            return false
        }
    }
}

/// Owns the Launcher sheet's form state end to end: which definitions exist
/// (`agents`/`workflows`, loaded in parallel), which one is selected, a
/// workflow's declared inputs and the values typed for them, the free-text
/// `prompt` an agent run/session takes instead, which host(s) to launch
/// against, and the client-side fan-out that fires one POST per target and
/// collects a `LaunchOutcome` for each.
///
/// **Progressive hosts** (Phase 2 lesson, carried over from
/// `ActivityStore.loadRemoteHosts`): `activate()` awaits only the
/// `agents`/`workflows` loads — the sheet can render and a definition can be
/// picked the moment those two land — and fires `GET /api/hosts` in the
/// background, filling in `hosts` whenever it answers. A slow or offline
/// fleet must never delay the sheet opening.
///
/// **No validate-on-launch**: unlike the workflow editor (not part of this
/// phase), `launch()` never calls `POST /api/workflows/validate` against the
/// selected definition's yaml — there is no local editing this phase for
/// that to be validating in the first place. The only client-side gate is
/// the declared-required-input check already folded into `canLaunch`;
/// `validationError` exists purely to surface *that* gap as a message when
/// `launch()` is called anyway (e.g. a race with a disabled-but-not-yet-
/// re-rendered button), not as a general validation surface.
@MainActor
@Observable
public final class LauncherStore {
    public var kind: LaunchKind = .agentRun

    public private(set) var agents: BlockState<[AgentDefinition]> = .loading
    public private(set) var workflows: BlockState<[WorkflowDefinition]> = .loading

    public var selectedDefinition: String?

    /// The picked row's scope, threaded through from `DefinitionPicker`'s
    /// `selectDefinition(_:scopeKind:scopeID:)` call (Phase 5A Task 4) —
    /// captured from the tapped row itself, never re-derived by a later
    /// name lookup against `agents`/`workflows` (those lists can carry the
    /// same definition name in two scopes at once, per `DefinitionPicker`'s
    /// doc comment; re-deriving by name would be ambiguous exactly where
    /// scope-pinning matters most). `private(set)` — direct assignment to
    /// `selectedDefinition` (as several existing tests and any future
    /// programmatic selection do) intentionally leaves these `nil`, the
    /// same "no scope pin" behavior launches had before this phase.
    ///
    /// `performLaunch` is the only reader: see its doc comment for the
    /// LOCAL-ONLY threading rule (never sent to a non-local target).
    public private(set) var selectedScopeKind: String?
    public private(set) var selectedScopeID: String?

    public private(set) var workflowInputs: [String: WorkflowInputDef] = [:]
    public var inputValues: [String: String] = [:]

    /// Review fix (fold 2): the previously-silent "fail open" on a
    /// `workflowDetail` fetch failure now leaves a message here too, so
    /// Task 8 can render "couldn't load declared inputs" instead of a
    /// silently-empty input list that looks identical to a workflow with
    /// genuinely no declared inputs. `selectDefinition` still fails open —
    /// `workflowInputs`/`inputValues` end up `[:]` either way, so
    /// `canLaunch`'s required-input check trivially passes with nothing to
    /// require — this is purely an added surface for the message, not a
    /// change to that behavior. Cleared on every new `selectDefinition`
    /// call and on a subsequent success.
    public private(set) var inputsLoadError: String?

    public var prompt: String = ""
    public var mode: String = "ask"

    public private(set) var hosts: [APIHostRow] = []
    public var selectedHosts: Set<String> = ["local"]
    public var fanOutAllHealthy: Bool = false

    public private(set) var validationError: String?
    public private(set) var launchResults: [LaunchOutcome] = []

    /// Sheet-local — deliberately **not** `backend.pendingActions`.
    /// `RunDetailStore`/`ActivityStore` share that instance because their
    /// mutations act on entities (runs) that outlive the screen and other
    /// screens need to observe the same pending state; a launch's pending
    /// state has no meaning once the sheet that fired it is gone, and a
    /// fresh `LauncherStore` per sheet presentation is exactly the reset
    /// every reopen should get. One key for the whole batch —
    /// `ActionKey("launcher", .launch)` — since a fan-out launch is one
    /// operator action even when it becomes several POSTs.
    public let pendingActions = PendingActions()

    private let client: CPClient
    private var hostsGeneration = 0

    /// Review fix (Important 1): guards against a stale `workflowDetail`
    /// fetch clobbering a newer `selectDefinition` call's result — see
    /// `selectDefinition`'s doc comment. Same recipe as `hostsGeneration`
    /// one field above (and `RunDetailStore.focusGeneration`): incremented
    /// synchronously on every call's entry, captured locally, and the
    /// fetched result is discarded if the captured value no longer matches
    /// by the time the `await` resolves.
    private var selectDefinitionGeneration = 0

    public init(client: CPClient) {
        self.client = client
    }

    /// Loads `agents`/`workflows` concurrently and returns once both land —
    /// the sheet has everything it needs to render its definition list at
    /// that point. `GET /api/hosts` is then fired in the background (never
    /// awaited here); see the type doc comment's "Progressive hosts"
    /// section.
    public func activate() async {
        async let agentsLoad: Void = loadAgents()
        async let workflowsLoad: Void = loadWorkflows()
        _ = await (agentsLoad, workflowsLoad)

        loadHostsInBackground()
    }

    /// Prefill seam (Phase 5A Task 7): opens the sheet with `name` already
    /// selected under `kind`, carrying the given row's scope — the smallest
    /// honest public API a caller outside this module (e.g. a Library
    /// screen's per-row "Launch" action) needs to preselect a definition
    /// before presenting the sheet, rather than reaching into
    /// `selectedDefinition`/`kind` directly and hand-rolling the same
    /// input-fetch/generation dance `selectDefinition` already owns.
    ///
    /// Just `kind = kind; await selectDefinition(...)` — for a workflow this
    /// also fetches its declared inputs exactly as a picker tap would, so
    /// the prefilled sheet's input form is populated, not empty.
    ///
    /// **Ordering with `activate()`**: call this before, after, or
    /// concurrently with `activate()` — order between the two never
    /// matters. `activate()` only ever populates `agents`/`workflows`/
    /// `hosts`; it never reads or writes `kind`/`selectedDefinition`/
    /// `selectedScopeKind`/`selectedScopeID`/`workflowInputs`/
    /// `inputValues`, so a prefilled selection survives an `activate()`
    /// call landing before, during, or after it. The only ordering that
    /// *does* matter is between overlapping `prefill`/`selectDefinition`
    /// calls themselves — `selectDefinitionGeneration` already guarantees
    /// last-*called*-wins for that (see `selectDefinition`'s doc comment);
    /// a `prefill` immediately followed by an operator tapping a different
    /// row resolves the same way a rapid double-tap already does.
    public func prefill(kind: LaunchKind, name: String, scopeKind: String? = nil, scopeID: String? = nil) async {
        self.kind = kind
        await selectDefinition(name, scopeKind: scopeKind, scopeID: scopeID)
    }

    /// Selects `name` as the definition to launch. For a workflow, also
    /// fetches its declared inputs (`GET /api/workflows/:name`) and seeds
    /// `workflowInputs` plus a defaulted `inputValues` (each declared
    /// input's own `default`, or `""` when it has none — never left
    /// unseeded, so every declared input has a row to render and edit).
    /// For an agent run/session, there are no declared inputs to fetch —
    /// `workflowInputs`/`inputValues` are cleared instead, so switching
    /// `kind` (or selecting a different definition) never leaves a stale
    /// prior workflow's input rows showing.
    ///
    /// **Overlapping calls** (review fix, Important 1): a rapid A→B
    /// selection — e.g. a fast typeahead, or a picker double-click — used to
    /// be last-*resolved*-wins, not last-*called*-wins: if A's fetch was
    /// slower than B's, A's stale result could land after B's and silently
    /// overwrite `workflowInputs`/`inputValues` out from under the operator,
    /// even though `selectedDefinition` correctly still read "B" — `launch()`
    /// would then submit A-shaped inputs under B's name. `selectDefinitionGeneration`
    /// closes that: bumped synchronously on every call's entry (before any
    /// `await`), so a call's own generation is fixed the instant it's
    /// invoked, independent of how long its fetch takes. A result is applied
    /// only if the generation captured at entry still matches when the fetch
    /// resolves — same "only the most recent call wins, by construction"
    /// recipe `RunDetailStore.focusStep`'s `focusGeneration` already
    /// documents.
    /// `scopeKind`/`scopeID` default to `nil` for callers that only have a
    /// name in hand (existing tests, and any direct `selectedDefinition =`
    /// assignment elsewhere) — pass the tapped row's own `scopeKind`/
    /// `scopeID` (see `AgentDefinition`/`WorkflowDefinition`) to pin the
    /// launch to that row's scope; see `selectedScopeKind`'s doc comment
    /// for why this is captured from the row at selection time rather than
    /// re-derived later by name.
    public func selectDefinition(_ name: String, scopeKind: String? = nil, scopeID: String? = nil) async {
        selectDefinitionGeneration += 1
        let generation = selectDefinitionGeneration

        selectedDefinition = name
        selectedScopeKind = scopeKind
        selectedScopeID = scopeID
        validationError = nil
        inputsLoadError = nil

        guard kind == .workflow else {
            workflowInputs = [:]
            inputValues = [:]
            return
        }

        do {
            let detail = try await client.workflowDetail(name: name)
            guard generation == selectDefinitionGeneration else { return }
            workflowInputs = detail.inputs
            var values: [String: String] = [:]
            for (key, input) in detail.inputs {
                values[key] = input.default ?? ""
            }
            inputValues = values
        } catch {
            guard !isCancellation(error) else { return }
            guard generation == selectDefinitionGeneration else { return }
            // Fail open (see `inputsLoadError`'s doc comment): an empty
            // declared-input set just means the form shows no input rows,
            // and `canLaunch`'s required-input check trivially passes with
            // nothing to require — `inputsLoadError` is purely an added
            // surface for the message, not a change to that behavior.
            workflowInputs = [:]
            inputValues = [:]
            inputsLoadError = String(describing: error)
        }
    }

    /// `true` once a definition is selected, every declared-required
    /// workflow input (or, for agent run/session, the free-text `prompt`)
    /// is non-empty, and at least one target host resolves (either an
    /// explicit `selectedHosts` or, under `fanOutAllHealthy`, at least one
    /// currently-known online host).
    public var canLaunch: Bool {
        guard selectedDefinition != nil else { return false }
        guard missingRequiredInputNames().isEmpty else { return false }
        if kind != .workflow, prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return false
        }
        return !resolvedTargets().isEmpty
    }

    /// Fires one POST per resolved target (`fanOutAllHealthy` ? every
    /// currently-known online host : `selectedHosts`), each with `host` in
    /// its body (`"local"` sent as `nil`, per api-facts.md), and records a
    /// `LaunchOutcome` for every one. A single-target launch that succeeds
    /// returns its destination `Route` directly, for the caller to
    /// auto-navigate; every other case (multi-target, or a single target
    /// that failed) returns `nil` — the caller reads `launchResults` for the
    /// per-host detail instead.
    ///
    /// The required-input client-side gap is checked again here (not just
    /// via `canLaunch`) and, if still open, sets `validationError` and
    /// returns without firing any request — see the type doc comment's
    /// "No validate-on-launch" section for why this is the only validation
    /// `launch()` performs.
    ///
    /// **Concurrent fan-out** (review fix, Important 2): a multi-target
    /// batch used to fire its POSTs one at a time in a plain `for` loop and
    /// only publish `launchResults` once every one of them had finished —
    /// so one slow or stuck host delayed every *later* target's request from
    /// even being *sent*, and Task 8's per-host outcome rows couldn't update
    /// progressively no matter how the view was built. Targets now launch
    /// concurrently via a `TaskGroup`, and each `LaunchOutcome` is appended
    /// to `launchResults` the moment its own POST resolves — a fast target's
    /// row can appear while a slow one is still in flight. The published
    /// order is completion order (not target order); once every task has
    /// landed, `launchResults` is sorted by host for a stable, deterministic
    /// final read. The batch's pending key follows the same "any success"
    /// policy as before — `confirm`ed if at least one target succeeded, else
    /// `fail`ed with the first (sorted) failure's message — but it now only
    /// resolves after the *last* task completes, never before, so it can't
    /// read as settled while a target is still outstanding.
    public func launch() async -> Route? {
        let missing = missingRequiredInputNames()
        guard missing.isEmpty else {
            validationError = "missing required input\(missing.count == 1 ? "" : "s"): \(missing.joined(separator: ", "))"
            return nil
        }
        validationError = nil
        guard canLaunch else { return nil }

        let key = ActionKey("launcher", .launch)
        pendingActions.begin(key)
        launchResults = []

        let targets = resolvedTargets()

        if targets.count == 1 {
            let target = targets[0]
            let hostField = target == "local" ? nil : target
            do {
                let route = try await performLaunch(hostField: hostField)
                launchResults = [LaunchOutcome(host: target, result: .success(route))]
                pendingActions.confirm(key)
                return route
            } catch {
                let message = mutationErrorMessage(error)
                launchResults = [LaunchOutcome(host: target, result: .failure(.message(message)))]
                pendingActions.fail(key, message)
                return nil
            }
        }

        await withTaskGroup(of: Void.self) { group in
            for target in targets {
                group.addTask { [weak self] in
                    guard let self else { return }
                    let hostField = target == "local" ? nil : target
                    do {
                        let route = try await self.performLaunch(hostField: hostField)
                        await self.recordOutcome(LaunchOutcome(host: target, result: .success(route)))
                    } catch {
                        let outcome = LaunchOutcome(host: target, result: .failure(.message(mutationErrorMessage(error))))
                        await self.recordOutcome(outcome)
                    }
                }
            }
            await group.waitForAll()
        }

        // Every target has now landed (`waitForAll()` above only returns
        // once the last child task's `recordOutcome` has run) — safe to
        // resolve the batch's pending key and give callers a deterministic
        // final order.
        launchResults.sort { $0.host < $1.host }
        let anySucceeded = launchResults.contains { if case .success = $0.result { return true } else { return false } }
        if anySucceeded {
            pendingActions.confirm(key)
        } else if case .failure(let error) = launchResults.first?.result {
            pendingActions.fail(key, error.text)
        } else {
            pendingActions.fail(key, "no targets")
        }
        return nil
    }

    // MARK: - Loads

    private func loadAgents() async {
        do {
            let list = try await client.agentDefinitions()
            agents = list.isEmpty ? .empty : .content(list)
        } catch {
            guard !isCancellation(error) else { return }
            agents = .failed(String(describing: error))
        }
    }

    private func loadWorkflows() async {
        do {
            let list = try await client.workflowDefinitions()
            workflows = list.isEmpty ? .empty : .content(list)
        } catch {
            guard !isCancellation(error) else { return }
            workflows = .failed(String(describing: error))
        }
    }

    /// Fired-and-forgotten from `activate()` — never awaited by it. A
    /// monotonic `hostsGeneration` guards against a stale result from a
    /// superseded `activate()` call landing after a newer one has already
    /// started (or reset `hosts`), same recipe as `ActivityStore`'s
    /// `remoteGeneration`.
    private func loadHostsInBackground() {
        hostsGeneration += 1
        let generation = hostsGeneration
        Task { [weak self] in
            guard let self else { return }
            guard let list = try? await self.client.hosts() else { return }
            guard generation == self.hostsGeneration else { return }
            self.hosts = list
        }
    }

    // MARK: - Launch mechanics

    /// Appends one target's outcome to `launchResults` as soon as it lands —
    /// called from each fan-out `TaskGroup` child task, hopping back onto
    /// this `@MainActor` type to mutate observable state safely. See
    /// `launch()`'s doc comment's "Concurrent fan-out" section.
    private func recordOutcome(_ outcome: LaunchOutcome) {
        launchResults.append(outcome)
    }

    /// Threads `selectedScopeKind`/`selectedScopeID` into a launch body's
    /// scope fields — LOCAL TARGETS ONLY (SERVER RULE, Phase 5A): scope is a
    /// local-only affordance (`resolve_launch_scope` on the server side only
    /// ever inspects this process's own local workspace registry), and the
    /// server silently ignores scope fields sent to a remote-proxied launch
    /// — sending them there would imply a guarantee ("this run targets the
    /// scope you picked") the server cannot honor for a remote host, so this
    /// omits them outright rather than sending a value the receiving end
    /// discards. `hostField` already carries this distinction for free:
    /// `nil` means "local" (see `launch()`'s `hostField` computation), any
    /// other value is a remote host id.
    ///
    /// SERVER RULE (scope + `workingDir` mutual exclusion, 400 if both are
    /// present): every `performLaunch` body below constructs `workingDir` as
    /// implicitly `nil` — this phase's Launcher sheet has no working-directory
    /// field of its own, so a body from this store can never carry both. A
    /// future phase that adds one must gate this function on it (only thread
    /// scope when no working directory is set) rather than assume the
    /// invariant still holds.
    private func scopeFields(forHostField hostField: String?) -> (kind: String?, id: String?) {
        guard hostField == nil else { return (nil, nil) }
        return (selectedScopeKind, selectedScopeID)
    }

    private func performLaunch(hostField: String?) async throws -> Route {
        guard let name = selectedDefinition else {
            throw CPError.transport("no definition selected")
        }
        let scope = scopeFields(forHostField: hostField)
        switch kind {
        case .agentRun:
            // Final-review fix (Important 3): a standalone agent run is not
            // an orchestrator run — `GET /api/runs/:id` 404s for it, the
            // same hotfix root cause C `AgentRunDetailStore`'s doc comment
            // documents for session-turn rows. `.runDetail` was therefore a
            // guaranteed dead end for a just-launched agent run; the
            // addressable destination is `.agentRunDetail`, the same route
            // `ActivityRow`/`SessionDetailScreen` already use for this row
            // shape. `transcriptPath: nil` — the launch response only ever
            // carries a `run_id`/`host_id`, never a transcript path — and
            // `AgentRunDetailStore` now makes one best-effort attempt to
            // resolve it (see that type's doc comment) before falling back
            // to an honest empty state.
            let body = AgentLaunchBody(prompt: prompt, mode: mode, host: hostField, scopeKind: scope.kind, scopeID: scope.id)
            let response = try await client.launchAgentRun(name: name, body: body)
            guard let runID = response.runID else {
                throw CPError.decoding("launchAgentRun response missing run_id")
            }
            return .agentRunDetail(id: runID, transcriptPath: nil, host: response.hostID == "local" ? nil : response.hostID)
        case .session:
            let body = AgentLaunchBody(prompt: prompt, mode: mode, host: hostField, scopeKind: scope.kind, scopeID: scope.id)
            let response = try await client.startAgentSession(name: name, body: body)
            guard let sessionID = response.sessionID else {
                throw CPError.decoding("startAgentSession response missing session_id")
            }
            return .sessionDetail(id: sessionID)
        case .workflow:
            let body = WorkflowLaunchBody(inputs: inputValues, mode: mode, host: hostField, scopeKind: scope.kind, scopeID: scope.id)
            let response = try await client.runWorkflow(name: name, body: body)
            guard let runID = response.runID else {
                throw CPError.decoding("runWorkflow response missing run_id")
            }
            return .runDetail(id: runID, host: response.hostID == "local" ? nil : response.hostID)
        }
    }

    /// Final-review fix (Important 2): `.session`'s launch route
    /// (`POST /api/agents/:name/session`) is proxied server-side to whatever
    /// host it targets, but `SessionDetailStore` — the screen a session
    /// launch always navigates to — is local-only this phase (see that
    /// type's doc comment's "Local by construction" section: no host-scoped
    /// session read/write surface exists yet). Routing a session launch at
    /// a non-local target is therefore a guaranteed dead end: a 404 on
    /// every subsequent read, and a send box that can never work.
    /// `HostChips`'s `localOnly` mode is the primary guard (it disables
    /// every non-local chip and the fan-out toggle while `kind == .session`
    /// so the operator can never select one), but this hard filter is
    /// defense in depth — it keeps `.session` launches local-only even if
    /// `selectedHosts`/`fanOutAllHealthy` end up set to something else
    /// without going through the chips at all (e.g. a future call site, or
    /// state left over from switching `kind` after selecting hosts).
    private func resolvedTargets() -> [String] {
        let targets: [String]
        if fanOutAllHealthy {
            targets = hosts.filter { $0.status == "online" }.map(\.id).sorted()
        } else {
            targets = selectedHosts.sorted()
        }
        guard kind == .session else { return targets }
        return targets.filter { $0 == "local" }
    }

    private func missingRequiredInputNames() -> [String] {
        guard kind == .workflow else { return [] }
        return workflowInputs
            .filter { $0.value.required && (inputValues[$0.key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .map(\.key)
            .sorted()
    }
}
