// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "RupuKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(
            name: "RupuKit",
            targets: ["RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuActivity", "RupuRunDetail", "RupuLauncher", "RupuShell"]
        )
    ],
    targets: [
        .target(name: "RupuAPI"),
        .target(name: "RupuBackend", dependencies: ["RupuAPI"]),
        .target(name: "RupuDesign", exclude: ["Icons/svg"]),
        .target(name: "RupuStore", dependencies: ["RupuAPI", "RupuBackend", "RupuDesign"]),
        .target(name: "RupuActivity", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuRunDetail", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(name: "RupuLauncher", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"]),
        .target(
            name: "RupuShell",
            dependencies: [
                "RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuActivity", "RupuRunDetail", "RupuLauncher",
            ]
        ),
        .testTarget(name: "RupuAPITests", dependencies: ["RupuAPI"]),
        .testTarget(name: "RupuBackendTests", dependencies: ["RupuBackend", "RupuAPI"]),
        .testTarget(name: "RupuDesignTests", dependencies: ["RupuDesign"]),
        .testTarget(name: "RupuStoreTests", dependencies: ["RupuStore", "RupuBackend", "RupuAPI", "RupuDesign"]),
        .testTarget(
            name: "RupuRunDetailTests",
            dependencies: ["RupuRunDetail", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
        .testTarget(
            name: "RupuShellTests",
            dependencies: ["RupuShell", "RupuStore", "RupuBackend", "RupuAPI", "RupuDesign", "RupuActivity"]
        ),
    ]
)
