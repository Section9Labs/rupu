import Testing
import RupuDesign
@testable import RupuRunDetail

/// `FindingsTabContent.severity(for:)` maps `APIFinding.severity`'s wire
/// vocabulary (rupu-coverage's `Severity` enum,
/// `#[serde(rename_all = "lowercase")]` over `Info|Low|Medium|High|Critical`
/// — see `crates/rupu-coverage/src/catalog/types.rs`) onto this app's own
/// `RupuDesign.Severity` case names. The two vocabularies only agree on
/// `high`/`low`/`info`; `critical`/`medium` don't match `crit`/`med`, so a
/// naive `Severity(rawValue:)` (the pre-fix code) silently mapped every
/// critical and medium finding to `.info` — this table pins every wire
/// value `Fixtures/findings_run.json` and the Rust enum actually produce.
///
/// Flows-composition Task 4: moved verbatim from `RailViewsSeverityTests`
/// alongside `severity(for:)` itself, which moved from the deleted
/// `RailViews.swift`'s `FindingsCard` into `RunDetailTabs.swift`'s
/// `FindingsTabContent` — the Findings rail card became the Findings tab's
/// content, single-vertical-stack recompose.
@Test func severityForMapsEveryWireStringToItsSeverityCase() {
    let table: [(wire: String, expected: Severity)] = [
        ("critical", .crit),
        ("high", .high),
        ("medium", .med),
        ("low", .low),
        ("info", .info),
    ]
    for (wire, expected) in table {
        #expect(FindingsTabContent.severity(for: wire) == expected, "severity(for: \"\(wire)\")")
    }
}

@Test func severityForFallsBackToInfoForAnUnrecognizedWireString() {
    #expect(FindingsTabContent.severity(for: "unknown_future_severity") == .info)
    #expect(FindingsTabContent.severity(for: "") == .info)
}
