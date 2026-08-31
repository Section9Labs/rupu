// swift-tools-version: 6.0
import PackageDescription

// Package-level cross-module optimization is on by default for release
// builds of a multi-target package (SwiftPM passes `-enable-default-cmo`
// alongside `-package-name`). Under Xcode 16.4 that makes `swift-frontend`
// crash while deserializing a protocol conformance out of a sibling module's
// `.swiftmodule` — `ProtocolConformanceDeserializer::
// readNormalProtocolConformanceXRef` — which took down the `v0.76.0-beta.1`
// release build of `RupuShell` (the target that gained `RupuBuilder` in #625,
// giving it the widest cross-module fan-in in the package).
//
// The crash is Release-only, so the PR gate never sees it: `make macos-test`
// and CI's `macos-app` job build Debug, where CMO is off. Only
// `make macos-release` hits it.
//
// Disabling CMO costs some cross-module inlining and nothing else — it is not
// a semantic change. Applied to every library target rather than just
// `RupuShell` so a later dependency edge cannot silently reintroduce the
// crash in a different module, at the cost of another release cycle to find
// out. Revisit when the app's Xcode pin moves past 16.4.
let cmoOff: [SwiftSetting] = [.unsafeFlags(["-disable-cmo"], .when(configuration: .release))]

let package = Package(
    name: "RupuKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(
            name: "RupuKit",
            targets: [
                "RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuActivity", "RupuRunDetail", "RupuLauncher", "RupuOverview",
                "RupuProjects", "RupuFleet", "RupuLibrary", "RupuSecurity", "RupuUsageKit", "RupuUsage", "RupuShell", "RupuMenuBar",
                "RupuSituation", "RupuFlowKit", "RupuBuilder",
            ]
        )
    ],
    dependencies: [
        // The app's FIRST third-party Swift dependency, under an explicit carve-out (matt,
        // 2026-08-25) — see CLAUDE.md's macOS rule 4. Exact-pinned to 3.1.0 (latest tagged
        // release at the time this was added): a Swift wrapper bundling highlight.js, MIT/
        // BSD-family licensed, with no dependencies of its own. Used by `RupuRunDetail`'s
        // `CodeHighlighter`/`CodeBlock` (`Rendering/CodeBlock.swift`) for transcript code-block
        // syntax highlighting.
        .package(url: "https://github.com/smittytone/HighlighterSwift", exact: "3.1.0")
    ],
    targets: [
        .target(name: "RupuAPI", swiftSettings: cmoOff),
        .target(name: "RupuBackend", dependencies: ["RupuAPI"], swiftSettings: cmoOff),
        // Workflow Builder (macOS design plan) Task 1: pure ordered
        // YAML-value tree — no dependencies, mirrors `RupuUsageKit`'s
        // no-deps shape. Everything else in the Builder module tree
        // (`RupuBuilder`) depends on this for the workflow document model.
        .target(name: "RupuFlowKit", swiftSettings: cmoOff),
        .target(name: "RupuDesign", exclude: ["Icons/svg"], swiftSettings: cmoOff),
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
        .target(name: "RupuUsageKit", dependencies: ["RupuAPI"], swiftSettings: cmoOff),
        // Phase 6B, Task 6: Situation Room's pure derivations (line-by-line
        // Swift port of `crates/rupu-cp/web/src/lib/situationRoom/{cards,
        // roster}.ts`) — a pure aggregation module in the same spirit as
        // `RupuUsageKit` above: depends only on `RupuAPI` (`CPEvent`,
        // `CPEventRow`, `APIFinding`, `APIProjectRow`) + `RupuDesign`
        // (`Severity`), never on `RupuStore`, so `RupuStore` (Task 7's
        // Situation Room store) can depend on this without a cycle.
        // Phase 6B, Task 7: gains `RupuStore` on top of Task 6's `RupuAPI`/
        // `RupuDesign` pair — the screen layer (`SituationRoomScreen` and
        // friends) needs `SituationStore`/`BackendController`/`AppModel`/
        // `Route`/`PendingActions`/`ActionKey`, and `SituationAssembly.swift`
        // needs `SituationStore`'s raw wire-type snapshot to fold. One
        // direction only — `RupuStore` does NOT depend on `RupuSituation`
        // back; see `RupuStore/SituationStore.swift`'s doc comment for why
        // (a `RupuStore` -> `RupuSituation` edge would cycle the graph) and
        // how its own `eventsPerMin`/`spark` avoid needing this module's
        // `EventRateRing` as a result.
        .target(name: "RupuSituation", dependencies: ["RupuAPI", "RupuDesign", "RupuStore"], swiftSettings: cmoOff),
        .target(name: "RupuStore", dependencies: ["RupuAPI", "RupuBackend", "RupuDesign", "RupuUsageKit"], swiftSettings: cmoOff),
        .target(name: "RupuActivity", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
        .target(
            name: "RupuRunDetail",
            dependencies: [
                "RupuAPI", "RupuStore", "RupuDesign",
                .product(name: "Highlighter", package: "HighlighterSwift"),
            ],
            swiftSettings: cmoOff
        ),
        .target(name: "RupuLauncher", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
        .target(name: "RupuOverview", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
        .target(name: "RupuProjects", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
        .target(name: "RupuFleet", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
        .target(name: "RupuLibrary", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
        .target(name: "RupuSecurity", dependencies: ["RupuAPI", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
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
        .target(name: "RupuUsage", dependencies: ["RupuAPI", "RupuStore", "RupuDesign", "RupuUsageKit", "RupuOverview"], swiftSettings: cmoOff),
        // Workflow Builder (macOS design plan): originally a placeholder
        // target (Task 1) so the umbrella product + `RupuShell` could gain
        // the `RupuBuilder` dependency ahead of real content —
        // `BuilderStore` (round-trip core, using the pre-wired `RupuStore`
        // dependency for `PendingActions`/`ActionKey`) landed Task 9; the
        // canvas UI lands in a later task.
        .target(name: "RupuBuilder", dependencies: ["RupuAPI", "RupuStore", "RupuDesign", "RupuFlowKit"], swiftSettings: cmoOff),
        .target(
            name: "RupuShell",
            dependencies: [
                "RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuActivity", "RupuRunDetail", "RupuLauncher", "RupuOverview",
                "RupuProjects", "RupuFleet", "RupuLibrary", "RupuSecurity", "RupuUsage", "RupuBuilder",
            ],
            swiftSettings: cmoOff
        ),
        // Task 8: the `MenuBarExtra` popover (`MenuBarStore` + `MenuBarView`)
        // reuses `deriveNeedsYou`/`NeedsYouItem` (not duplicated) — those
        // moved from `RupuOverview` to `RupuStore/NeedsYou.swift` (perf &
        // interaction arc, Plan 5 Task 4, for `RupuActivity`'s own stats
        // surface to reuse them too without a `RupuActivity`→`RupuOverview`
        // edge), so this target no longer needs `RupuOverview` at all — just
        // the usual `RupuAPI`/`RupuStore`/`RupuDesign` trio every other
        // screen module depends on.
        .target(name: "RupuMenuBar", dependencies: ["RupuAPI", "RupuBackend", "RupuStore", "RupuDesign"], swiftSettings: cmoOff),
        .testTarget(name: "RupuAPITests", dependencies: ["RupuAPI"]),
        .testTarget(name: "RupuBackendTests", dependencies: ["RupuBackend", "RupuAPI"]),
        .testTarget(name: "RupuDesignTests", dependencies: ["RupuDesign"]),
        .testTarget(name: "RupuUsageKitTests", dependencies: ["RupuUsageKit", "RupuAPI"]),
        .testTarget(name: "RupuSituationTests", dependencies: ["RupuSituation", "RupuAPI", "RupuDesign"]),
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
        .testTarget(
            name: "RupuMenuBarTests",
            dependencies: ["RupuMenuBar", "RupuAPI", "RupuStore", "RupuBackend", "RupuDesign"]
        ),
        // Phase 6B, Task 3: `ClaimsTableTests` — the first test file for
        // `RupuActivity`'s own View-member pure logic (`ClaimsTable`'s
        // static seams), hence the first `RupuActivityTests` target.
        .testTarget(
            name: "RupuActivityTests",
            dependencies: ["RupuActivity", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
        // Redesign-pass Task 5: the first test file for `RupuLibrary`'s own
        // View-member pure logic (`LibraryScreen`'s `agentSortValue`/
        // `workflowSortValue`/`autoflowSortValue` seams), hence the first
        // `RupuLibraryTests` target — same "one test target per screen
        // module, `@testable import` reaching the module-internal seams"
        // convention every sibling screen target above already follows.
        .testTarget(
            name: "RupuLibraryTests",
            dependencies: ["RupuLibrary", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
        .testTarget(name: "RupuFlowKitTests", dependencies: ["RupuFlowKit"]),
        .testTarget(
            name: "RupuBuilderTests",
            dependencies: ["RupuBuilder", "RupuFlowKit", "RupuAPI", "RupuStore", "RupuDesign"]
        ),
    ]
)
