import Foundation
import RupuAPI

/// Same check as `RupuStore.isCancellation(_:)` — deliberately duplicated,
/// not shared, because that function is module-internal (`RupuStore`'s
/// stores reach it via `@testable import`, not a public API this module
/// could depend on). `AgentDetailScreen`/`WorkflowDetailScreen` are plain
/// one-shot fetches with no dedicated store class (see `AgentDetailScreen`'s
/// doc comment), so this is their equivalent of every store's own guard: a
/// cancelled `.task(id:)` fetch (the id changed mid-flight) must never
/// surface as a user-visible `.failed` with a "Retry" button — it didn't
/// fail, it was superseded.
func isCancellation(_ error: Error) -> Bool {
    if error is CancellationError { return true }
    if let cpError = error as? CPError, cpError == .cancelled { return true }
    return false
}
