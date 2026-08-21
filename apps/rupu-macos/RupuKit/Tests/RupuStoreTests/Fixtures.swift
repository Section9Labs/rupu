import Foundation

/// Local copy of `RupuAPITests/FixtureLoader.swift`'s loader — each test
/// target resolves `#filePath` relative to itself, so `RupuStoreTests`
/// needs its own (identical depth: `Tests/RupuStoreTests/File.swift` →
/// `RupuStoreTests/` → `Tests/` → `RupuKit/` → `rupu-macos/` →
/// `Fixtures/`).
enum Fixtures {
    static var dir: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // Fixtures.swift -> RupuStoreTests/
            .deletingLastPathComponent()  // RupuStoreTests/ -> Tests/
            .deletingLastPathComponent()  // Tests/ -> RupuKit/
            .deletingLastPathComponent()  // RupuKit/ -> rupu-macos/
            .appendingPathComponent("Fixtures")
    }
    static func data(_ name: String) throws -> Data {
        try Data(contentsOf: dir.appendingPathComponent(name))
    }
}
