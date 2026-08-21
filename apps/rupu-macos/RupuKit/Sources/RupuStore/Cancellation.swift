import Foundation
import RupuAPI

/// True when `error` represents an in-flight load being cancelled — either
/// a plain Swift `CancellationError` (a fake fetch closure in tests, or any
/// non-`CPClient` cancellation source) or `CPError.cancelled` (`CPClient.get`'s
/// mapping of a cancelled `URLSession` request — see that type's doc
/// comment). Every store's load path (`PagedSnapshot`, `ActivityStore`,
/// `RunDetailStore`, `SessionDetailStore`, `AgentRunDetailStore`) checks this
/// before touching `state`/`rows`: a cancelled load — most commonly a
/// SwiftUI `.task(id:)` whose id changed mid-fetch — must never surface as
/// a user-visible `.failed` with a "Retry" button. It didn't fail; it was
/// superseded.
func isCancellation(_ error: Error) -> Bool {
    if error is CancellationError { return true }
    if let cpError = error as? CPError, cpError == .cancelled { return true }
    return false
}
