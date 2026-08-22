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

/// `Result`'s `Failure` generic parameter requires `Error` conformance, but
/// `LaunchOutcome.result` is specified (brief, Task 7) as plain
/// `Result<Route, String>` — a bare message, not a typed error — matching
/// every other mutation surface in this module (`mutationErrorMessage(_:)`
/// already collapses every failure down to a `String`). This conformance
/// exists solely to satisfy that generic constraint for this one type; nothing
/// here throws a bare `String` as an `Error` elsewhere in the module.
extension String: @retroactive Error {}

/// One target host's launch attempt, recorded after `launch()`'s POST for
/// it resolves — `host` is the target's id (`"local"` for the embedded
/// backend, a Fleet node id otherwise), never the request's `nil`-for-local
/// body encoding. `result` carries either the destination `Route` a
/// successful launch reached, or the same `mutationErrorMessage(_:)`-mapped
/// string every other mutation in this module surfaces on failure (a 501
/// reads as "server lacks launch runtime — start with `rupu cp serve`").
public struct LaunchOutcome: Equatable, Sendable {
    public let host: String
    public let result: Result<Route, String>

    public init(host: String, result: Result<Route, String>) {
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

    public private(set) var workflowInputs: [String: WorkflowInputDef] = [:]
    public var inputValues: [String: String] = [:]

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

    /// Selects `name` as the definition to launch. For a workflow, also
    /// fetches its declared inputs (`GET /api/workflows/:name`) and seeds
    /// `workflowInputs` plus a defaulted `inputValues` (each declared
    /// input's own `default`, or `""` when it has none — never left
    /// unseeded, so every declared input has a row to render and edit).
    /// For an agent run/session, there are no declared inputs to fetch —
    /// `workflowInputs`/`inputValues` are cleared instead, so switching
    /// `kind` (or selecting a different definition) never leaves a stale
    /// prior workflow's input rows showing.
    public func selectDefinition(_ name: String) async {
        selectedDefinition = name
        validationError = nil

        guard kind == .workflow else {
            workflowInputs = [:]
            inputValues = [:]
            return
        }

        do {
            let detail = try await client.workflowDetail(name: name)
            workflowInputs = detail.inputs
            var values: [String: String] = [:]
            for (key, input) in detail.inputs {
                values[key] = input.default ?? ""
            }
            inputValues = values
        } catch {
            guard !isCancellation(error) else { return }
            // No dedicated failure surface for this fetch this phase (same
            // "fail open" reasoning `RunDetailStore.focusStep`'s transcript
            // fallback uses) — an empty declared-input set just means the
            // form shows no input rows, and `canLaunch`'s required-input
            // check trivially passes with nothing to require.
            workflowInputs = [:]
            inputValues = [:]
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

        let targets = resolvedTargets()
        var outcomes: [LaunchOutcome] = []
        for target in targets {
            let hostField = target == "local" ? nil : target
            do {
                let route = try await performLaunch(hostField: hostField)
                outcomes.append(LaunchOutcome(host: target, result: .success(route)))
            } catch {
                outcomes.append(LaunchOutcome(host: target, result: .failure(mutationErrorMessage(error))))
            }
        }
        launchResults = outcomes

        if outcomes.count == 1 {
            switch outcomes[0].result {
            case .success(let route):
                pendingActions.confirm(key)
                return route
            case .failure(let message):
                pendingActions.fail(key, message)
                return nil
            }
        }

        // Multi-target: the batch itself is "done" the moment every POST
        // has resolved, whatever their individual outcomes — the caller
        // renders `launchResults` per row rather than this method picking
        // one aggregate verdict. `confirm` when at least one target
        // succeeded; `fail` (with the first failure's message, as a
        // representative summary) only when every target failed.
        let anySucceeded = outcomes.contains { if case .success = $0.result { return true } else { return false } }
        if anySucceeded {
            pendingActions.confirm(key)
        } else if case .failure(let message) = outcomes.first?.result {
            pendingActions.fail(key, message)
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

    private func performLaunch(hostField: String?) async throws -> Route {
        guard let name = selectedDefinition else {
            throw CPError.transport("no definition selected")
        }
        switch kind {
        case .agentRun:
            let body = AgentLaunchBody(prompt: prompt, mode: mode, host: hostField)
            let response = try await client.launchAgentRun(name: name, body: body)
            guard let runID = response.runID else {
                throw CPError.decoding("launchAgentRun response missing run_id")
            }
            return .runDetail(id: runID, host: response.hostID == "local" ? nil : response.hostID)
        case .session:
            let body = AgentLaunchBody(prompt: prompt, mode: mode, host: hostField)
            let response = try await client.startAgentSession(name: name, body: body)
            guard let sessionID = response.sessionID else {
                throw CPError.decoding("startAgentSession response missing session_id")
            }
            return .sessionDetail(id: sessionID)
        case .workflow:
            let body = WorkflowLaunchBody(inputs: inputValues, mode: mode, host: hostField)
            let response = try await client.runWorkflow(name: name, body: body)
            guard let runID = response.runID else {
                throw CPError.decoding("runWorkflow response missing run_id")
            }
            return .runDetail(id: runID, host: response.hostID == "local" ? nil : response.hostID)
        }
    }

    private func resolvedTargets() -> [String] {
        if fanOutAllHealthy {
            return hosts.filter { $0.status == "online" }.map(\.id).sorted()
        }
        return selectedHosts.sorted()
    }

    private func missingRequiredInputNames() -> [String] {
        guard kind == .workflow else { return [] }
        return workflowInputs
            .filter { $0.value.required && (inputValues[$0.key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
            .map(\.key)
            .sorted()
    }
}
