import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// The Run Detail screen's fixed-width right column: a detail-facts card, a
/// netflow-rollup card, and a findings card. Each card is fed straight from
/// its own `BlockState` so a failure in one never blanks the others (the
/// screen's per-block-independence rule) — `RailColumn` itself just lays the
/// three out; it does not know about `RunDetailStore`.
struct RailColumn: View {
    let detail: BlockState<APIRunDetail>
    let netflow: BlockState<APINetflow>
    let findings: BlockState<APIFindings>

    static let width: CGFloat = 280

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            FactsCard(detail: detail)
            NetflowCard(netflow: netflow)
            FindingsCard(findings: findings)
        }
        .frame(width: Self.width, alignment: .top)
    }
}

/// Rail-side facts card: identifiers and a token/cost breakdown the header's
/// own compact facts row doesn't have room for (run id, workspace,
/// permission mode, per-kind token counts) — not a duplicate of the header,
/// a detail expansion of it.
struct FactsCard: View {
    let detail: BlockState<APIRunDetail>

    var body: some View {
        CardShell(title: "RUN FACTS") {
            switch detail {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedRow(message: message)
            case .empty:
                MicroLabel("NO DATA").foregroundStyle(Color.rupuMute)
            case .content(let value):
                VStack(alignment: .leading, spacing: 6) {
                    factRow("Run ID", value.run.id)
                    factRow("Workspace", value.run.workspaceID)
                    factRow("Permission", value.run.permissionMode ?? "—")
                    Divider()
                    factRow("Input tok", Fmt.count(Int(value.usage.inputTokens)))
                    factRow("Output tok", Fmt.count(Int(value.usage.outputTokens)))
                    factRow("Cached tok", Fmt.count(Int(value.usage.cachedTokens)))
                    factRow("Cost", Fmt.cost(value.usage.costUSD))
                }
            }
        }
    }

    private func factRow(_ label: String, _ value: String) -> some View {
        HStack {
            MicroLabel(label).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 8)
            Text(value)
                .font(.identifier)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }
}

/// `HostRollup` rows. There is no server-side "unexpected host" flag (see
/// api-facts.md's netflow section and `APINetflow`'s own doc comment) — a
/// row renders in `Color.status(.fail)` when `errors > 0`, the only signal
/// available this phase; a true allowlist diff is future work.
struct NetflowCard: View {
    let netflow: BlockState<APINetflow>

    var body: some View {
        CardShell(title: "NETFLOW") {
            switch netflow {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedRow(message: message)
            case .empty:
                MicroLabel("NO NETWORK CALLS").foregroundStyle(Color.rupuMute)
            case .content(let value):
                if value.hosts.isEmpty {
                    MicroLabel("NO NETWORK CALLS").foregroundStyle(Color.rupuMute)
                } else {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(Array(value.hosts.enumerated()), id: \.offset) { _, host in
                            hostRow(host)
                        }
                        if value.droppedTotal > 0 {
                            MicroLabel("\(Fmt.count(Int(value.droppedTotal))) DROPPED")
                                .foregroundStyle(Color.rupuMute)
                        }
                    }
                }
            }
        }
    }

    private func hostRow(_ host: APIHostRollup) -> some View {
        let hasErrors = host.errors > 0
        let tone: Color = hasErrors ? Color.status(.fail) : Color.rupuInk
        return VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text("\(host.host):\(host.port)")
                    .font(.identifier)
                    .foregroundStyle(tone)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 8)
                if hasErrors {
                    MicroLabel("\(host.errors) ERR")
                        .foregroundStyle(Color.status(.fail))
                }
            }
            HStack(spacing: 10) {
                MicroLabel("\(Fmt.count(Int(host.calls))) calls")
                    .foregroundStyle(Color.rupuDim)
                if let p95 = host.p95MS {
                    MicroLabel("p95 \(Fmt.duration(ms: p95))")
                        .foregroundStyle(Color.rupuDim)
                }
            }
        }
        .padding(.vertical, 2)
    }
}

/// Findings card: severity 2px left edge, a summary line, and per-severity
/// count badges straight from `APIFindingsSummary` — no client-side
/// recount.
struct FindingsCard: View {
    let findings: BlockState<APIFindings>

    var body: some View {
        CardShell(title: "FINDINGS") {
            switch findings {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedRow(message: message)
            case .empty:
                MicroLabel("NO FINDINGS").foregroundStyle(Color.rupuMute)
            case .content(let value):
                if value.findings.isEmpty {
                    MicroLabel("NO FINDINGS").foregroundStyle(Color.rupuMute)
                } else {
                    VStack(alignment: .leading, spacing: 8) {
                        summaryBadges(value.summary)
                        ForEach(value.findings, id: \.id) { finding in
                            findingRow(finding)
                        }
                    }
                }
            }
        }
    }

    private func summaryBadges(_ summary: APIFindingsSummary) -> some View {
        HStack(spacing: 6) {
            badge("C", summary.critical, .crit)
            badge("H", summary.high, .high)
            badge("M", summary.medium, .med)
            badge("L", summary.low, .low)
            badge("I", summary.info, .info)
            Spacer(minLength: 0)
        }
    }

    private func badge(_ label: String, _ count: Int, _ severity: Severity) -> some View {
        HStack(spacing: 3) {
            Text(label)
                .font(.numeral(size: 10))
                .foregroundStyle(Color.severity(severity))
            Text("\(count)")
                .font(.numeral(size: 10))
                .foregroundStyle(Color.rupuDim)
        }
        .opacity(count == 0 ? 0.35 : 1)
    }

    private func findingRow(_ finding: APIFinding) -> some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Color.severity(severity(for: finding.severity)))
                .frame(width: 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(finding.summary)
                    .font(.system(size: 11.5))
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(2)
                if let filePath = finding.filePath {
                    MicroLabel(fileLabel(filePath, finding.lineRange))
                        .foregroundStyle(Color.rupuMute)
                }
            }
            .padding(.leading, 8)
        }
    }

    private func fileLabel(_ path: String, _ lineRange: [UInt32]?) -> String {
        guard let lineRange, lineRange.count == 2 else { return path }
        return "\(path):\(lineRange[0])-\(lineRange[1])"
    }

    private func severity(for raw: String) -> Severity {
        Severity(rawValue: raw) ?? .info
    }
}

/// Shared card chrome: `MicroLabel` title over `.innerCard`-styled content.
private struct CardShell<Content: View>: View {
    let title: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            MicroLabel(title)
                .foregroundStyle(Color.rupuMute)
            content()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .panelStyle(.panel)
    }
}

private struct FailedRow: View {
    let message: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            MicroLabel("FAILED TO LOAD")
                .foregroundStyle(Color.status(.fail))
            Text(message)
                .font(.system(size: 10.5))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(3)
        }
    }
}
