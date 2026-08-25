import RupuDesign
import RupuStore
import SwiftUI

/// Situation Room — the header instrument strip. Port of `PulseStrip.tsx`'s
/// layout: a brand cell + connection indicator, then six KPI tiles (active
/// runs, projects live/total, findings-by-severity, awaiting-you, errors
/// this session, events/min with a mini sparkline). Every number is real —
/// see `Vitals`'s own doc comment (`RupuSituation/Vitals.swift`) on how each
/// field degrades to zero rather than fabricating one when its source
/// hasn't loaded yet.
struct PulseStrip: View {
    let vitals: Vitals
    let freshness: StreamLifecycle.Freshness
    let spark: [Int]

    var body: some View {
        HStack(spacing: 0) {
            brandCell
            divider
            HStack(spacing: 0) {
                kpi("Active runs") { Text("\(vitals.activeRuns)").font(.dataMono(18)) }
                divider
                kpi("Projects live") {
                    (Text("\(vitals.projectsLive)").font(.dataMono(18))
                        + Text(" / \(vitals.projectsTotal)").font(.dataMono(12)).foregroundStyle(Color.rupuMute))
                }
                divider
                findingsKPI
                divider
                kpi("Awaiting you") {
                    Text("\(vitals.awaiting)")
                        .font(.dataMono(18))
                        .foregroundStyle(vitals.awaiting > 0 ? Color.status(.awaiting) : Color.rupuInk)
                }
                divider
                kpi("Errors · session") {
                    Text("\(vitals.errors)")
                        .font(.dataMono(18))
                        .foregroundStyle(vitals.errors > 0 ? Color.status(.failed) : Color.rupuInk)
                }
                divider
                eventsKPI
            }
        }
        .padding(.horizontal, 20)
        .frame(height: 64)
        .background(Color.rupuPanel.opacity(0.7))
        .overlay(alignment: .bottom) {
            Rectangle().fill(Color.rupuBorder).frame(height: 1)
        }
    }

    private var divider: some View {
        Rectangle().fill(Color.rupuBorder).frame(width: 1).padding(.vertical, 14)
    }

    private var brandCell: some View {
        HStack(spacing: 10) {
            Icon(.radio, size: 16).foregroundStyle(Color.rupuDim)
            VStack(alignment: .leading, spacing: 1) {
                Text("Live Events").font(.leadText.weight(.semibold)).foregroundStyle(Color.rupuInk)
                Eyebrow("Situation Room")
            }
            connectionBadge
        }
        .padding(.trailing, 20)
    }

    private var connectionBadge: some View {
        let (label, tone): (String, Color) = switch freshness {
        case .live: ("Live", Color.rupuBrand)
        case .stale: ("Reconnecting", Color.status(.awaiting))
        case .idle: ("Connecting", Color.rupuMute)
        }
        return HStack(spacing: 4) {
            Circle().fill(tone).frame(width: 6, height: 6)
            Text(label).font(.metaText).foregroundStyle(tone)
        }
        .padding(.horizontal, 4)
    }

    private func kpi(_ label: String, @ViewBuilder value: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Eyebrow(label)
            value()
        }
        .frame(minWidth: 96, alignment: .leading)
        .padding(.horizontal, 16)
    }

    private var findingsKPI: some View {
        let f = vitals.findings
        return kpi("Findings · open") {
            VStack(alignment: .leading, spacing: 3) {
                Text("\(f.total)").font(.dataMono(18))
                HStack(spacing: 6) {
                    if f.total == 0 {
                        Text("none").font(.metaText).foregroundStyle(Color.rupuMute)
                    } else {
                        sevPip("C", f.critical, .crit)
                        sevPip("H", f.high, .high)
                        sevPip("M", f.medium, .med)
                        sevPip("L", f.low, .low)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func sevPip(_ label: String, _ count: Int, _ severity: Severity) -> some View {
        if count > 0 {
            Text("\(label) \(count)")
                .font(.metaText.weight(.semibold))
                .foregroundStyle(Color.severity(severity))
                .padding(.horizontal, 5)
                .padding(.vertical, 1)
                .background(Color.severityBg(severity), in: Capsule())
        }
    }

    private var eventsKPI: some View {
        let maxSample = max(1, spark.max() ?? 1)
        return kpi("Events / min") {
            HStack(alignment: .bottom, spacing: 8) {
                Text("\(vitals.eventsPerMin)").font(.dataMono(18))
                HStack(alignment: .bottom, spacing: 1.5) {
                    ForEach(Array(spark.enumerated()), id: \.offset) { _, sample in
                        RoundedRectangle(cornerRadius: 1)
                            .fill(Color.rupuBrand.opacity(0.55))
                            .frame(width: 2, height: max(2, CGFloat(sample) / CGFloat(maxSample) * 20))
                    }
                }
                .frame(height: 20, alignment: .bottom)
            }
        }
    }
}
