import Testing
@testable import RupuBackend

@Test func rupuBackendModuleExists() {
    #expect(String(describing: RupuBackendModule.self) == "RupuBackendModule")
}
