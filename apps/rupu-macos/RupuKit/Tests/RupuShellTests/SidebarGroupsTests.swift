import Testing
import Foundation
@testable import RupuShell
import RupuStore

/// `SidebarGroups` (Task 0, sidebar disclosure sub-items) — the persisted
/// expand/collapse contract behind the rail's four disclosure groups. A
/// plain `Codable`/`Equatable` struct, not a `View` member, so no
/// `@MainActor` is needed (CI's isolation-inference rule only bites tests
/// touching `View`-type members — same reasoning `OverviewWidgets`'
/// `WidgetConfigTests` document for themselves).
struct SidebarGroupsTests {
    /// Every parent `SidebarItem` a disclosure group exists for — the enum
    /// cases `isExpanded(_:defaultOpen:)`/`setExpanded(_:_:)` actually
    /// switch over ( `.overview`/`.projects`/`.usage` fall to the
    /// no-children default arm, covered separately below). An instance
    /// property, not `static` — `SidebarItem` isn't `Sendable`, so a static
    /// stored array trips Swift 6's global-mutable-state check.
    private let groupItems: [SidebarItem] = [.activity, .security, .library, .fleet]

    // (a) The brief's disclosure-state default: a never-toggled group
    // (field still `nil`) falls back to whatever `defaultOpen` says —
    // "defaulting open when the group contains the active route" is the
    // caller passing `defaultOpen: true`; not containing it is `false`.
    @Test func unsetGroupFallsBackToDefaultOpenBothWays() {
        let groups = SidebarGroups()
        for item in groupItems {
            #expect(groups.isExpanded(item, defaultOpen: true))
            #expect(!groups.isExpanded(item, defaultOpen: false))
        }
    }

    // (b) An explicit toggle always wins over `defaultOpen`, in both
    // directions: an explicit collapse stays collapsed even while the group
    // contains the active route (`defaultOpen: true`), and an explicit
    // expand stays expanded even when it doesn't (`defaultOpen: false`).
    @Test func explicitToggleWinsOverDefaultOpenInBothDirections() {
        for item in groupItems {
            var groups = SidebarGroups()

            groups.setExpanded(item, false)
            #expect(!groups.isExpanded(item, defaultOpen: true), "explicit collapse must beat defaultOpen: true")
            #expect(!groups.isExpanded(item, defaultOpen: false))

            groups.setExpanded(item, true)
            #expect(groups.isExpanded(item, defaultOpen: false), "explicit expand must beat defaultOpen: false")
            #expect(groups.isExpanded(item, defaultOpen: true))
        }
    }

    // (c) `setExpanded` overwrites a prior explicit value — the operator
    // can toggle the same chevron repeatedly and the latest click is what
    // persists (covered by (b)'s collapse-then-expand sequence per item,
    // pinned here as the full flip-flop on one group with the others left
    // untouched).
    @Test func setExpandedOverwritesAPriorValueAndTouchesOnlyThatGroup() {
        var groups = SidebarGroups()
        groups.setExpanded(.security, true)
        groups.setExpanded(.security, false)
        #expect(!groups.isExpanded(.security, defaultOpen: true))

        // The other three groups are still unset — their defaultOpen
        // fall-through is untouched by Security's toggling.
        for item in groupItems where item != .security {
            #expect(groups.isExpanded(item, defaultOpen: true))
            #expect(!groups.isExpanded(item, defaultOpen: false))
        }
    }

    // (d) Round-trip through the exact `Data` encoding the `@AppStorage`
    // backing uses (`encoded` → `decode(_:)`): explicit values and unset
    // `nil`s both survive — a `nil` must NOT come back as an explicit
    // `false` (that would permanently defeat the defaultOpen fall-through
    // for groups the operator never touched).
    @Test func codableRoundTripPreservesExplicitValuesAndUnsetNils() {
        var groups = SidebarGroups()
        groups.setExpanded(.activity, false)
        groups.setExpanded(.library, true)
        // .security / .fleet deliberately left unset.

        let reloaded = SidebarGroups.decode(groups.encoded)
        #expect(reloaded == groups)
        #expect(!reloaded.isExpanded(.activity, defaultOpen: true), "explicit collapse survives the round trip")
        #expect(reloaded.isExpanded(.library, defaultOpen: false), "explicit expand survives the round trip")
        #expect(reloaded.isExpanded(.security, defaultOpen: true), "unset stays unset — defaultOpen still applies")
        #expect(!reloaded.isExpanded(.fleet, defaultOpen: false), "unset stays unset — defaultOpen still applies")
    }

    // (e) The `@AppStorage` declaration's initial value is `Data()` (nothing
    // ever saved) — `decode(_:)` must fall back to the all-unset default
    // rather than crashing or fabricating explicit states. Corrupt blobs
    // take the same path.
    @Test func decodeOfEmptyOrCorruptDataFallsBackToAllUnset() {
        #expect(SidebarGroups.decode(Data()) == SidebarGroups())
        #expect(SidebarGroups.decode(Data("not json".utf8)) == SidebarGroups())
    }

    // (f) Items without a disclosure group (`.overview`/`.projects`/
    // `.usage`) are never expanded and ignore `setExpanded` — the
    // `default:` arms in both methods, pinned so a future refactor can't
    // silently give a childless row phantom expand state.
    @Test func nonGroupItemsAreNeverExpandedAndIgnoreSetExpanded() {
        var groups = SidebarGroups()
        for item in [SidebarItem.overview, .projects, .usage] {
            #expect(!groups.isExpanded(item, defaultOpen: true))
            groups.setExpanded(item, true)
            #expect(!groups.isExpanded(item, defaultOpen: true))
        }
        #expect(groups == SidebarGroups(), "setExpanded on a non-group item must not mutate anything")
    }
}
