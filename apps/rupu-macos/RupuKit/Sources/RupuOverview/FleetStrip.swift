import RupuAPI
import RupuDesign
import SwiftUI

// MARK: - Pure seam (the tested formatting)

/// The three formatted figures `CycleSummaryLine` renders — a plain,
/// non-View struct so `compute(cycles:partial:)`'s formatting is testable
/// without `@MainActor` (CI rule: only tests that touch a `View`-type member
/// need it; same pattern as `InstrumentValues`/`InstrumentStrip`).
public struct CycleSummaryFigures: Equatable, Sendable {
    public let total: String
    public let clean: String
    public let withFailures: String

    public init(total: String, clean: String, withFailures: String) {
        self.total = total
        self.clean = clean
        self.withFailures = withFailures
    }

    /// `total` is always a plain `Fmt.count` — it's a complete sum
    /// regardless of `partial` (see `DashboardStore.merge`'s doc comment).
    /// `clean`/`withFailures` are each a plain em dash when `nil` (nothing
    /// reported, nothing to mark as partial), otherwise `Fmt.partial`'s
    /// trailing `+` when `partial` is set — the final-review (Task 6)
    /// controller ruling: the null-discipline `+` marker governs the
    /// figures themselves, alongside (not instead of) the line's existing
    /// "(partial)" caption + tooltip.
    public static func compute(cycles: APICycleCounts, partial: Bool) -> CycleSummaryFigures {
        func figure(_ n: Int?) -> String {
            guard let n else { return Fmt.count(nil) }
            return partial ? Fmt.partial(n, isPartial: true) : Fmt.count(n)
        }
        return CycleSummaryFigures(
            total: Fmt.count(cycles.total),
            clean: figure(cycles.clean),
            withFailures: figure(cycles.withFailures)
        )
    }
}

// MARK: - View

/// The one line of aggregate cycle numbers beneath the throughput chart
/// (spec §5.5, ported from the web's `CycleSummaryLine`). Three scalars,
/// nothing more — the per-cycle detail and per-run drill-in belong to
/// `/runs`, not the dashboard. `cycles.clean`/`cycles.withFailures` are
/// `nil` when a reporting host can't supply the breakdown (SSH) — rendered
/// as an em dash regardless of `partial` (nothing to mark up on an absent
/// figure). `cycles.total` is always a complete sum (see `DashboardStore.
/// merge`'s doc comment — poisoning never touches it) and so never carries
/// the `+` marker either way.
///
/// Final-review fix (Task 6, controller ruling): when `partial` is set, the
/// clean/with-failures FIGURES themselves carry `Fmt.partial`'s trailing
/// `+` — the same null-discipline marker every other partial-sum figure in
/// this screen uses (`InstrumentStrip`'s open-findings tile, `FleetStrip`
/// below) — in addition to, not instead of, the existing "(partial)"
/// caption + tooltip. The two together, not either alone: the caption names
/// *which* figures are incomplete, the `+` marks each one at the point a
/// reader's eye actually lands on it.
public struct CycleSummaryLine: View {
    private let cycles: APICycleCounts
    private let partial: Bool

    public init(cycles: APICycleCounts, partial: Bool) {
        self.cycles = cycles
        self.partial = partial
    }

    public var body: some View {
        let figures = CycleSummaryFigures.compute(cycles: cycles, partial: partial)
        HStack(spacing: 4) {
            figure(figures.total)
            label("cycles ·")
            figure(figures.clean)
            label("clean ·")
            figure(figures.withFailures)
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
