import Foundation

enum Fixtures {
    static var dir: URL {
        // …/apps/rupu-macos/RupuKit/Tests/RupuRunDetailTests/FixtureLoader.swift → …/apps/rupu-macos/Fixtures
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // FixtureLoader.swift -> RupuRunDetailTests/
            .deletingLastPathComponent()  // RupuRunDetailTests/ -> Tests/
            .deletingLastPathComponent()  // Tests/ -> RupuKit/
            .deletingLastPathComponent()  // RupuKit/ -> rupu-macos/
            .appendingPathComponent("Fixtures")
    }
    static func data(_ name: String) throws -> Data {
        try Data(contentsOf: dir.appendingPathComponent(name))
    }
}
