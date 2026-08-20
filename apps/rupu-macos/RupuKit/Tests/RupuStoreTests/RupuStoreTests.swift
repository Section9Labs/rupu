import Testing
@testable import RupuStore

@Test func rupuStoreModuleExists() {
    #expect(String(describing: RupuStoreModule.self) == "RupuStoreModule")
}
