import Foundation
import Testing
@testable import RupuOverview

/// `OverviewWidgets` is a plain `Codable` struct (CI rule: only tests
/// touching a `View`-type member need `@MainActor`) — these exercise the
/// actual `UserDefaults`-backed seam (`load(defaults:)`/`save(defaults:)`),
/// injected via a fresh named suite per test the same way `AppModelTests`
/// (`RoutingTests.swift`) verifies `AppModel`'s own persisted fields, rather
/// than only round-tripping `encode`/`decode` in memory.
@Suite struct WidgetConfigTests {
    @Test func absentKeyDefaultsAllVisible() {
        let suite = "test-\(UUID())"
        let d = UserDefaults(suiteName: suite)!

        let loaded = OverviewWidgets.load(defaults: d)

        #expect(loaded == OverviewWidgets(needsYou: true, instruments: true, charts: true, cycles: true, fleet: true))
    }

    @Test func roundTripsThroughUserDefaults() {
        let suite = "test-\(UUID())"
        let d = UserDefaults(suiteName: suite)!

        var widgets = OverviewWidgets.load(defaults: d)
        widgets.instruments = false
        widgets.fleet = false
        widgets.save(defaults: d)

        // A fresh read (not the mutated in-memory value) is what proves the
        // round trip — same "second instance over the same suite" idiom
        // `rangeAndScopePersistAcrossAppModelInstances` uses for `AppModel`.
        let reloaded = OverviewWidgets.load(defaults: d)

        #expect(reloaded == widgets)
        #expect(reloaded.instruments == false)
        #expect(reloaded.fleet == false)
        #expect(reloaded.needsYou == true)
        #expect(reloaded.charts == true)
        #expect(reloaded.cycles == true)
    }

    @Test func decodeToleratesCorruptData() {
        let suite = "test-\(UUID())"
        let d = UserDefaults(suiteName: suite)!
        d.set(Data("not json".utf8), forKey: OverviewWidgets.storageKey)

        let loaded = OverviewWidgets.load(defaults: d)

        #expect(loaded == OverviewWidgets())
    }

    @Test func encodeDecodeRoundTrip() {
        let widgets = OverviewWidgets(needsYou: false, instruments: true, charts: false, cycles: true, fleet: false)
        let decoded = OverviewWidgets.decode(widgets.encoded)
        #expect(decoded == widgets)
    }
}
