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

    // MARK: - Order (Task 3: Customize drag/reorder)

    @Test func absentKeyDefaultsToDefaultOrder() {
        let suite = "test-\(UUID())"
        let d = UserDefaults(suiteName: suite)!

        let loaded = OverviewWidgets.load(defaults: d)

        #expect(loaded.order == OverviewWidgets.defaultOrder)
    }

    @Test func orderRoundTripsThroughUserDefaults() {
        let suite = "test-\(UUID())"
        let d = UserDefaults(suiteName: suite)!

        var widgets = OverviewWidgets.load(defaults: d)
        widgets.order = ["fleet", "needsYou", "charts", "instruments", "cycles"]
        widgets.save(defaults: d)

        let reloaded = OverviewWidgets.load(defaults: d)

        #expect(reloaded.order == ["fleet", "needsYou", "charts", "instruments", "cycles"])
    }

    @Test func encodedOrderRoundTripsThroughDecode() {
        var widgets = OverviewWidgets()
        widgets.order = ["cycles", "fleet", "charts", "instruments", "needsYou"]

        let decoded = OverviewWidgets.decode(widgets.encoded)

        #expect(decoded.order == widgets.order)
    }

    @Test func normalizedOrderAppendsMissingCanonicalIDsAtTheTail() {
        // "cycles" is a canonical id but absent here — forward-compat
        // (Task 3's binding rule) requires it survive, not vanish, appended
        // after everything actually persisted rather than spliced into the
        // middle.
        let normalized = OverviewWidgets.normalized(["fleet", "needsYou", "instruments"])

        #expect(normalized == ["fleet", "needsYou", "instruments", "charts", "cycles"])
    }

    @Test func normalizedOrderDropsUnrecognizedIDs() {
        // An id `defaultOrder` doesn't know (a retired block, or a
        // downgrade reading a newer install's data) is dropped rather than
        // reserving a permanent, unrenderable slot.
        let normalized = OverviewWidgets.normalized(["needsYou", "spendWidget", "fleet"])

        #expect(normalized == ["needsYou", "fleet", "instruments", "charts", "cycles"])
    }

    @Test func normalizedOrderDropsDuplicateIDs() {
        let normalized = OverviewWidgets.normalized(["needsYou", "needsYou", "fleet"])

        #expect(normalized == ["needsYou", "fleet", "instruments", "charts", "cycles"])
    }

    @Test func normalizedOrderIsIdempotentOnDefaultOrder() {
        #expect(OverviewWidgets.normalized(OverviewWidgets.defaultOrder) == OverviewWidgets.defaultOrder)
    }

    @Test func decodeOfDataMissingTheOrderKeyFallsBackToDefaultOrder() {
        // Exactly what every install's persisted blob looked like before
        // this task: a JSON object with only the five boolean fields, no
        // `order` key at all.
        let json = """
        {"needsYou":true,"instruments":false,"charts":true,"cycles":true,"fleet":false}
        """
        let decoded = OverviewWidgets.decode(Data(json.utf8))

        #expect(decoded.order == OverviewWidgets.defaultOrder)
        #expect(decoded.instruments == false)
        #expect(decoded.fleet == false)
    }

    @Test func decodeOfAPersistedOrderMissingANewerBlockIDAppendsIt() {
        // Forward-compat (Task 3's binding rule): a block id added in a
        // later release than the one that saved this order must still
        // render for this operator — appended at the tail, not vanished.
        let json = """
        {"needsYou":true,"instruments":true,"charts":true,"cycles":true,"fleet":true,
         "order":["fleet","needsYou","instruments","charts"]}
        """
        let decoded = OverviewWidgets.decode(Data(json.utf8))

        #expect(decoded.order == ["fleet", "needsYou", "instruments", "charts", "cycles"])
    }

    @Test func decodeOfAPersistedOrderWithAnUnrecognizedIDDropsIt() {
        let json = """
        {"needsYou":true,"instruments":true,"charts":true,"cycles":true,"fleet":true,
         "order":["needsYou","retiredWidget","fleet","instruments","charts","cycles"]}
        """
        let decoded = OverviewWidgets.decode(Data(json.utf8))

        #expect(decoded.order == ["needsYou", "fleet", "instruments", "charts", "cycles"])
    }
}
