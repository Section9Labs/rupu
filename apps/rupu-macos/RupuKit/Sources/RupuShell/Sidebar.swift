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
