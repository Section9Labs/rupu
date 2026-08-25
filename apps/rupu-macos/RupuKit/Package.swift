// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "RupuKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(
            name: "RupuKit",
            targets: [
                "RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuActivity", "RupuRunDetail", "RupuLauncher", "RupuOverview",
                "RupuProjects", "RupuFleet", "RupuLibrary", "RupuSecurity", "RupuUsage", "RupuShell",
            ]
        )
    ],
    targets: [
        .target(name: "RupuAPI"),
        .target(name: "RupuBackend", dependencies: ["RupuAPI"]),
        .target(name: "RupuDesign", exclude: ["Icons/svg"]),
        // Pure aggregation port (no View/Observation deps) — depends only on
        // `RupuAPI` for `APIUsageRunRow`, never on `RupuStore`, so `RupuStore`
        // (which owns `UsageStore.swift`, the consumer) can depend on THIS
        // without a cycle.
        .target(name: "RupuUsage", dependencies: ["RupuAPI"]),
        .target(name: "RupuStore", dependencies: ["RupuAPI", "RupuBackend", "RupuDesign", "RupuUsage"]),
        .target(name: "RupuActivity", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuRunDetail", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuLauncher", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuOverview", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuProjects", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuFleet", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuLibrary", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuSecurity", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(
            name: "RupuShell",
            dependencies: [
                "RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuActivity", "RupuRunDetail", "RupuLauncher", "RupuOverview",
                "RupuProjects", "RupuFleet", "RupuLibrary", "RupuSecurity",
            ]
        ),
        .testTarget(name: "RupuAPITests", dependencies: ["RupuAPI"]),
        .testTarget(name: "RupuBackendTests", dependencies: ["RupuBackend", "RupuAPI"]),
        .testTarget(name: "RupuDesignTests", dependencies: ["RupuDesign"]),
        .testTarget(name: "RupuUsageTests", dependencies: ["RupuUsage", "RupuAPI"]),
        .testTarget(name: "RupuStoreTests", dependencies: ["RupuStore", "RupuBackend", "RupuAPI", "RupuDesign", "RupuUsage"]),
        .testTarget(
            name: "RupuRunDetailTests",
            dependencies: ["RupuRunDetail", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
        .testTarget(
            name: "RupuOverviewTests",
            dependencies: ["RupuOverview", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
        .testTarget(
            name: "RupuProjectsTests",
            dependencies: ["RupuProjects", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
        .testTarget(
            name: "RupuShellTests",
            dependencies: ["RupuShell", "RupuStore", "RupuBackend", "RupuAPI", "RupuDesign", "RupuActivity"]
        ),
        .testTarget(
            name: "RupuSecurityTests",
            dependencies: ["RupuSecurity", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
    ]
)
