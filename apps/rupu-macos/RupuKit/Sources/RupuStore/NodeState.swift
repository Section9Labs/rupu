/// The rendered state of one step-graph node — shared between
/// `RupuRunDetail.layoutGraph` (which reads this) and `RunDetailStore`
/// (which writes it from live `CPEvent`s). Lives in `RupuStore` rather than
/// `RupuRunDetail` because `RunDetailStore.liveStates: [String: NodeState]`
/// needs it and the module graph only allows `RupuRunDetail -> RupuStore`,
/// never the reverse (`RupuRunDetail`'s `layoutGraph`/`StepGraphView`
/// `import RupuStore` to reuse this type).
///
/// `layoutGraph` itself only ever produces `.done`/`.skipped` (from a
/// matching `APIStepResult`), `.running` (ONLY the `for_each` in-flight-unit
/// promotion described below — never inferred from position otherwise), or
/// `.pending` (the "not reached yet, and we aren't guessing" default).
/// `.running` (via `stepStarted`), `.gatePending` (via
/// `stepAwaitingApproval`), and `.paused` (via `stepPaused`/`stepResumed`,
/// the latter reverting to `.running`) are otherwise always supplied by the
/// caller via `liveStates` — because knowing which step is *currently*
/// executing, or whether the run itself is parked, requires live event/run
/// knowledge that a pure function over the static DAG + finished results
/// cannot derive on its own. The one documented exception: a `for_each`
/// node still `.pending` gets promoted to `.running` when `layoutGraph`
/// observes any of its fan-out units in flight (mirrors the web's
/// `runGraphModel.ts` phase 5, L296-301).
public enum NodeState: Equatable, Sendable {
    case done(success: Bool)
    case running
    case gatePending
    case paused
    case pending
    case skipped
}

/// Live overlay for one fan-out unit — folded from a `unit_started`/
/// `unit_completed` `CPEvent` on top of the REST `APIUnitRow` snapshot.
/// REST rows have no `unit_key` (the wire event's correlation id), hence
/// `key: String?` — `layoutGraph` falls back to a positional `"#\(index)"`
/// label when nothing live has supplied one.
public struct UnitLiveState: Equatable, Sendable {
    public let key: String?
    public let transcriptPath: String?
    public let success: Bool?

    public init(key: String?, transcriptPath: String?, success: Bool?) {
        self.key = key
        self.transcriptPath = transcriptPath
        self.success = success
    }
}

/// Live panel-review iteration counter — folded from a `panel_round`
/// `CPEvent`, keyed by step id. `maxIterations` mirrors the step's own
/// `APIPanelGate.maxIterations` so the view doesn't need to cross-reference
/// the static DAG a second time.
public struct PanelRoundState: Equatable, Sendable {
    public let round: UInt32
    public let maxIterations: UInt32

    public init(round: UInt32, maxIterations: UInt32) {
        self.round = round
        self.maxIterations = maxIterations
    }
}
