import Foundation
import RupuAPI

/// Shared error-to-message mapping for every pending-state mutation method
/// (`RunDetailStore`'s approve/reject/cancel/pause/resume/archive/restore,
/// `ActivityStore`'s awaiting-row approve/reject) — the POST itself
/// throwing is what `PendingActions.fail(_:_:)` records, and this is the one
/// place that turns the thrown `Error` into the message shown to the
/// operator.
///
/// A 501 (`CPError.http(status: 501, ...)`) means the backend has no
/// launch-runtime configured — every marker-only write route (approve/
/// resume most visibly, per api-facts.md) can return it — so it gets one
/// specific, actionable message rather than the raw HTTP body. Every other
/// error (a different HTTP status, a transport failure, a decoding failure)
/// falls back to a plain `String(describing:)` of the underlying error,
/// same as every REST-load failure path in this module already does.
func mutationErrorMessage(_ error: Error) -> String {
    if let cpError = error as? CPError, case .http(let status, _) = cpError, status == 501 {
        return "server lacks launch runtime — start with `rupu cp serve`"
    }
    return String(describing: error)
}
