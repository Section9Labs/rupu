import Charts
import Foundation
import RupuAPI
import RupuDesign
import SwiftUI

// MARK: - Timestamp parsing

/// Parses a server-emitted RFC 3339 `ts`, tolerating both shapes chrono
/// emits (fractional seconds only when nanos are non-zero) — mirrors
/// `RupuStore/DashboardStore.swift`'s `parseTimestamp`. `nil` on a genuinely
/// unparseable string; callers drop that bucket rather than fabricating a
/// date, since the server contract guarantees well-formed, zero-filled,
/// midnight-UTC-aligned `ts` values.
func parseBucketTimestamp(_ s: String) -> Date? {
    let withFractional = ISO8601DateFormatter()
    withFractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = withFractional.date(from: s) { return date }
    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    return plain.date(from: s)
}

// MARK: - Pure adapters (the testable seam)

/// Reshapes terminal-outcome buckets into stacked-area chart rows, one row
/// per (bucket, series) pair. Series order is fixed — completed → failed →
/// rejected → cancelled — matching the web's stack order (bottom to top;
/// `crates/rupu-cp/web/src/components/dashboard/TerminalTrend.tsx`). Buckets
/// are walked in the order given (the server already zero-fills and sorts
/// them chronologically) — this adapter never invents a bucket for a gap,
/// it only reshapes what it's handed. A bucket whose `ts` fails to parse is
/// dropped rather than guessed at.
public func chartRows(from buckets: [APITerminalBucket]) -> [(ts: Date, series: String, value: Int)] {
    buckets.flatMap { bucket -> [(ts: Date, series: String, value: Int)] in
        guard let ts = parseBucketTimestamp(bucket.ts) else { return [] }
        return [
            (ts: ts, series: "completed", value: bucket.completed),
            (ts: ts, series: "failed", value: bucket.failed),
            (ts: ts, series: "rejected", value: bucket.rejected),
            (ts: ts, series: "cancelled", value: bucket.cancelled),
        ]
    }
}

/// Same reshape for throughput-by-trigger buckets. Series order — manual →
/// cron → event — matches the web's `ThroughputChart`
/// (`crates/rupu-cp/web/src/components/dashboard/ThroughputChart.tsx`).
public func throughputRows(from buckets: [APIThroughputBucket]) -> [(ts: Date, series: String, value: Int)] {
    buckets.flatMap { bucket -> [(ts: Date, series: String, value: Int)] in
        guard let ts = parseBucketTimestamp(bucket.ts) else { return [] }
        return [
            (ts: ts, series: "manual", value: bucket.manual),
            (ts: ts, series: "cron", value: bucket.cron),
            (ts: ts, series: "event", value: bucket.event),
        ]
    }
}

// MARK: - Series → color / label

private func outcomeColor(_ series: String) -> Color {
    switch series {
    case "completed": .status(.done)
    case "failed": .status(.failed)
    case "rejected": .status(.rejected)
    case "cancelled": .status(.cancelled)
    default: .status(.pending)
    }
}

private func triggerSeriesColor(_ series: String) -> Color {
    switch series {
    case "manual": .trigger(.manual)
    case "cron": .trigger(.cron)
    case "event": .trigger(.event)
    default: .trigger(.manual)
    }
}

private func seriesLabel(_ series: String) -> String {
    series.capitalized
}

// MARK: - Chart chrome shared by the two stacked-area charts

private let chartPlotHeight: CGFloat = 164
private let sparklineHeight: CGFloat = 32
private let chartFillOpacity: Double = 0.25

private func dayAxisLabelFormat() -> Date.FormatStyle {
    .dateTime.month(.abbreviated).day()
}

/// Applies the chrome shared by `OutcomesChart`/`ThroughputChart`: fixed
/// plot height, an inline bottom legend, day-formatted x labels with no
/// other x chrome, and an integer-formatted y axis with just a leading
/// gridline (mirrors the web's `vertical={false}` `CartesianGrid` +
/// `tickLine={false}` axes). No `.animation(...)` is applied anywhere —
/// these charts are deliberately static (liveness is per-transport, per the
/// web's `isAnimationActive={false}` rationale).
private struct StackedAreaChrome: ViewModifier {
    func body(content: Content) -> some View {
        content
            .chartLegend(position: .bottom, alignment: .leading, spacing: 8)
            .chartXAxis {
                AxisMarks(values: .automatic) { _ in
                    AxisValueLabel(format: dayAxisLabelFormat())
                }
            }
            .chartYAxis {
                AxisMarks(position: .leading) { _ in
                    AxisGridLine()
                    AxisValueLabel(format: FloatingPointFormatStyle<Double>.number.precision(.fractionLength(0)))
                }
            }
            .frame(height: chartPlotHeight)
    }
}

// MARK: - Views

/// Stacked-area chart of terminal run outcomes over time (spec §5.5's
/// outcome half of the status split). Series colors are `Color.status(...)`
/// so this chart agrees with every other outcome-tone element in the app by
/// construction.
public struct OutcomesChart: View {
    private static let seriesOrder = ["completed", "failed", "rejected", "cancelled"]
    private let rows: [(ts: Date, series: String, value: Int)]

    public init(buckets: [APITerminalBucket]) {
        rows = chartRows(from: buckets)
    }

    public var body: some View {
        Chart {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                AreaMark(
                    x: .value("Date", row.ts),
                    y: .value("Count", row.value)
                )
                .foregroundStyle(by: .value("Outcome", seriesLabel(row.series)))
                .opacity(chartFillOpacity)
            }
        }
        .chartForegroundStyleScale(
            domain: Self.seriesOrder.map(seriesLabel),
            range: Self.seriesOrder.map(outcomeColor)
        )
        .modifier(StackedAreaChrome())
    }
}

/// Stacked-area chart of runs started per bucket, split by trigger. Series
/// colors are `Color.trigger(...)` so this chart agrees with `TriggerChip`-
/// style badges elsewhere without needing a shared legend.
public struct ThroughputChart: View {
    private static let seriesOrder = ["manual", "cron", "event"]
    private let rows: [(ts: Date, series: String, value: Int)]

    public init(buckets: [APIThroughputBucket]) {
        rows = throughputRows(from: buckets)
    }

    public var body: some View {
        Chart {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                AreaMark(
                    x: .value("Date", row.ts),
                    y: .value("Count", row.value)
                )
                .foregroundStyle(by: .value("Trigger", seriesLabel(row.series)))
                .opacity(chartFillOpacity)
            }
        }
        .chartForegroundStyleScale(
            domain: Self.seriesOrder.map(seriesLabel),
            range: Self.seriesOrder.map(triggerSeriesColor)
        )
        .modifier(StackedAreaChrome())
    }
}

/// 32pt single-series area of the `failed` bucket only — the KPI-tile
/// sparkline (mirrors the web's `KeyPointTiles` failed-tile sparkline: same
/// series `OutcomesChart`/`TerminalTrend` renders, so the two never
/// disagree). No axis/legend chrome at all — just the shape.
public struct FailedSparkline: View {
    private let rows: [(ts: Date, value: Int)]

    public init(buckets: [APITerminalBucket]) {
        rows = chartRows(from: buckets)
            .filter { $0.series == "failed" }
            .map { (ts: $0.ts, value: $0.value) }
    }

    public var body: some View {
        Chart {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                AreaMark(
                    x: .value("Date", row.ts),
                    y: .value("Failed", row.value)
                )
                .foregroundStyle(Color.status(.failed))
                .opacity(chartFillOpacity)
            }
        }
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
        .chartLegend(.hidden)
        .frame(height: sparklineHeight)
    }
}
