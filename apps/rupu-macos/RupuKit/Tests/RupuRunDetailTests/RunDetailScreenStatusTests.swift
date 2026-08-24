import Testing
import RupuStore
@testable import RupuRunDetail

/// `RunDetailScreen.unrecognizedStatusRaw` is the pure seam behind the
/// header status pill's fallback (Fix round 1): for any raw status
/// `ActivityStatus.normalize` recognizes, the header renders through
/// `StatusPill` as usual (this returns `nil`); for one it doesn't, it
/// returns the original raw string so the header can render it directly
/// instead of silently discarding it behind a generic "Pending" pill.
@Test func unrecognizedStatusRawIsNilForEveryKnownStatus() {
    // Exactly the raw vocabulary `ActivityStatus.normalize` switches on —
    // including its `ok`/`error`/`aborted` agent-row synonyms and
    // `awaiting_approval` (not the bare `"awaiting"` the run's own
    // `StatusTone` case is named after).
    let known = [
        "pending", "running", "completed", "ok", "failed", "error",
        "awaiting_approval", "rejected", "cancelled", "aborted", "paused",
    ]
    for raw in known {
        #expect(RunDetailScreen.unrecognizedStatusRaw(raw) == nil, "raw status \"\(raw)\" should be recognized")
    }
}

@Test func unrecognizedStatusRawSurfacesTheOriginalStringForAnUnknownStatus() {
    #expect(RunDetailScreen.unrecognizedStatusRaw("provisioning") == "provisioning")
    #expect(RunDetailScreen.unrecognizedStatusRaw("weird_new_status") == "weird_new_status")
}

@Test func unrecognizedStatusRawSurfacesTheAbsentMarkerTooRatherThanSwallowingIt() {
    // `ActivityStatus.normalize` maps a `nil` raw status to `.unknown("—")`
    // (an explicit "genuinely absent" marker, per that type's own doc
    // comment). This helper only ever sees the non-optional `String` the
    // header already has in hand, but a literal "—" reaching it the same
    // way any other unrecognized string would should still surface, not
    // silently become "Pending".
    #expect(RunDetailScreen.unrecognizedStatusRaw("—") == "—")
}
