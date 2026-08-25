import Foundation

/// Local copy of `RupuRunDetailTests`/`RupuAPITests`'s own `Fixtures` helper
/// — each test target keeps its own file-scoped copy rather than sharing one
/// (see those targets' own `FixtureLoader.swift` for the precedent this
/// mirrors byte-for-byte, path-depth comment included).
enum Fixtures {
    static var dir: URL {
        // …/apps/rupu-macos/RupuKit/Tests/RupuShellTests/FixtureLoader.swift → …/apps/rupu-macos/Fixtures
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // FixtureLoader.swift -> RupuShellTests/
            .deletingLastPathComponent()  // RupuShellTests/ -> Tests/
            .deletingLastPathComponent()  // Tests/ -> RupuKit/
            .deletingLastPathComponent()  // RupuKit/ -> rupu-macos/
            .appendingPathComponent("Fixtures")
    }
    static func data(_ name: String) throws -> Data {
        try Data(contentsOf: dir.appendingPathComponent(name))
    }
}
