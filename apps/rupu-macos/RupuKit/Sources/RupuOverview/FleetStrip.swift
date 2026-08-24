import RupuAPI
import RupuDesign
import SwiftUI

/// The one line of aggregate cycle numbers beneath the throughput chart
/// (spec §5.5, ported from the web's `CycleSummaryLine`). Three scalars,
/// nothing more — the per-cycle detail and per-run drill-in belong to
/// `/runs`, not the dashboard. `cycles.clean`/`cycles.withFailures` are
/// `nil` when a reporting host can't supply the breakdown (SSH) — `Fmt.
/// count` renders that as an em dash, never a fabricated `0`; `partial`
/// marks the pair as a split over only the hosts that DID report it.
public struct CycleSummaryLine: View {
    private let cycles: APICycleCounts
    private let partial: Bool

    public init(cycles: APICycleCounts, partial: Bool) {
        self.cycles = cycles
        self.partial = partial
    }

    public var body: some View {
        HStack(spacing: 4) {
            figure(Fmt.count(cycles.total))
            label("cycles ·")
            figure(Fmt.count(cycles.clean))
            label("clean ·")
            figure(Fmt.count(cycles.withFailures))
            label("with failures")
            if partial {
                Text("(partial)")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
                    .help("Some reporting hosts do not supply the clean/failed breakdown — this is a partial split, not a fleet total.")
            }
            Spacer(minLength: 0)
        }
    }

    private func figure(_ text: String) -> some View {
        Text(text)
            .font(.dataMono(12))
            .foregroundStyle(Color.rupuInk)
    }

    private func label(_ text: String) -> some View {
        Text(text)
            .font(.uiText)
            .foregroundStyle(Color.rupuDim)
    }
}

/// The inventory band beneath the ops blocks (spec §1, ported from the
/// web's `FleetStrip`). Supporting context, not a second subject — dim by
/// default, and takes weight ONLY where something is actually wrong: today
/// that's exclusively a configured provider failing its last probe (`
/// providersUnhealthy > 0`). A count alone is never an alarm. Every field on
/// `APIFleetCounts` is optional for the "not reported ≠ 0" reason — `Fmt.
/// count` renders `nil` as an em dash throughout, never a fabricated zero;
/// `issuesCapped` renders the open-issues figure as `N+`.
public struct FleetStrip: View {
    private let fleet: APIFleetCounts
    private let partial: Bool

    public init(fleet: APIFleetCounts, partial: Bool) {
        self.fleet = fleet
        self.partial = partial
    }

    /// `nil` (no probe has run yet) is an absence of information, not a
    /// clean bill of health — it must not weight this segment.
    private var providersFault: Bool {
        (fleet.providersUnhealthy ?? 0) > 0
    }

    private var showDisabled: Bool {
        (fleet.autoflowsDisabled ?? 0) > 0
    }

    private var showOpen: Bool {
        fleet.issuesOpen != nil
    }

    public var body: some View {
        HStack(spacing: 8) {
            segment("\(Fmt.count(fleet.repos)) repos")
            dot
            segment(providersText, fault: providersFault)
            dot
            segment(autoflowsText)
            dot
            segment("\(Fmt.count(fleet.workers)) workers")
            dot
            segment("\(Fmt.count(fleet.claimsActive)) claimed")
            dot
            segment(issuesText)
            if partial {
                Text("(partial)")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
                    .help("Some reporting hosts do not supply these counts — this is a partial sum, not a fleet total.")
            }
            Spacer(minLength: 0)
        }
        .padding(.top, 8)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Color.rupuBorder)
                .frame(height: 1)
        }
    }

    private var providersText: String {
        var text = "\(Fmt.count(fleet.providersConfigured)) providers"
        if providersFault {
            text += " (\(Fmt.count(fleet.providersUnhealthy)) unhealthy)"
        }
        return text
    }

    private var autoflowsText: String {
        var text = "\(Fmt.count(fleet.autoflowsEnabled)) autoflows"
        if showDisabled {
            text += " (\(Fmt.count(fleet.autoflowsDisabled)) off)"
        }
        return text
    }

    private var issuesText: String {
        var text = "\(Fmt.count(fleet.issuesPending)) pending"
        if showOpen {
            let capped = fleet.issuesCapped ? "+" : ""
            text += " (\(Fmt.count(fleet.issuesOpen))\(capped) open)"
        }
        return text
    }

    private func segment(_ text: String, fault: Bool = false) -> some View {
        Text(text)
            .font(.metaText)
            .foregroundStyle(fault ? Color.rupuErr : Color.rupuMute)
    }

    private var dot: some View {
        Text("·")
            .font(.metaText)
            .foregroundStyle(Color.rupuMute)
    }
}
