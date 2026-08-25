import RupuDesign
import SwiftUI

/// Situation Room — the right-rail project roster. Port of
/// `ProjectRoster.tsx`: one compact card per project, ordered awaiting →
/// running → idle (`foldRoster`'s own sort — see `Roster.swift`), status
/// dot, current live action, findings-by-severity pips, active-run count.
/// Everything real, nothing fabricated — a project with no active run and
/// no findings just reads as idle with "no findings", matching the web.
struct RosterColumn: View {
    let roster: [RosterEntry]
    let onSelect: (String) -> Void

    private var liveCount: Int {
        roster.filter { $0.status != .idle }.count
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Rectangle().fill(Color.rupuBorder).frame(height: 1)
            ScrollView {
                LazyVStack(spacing: 8) {
                    if roster.isEmpty {
                        Text("No projects yet.")
                            .font(.noteText)
                            .foregroundStyle(Color.rupuDim)
                            .padding(.top, 40)
                    } else {
                        ForEach(roster, id: \.wsID) { entry in
                            Button {
                                onSelect(entry.wsID)
                            } label: {
                                RosterCard(entry: entry)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
                .padding(10)
            }
        }
        .frame(width: 336)
        .frame(maxHeight: .infinity)
        .background(Color.rupuPanel.opacity(0.5))
        .overlay(alignment: .leading) {
            Rectangle().fill(Color.rupuBorder).frame(width: 1)
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Eyebrow("Projects")
            Spacer(minLength: 0)
            Text("\(roster.count) · \(liveCount) live")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
                .monospacedDigit()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }
}

private struct RosterCard: View {
    let entry: RosterEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle().fill(statusColor).frame(width: 7, height: 7)
                Text(entry.name).font(.leadText).foregroundStyle(Color.rupuInk).lineLimit(1)
                Text(statusLabel).font(.metaText).foregroundStyle(statusColor)
            }
            if let action = entry.action {
                HStack(spacing: 5) {
                    if entry.status == .running {
                        ProgressView().controlSize(.mini)
                    }
                    Text(action).font(.noteText).foregroundStyle(Color.rupuDim).lineLimit(1)
                }
            } else if let branch = entry.branch {
                Text(branch).font(.noteText).foregroundStyle(Color.rupuMute).lineLimit(1)
            }
            HStack(spacing: 8) {
                if entry.activeRuns > 0 {
                    Text("\(entry.activeRuns) active").font(.metaText).foregroundStyle(Color.rupuMute)
                }
                Spacer(minLength: 0)
                findingsPips
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(entry.status == .idle ? Color.rupuPanel : Color.rupuSurface)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color.rupuBorder, lineWidth: 1))
    }

    private var statusColor: Color {
        switch entry.status {
        case .await_: Color.status(.awaiting)
        case .running: Color.status(.running)
        case .idle: Color.rupuMute
        }
    }

    private var statusLabel: String {
        switch entry.status {
        case .await_: "awaiting"
        case .running: "running"
        case .idle: "idle"
        }
    }

    @ViewBuilder
    private var findingsPips: some View {
        let f = entry.findings
        if f.total == 0 {
            Text("no findings").font(.metaText).foregroundStyle(Color.rupuMute)
        } else {
            HStack(spacing: 4) {
                pip(f.critical, .crit)
                pip(f.high, .high)
                pip(f.medium, .med)
                pip(f.low, .low)
                pip(f.info, .info)
            }
        }
    }

    @ViewBuilder
    private func pip(_ count: Int, _ severity: Severity) -> some View {
        if count > 0 {
            Text("\(count)")
                .font(.metaText.weight(.semibold))
                .foregroundStyle(Color.severity(severity))
        }
    }
}
