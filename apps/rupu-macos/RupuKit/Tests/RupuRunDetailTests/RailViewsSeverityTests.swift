import Testing
import RupuDesign
@testable import RupuRunDetail

/// `FindingsCard.severity(for:)` maps `APIFinding.severity`'s wire vocabulary
/// (rupu-coverage's `Severity` enum, `#[serde(rename_all = "lowercase")]`
/// over `Info|Low|Medium|High|Critical` — see
/// `crates/rupu-coverage/src/catalog/types.rs`) onto this app's own
/// `RupuDesign.Severity` case names. The two vocabularies only agree on
/// `high`/`low`/`info`; `critical`/`medium` don't match `crit`/`med`, so a
/// naive `Severity(rawValue:)` (the pre-fix code) silently mapped every
/// critical and medium finding to `.info` — this table pins every wire
/// value `Fixtures/findings_run.json` and the Rust enum actually produce.
@Test func severityForMapsEveryWireStringToItsSeverityCase() {
    let table: [(wire: String, expected: Severity)] = [
        ("critical", .crit),
        ("high", .high),
        ("medium", .med),
        ("low", .low),
        ("info", .info),
    ]
    for (wire, expected) in table {
        #expect(FindingsCard.severity(for: wire) == expected, "severity(for: \"\(wire)\")")
    }
}

@Test func severityForFallsBackToInfoForAnUnrecognizedWireString() {
    #expect(FindingsCard.severity(for: "unknown_future_severity") == .info)
    #expect(FindingsCard.severity(for: "") == .info)
}
