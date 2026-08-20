// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "RupuKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(
            name: "RupuKit",
            targets: ["RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuShell"]
        )
    ],
    targets: [
        .target(name: "RupuAPI"),
        .target(name: "RupuBackend", dependencies: ["RupuAPI"]),
        .target(name: "RupuDesign"),
        .target(name: "RupuStore", dependencies: ["RupuAPI", "RupuBackend"]),
        .target(name: "RupuShell", dependencies: ["RupuAPI", "RupuBackend", "RupuStore", "RupuDesign"]),
        .testTarget(name: "RupuAPITests", dependencies: ["RupuAPI"]),
        .testTarget(name: "RupuBackendTests", dependencies: ["RupuBackend", "RupuAPI"]),
        .testTarget(name: "RupuDesignTests", dependencies: ["RupuDesign"]),
        .testTarget(name: "RupuStoreTests", dependencies: ["RupuStore"]),
    ]
)
