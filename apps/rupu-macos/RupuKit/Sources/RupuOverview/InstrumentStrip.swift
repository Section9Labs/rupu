import RupuDesign
import RupuStore
import SwiftUI

// MARK: - Pure seam (the tested arithmetic)

/// The six formatted KPI strings `InstrumentStrip` renders — a plain,
/// non-View struct so `compute(from:)`'s arithmetic is testable without
/// `@MainActor` (CI rule: only tests that touch a `View`-type member need
/// it). Ported field-by-field from the web's `KeyPointTiles.tsx`, read at
/// implementation time:
/// - `awaiting`/`activeNow`/`paused` are plain sums off `MergedDashboard.
///   active` — every field there is non-optional on the wire, so there is
///   nothing to poison and no figure is ever partial-suffixed.
/// - `failedTotal`/`successRate` are derived from `terminalBuckets`, the
///   same series `FailedSparkline`/`OutcomesChart` render, so the sparkline
///   and this tile agree by construction. `successRate` is `nil` (renders
///   "—") when the terminal denominator (completed+failed+rejected+
///   cancelled, summed across every bucket) is zero — never a fabricated
///   `0%`.
/// - `openFindings` is the one figure that can carry a partial suffix:
///   `findingsOpen == nil` renders "—" (nobody reported, not "zero
///   findings"); otherwise `findingsPartial` suffixes a trailing `+` via
///   `Fmt.partial`.
public struct InstrumentValues: Equatable, Sendable {
    public let awaiting: String
    public let activeNow: String
    public let paused: String
    public let failedTotal: String
    public let successRate: String
    public let openFindings: String

    public init(
        awaiting: String,
        activeNow: String,
        paused: String,
        failedTotal: String,
        successRate: String,
        openFindings: String
    ) {
        self.awaiting = awaiting
        self.activeNow = activeNow
        self.paused = paused
        self.failedTotal = failedTotal
        self.successRate = successRate
        self.openFindings = openFindings
    }

    /// `merged == nil` (no host has reported anything yet — see
    /// `DashboardStore.merged`'s doc comment) renders every figure as "—",
    /// never a fabricated zero.
    public static func compute(from merged: MergedDashboard?) -> InstrumentValues {
        guard let merged else {
            return InstrumentValues(
                awaiting: "—", activeNow: "—", paused: "—",
                failedTotal: "—", successRate: "—", openFindings: "—"
            )
        }

        let failedTotal = merged.terminalBuckets.reduce(0) { $0 + $1.failed }
        let terminalTotal = merged.terminalBuckets.reduce(0) { $0 + $1.completed + $1.failed + $1.rejected + $1.cancelled }
        let completedTotal = merged.terminalBuckets.reduce(0) { $0 + $1.completed }

        let successRate: String
        if terminalTotal > 0 {
            let rate = Int((Double(completedTotal) / Double(terminalTotal) * 100).rounded())
            successRate = "\(rate)%"
        } else {
            successRate = "—"
        }

        let openFindings = merged.findingsOpen.map { Fmt.partial($0, isPartial: merged.findingsPartial) } ?? "—"

        return InstrumentValues(
            awaiting: Fmt.count(merged.active.awaitingApproval),
            activeNow: Fmt.count(merged.active.running),
            paused: Fmt.count(merged.active.paused),
            failedTotal: Fmt.count(failedTotal),
            successRate: successRate,
            openFindings: openFindings
        )
    }
}

// MARK: - View

/// Six equal cells answering the operator's glance-questions (spec §5.2,
/// ported from the web's `KeyPointTiles`): Awaiting you / Active now /
/// Paused / Failed (+ `FailedSparkline`) / Success rate / Open findings.
/// "Awaiting you" and "Paused" are the two states where nothing moves until
/// the operator acts, so — like the web — they (and Failed) take visual
/// weight (tone-colored border + bolder numeral) when nonzero; Active now,
/// Success rate, and Open findings never do.
public struct InstrumentStrip: View {
    private let merged: MergedDashboard?

    public init(merged: MergedDashboard?) {
        self.merged = merged
    }

    public var body: some View {
        let values = InstrumentValues.compute(from: merged)
        let active = merged?.active

        HStack(spacing: 10) {
            InstrumentCell(
                label: "Awaiting you",
                value: values.awaiting,
                weighted: (active?.awaitingApproval ?? 0) > 0,
                tone: .awaiting
            )
            InstrumentCell(
                label: "Active now",
                value: values.activeNow,
                weighted: false,
                tone: .running
            ) {
                if let longest = merged?.activeLongest {
                    Text("longest \(Fmt.duration(ms: UInt64(max(0, longest.ageMs)))) — \(longest.workflowName)")
                        .font(.metaText)
                        .foregroundStyle(Color.rupuDim)
                        .lineLimit(1)
                }
            }
            InstrumentCell(
                label: "Paused",
                value: values.paused,
                weighted: (active?.paused ?? 0) > 0,
                tone: .paused
            )
            InstrumentCell(
                label: "Failed",
                value: values.failedTotal,
                weighted: (merged?.terminalBuckets.reduce(0) { $0 + $1.failed } ?? 0) > 0,
                tone: .failed
            ) {
                FailedSparkline(buckets: merged?.terminalBuckets ?? [])
            }
            InstrumentCell(
                label: "Success rate",
                value: values.successRate,
                weighted: false,
                tone: .pending
            )
            InstrumentCell(
                label: "Open findings",
                value: values.openFindings,
                weighted: false,
                tone: .pending,
                captionSuffix: (merged?.findingsPartial ?? false) ? "(partial)" : nil
            )
        }
    }
}

/// One instrument-strip tile. Mirrors the web's `TileShell`: dim `rupuPanel`
/// background + `rupuBorder` stroke by default, or (when `weighted`)
/// `rupuSurface` background + a `tone`-colored stroke and a bolder numeral —
/// built by hand rather than via `PanelStyle` so the border color can vary
/// per state without stacking two strokes.
private struct InstrumentCell<Accessory: View>: View {
    let label: String
    let value: String
    let weighted: Bool
    let tone: StatusTone
    var captionSuffix: String?
    @ViewBuilder var accessory: () -> Accessory

    init(
        label: String,
        value: String,
        weighted: Bool,
        tone: StatusTone,
        captionSuffix: String? = nil,
        @ViewBuilder accessory: @escaping () -> Accessory = { EmptyView() }
    ) {
        self.label = label
        self.value = value
        self.weighted = weighted
        self.tone = tone
        self.captionSuffix = captionSuffix
        self.accessory = accessory
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 4) {
                Eyebrow(label)
                if let captionSuffix {
                    Text(captionSuffix)
                        .font(.metaText)
                        .foregroundStyle(Color.rupuMute)
                        .help("Some reporting hosts do not supply a findings count — this is a partial sum, not a fleet total.")
                }
            }
            Text(value)
                .font(.dataMono(weighted ? 22 : 18))
                .fontWeight(weighted ? .semibold : .regular)
                .foregroundStyle(weighted ? Color.status(tone) : Color.rupuInk)
            accessory()
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(weighted ? Color.rupuSurface : Color.rupuPanel)
        .clipShape(RoundedRectangle(cornerRadius: 7))
        .overlay(
            RoundedRectangle(cornerRadius: 7)
                .stroke(weighted ? Color.status(tone) : Color.rupuBorder, lineWidth: 1)
        )
    }
}
