import Testing
@testable import RupuBackend

@Test func versionGate() {
    #expect(VersionGate.compatible("0.74.0"))
    #expect(VersionGate.compatible("0.75.3"))
    #expect(VersionGate.compatible("1.0.0"))
    #expect(!VersionGate.compatible("0.73.9"))
    // Phase 2 bumped the floor from 0.71.0 to 0.74.0 (netflow-era endpoints
    // don't exist before it) — a version that used to be compatible must
    // now read as incompatible.
    #expect(!VersionGate.compatible("0.71.0"))
    #expect(VersionGate.compatible("0.74.0-beta.1"))
    #expect(!VersionGate.compatible("garbage"))
}
