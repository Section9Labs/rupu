import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Flows-composition Task 4: the Run Detail screen's single selection-
/// following tab panel, replacing the old fixed-width `RailColumn` (deleted
/// `RailViews.swift`). `FactsCard`'s identity rows folded into the header's
/// own second meta line (`RunDetailScreen.identityMetaLine`); `NetflowCard`/
/// `FindingsCard` became this file's `NetflowTabContent`/`FindingsTabContent`
/// — same render logic, full width instead of a 280pt rail, no card chrome
/// of their own (the tab panel's `panelStyle(.panel)` already provides it).
///
/// Web parity anchor: `crates/rupu-cp/web/src/pages/RunDetail.tsx` — a
/// `Transcript | Events | Findings | Netflow` tab bar (`cycles` skipped; no
/// autoflow-run concept on this screen yet) that follows whichever step is
/// selected in the graph above it.
public enum RunDetailTab: String, CaseIterable, Sendable {
    case transcript, events, findings, netflow
}

/// The tab bar + content area beneath the step graph. `GeometryReader`-driven
/// per the brief: the content area gets `.frame(minHeight: 420, idealHeight:
/// geo.size.height * 0.65)` so it reads as a substantial panel without
/// forcing itself to consume every last pixel of whatever's left in the
/// screen's vertical stack.
struct RunDetailTabPanel: View {
    let store: RunDetailStore
    @Binding var tab: RunDetailTab
    /// Forwarded to `TranscriptTabContent` alone (every other tab ignores
    /// them) — `RunDetailStore` doesn't expose its own `runID`/`host`
    /// publicly (plumbing, not display state), so `RunDetailScreen` (which
    /// already holds both as its own properties) passes them straight
    /// through rather than this panel reaching into the store.
    let runID: String
    let host: String?
    /// Phase 6B, Task 5: forwarded to `TranscriptTabContent` alone (every
    /// other tab ignores it) — see `TranscriptFeed`'s own doc comment for
    /// why this is the one screen that wires a `SourcePreviewStore` in.
    let sourcePreviewStore: SourcePreviewStore?

    var body: some View {
        GeometryReader { geo in
            VStack(alignment: .leading, spacing: 0) {
                RunDetailTabBar(tab: $tab, findingsCount: findingsCount)
                Divider()
                content
                    .frame(minHeight: 420, idealHeight: geo.size.height * 0.65)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
    }

    private var findingsCount: Int {
        guard case .content(let value) = store.findings else { return 0 }
        return value.summary.total
    }

    @ViewBuilder
    private var content: some View {
        switch tab {
        case .transcript:
            TranscriptTabContent(store: store, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore)
        case .events:
            EventsTabContent(store: store)
        case .findings:
            FindingsTabContent(findings: store.findings, onRetry: { await store.loadFindings() })
        case .netflow:
            NetflowTabContent(netflow: store.netflow, onRetry: { await store.loadNetflow() })
        }
    }
}

/// Four text buttons (`.uiText`); the active one renders in `.rupuInk` with
/// a 2px `Color.rupuBrand` bottom underline, everything else dims to
/// `.rupuDim` with no underline. Findings shows a count `Badge` once
/// `findingsCount > 0` — web parity's `Findings (${findingsCount})` label,
/// rendered here as a separate badge rather than baked into the text.
struct RunDetailTabBar: View {
    @Binding var tab: RunDetailTab
    let findingsCount: Int

    var body: some View {
        HStack(spacing: 18) {
            ForEach(RunDetailTab.allCases, id: \.self) { candidate in
                tabButton(candidate)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
    }

    private func tabButton(_ candidate: RunDetailTab) -> some View {
        let active = tab == candidate
        return Button {
            tab = candidate
        } label: {
            VStack(spacing: 6) {
                HStack(spacing: 6) {
                    Text(title(for: candidate))
                        .font(.uiText)
                        .foregroundStyle(active ? Color.rupuInk : Color.rupuDim)
                    if candidate == .findings, findingsCount > 0 {
                        Badge("\(findingsCount)", tone: Color.status(.awaiting))
                    }
                }
                Rectangle()
                    .fill(active ? Color.rupuBrand : Color.clear)
                    .frame(height: 2)
            }
        }
        .buttonStyle(.plain)
    }

    private func title(for candidate: RunDetailTab) -> String {
        switch candidate {
        case .transcript: "Transcript"
        case .events: "Events"
        case .findings: "Findings"
        case .netflow: "Netflow"
        }
    }
}

// MARK: - Transcript tab

/// The existing `TranscriptFeed` plus the same live/remote indicator
/// `RunDetailScreen` rendered above the old transcript column.
struct TranscriptTabContent: View {
    let store: RunDetailStore
    let runID: String
    let host: String?
    let sourcePreviewStore: SourcePreviewStore?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            liveIndicator
            TranscriptFeed(events: store.transcript, runID: runID, host: host, sourcePreviewStore: sourcePreviewStore)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    @ViewBuilder
    private var liveIndicator: some View {
        if store.isRemote {
            Text("Remote streaming lands with Fleet (Phase 5)")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
        } else if store.transcriptTailActive {
            HStack(spacing: 6) {
                Circle().fill(Color.status(.running)).frame(width: 6, height: 6)
                Text("Live")
                    .font(.metaText)
                    .foregroundStyle(Color.status(.running))
            }
        }
    }
}

// MARK: - Events tab

/// Raw `CPEvent` feed, filtered to the selected step (or unfiltered when
/// nothing is selected) via `store.eventsForSelection()`. Remote runs never
/// open a run stream at all (`isRemote`, see `RunDetailStore`'s type doc
/// comment) — same "Fleet (Phase 5)" note the Transcript tab already shows,
/// rather than a permanently-empty feed with no explanation.
struct EventsTabContent: View {
    let store: RunDetailStore

    var body: some View {
        Group {
            if store.isRemote {
                Text("Remote streaming lands with Fleet (Phase 5)")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
                    .padding(12)
            } else {
                let events = store.eventsForSelection()
                if events.isEmpty {
                    Text("No events yet")
                        .font(.noteText)
                        .foregroundStyle(Color.rupuMute)
                        .padding(12)
                } else {
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 6) {
                            ForEach(Array(events.enumerated()), id: \.offset) { _, event in
                                eventRow(event)
                            }
                        }
                        .padding(12)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func eventRow(_ event: CPEvent) -> some View {
        HStack(spacing: 10) {
            Text(Self.timestampLabel(event))
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
                .frame(width: 150, alignment: .leading)
            Text(Self.typeLabel(event))
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
            Spacer(minLength: 0)
            if let stepID = RunDetailStore.stepID(for: event) {
                Badge(stepID)
            }
        }
        .padding(8)
        .panelStyle(.innerCard)
    }

    /// Most `CPEvent` cases carry no timestamp field at all (see
    /// `CPEvent.swift`'s own doc comment — the Rust executor only stamps
    /// `run_started`/`run_completed`/`run_failed`); rendering an em dash for
    /// the rest is the honest reading of what's actually on the wire rather
    /// than fabricating a client-observed receive time this row would then
    /// mislabel as the event's own "ts".
    static func timestampLabel(_ event: CPEvent) -> String {
        switch event {
        case .runStarted(_, _, let startedAt): startedAt
        case .runCompleted(_, _, let finishedAt): finishedAt
        case .runFailed(_, _, let finishedAt): finishedAt
        default: "—"
        }
    }

    static func typeLabel(_ event: CPEvent) -> String {
        switch event {
        case .runStarted: "Run Started"
        case .stepStarted: "Step Started"
        case .stepWorking: "Step Working"
        // Deliberately NOT Title Case, unlike every sibling case in this
        // switch (redesign-pass fix — audit A4): "Awaiting approval" is
        // this app's one canonical string for the gate-pending status
        // (`StatusPill`'s `.awaiting` descriptor, `EventStreamColumn.swift`'s
        // Situation Room card, `status.ts:95` on the web side), so this
        // Events-tab type label matches that string exactly rather than
        // following the Title Case convention every other event-type label
        // here uses for its own, unrelated reason (labeling an event TYPE,
        // not a status).
        case .stepAwaitingApproval: "Awaiting approval"
        case .stepCompleted: "Step Completed"
        case .stepFailed: "Step Failed"
        case .stepSkipped: "Step Skipped"
        case .unitStarted: "Unit Started"
        case .unitCompleted: "Unit Completed"
        case .panelRound: "Panel Round"
        case .runCompleted: "Run Completed"
        case .runFailed: "Run Failed"
        case .runPaused: "Run Paused"
        case .runResumed: "Run Resumed"
        case .stepPaused: "Step Paused"
        case .stepResumed: "Step Resumed"
        case .dispatchStarted: "Dispatch Started"
        case .dispatchCompleted: "Dispatch Completed"
        case .unknown(let type, _): type
        }
    }
}

// MARK: - Findings tab (moved from the deleted `RailViews.swift`'s `FindingsCard`)

/// Severity 2px left edge, a summary badge row, and per-severity count
/// badges straight from `APIFindingsSummary` — no client-side recount. Full
/// width, list layout (the rail's fixed 280pt column is gone).
struct FindingsTabContent: View {
    let findings: BlockState<APIFindings>
    /// `RunDetailStore.loadFindings()` — the failed block's Retry target,
    /// threaded in because this view holds only the `BlockState`, never the
    /// store.
    let onRetry: () async -> Void

    var body: some View {
        Group {
            switch findings {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedBlock(subject: "findings", message: message, retry: onRetry)
                    .padding(12)
            case .empty:
                Text("No findings").font(.noteText).foregroundStyle(Color.rupuMute)
            case .content(let value):
                if value.findings.isEmpty {
                    Text("No findings").font(.noteText).foregroundStyle(Color.rupuMute)
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 10) {
                            summaryBadges(value.summary)
                            ForEach(value.findings, id: \.id) { finding in
                                findingRow(finding)
                            }
                        }
                        .padding(12)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func summaryBadges(_ summary: APIFindingsSummary) -> some View {
        HStack(spacing: 8) {
            severityCountBadge("C", summary.critical, .crit)
            severityCountBadge("H", summary.high, .high)
            severityCountBadge("M", summary.medium, .med)
            severityCountBadge("L", summary.low, .low)
            severityCountBadge("I", summary.info, .info)
            Spacer(minLength: 0)
        }
    }

    private func severityCountBadge(_ label: String, _ count: Int, _ severity: Severity) -> some View {
        HStack(spacing: 3) {
            Text(label)
                .font(.dataMono(10))
                .foregroundStyle(Color.severity(severity))
            Text("\(count)")
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
        }
        .opacity(count == 0 ? 0.35 : 1)
    }

    private func findingRow(_ finding: APIFinding) -> some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Color.severity(Severity(wireString: finding.severity)))
                .frame(width: 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(finding.summary)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(2)
                if let filePath = finding.filePath {
                    Text(fileLabel(filePath, finding.lineRange))
                        .font(.dataMono(10))
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
}

// MARK: - Netflow tab (moved from the deleted `RailViews.swift`'s `NetflowCard`)

/// `HostRollup` rows, full width. There is no server-side "unexpected host"
/// flag (see api-facts.md's netflow section and `APINetflow`'s own doc
/// comment) — a row renders in `Color.status(.failed)` when `errors > 0`,
/// the only signal available this phase; a true allowlist diff is future
/// work.
struct NetflowTabContent: View {
    let netflow: BlockState<APINetflow>
    /// `RunDetailStore.loadNetflow()` — the failed block's Retry target,
    /// threaded in because this view holds only the `BlockState`, never the
    /// store.
    let onRetry: () async -> Void

    var body: some View {
        Group {
            switch netflow {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedBlock(subject: "netflow", message: message, retry: onRetry)
                    .padding(12)
            case .empty:
                Text("No network calls").font(.noteText).foregroundStyle(Color.rupuMute)
            case .content(let value):
                if value.hosts.isEmpty {
                    Text("No network calls").font(.noteText).foregroundStyle(Color.rupuMute)
                } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(Array(value.hosts.enumerated()), id: \.offset) { _, host in
                                hostRow(host)
                            }
                            if value.droppedTotal > 0 {
                                Text("\(Fmt.count(Int(value.droppedTotal))) dropped")
                                    .font(.dataMono(10))
                                    .foregroundStyle(Color.rupuMute)
                            }
                        }
                        .padding(12)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func hostRow(_ host: APIHostRollup) -> some View {
        let hasErrors = host.errors > 0
        let tone: Color = hasErrors ? Color.status(.failed) : Color.rupuInk
        return VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text("\(host.host):\(host.port)")
                    .font(.dataMono(11.5))
                    .foregroundStyle(tone)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 8)
                if hasErrors {
                    Text("\(host.errors) ERR")
                        .font(.dataMono(10))
                        .foregroundStyle(Color.status(.failed))
                }
            }
            HStack(spacing: 10) {
                Text("\(Fmt.count(Int(host.calls))) calls")
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuDim)
                if let p95 = host.p95MS {
                    Text("p95 \(Fmt.duration(ms: p95))")
                        .font(.dataMono(10))
                        .foregroundStyle(Color.rupuDim)
                }
            }
        }
        .padding(.vertical, 2)
    }
}
