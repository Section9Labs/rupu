import SwiftUI
import RupuStore
import RupuBackend
import RupuDesign

/// The app's primary navigation surface: the v2 flat rail (flows-composition
/// Task 1) — fixed 204pt column, a custom row list (no `List`/`.sidebar`
/// vibrancy, no system-blue selection), a pinned Settings row, and a
/// host-status footer. The old "Runs" section (one row per `RunKindFilter`)
/// collapsed to the single `.activity` row; kind selection now lives
/// entirely in the Activity screen's own `FilterBar`.
struct Sidebar: View {
    @Bindable var model: AppModel
    let hostsFooter: HostsFooterStore

    @State private var hovered: SidebarItem?

    private let railItems: [SidebarItem] = [
        .overview, .activity, .projects, .security, .library, .fleet, .usage,
    ]

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

    /// App-name header row, matching `Shell.tsx:95`'s rail header — height
    /// 48, bottom border.
    private var brandHeader: some View {
        HStack(spacing: 8) {
            Text("rupu")
                .font(.leadText)
                .fontWeight(.semibold)
                .foregroundStyle(Color.rupuInk)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .frame(height: 48)
        .overlay(alignment: .bottom) { Color.rupuBorder.frame(height: 1) }
    }

    private var nav: some View {
        VStack(spacing: 2) {
            ForEach(railItems, id: \.self) { item in railRow(item) }
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
        return Button {
            model.selectedSidebarItem = item
        } label: {
            railLabel(title(for: item), icon: icon(for: item), active: active)
                .background(active ? Color.rupuSurface : hovered == item ? Color.rupuSurfaceHover : .clear)
                .overlay(alignment: .leading) {
                    if active { Color.rupuBrand.frame(width: 2) }
                }
                .clipShape(RoundedRectangle(cornerRadius: 5))
        }
        .buttonStyle(.plain)
        .onHover { hovered = $0 ? item : (hovered == item ? nil : hovered) }
    }

    /// Shared row content — used by both `railRow(_:)` and the pinned
    /// Settings row (which never highlights: `active` is always `false`
    /// there, and it doesn't participate in `hovered`).
    private func railLabel(_ title: String, icon: LucideIcon, active: Bool) -> some View {
        HStack(spacing: 8) {
            Icon(icon, size: 15)
            Text(title).font(.leadText)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(height: 30)
        .foregroundStyle(active ? Color.rupuInk : Color.rupuDim)
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

    /// Backend-health line (unchanged from the pre-v2 sidebar footer) plus
    /// a new host-fleet line driven by `HostsFooterStore`.
    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            healthLine
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
