import Testing
import Foundation
@testable import RupuBackend

@Test func discoveryPrefersOverrideThenWhichThenPaths() {
    let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("fake-rupu-\(UUID())").path
    FileManager.default.createFile(atPath: tmp, contents: Data(), attributes: [.posixPermissions: 0o755])
    #expect(RupuDiscovery.find(override: tmp, which: { _ in nil }) == tmp)
    #expect(RupuDiscovery.find(override: nil, searchPaths: [], which: { _ in tmp }) == tmp)
    #expect(RupuDiscovery.find(override: nil, searchPaths: [tmp], which: { _ in nil }) == tmp)
    #expect(RupuDiscovery.find(override: nil, searchPaths: [], which: { _ in nil }) == nil)
}
