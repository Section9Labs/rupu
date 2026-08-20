import Foundation

enum Fixtures {
    static var dir: URL {
        // …/apps/rupu-macos/RupuKit/Tests/RupuAPITests/FixtureLoader.swift → …/apps/rupu-macos/Fixtures
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // FixtureLoader.swift -> RupuAPITests/
            .deletingLastPathComponent()  // RupuAPITests/ -> Tests/
            .deletingLastPathComponent()  // Tests/ -> RupuKit/
            .deletingLastPathComponent()  // RupuKit/ -> rupu-macos/
            .appendingPathComponent("Fixtures")
    }
    static func data(_ name: String) throws -> Data {
        try Data(contentsOf: dir.appendingPathComponent(name))
    }
}
