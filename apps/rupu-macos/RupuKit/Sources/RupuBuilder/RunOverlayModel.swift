import RupuAPI
import RupuStore

/// Workflow Builder Task 14: the Run-mode overlay's pure derivation. This is
/// a SMALL, PURPOSE-BUILT PORT of `RupuStore.layoutGraph`'s priority-merge
/// rule (`GraphLayout.swift`) — not a reuse of that function, and
/// deliberately not a dependency on `RupuRunDetail` (which `RupuBuilder`
/// does not, and should not, depend on: see `Package.swift`'s target
/// graph). `layoutGraph` produces a whole render-ready `[GraphNodeVM]` tree
/// (sub-steps, fanout unit squares, panel rounds, transcript paths — none of
/// which the canvas overlay needs); this only needs the one precedence rule
/// (`liveStates` wins outright, else a finished `APIStepResult` settles it,
/// else `.pending`) plus a simple `(done, total)` count per fan-out step, so
/// porting just that rule keeps this module honestly independent rather than
/// reaching for a bigger dependency to save a few lines.
public struct RunOverlay: Equatable {
    public let states: [String: NodeState]
    /// `for_each` step id -> (finished units, total units), from the run's
    /// REST `APIUnitRow` snapshot. A step with no units at all (every
    /// non-`for_each` kind, or a `for_each` that hasn't started) has no
    /// entry here.
    public let unitProgress: [String: (done: Int, total: Int)]

    public init(states: [String: NodeState], unitProgress: [String: (done: Int, total: Int)]) {
        self.states = states
        self.unitProgress = unitProgress
    }

    /// Manual `==`: a `(done: Int, total: Int)` tuple's own structural `==`
    /// works fine element-to-element, but `Dictionary<String, (Int, Int)>`
    /// itself has no synthesized `Equatable` conformance (tuples aren't a
    /// `Hashable`/`Equatable`-conforming TYPE, even though `==` between two
    /// tuple VALUES compiles) — so the compiler can't synthesize this for
    /// the struct as a whole either. `states` already conforms
    /// (`NodeState: Equatable`), hence only `unitProgress` needs the
    /// explicit walk.
    public static func == (lhs: RunOverlay, rhs: RunOverlay) -> Bool {
        guard lhs.states == rhs.states, lhs.unitProgress.count == rhs.unitProgress.count else { return false }
        for (key, value) in lhs.unitProgress {
            guard let other = rhs.unitProgress[key], other == value else { return false }
        }
        return true
    }
}

/// The priority-merge port of `layoutGraph`'s rule 1-3 (`GraphLayout.swift`,
/// `RupuStore`), keyed by step id — the same YAML step ids the canvas draws
/// (`GraphNode.id`/`StepNodeData.id`) and the run's own `APIStepResult.
/// stepID`/`APIUnitRow.stepID` share, by construction (the run was executed
/// FROM this exact workflow's step DAG).
///
/// Per step id, in priority order:
/// 1. `liveStates[id]` wins outright — covers `.running`, `.gatePending`,
///    `.paused`, exactly like `layoutGraph`.
/// 2. Else a matching `APIStepResult` settles it: `skipped` -> `.skipped`,
///    otherwise -> `.done(success:)`.
/// 3. Else, if `units` shows this step id with any unit still in flight
///    (`success == nil`) — the same `for_each`-in-flight promotion
///    `layoutGraph` layers on top of its own rule 3 — `.running`.
/// 4. Else no entry at all: the caller (`NodeView`) treats a missing key as
///    `.pending`, matching `layoutGraph`'s own "never guess, default
///    `.pending`" contract.
///
/// `unitProgress` is a simple `(done, total)` per step id present in
/// `units`, `done` counting every unit whose `success != nil` — i.e.
/// FINISHED, whether it passed or failed, per the brief's exact "success !=
/// nil = done" rule (not "succeeded"). This intentionally does NOT fold in
/// any live per-unit overlay the way `layoutGraph`'s own `fanout(for:)`
/// does — the Builder's Run mode has no live per-unit event stream wired
/// (Task 14's `liveStates` input is step-level only), so the REST `units`
/// snapshot is the only signal available; a running unit that hasn't yet
/// been polled into `units` simply isn't counted yet, same honest-degrade
/// contract every other REST-only view in this app already follows.
public func runOverlay(
    results: [APIStepResult],
    units: [APIUnitRow],
    liveStates: [String: NodeState]
) -> RunOverlay {
    var resultsByStepID: [String: APIStepResult] = [:]
    for result in results {
        resultsByStepID[result.stepID] = result
    }

    var unitsByStepID: [String: [APIUnitRow]] = [:]
    for unit in units {
        unitsByStepID[unit.stepID, default: []].append(unit)
    }

    var states: [String: NodeState] = liveStates
    for (stepID, result) in resultsByStepID where states[stepID] == nil {
        states[stepID] = result.skipped ? .skipped : .done(success: result.success)
    }
    for (stepID, stepUnits) in unitsByStepID where states[stepID] == nil {
        if stepUnits.contains(where: { $0.success == nil }) {
            states[stepID] = .running
        }
    }

    var unitProgress: [String: (done: Int, total: Int)] = [:]
    for (stepID, stepUnits) in unitsByStepID {
        let done = stepUnits.count { $0.success != nil }
        unitProgress[stepID] = (done: done, total: stepUnits.count)
    }

    return RunOverlay(states: states, unitProgress: unitProgress)
}

/// The run to follow when entering Run mode with no `launchedRunID` recorded
/// this session (`BuilderStore.enterRunMode(backend:)`'s fallback path):
/// the first row in `rows` whose `workflowName` matches — `rows` is assumed
/// already newest-first, the same paging order `client.workflowRuns(offset:
/// limit:)` returns (server-side newest-first, per every other list screen's
/// existing convention; see `ActivityStore`), so "first match" IS "newest
/// match" without this function re-sorting anything itself. `nil` when no
/// row matches (a workflow that has never run) — `enterRunMode` renders the
/// "No runs yet" empty state in that case.
public func latestRunID(rows: [APIRunListRow], workflowName: String) -> (id: String, host: String?)? {
    guard let row = rows.first(where: { $0.workflowName == workflowName }) else { return nil }
    return (row.id, row.hostID)
}
