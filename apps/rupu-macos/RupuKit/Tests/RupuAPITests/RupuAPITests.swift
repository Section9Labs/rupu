import Testing
@testable import RupuAPI

@Test func rupuAPIModuleExists() {
    #expect(String(describing: RupuAPIModule.self) == "RupuAPIModule")
}
