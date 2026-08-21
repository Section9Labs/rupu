import Foundation
import RupuAPI

/// A normalized instruction for updating an in-memory `ActivityRow` cache
/// from a live `CPEvent` — the write side of the same normalization
/// `ActivityRow`'s API-model initializers provide for the initial page
/// load. `reduce` is a pure function so a store can call it without any
/// dependency on `StreamLifecycle` or networking, and tests can exercise it
/// directly against a `CPEvent` value.
public enum ActivityDelta: Equatable, Sendable {
    case statusPatch(runID: String, status: ActivityStatus, durationMS: UInt64?)
    case newRun(runID: String)
    case none

    /// Maps one `CPEvent` to the delta it implies for the Activity feed.
    /// Only run-lifecycle-shaped events produce a delta — Activity rows
    /// are run-granular, not step-granular, so step/unit/panel/dispatch
    /// events (and any unrecognized `.unknown` type) reduce to `.none`.
    public static func reduce(_ e: CPEvent) -> ActivityDelta {
        switch e {
        case .runStarted(let runID, _, _):
            return .newRun(runID: runID)
        case .runCompleted(let runID, let status, _):
            return .statusPatch(runID: runID, status: .normalize(status), durationMS: nil)
        case .runFailed(let runID, _, _):
            return .statusPatch(runID: runID, status: .failed, durationMS: nil)
        case .runPaused(let runID):
            return .statusPatch(runID: runID, status: .paused, durationMS: nil)
        case .runResumed(let runID):
            return .statusPatch(runID: runID, status: .running, durationMS: nil)
        case .stepAwaitingApproval(let runID, _, _):
            return .statusPatch(runID: runID, status: .awaiting, durationMS: nil)
        default:
            return .none
        }
    }
}
