/// The rendered state of one step-graph node — shared between
/// `RupuRunDetail.layoutGraph` (which reads this) and `RunDetailStore`
/// (which writes it from live `CPEvent`s). Lives in `RupuStore` rather than
/// `RupuRunDetail` because `RunDetailStore.liveStates: [String: NodeState]`
/// needs it and the module graph only allows `RupuRunDetail -> RupuStore`,
/// never the reverse (`RupuRunDetail`'s `layoutGraph`/`StepGraphView`
/// `import RupuStore` to reuse this type).
///
/// `layoutGraph` itself only ever produces `.done`/`.skipped` (from a
/// matching `APIStepResult`) or `.pending` (the "not reached yet, and we
/// aren't guessing" default). `.running` and `.gatePending` are always
/// supplied by the caller via `liveStates` — `.running` from a live
/// `stepStarted` event (or the run's `activeStepID`), `.gatePending` from a
/// live `stepAwaitingApproval` event (or `RunRecord.awaiting`) — because
/// both require knowledge (which step is *currently* executing, whether the
/// run itself is parked) that a pure function over the static DAG + finished
/// results cannot derive on its own.
public enum NodeState: Equatable, Sendable {
    case done(success: Bool)
    case running
    case gatePending
    case pending
    case skipped
}
