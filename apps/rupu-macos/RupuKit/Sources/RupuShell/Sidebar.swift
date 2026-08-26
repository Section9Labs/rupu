import SwiftUI
import RupuStore
import RupuBackend
import RupuDesign

/// Persisted expand/collapse state for the sidebar's four disclosure groups
/// (Activity/Security/Library/Fleet — Task 0). `Codable` struct encoded as
/// `Data` under one `@AppStorage` key, same "Data via AppStorage, plain
/// Codable struct with `decode(_:)`/`encoded` helpers" recipe
/// `RupuOverview.OverviewWidgets` establishes — kept a plain struct (not a
/// `View` member) so tests touching it never need `@MainActor` (CI's
/// isolation-inference rule only bites `View`-type members).
///
/// Each field is `Bool?`, not `Bool` — `nil` means "never explicitly
/// toggled," which is what lets `isExpanded(_:defaultOpen:)` fall through to
/// "open while this group contains the active route" for a group the
/// operator hasn't touched, while an explicit `true`/`false` always wins
/// once they've clicked a chevron (an explicit collapse survives navigating
/// away and back, per the brief's disclosure-state contract).
struct SidebarGroups: Codable, Equatable {
    var activity: Bool?
    var security: Bool?
    var library: Bool?
    var fleet: Bool?

    static let storageKey = "sidebar.groups"

    static func decode(_ data: Data) -> SidebarGroups {
        (try? JSONDecoder().decode(SidebarGroups.self, from: data)) ?? SidebarGroups()
    }

    var encoded: Data {
        (try? JSONEncoder().encode(self)) ?? Data()
    }

    /// `nil` (never toggled) resolves to `defaultOpen`; an explicit prior
    /// toggle always wins over it.
    func isExpanded(_ item: SidebarItem, defaultOpen: Bool) -> Bool {
        switch item {
        case .activity: activity ?? defaultOpen
        case .security: security ?? defaultOpen
        case .library: library ?? defaultOpen
        case .fleet: fleet ?? defaultOpen
        default: false
        }
    }

    mutating func setExpanded(_ item: SidebarItem, _ value: Bool) {
        switch item {
        case .activity: activity = value
        case .security: security = value
        case .library: library = value
        case .fleet: fleet = value
        default: break
        }
    }
}

/// Hover-tracking identity for a disclosure group's child row — pairs the
/// parent `SidebarItem` with the child's own value (type-erased via
/// `AnyHashable`, since `childRow(_:)` is generic over whichever tab/kind
/// enum a given parent carries) so two different parents' same-named child
/// (there are none today, but nothing stops it) can never collide.
private struct ChildKey: Hashable {
    let item: SidebarItem
    let child: AnyHashable
}

/// The app's primary navigation surface: the v2 rail (flows-composition
/// Task 1; disclosure sub-items restored by the design-alignment amendment's
/// Task 0) — fixed 204pt column, a custom row list (no `List`/`.sidebar`
/// vibrancy, no system-blue selection), a pinned Settings row, and a
/// host-status footer.
///
/// **Task 0 restores sub-item reachability**: Activity/Security/Library/
/// Fleet each expand into per-tab child rows (`RunKindFilter`/`SecurityTab`/
/// `LibraryTab`/`FleetTab` respectively — see `disclosureGroup(_:children:
/// activeChild:label:onSelectChild:)`) rather than leaving kind/tab
/// selection to live only inside each screen's own segmented control.
/// `.overview`/`.projects`/`.usage` have no children and keep the old plain
/// `railRow(_:)` chrome unchanged.
struct Sidebar: View {
    @Bindable var model: AppModel
    let hostsFooter: HostsFooterStore

    /// `AnyHashable` (not `SidebarItem?`) because a hovered row can now be
    /// either a parent (`SidebarItem`) or a child (`ChildKey`) — one shared
    /// key space keeps `railRow`/`groupParentRow`/`childRow` using the exact
    /// same hover-tracking idiom instead of three parallel `@State` fields.
    @State private var hovered: AnyHashable?

    /// Persisted expand/collapse state for the four disclosure groups —
    /// `SidebarGroups.decode(_:)` never throws (an empty/corrupt blob just
    /// reads back as "nothing explicitly toggled yet"), so every group's
    /// open/closed state falls through to `defaultOpen` (contains the active
    /// route) until the operator actually clicks a chevron.
    @AppStorage(SidebarGroups.storageKey) private var groupsData: Data = Data()

    private var groups: SidebarGroups { SidebarGroups.decode(groupsData) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            brandHeader
            nav
            Spacer(minLength: 0)
            SettingsLink { railLabel("Settings", icon: .settings, active: false) }
                .buttonStyle(.plain)
                .padding(.horizontal, 8)
            footer
        }
        .frame(width: 204)
        .background(Color.rupuPanel)
        .overlay(alignment: .trailing) { Color.rupuBorder.frame(width: 1) }
    }

    /// App-name header row — height 48, bottom border, same row rhythm as
    /// `Shell.tsx:95`'s rail header. Redesign-pass fix (audit A3): the bare
    /// bold "rupu" wordmark this row used to render was missing the badge +
    /// subtitle the audit found on the LIVE web CP (`127.0.0.1:7420`,
    /// unflagged — `getShell()` in `crates/rupu-cp/web/src/lib/shell.ts`
    /// defaults to `'v1'` absent a `[ui.cp] shell = "v2"` config override or
    /// a dev-only `localStorage` opt-in, so a stock `rupu cp serve` serves
    /// the v1 `Layout` shell, not `ShellV2`'s bare-wordmark rail). That v1
    /// shell renders `<Brand />` at its default variant
    /// (`crates/rupu-cp/web/src/components/Brand.tsx:29-43`, mounted via
    /// `crates/rupu-cp/web/src/components/Layout.tsx:32`) — a 28px
    /// (`h-7 w-7`) `rounded-md` (6px radius) tile, SOLID `bg-brand-600`
    /// (not `ShellV2`'s gradient rail tile), holding a white 15px
    /// font-light `&#8734;` glyph, next to a two-line wordmark: "rupu"
    /// (14px/`text-sm` semibold, `text-ink`) over "Control Plane"
    /// (11px/`text-[11px]`, `text-ink-mute`). Ported verbatim below, kept
    /// inside the row's existing 48pt height (the v1 header's own `py-5`
    /// padding doesn't fit this app's denser v2 rail rhythm, but the badge
    /// + two text lines fit the 48pt row with room to spare).
    private var brandHeader: some View {
        HStack(spacing: 8) {
            brandBadge
            VStack(alignment: .leading, spacing: 0) {
                Text("rupu")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(Color.rupuInk)
                Text("Control Plane")
                    .font(.system(size: 11))
                    .foregroundStyle(Color.rupuMute)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .frame(height: 48)
        .overlay(alignment: .bottom) { Color.rupuBorder.frame(height: 1) }
    }

    /// The badge itself — `Brand.tsx`'s default-variant tile
    /// (`h-7 w-7 rounded-md bg-brand-600` + a centered 15px font-light
    /// `&#8734;` glyph in white). `rupuBrand600` is the exact RGB pair
    /// `--c-brand-600` resolves to (`Tokens.swift`'s doc comment — ported
    /// verbatim from `styles.css`), so this is a byte-match to the web's
    /// `bg-brand-600`, not an approximation.
    private var brandBadge: some View {
        RoundedRectangle(cornerRadius: 6)
            .fill(Color.rupuBrand600)
            .frame(width: 28, height: 28)
            .overlay {
                Text("\u{221E}")
                    .font(.system(size: 15, weight: .light))
                    .foregroundStyle(.white)
            }
    }

    /// Fixed row order — `.activity`/`.security`/`.library`/`.fleet` render
    /// as disclosure groups (chevron + children); the rest keep the plain
    /// `railRow(_:)` chrome.
    private var nav: some View {
        VStack(spacing: 2) {
            railRow(.overview)
            disclosureGroup(
                .activity, children: RunKindFilter.allCases, activeChild: activityActiveKind(),
                label: activityChildLabel, onSelectChild: { model.route = .activity($0) }
            )
            railRow(.projects)
            disclosureGroup(
                .security, children: SecurityTab.allCases, activeChild: securityActiveTab(),
                label: \.title, onSelectChild: { model.route = .security($0) }
            )
            disclosureGroup(
                .library, children: LibraryTab.allCases, activeChild: libraryActiveTab(),
                label: \.title, onSelectChild: { model.route = .library($0) }
            )
            disclosureGroup(
                .fleet, children: FleetTab.allCases, activeChild: fleetActiveTab(),
                label: \.title, onSelectChild: { model.route = .fleet($0) }
            )
            railRow(.usage)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 8)
    }

    /// A rail row: `Icon`, not `systemImage:`, per the v2 nav icon contract
    /// (`Icon.swift`'s `LucideIcon` table). Active state is surface fill +
    /// an inset 2px `rupuBrand` (brand-500) left accent, radius 5 — no
    /// shadows, no system-blue `List` selection.
    private func railRow(_ item: SidebarItem) -> some View {
        let active = model.selectedSidebarItem == item
        let key = AnyHashable(item)
        return Button {
            model.selectedSidebarItem = item
        } label: {
            railLabel(title(for: item), icon: icon(for: item), active: active)
                .background(active ? Color.rupuSurface : hovered == key ? Color.rupuSurfaceHover : .clear)
                .overlay(alignment: .leading) {
                    if active { Color.rupuBrand.frame(width: 2) }
                }
                .clipShape(RoundedRectangle(cornerRadius: 5))
        }
        .buttonStyle(.plain)
        .onHover { hovered = $0 ? key : (hovered == key ? nil : hovered) }
    }

    /// Shared row content — used by `railRow(_:)`, `groupParentRow(_:
    /// fullActive:tinted:expanded:)`, and the pinned Settings row (which
    /// never highlights or tints: both flags default `false` there, and it
    /// doesn't participate in `hovered`).
    ///
    /// `tinted` (Task 0): brand-colored text with no surface fill —
    /// distinct from `active`'s full highlight — used when a disclosure
    /// group's child is the one actually selected, so the parent row still
    /// reads as "you're in here somewhere" without visually competing with
    /// the child's own full highlight (web v1's "contains active" idiom).
    private func railLabel(_ title: String, icon: LucideIcon, active: Bool, tinted: Bool = false) -> some View {
        HStack(spacing: 8) {
            Icon(icon, size: 15)
            Text(title).font(.leadText)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(height: 30)
        .foregroundStyle(tinted ? Color.rupuBrand : (active ? Color.rupuInk : Color.rupuDim))
    }

    // MARK: - Disclosure groups (Task 0)

    /// One parent + its expandable children. `activeChild` is the tab/kind
    /// `model.route` currently carries when this parent's `SidebarItem` is
    /// the active one (`nil` when we're on a pushed detail route reached
    /// from this section — e.g. a run/session/coverage/definition detail —
    /// which carries no tab of its own).
    ///
    /// Highlight split: when a child IS exactly active, the FULL highlight
    /// (surface fill + accent) moves to that child row and the parent gets
    /// only the brand tint (`railLabel`'s `tinted`); when nothing more
    /// specific is active (a pushed detail route, or this section simply
    /// isn't current), the parent behaves exactly like a plain `railRow`.
    ///
    /// Expand state: `groups.isExpanded(item, defaultOpen:)` defaults a
    /// never-toggled group open exactly while it contains the active route
    /// — the same "contains active" signal that drives the tint above —
    /// and stays whatever the operator last set it to otherwise (an
    /// explicit collapse survives navigating away and back).
    private func disclosureGroup<Child: Hashable>(
        _ item: SidebarItem,
        children: [Child],
        activeChild: Child?,
        label: @escaping (Child) -> String,
        onSelectChild: @escaping (Child) -> Void
    ) -> some View {
        let containsActive = model.selectedSidebarItem == item
        let hasActiveChild = containsActive && activeChild != nil
        let expanded = groups.isExpanded(item, defaultOpen: containsActive)
        return VStack(spacing: 2) {
            groupParentRow(item, fullActive: containsActive && !hasActiveChild, tinted: hasActiveChild, expanded: expanded)
            if expanded {
                ForEach(children, id: \.self) { child in
                    childRow(
                        parent: item, child: child, label: label(child),
                        active: containsActive && activeChild == child,
                        onSelect: { onSelectChild(child) }
                    )
                }
            }
        }
    }

    /// The parent row itself: a disclosure chevron (toggles `groups`,
    /// persisted, never navigates) beside the ordinary `railLabel` button
    /// (navigates to `item`'s default tab via `AppModel.selectedSidebarItem`
    /// — per the design-alignment amendment's ruling, this always resets to
    /// the fixed default, it does NOT restore whichever tab was last
    /// showing, unlike `.activity`'s own `lastActivityRoute` restore).
    /// Clicking the label never changes `expanded` either way — "parent row
    /// click navigates to the default tab AND keeps its expand state."
    private func groupParentRow(_ item: SidebarItem, fullActive: Bool, tinted: Bool, expanded: Bool) -> some View {
        let key = AnyHashable(item)
        return HStack(spacing: 0) {
            Button {
                var next = groups
                next.setExpanded(item, !expanded)
                groupsData = next.encoded
            } label: {
                Icon(.chevronDown, size: 10, weight: 2.5)
                    .foregroundStyle(Color.rupuMute)
                    .rotationEffect(.degrees(expanded ? 0 : -90))
                    .frame(width: 16, height: 30)
            }
            .buttonStyle(.plain)

            Button {
                model.selectedSidebarItem = item
            } label: {
                railLabel(title(for: item), icon: icon(for: item), active: fullActive, tinted: tinted)
            }
            .buttonStyle(.plain)
        }
        .background(fullActive ? Color.rupuSurface : hovered == key ? Color.rupuSurfaceHover : .clear)
        .overlay(alignment: .leading) {
            if fullActive { Color.rupuBrand.frame(width: 2) }
        }
        .clipShape(RoundedRectangle(cornerRadius: 5))
        .onHover { hovered = $0 ? key : (hovered == key ? nil : hovered) }
    }

    /// A child row: same 30pt `railRow` chrome, indented one level (+16pt
    /// leading beyond the rail's own 8pt horizontal inset) — no icon (the
    /// parent's icon already identifies the section; a second icon per
    /// child would be visual noise this rail doesn't otherwise carry).
    private func childRow<Child: Hashable>(
        parent: SidebarItem, child: Child, label: String, active: Bool, onSelect: @escaping () -> Void
    ) -> some View {
        let key = AnyHashable(ChildKey(item: parent, child: AnyHashable(child)))
        return Button(action: onSelect) {
            HStack(spacing: 8) {
                Text(label).font(.leadText)
                Spacer(minLength: 0)
            }
            .padding(.leading, 24)
            .padding(.trailing, 8)
            .frame(height: 30)
            .foregroundStyle(active ? Color.rupuInk : Color.rupuDim)
            .background(active ? Color.rupuSurface : hovered == key ? Color.rupuSurfaceHover : .clear)
            .overlay(alignment: .leading) {
                if active { Color.rupuBrand.frame(width: 2) }
            }
            .clipShape(RoundedRectangle(cornerRadius: 5))
        }
        .buttonStyle(.plain)
        .onHover { hovered = $0 ? key : (hovered == key ? nil : hovered) }
    }

    /// Which `RunKindFilter`/`SecurityTab`/`LibraryTab`/`FleetTab` `model.
    /// route` currently carries for a given parent — `nil` when `route` is
    /// some OTHER section entirely, or a pushed detail route reached from
    /// this one (no tab of its own). Callers additionally gate on
    /// `model.selectedSidebarItem == item` before trusting a non-`nil`
    /// result as "this child is the active one" — see `disclosureGroup`'s
    /// `hasActiveChild`.
    private func activityActiveKind() -> RunKindFilter? {
        if case .activity(let kind) = model.route { return kind }
        return nil
    }

    private func securityActiveTab() -> SecurityTab? {
        if case .security(let tab) = model.route { return tab }
        return nil
    }

    private func libraryActiveTab() -> LibraryTab? {
        if case .library(let tab) = model.route { return tab }
        return nil
    }

    private func fleetActiveTab() -> FleetTab? {
        if case .fleet(let tab) = model.route { return tab }
        return nil
    }

    /// Activity's child labels are hand-written, not `RunKindFilter.
    /// screenTitle` (`RouteDisplay.swift`) — that mapping's `.all: "Activity"`
    /// duplicates the parent row's own title; "All runs" reads correctly as
    /// a child alongside "Agents"/"Workflows"/"Autoflows"/"Sessions".
    private func activityChildLabel(_ kind: RunKindFilter) -> String {
        switch kind {
        case .all: "All runs"
        case .agents: "Agents"
        case .workflows: "Workflows"
        case .autoflows: "Autoflows"
        case .sessions: "Sessions"
        }
    }

    private func icon(for item: SidebarItem) -> LucideIcon {
        switch item {
        case .overview: .layoutDashboard
        case .activity: .activity
        case .projects: .folderGit2
        case .security: .shieldCheck
        case .library: .bookMarked
        case .fleet: .server
        case .usage: .dollarSign
        }
    }

    private func title(for item: SidebarItem) -> String {
        switch item {
        case .overview: "Overview"
        case .activity: "Activity"
        case .projects: "Projects"
        case .security: "Security"
        case .library: "Library"
        case .fleet: "Fleet"
        case .usage: "Usage"
        }
    }

    /// Backend-health line (unchanged from the pre-v2 sidebar footer), the
    /// live event-stream line (moved here from the toolbar — a passive
    /// status had no honest chrome in a row of buttons; the footer is the
    /// app's status block), and the host-fleet line driven by
    /// `HostsFooterStore`.
    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            healthLine
            liveLine
            hostsLine
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .overlay(alignment: .top) { Color.rupuBorder.frame(height: 1) }
    }

    private var healthLine: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(healthDotColor)
                .frame(width: 6, height: 6)
            Text(healthLabel)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
        }
    }

    private var healthDotColor: Color {
        switch model.backendHealth {
        case .healthy: .status(.done)
        case .degraded: .status(.awaiting)
        case .down, .incompatible: .status(.failed)
        case .starting: .status(.paused)
        }
    }

    private var healthLabel: String {
        switch model.backendHealth {
        case .starting: "Starting"
        case .healthy(let version): "Connected \u{2013} \(version)"
        case .degraded: "Degraded"
        case .down: "Offline"
        case .incompatible: "Incompatible"
        }
    }

    /// Live event-stream status, driven by `model.liveConnected` (set by
    /// `BackendController.onLiveConnectionChange` — connection-level, not
    /// frame-level; see `RootView.init`). The dot is brand purple, not the
    /// footer's green/red status tones: purple is the app's "live"
    /// identity (the Activity screen's Live-tail toggle), and it keeps
    /// this line visually distinct from the backend-health line above.
    private var liveLine: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(model.liveConnected ? Color.rupuBrand700 : Color.rupuMute)
                .frame(width: 6, height: 6)
            Text(model.liveConnected ? "Live events" : "Events offline")
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
        }
        .help(model.liveConnected ? "Event stream connected" : "Event stream disconnected")
    }

    /// `"N hosts"`, `" · M down"` appended when any host isn't `"online"` —
    /// `HostsFooterStore.summary == nil` (never polled yet, or the backend
    /// isn't healthy) renders a pending-tone dot and `"— hosts"` instead of
    /// guessing.
    private var hostsLine: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(hostsDotColor)
                .frame(width: 6, height: 6)
            Text(hostsLabel)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
            Spacer(minLength: 0)
        }
    }

    private var hostsDotColor: Color {
        guard let summary = hostsFooter.summary else { return .status(.pending) }
        return .status(summary.down == 0 ? .done : .failed)
    }

    private var hostsLabel: String {
        guard let summary = hostsFooter.summary else { return "\u{2014} hosts" }
        let base = "\(summary.total) hosts"
        return summary.down > 0 ? "\(base) \u{b7} \(summary.down) down" : base
    }
}
