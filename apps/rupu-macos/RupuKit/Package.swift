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
                "RupuProjects", "RupuFleet", "RupuLibrary", "RupuSecurity", "RupuUsageKit", "RupuUsage", "RupuShell",
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
        // without a cycle. Named `RupuUsageKit` (not `RupuUsage`) — Task 6's
        // Usage SCREEN module needs the `RupuUsage` name (the "one module per
        // screen" convention every other screen module already follows) but
        // also needs to depend on both this pure module AND `RupuStore`
        // (which itself depends on this pure module), so this pure module
        // was renamed out of the way rather than colliding with (or being
        // depended on circularly by) the screen target — see
        // `UsageAggregation.swift`'s file-header doc comment for the full
        // rationale.
        .target(name: "RupuUsageKit", dependencies: ["RupuAPI"]),
        .target(name: "RupuStore", dependencies: ["RupuAPI", "RupuBackend", "RupuDesign", "RupuUsageKit"]),
        .target(name: "RupuActivity", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuRunDetail", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuLauncher", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuOverview", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuProjects", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuFleet", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuLibrary", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuSecurity", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        // The Usage screen (Phase 5B, Task 6) — depends on `RupuUsageKit`
        // directly (for `UsagePivot`/`PivotRow`/`SpendBucket`/
        // `aggregateRows`/`buildSpendTimeline`, used to drive the pivot
        // picker and build the spend chart from `UsageStore.usageRuns`) in
        // addition to `RupuStore` (for `UsageStore` itself) — a diamond
        // dependency (`RupuUsage` -> `RupuStore` -> `RupuUsageKit`, and
        // `RupuUsage` -> `RupuUsageKit` directly), not a cycle. Also depends
        // on `RupuOverview` — REUSING Phase 4's `FreshnessStrip` (brief's
        // explicit ask) rather than forking a parallel one; `HostSlice`/
        // `SliceState` themselves live in `RupuStore` (already a
        // dependency), only the `FreshnessStrip` View is `RupuOverview`'s.
        .target(name: "RupuUsage", dependencies: ["RupuAPI", "RupuStore", "RupuDesign", "RupuUsageKit", "RupuOverview"]),
        .target(
            name: "RupuShell",
            dependencies: [
                "RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuActivity", "RupuRunDetail", "RupuLauncher", "RupuOverview",
                "RupuProjects", "RupuFleet", "RupuLibrary", "RupuSecurity", "RupuUsage",
            ]
        ),
        .testTarget(name: "RupuAPITests", dependencies: ["RupuAPI"]),
        .testTarget(name: "RupuBackendTests", dependencies: ["RupuBackend", "RupuAPI"]),
        .testTarget(name: "RupuDesignTests", dependencies: ["RupuDesign"]),
        .testTarget(name: "RupuUsageKitTests", dependencies: ["RupuUsageKit", "RupuAPI"]),
        .testTarget(name: "RupuStoreTests", dependencies: ["RupuStore", "RupuBackend", "RupuAPI", "RupuDesign", "RupuUsageKit"]),
        .testTarget(
            name: "RupuUsageTests",
            dependencies: ["RupuUsage", "RupuAPI", "RupuStore", "RupuDesign", "RupuUsageKit"]
        ),
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
