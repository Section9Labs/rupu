import Testing
@testable import RupuDesign

/// `Severity.init(wireString:)` maps `APIFinding.severity`'s (and
/// `APICoverageSummary`/`APICoverageFinding`'s) wire vocabulary
/// (rupu-coverage's `Severity` enum, `#[serde(rename_all = "lowercase")]`
/// over `Info|Low|Medium|High|Critical` — see `crates/rupu-coverage/src/
/// catalog/types.rs`) onto this app's own `RupuDesign.Severity` case names.
/// The two vocabularies only agree on `high`/`low`/`info`; `critical`/
/// `medium` don't match `crit`/`med`, so a naive `Severity(rawValue:)` (the
/// pre-fix code) silently mapped every critical and medium finding to
/// `.info` — this table pins every wire value the fixtures and the Rust
/// enum actually produce.
///
/// Phase 5B, Task 3 (the "severity lift"): moved verbatim from
/// `RailViewsSeverityTests` → `FindingsSeverityTests.swift`
/// (`RupuRunDetailTests`), alongside `severity(for:)` itself, which moved
/// from `RunDetailTabs.swift`'s `FindingsTabContent` (and its exact
/// duplicate in `RupuProjects/ProjectDetailScreen.swift`) into this shared
/// `RupuDesign` init — see that init's doc comment. Renamed and moved to
/// `RupuDesignTests` since the seam it tests is now a plain, pure `Severity`
/// initializer with no `View`/`@MainActor` dependency — no `@MainActor` on
/// these tests, unlike the `FindingsTabContent.severity(for:)`-calling
/// originals.
@Test func severityWireStringMapsEveryWireStringToItsSeverityCase() {
    let table: [(wire: String, expected: Severity)] = [
        ("critical", .crit),
        ("high", .high),
        ("medium", .med),
        ("low", .low),
        ("info", .info),
    ]
    for (wire, expected) in table {
        #expect(Severity(wireString: wire) == expected, "Severity(wireString: \"\(wire)\")")
    }
}

@Test func severityWireStringFallsBackToInfoForAnUnrecognizedWireString() {
    #expect(Severity(wireString: "unknown_future_severity") == .info)
    #expect(Severity(wireString: "") == .info)
}
