import Testing
@testable import RupuBackend

@Test func versionGate() {
    #expect(VersionGate.compatible("0.71.0"))
    #expect(VersionGate.compatible("0.72.3"))
    #expect(VersionGate.compatible("1.0.0"))
    #expect(!VersionGate.compatible("0.70.9"))
    #expect(VersionGate.compatible("0.72.0-beta.1"))
    #expect(!VersionGate.compatible("garbage"))
}
