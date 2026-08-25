import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

/// The Project Detail screen's tab bar (Phase 5A, Task 5) — same
/// text-button-plus-underline idiom as `RupuRunDetail.RunDetailTabBar`
/// (deliberately re-derived here rather than shared: that type is private
/// to `RupuRunDetail`, and this screen's tab set — six, not four — and
/// findings-badge placement differ enough that a shared generic type isn't
/// worth the indirection for one more screen).
public enum ProjectDetailTab: String, CaseIterable, Sendable {
    case overview, runs, sessions, findings, coverage, definitions

    var title: String {
        switch self {
        case .overview: "Overview"
        case .runs: "Runs"
        case .sessions: "Sessions"
        case .findings: "Findings"
        case .coverage: "Coverage"
        case .definitions: "Definitions"
        }
    }
}

/// The Project Detail screen (Phase 5A, Task 5), pushed from a Projects list
/// row tap (`.projectDetail(wsID:)`): header (back chevron, breadcrumb,
/// facts line) + a six-tab panel — Overview/Runs/Sessions/Findings/Coverage/
/// Definitions. Owns a `ProjectDetailStore` lifecycle the same way
/// `RunDetailScreen` owns a `RunDetailStore`: built lazily once
/// `backend.client()` exists, rebuilt on a `wsID` change OR a backend client
/// swap, `store.activate()`d on appear (loads `detail` only — every other
/// tab's block is fetched lazily, see `ProjectDetailStore`'s own doc
/// comment).
///
/// **Lazy tab loading**: `tabPanel`'s `.task(id:)` is keyed on `wsID` AND
/// `tab` together (not `tab` alone) — a plain `.task(id: tab)` would miss a
/// `wsID` change that leaves `tab` unchanged (e.g. still `.overview` after
/// `activate()` resets it back to `.overview` for the new project), since
/// `.task(id:)` only re-fires when its id's *value* changes. Combining both
/// into one id makes every `wsID`/`tab` combination its own fetch trigger.
///
/// **Does NOT need `OverviewScreen`'s cold-launch fix** — same reasoning
/// `RunDetailScreen`/`ProjectsScreen` already document: `.projectDetail` is
/// only ever reached by pushing from `.projects`, never a cold-launch route.
public struct ProjectDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let wsID: String

    @State private var store: ProjectDetailStore?
    @State private var storeWsID: String?
    @State private var storeClientID: ObjectIdentifier?
    @State private var tab: ProjectDetailTab = .overview

    public init(model: AppModel, backend: BackendController, wsID: String) {
        self.model = model
        self.backend = backend
        self.wsID = wsID
    }

    public var body: some View {
        Group {
            if let store {
                content(store: store)
            } else {
                centeredLabel("Backend not connected")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Color.rupuBg)
        .task(id: wsID) {
            await activate()
        }
    }

    /// Builds (or rebuilds, on a `wsID` change OR a backend client swap) the
    /// store, resets `tab` back to `.overview` for the new project, and
    /// activates it. Same `storeWsID`/`storeClientID` recipe
    /// `RunDetailScreen` uses for `storeRunID`/`storeClientID` — see that
    /// type's `activate()` doc comment.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if storeWsID != wsID || storeClientID != clientID {
            store = ProjectDetailStore(wsID: wsID, client: client)
            storeWsID = wsID
            storeClientID = clientID
            tab = .overview
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: ProjectDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            header(store: store)
            tabPanel(store: store)
        }
        .padding(16)
    }

    // MARK: - Header

    private func header(store: ProjectDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Button {
                    model.navigateBack()
                } label: {
                    Icon(.arrowLeft)
                        .foregroundStyle(Color.rupuDim)
                }
                .buttonStyle(.plain)

                if case .content(let detail) = store.detail {
                    Text("Projects ▸ \(detail.project.name)")
                        .font(.leadText)
                        .foregroundStyle(Color.rupuInk)
                } else {
                    Text("Projects")
                        .font(.leadText)
                        .foregroundStyle(Color.rupuInk)
                }
                Spacer(minLength: 0)
            }

            switch store.detail {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedNote(message: message)
            case .empty:
                EmptyView()
            case .content(let detail):
                factsLine(detail)
                metaLine(detail)
            }
        }
    }

    private func factsLine(_ detail: APIProjectDetail) -> some View {
        HStack(spacing: 14) {
            fact("Runs", Fmt.count(detail.runs.total))
            fact("Running", Fmt.count(detail.runs.running))
            fact("Sessions", Fmt.count(detail.sessions.total))
            fact("Active sessions", Fmt.count(detail.sessions.active))
            fact("Spend", Fmt.cost(detail.usage.costUSD))
            Spacer(minLength: 0)
        }
    }

    /// Second facts row (final-review fix wave): the identity/provenance
    /// line `factsLine` above never carried — mono `path` (middle-truncated,
    /// same idiom `RunDetailScreen.identityMetaLine` uses for a long id),
    /// a tappable repo link when the project has one, and created/last-run
    /// relative times. Null discipline throughout: any of `path`/
    /// `repoHomeURL`/`createdAt` can be absent on the wire (see
    /// `APIProjectRow`'s doc comment) and each renders nothing rather than a
    /// blank or a fabricated placeholder — `detail.project.lastRunAt` already
    /// has an established fallback via `Fmt`-style em dash handling in
    /// `relativeLabel(_:)`.
    private func metaLine(_ detail: APIProjectDetail) -> some View {
        HStack(spacing: 14) {
            if let path = detail.project.path {
                Text(path)
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            if let repoHomeURL = detail.project.repoHomeURL, let url = URL(string: repoHomeURL) {
                Link(repoHomeURL, destination: url)
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuBrand)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: 0)
            fact("Created", relativeLabel(detail.project.createdAt))
            fact("Last run", relativeLabel(detail.project.lastRunAt))
        }
    }

    private func fact(_ label: String, _ value: String) -> some View {
        HStack(spacing: 4) {
            Text(label).font(.metaText).foregroundStyle(Color.rupuMute)
            Text(value).font(.dataMono(11)).foregroundStyle(Color.rupuInk)
        }
    }

    // MARK: - Tab panel

    private func tabPanel(store: ProjectDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ProjectDetailTabBar(tab: $tab, findingsCount: findingsCount(store: store))
            Divider()
            tabContent(store: store)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
        // See the type doc comment's "Lazy tab loading" section for why
        // both `wsID` and `tab` are folded into one id.
        .task(id: "\(wsID)|\(tab.rawValue)") {
            await loadTab(tab, store: store)
        }
    }

    private func findingsCount(store: ProjectDetailStore) -> Int {
        guard case .content(let value) = store.findings else { return 0 }
        return value.summary.total
    }

    /// Dispatches the lazy fetch for whichever tab is now selected —
    /// `.overview`/`.coverage` need nothing beyond `detail` (already loaded
    /// by `activate()`) and the honest placeholder note respectively.
    /// `.definitions` fires all three of its own sub-lists concurrently —
    /// each is independently idempotent (`loadXIfNeeded()`), so a repeat
    /// visit to this tab is a no-op fan-out, not three redundant fetches.
    private func loadTab(_ tab: ProjectDetailTab, store: ProjectDetailStore) async {
        switch tab {
        case .overview, .coverage:
            break
        case .runs:
            await store.loadRunsIfNeeded()
        case .sessions:
            await store.loadSessionsIfNeeded()
        case .findings:
            await store.loadFindingsIfNeeded()
        case .definitions:
            async let agentsLoad: Void = store.loadAgentsIfNeeded()
            async let workflowsLoad: Void = store.loadWorkflowsIfNeeded()
            async let autoflowsLoad: Void = store.loadAutoflowsIfNeeded()
            _ = await (agentsLoad, workflowsLoad, autoflowsLoad)
        }
    }

    @ViewBuilder
    private func tabContent(store: ProjectDetailStore) -> some View {
        switch tab {
        case .overview:
            OverviewTabContent(store: store, onSelect: { model.navigate(to: $0) })
        case .runs:
            RunsTabContent(store: store, onSelect: { model.navigate(to: $0) })
        case .sessions:
            SessionsTabContent(store: store, onSelect: { model.navigate(to: $0) })
        case .findings:
            ProjectFindingsTabContent(findings: store.findings)
        case .coverage:
            CoverageTabContent()
        case .definitions:
            DefinitionsTabContent(
                store: store,
                onSelect: { model.navigate(to: $0) },
                onLaunch: { kind, name, scopeKind, scopeID in
                    model.presentLauncher(kind: kind, name: name, scopeKind: scopeKind, scopeID: scopeID)
                }
            )
        }
    }

    private func centeredLabel(_ label: String) -> some View {
        VStack {
            Spacer(minLength: 0)
            Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - Tab bar

private struct ProjectDetailTabBar: View {
    @Binding var tab: ProjectDetailTab
    let findingsCount: Int

    var body: some View {
        HStack(spacing: 18) {
            ForEach(ProjectDetailTab.allCases, id: \.self) { candidate in
                tabButton(candidate)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
    }

    private func tabButton(_ candidate: ProjectDetailTab) -> some View {
        let active = tab == candidate
        return Button {
            tab = candidate
        } label: {
            VStack(spacing: 6) {
                HStack(spacing: 6) {
                    Text(candidate.title)
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
}

// MARK: - Overview tab

private struct OverviewTabContent: View {
    let store: ProjectDetailStore
    let onSelect: (Route) -> Void

    var body: some View {
        Group {
            switch store.detail {
            case .loading:
                ProgressView().controlSize(.small).padding(12)
            case .failed(let message):
                FailedNote(message: message).padding(12)
            case .empty:
                Text("No data").font(.noteText).foregroundStyle(Color.rupuMute).padding(12)
            case .content(let detail):
                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        statusBreakdown(detail)
                        surfaceBreakdown(detail)
                        recentRuns(detail)
                    }
                    .padding(12)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func statusBreakdown(_ detail: APIProjectDetail) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("By status")
            HStack(spacing: 8) {
                ForEach(detail.runs.byStatus.sorted(by: { $0.key < $1.key }), id: \.key) { key, count in
                    Badge("\(key) \(count)")
                }
                Spacer(minLength: 0)
            }
        }
    }

    private func surfaceBreakdown(_ detail: APIProjectDetail) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("By surface")
            HStack(spacing: 8) {
                Badge("workflow \(detail.runs.bySurface.workflow)")
                Badge("autoflow \(detail.runs.bySurface.autoflow)")
                Spacer(minLength: 0)
            }
        }
    }

    private func recentRuns(_ detail: APIProjectDetail) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Recent runs")
            if detail.recentRuns.isEmpty {
                Text("No runs yet").font(.noteText).foregroundStyle(Color.rupuMute)
            } else {
                VStack(spacing: 0) {
                    ForEach(detail.recentRuns, id: \.id) { row in
                        RunRow(row: row, onSelect: onSelect)
                        Divider()
                    }
                }
            }
        }
    }
}

// MARK: - "Show all" footer contract (Runs + Sessions)

/// The three honest states the Runs/Sessions tab's windowed-list footer can
/// be in. Pure, no `View` dependency (review fix) — so
/// `ProjectDetailTabTests` can assert the reconciliation directly, and both
/// tabs share one implementation instead of duplicating the same "is there
/// more, and can this store actually fetch it" logic per resource.
enum ShowAllFooterState: Equatable {
    case hidden
    case button(label: String)
    case note(String)
}

enum ShowAllFooter {
    /// `noun` is `"runs"` or `"sessions"` — the only per-resource variable
    /// in an otherwise identical contract. `showAllLimit` is threaded in
    /// (rather than read from `ProjectDetailStore.showAllLimit` directly)
    /// so this stays testable with arbitrary values.
    ///
    /// **Review fix**: the previous contract rendered a "Show all N" button
    /// whenever `!showingAll`, regardless of whether `showAllLimit` could
    /// actually reach `N` — tapping it silently capped at 1,000 rows while
    /// the button (and, worse, the resulting `showingAll = true` it used to
    /// set unconditionally on any fetch success) both claimed completeness
    /// that was never true for a project with more than `showAllLimit`
    /// rows. Three states now, each honest about what happens next:
    ///
    /// - `.hidden` — either every row is already loaded (`showingAll ==
    ///   true`, which `ProjectDetailStore` now only ever sets when `loaded
    ///   >= total`), `total` isn't known yet (`detail` hasn't loaded), or
    ///   there's nothing left to load at all (`loaded >= total` even before
    ///   the store's own flag catches up, belt-and-suspenders).
    /// - `.button` — more to fetch, and the fetch can actually get there:
    ///   `"Show all N runs"` when `total` fits inside `showAllLimit`,
    ///   `"Show first 1,000 of N runs"` (both figures formatted, no lie by
    ///   omission) when it doesn't — the label itself commits to exactly
    ///   what tapping it will produce.
    /// - `.note` — already fetched at `showAllLimit` (`loaded ==
    ///   showAllLimit`) and still short of `total`: there is nothing this
    ///   store can fetch beyond the cap without real server-side
    ///   pagination — `GET /api/projects/:ws_id/runs`/`.../sessions` have
    ///   no cursor this phase (a future need, not built here) — so a
    ///   "show more" button here would just refetch the identical capped
    ///   page again. Persistent and quiet instead of a dead control.
    static func resolve(loaded: Int, total: Int?, showingAll: Bool, showAllLimit: Int, noun: String) -> ShowAllFooterState {
        guard let total, !showingAll, loaded < total else { return .hidden }
        if loaded >= showAllLimit {
            return .note("Showing \(Fmt.count(loaded)) of \(Fmt.count(total)) \(noun)")
        }
        if total > showAllLimit {
            return .button(label: "Show first \(Fmt.count(showAllLimit)) of \(Fmt.count(total)) \(noun)")
        }
        return .button(label: "Show all \(Fmt.count(total)) \(noun)")
    }
}

// MARK: - Runs tab (windowed)

private struct RunsTabContent: View {
    let store: ProjectDetailStore
    let onSelect: (Route) -> Void

    var body: some View {
        Group {
            switch store.runs {
            case .loading:
                ProgressView().controlSize(.small).padding(12)
            case .failed(let message):
                FailedNote(message: message).padding(12)
            case .empty:
                Text("No runs yet").font(.noteText).foregroundStyle(Color.rupuMute).padding(12)
            case .content(let rows):
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(rows, id: \.id) { row in
                            RunRow(row: row, onSelect: onSelect)
                            Divider()
                        }
                        showAllFooter(loaded: rows.count)
                    }
                    .padding(.vertical, 4)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    /// See `ShowAllFooter.resolve(...)`'s doc comment for the three-state
    /// contract this renders.
    @ViewBuilder
    private func showAllFooter(loaded: Int) -> some View {
        switch ShowAllFooter.resolve(
            loaded: loaded, total: store.detail.value?.runs.total, showingAll: store.runsShowingAll,
            showAllLimit: ProjectDetailStore.showAllLimit, noun: "runs"
        ) {
        case .hidden:
            EmptyView()
        case .button(let label):
            Button(label) {
                Task { await store.showAllRuns() }
            }
            .buttonStyle(.plain)
            .font(.noteText)
            .foregroundStyle(Color.rupuBrand)
            .padding(12)
        case .note(let text):
            Text(text)
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .padding(12)
        }
    }
}

// MARK: - Sessions tab (windowed)

private struct SessionsTabContent: View {
    let store: ProjectDetailStore
    let onSelect: (Route) -> Void

    var body: some View {
        Group {
            switch store.sessions {
            case .loading:
                ProgressView().controlSize(.small).padding(12)
            case .failed(let message):
                FailedNote(message: message).padding(12)
            case .empty:
                Text("No sessions yet").font(.noteText).foregroundStyle(Color.rupuMute).padding(12)
            case .content(let rows):
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(rows, id: \.sessionID) { row in
                            SessionRow(row: row, onSelect: onSelect)
                            Divider()
                        }
                        showAllFooter(loaded: rows.count)
                    }
                    .padding(.vertical, 4)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    /// Same `ShowAllFooter.resolve(...)` contract as `RunsTabContent.
    /// showAllFooter` — see that type's doc comment.
    @ViewBuilder
    private func showAllFooter(loaded: Int) -> some View {
        switch ShowAllFooter.resolve(
            loaded: loaded, total: store.detail.value?.sessions.total, showingAll: store.sessionsShowingAll,
            showAllLimit: ProjectDetailStore.showAllLimit, noun: "sessions"
        ) {
        case .hidden:
            EmptyView()
        case .button(let label):
            Button(label) {
                Task { await store.showAllSessions() }
            }
            .buttonStyle(.plain)
            .font(.noteText)
            .foregroundStyle(Color.rupuBrand)
            .padding(12)
        case .note(let text):
            Text(text)
                .font(.noteText)
                .foregroundStyle(Color.rupuMute)
                .padding(12)
        }
    }
}

// MARK: - Findings tab

/// Local, minimal rendering — deliberately not `RupuRunDetail.
/// FindingsTabContent`, which is private to that module: reaching for it
/// would mean adding a `RupuProjects → RupuRunDetail` module dependency for
/// one shared view. `Severity`/`Color.severity(_:)` (the piece that
/// actually needs sharing — the severity vocabulary itself) already lives
/// in `RupuDesign`, which this module depends on regardless.
private struct ProjectFindingsTabContent: View {
    let findings: BlockState<APIFindings>

    var body: some View {
        Group {
            switch findings {
            case .loading:
                ProgressView().controlSize(.small).padding(12)
            case .failed(let message):
                FailedNote(message: message).padding(12)
            case .empty:
                Text("No findings").font(.noteText).foregroundStyle(Color.rupuMute).padding(12)
            case .content(let value):
                if value.findings.isEmpty {
                    Text("No findings").font(.noteText).foregroundStyle(Color.rupuMute).padding(12)
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
            severityCount("C", summary.critical, .crit)
            severityCount("H", summary.high, .high)
            severityCount("M", summary.medium, .med)
            severityCount("L", summary.low, .low)
            severityCount("I", summary.info, .info)
            Spacer(minLength: 0)
        }
    }

    private func severityCount(_ label: String, _ count: Int, _ severity: Severity) -> some View {
        HStack(spacing: 3) {
            Text(label).font(.dataMono(10)).foregroundStyle(Color.severity(severity))
            Text("\(count)").font(.dataMono(10)).foregroundStyle(Color.rupuDim)
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

    /// Same wire vocabulary mapping as `RupuRunDetail.FindingsTabContent.
    /// severity(for:)` — see that method's doc comment for why an
    /// unrecognized string falls back to `.info` rather than crashing.
    private func severity(for raw: String) -> Severity {
        switch raw {
        case "critical": .crit
        case "high": .high
        case "medium": .med
        case "low": .low
        case "info": .info
        default: .info
        }
    }
}

// MARK: - Coverage tab (honest placeholder)

/// Spec-ruled honest placeholder — the Phase 5 breadth spec's Coverage line
/// (`docs/superpowers/specs/2026-08-24-rupu-macos-phase-5-breadth-design.md`
/// §2, "Projects": "this one tab ships with Plan B, which owns the coverage
/// models; the tab bar carries it from day one with an honest 'arrives with
/// Security' placeholder"), carried into the Phase 5A plan's Task 5
/// ("Coverage tab renders the spec's 'arrives with Security' placeholder
/// note (NO fake data)"): the cheap `coverage.targets`/`coverage.findings`
/// counts `APIProjectDetail` already carries are deliberately NOT rendered
/// here — the metric that would make them meaningful (`assessed_pct`, a
/// separate `GET /api/projects/:ws_id/coverage/assessed` call this phase
/// doesn't cover) is absent, and half a coverage picture is worse than an
/// honest "not yet" note. No fake data, no dead chrome. (Review fix, final
/// wave — this previously cited a non-existent "plan §5"; the plan has no
/// numbered §5, only an unnumbered Task 5 and a separate "Self-review
/// notes" section whose own "§5" shorthand refers to the DESIGN SPEC's §5,
/// not the plan's own structure.)
private struct CoverageTabContent: View {
    var body: some View {
        VStack(spacing: 8) {
            Spacer(minLength: 0)
            Text("Coverage").font(.noteText.weight(.semibold)).foregroundStyle(Color.rupuInk)
            Text("Arrives with Security (Phase 5B)").font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - Definitions tab

/// Local definition rows, now visually/behaviorally unified with Library's
/// rendering (final-review fix wave, item 7) — Task 7 (`RupuLibrary`)
/// hadn't landed when this tab was first built, so it shipped with bare
/// name/badge rows and no navigation (the brief's own documented fallback
/// at the time). Deliberately still NOT the same Swift types as
/// `RupuLibrary.LibraryScreen`'s private `AgentDefRow`/`WorkflowDefRow`/
/// `AutoflowDefRow` — literally sharing them would mean adding a
/// `RupuProjects → RupuLibrary` module dependency and widening those types'
/// access level to `public` for one set of rows, the same "not worth the
/// indirection for one more screen" call `ProjectFindingsTabContent`'s own
/// doc comment already makes for `RupuRunDetail.FindingsTabContent` above.
/// Instead these rows re-derive the same chrome from the vocabulary that
/// already IS shared (`RupuStore.agentPermissionTone(mode:)`, `RupuDesign.
/// Badge`) and now tap through to the same scoped detail routes Library's
/// rows push (`.agentDefinition`/`.workflowDefinition`, carrying the row's
/// own `scopeKind`/`scopeID` — see `Route`'s doc comment on why that
/// matters). Three independently-loaded, independently-rendered sections —
/// Agents/Workflows/Autoflows — each its own `BlockState` from
/// `ProjectDetailStore`.
private struct DefinitionsTabContent: View {
    let store: ProjectDetailStore
    let onSelect: (Route) -> Void
    let onLaunch: (LaunchKind, String, String?, String?) -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                agentsSection
                workflowsSection
                autoflowsSection
            }
            .padding(12)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    @ViewBuilder
    private var agentsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Agents")
            switch store.agents {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedNote(message: message)
            case .empty:
                Text("None").font(.noteText).foregroundStyle(Color.rupuMute)
            case .content(let rows):
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, def in
                        AgentDefRow(
                            def: def,
                            onSelect: { onSelect(.agentDefinition(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID)) },
                            onLaunch: { onLaunch(.agentRun, def.name, def.scopeKind, def.scopeID) }
                        )
                        Divider()
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var workflowsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Workflows")
            switch store.workflows {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedNote(message: message)
            case .empty:
                Text("None").font(.noteText).foregroundStyle(Color.rupuMute)
            case .content(let rows):
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, def in
                        WorkflowDefRow(
                            def: def,
                            onSelect: { onSelect(.workflowDefinition(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID)) },
                            onLaunch: { onLaunch(.workflow, def.name, def.scopeKind, def.scopeID) }
                        )
                        Divider()
                    }
                }
            }
        }
    }

    /// **No enable/disable toggle** (unlike `RupuLibrary.AutoflowDefRow`) —
    /// `ProjectDetailStore` has no `setAutoflowEnabled`/`PendingActions`
    /// capability this phase (it's a read-only store, see its own type doc
    /// comment); wiring a toggle here would mean either faking one against
    /// nothing (a silent no-op — the exact anti-pattern this codebase
    /// rejects elsewhere: "no dead controls, per the brief") or duplicating
    /// `LibraryStore`'s mutation + pending-state plumbing onto a second
    /// store for one tab. Deferred rather than faked; the enabled/disabled
    /// badge stays read-only chrome, same as before this fix wave — only
    /// navigation is new here.
    @ViewBuilder
    private var autoflowsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Autoflows")
            switch store.autoflows {
            case .loading:
                ProgressView().controlSize(.small)
            case .failed(let message):
                FailedNote(message: message)
            case .empty:
                Text("None").font(.noteText).foregroundStyle(Color.rupuMute)
            case .content(let rows):
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, def in
                        AutoflowDefRow(
                            def: def,
                            onSelect: { onSelect(.workflowDefinition(name: def.name, scopeKind: def.scopeKind, scopeID: def.scopeID)) }
                        )
                        Divider()
                    }
                }
            }
        }
    }
}

private struct AgentDefRow: View {
    let def: AgentDefinition
    let onSelect: () -> Void
    let onLaunch: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text(def.name).font(.uiText).foregroundStyle(Color.rupuInk).lineLimit(1)
            if let mode = def.mode, let tone = agentPermissionTone(mode: mode) {
                Badge(mode, tone: Color.status(tone))
            }
            if let provider = def.provider {
                Badge(provider)
            }
            if let model = def.model {
                Text(model).font(.dataMono(10)).foregroundStyle(Color.rupuDim).lineLimit(1)
            }
            Spacer(minLength: 8)
            Text("\(def.runCount) runs").font(.metaText).foregroundStyle(Color.rupuMute)
            DefRowLaunchButton(action: onLaunch)
        }
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .onTapGesture(perform: onSelect)
        .onHover(perform: DefRowHover.apply)
    }
}

private struct WorkflowDefRow: View {
    let def: WorkflowDefinition
    let onSelect: () -> Void
    let onLaunch: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text(def.name).font(.uiText).foregroundStyle(Color.rupuInk).lineLimit(1)
            if def.autoflowEnabled == true {
                Badge("autoflow", tone: Color.status(.running))
            }
            Spacer(minLength: 8)
            Text("\(def.runCount) runs").font(.metaText).foregroundStyle(Color.rupuMute)
            DefRowLaunchButton(action: onLaunch)
        }
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .onTapGesture(perform: onSelect)
        .onHover(perform: DefRowHover.apply)
    }
}

private struct AutoflowDefRow: View {
    let def: AutoflowDefinition
    let onSelect: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Text(def.name).font(.uiText).foregroundStyle(Color.rupuInk).lineLimit(1)
            Badge(def.trigger)
            Spacer(minLength: 8)
            Badge(def.enabled ? "enabled" : "disabled", tone: def.enabled ? Color.status(.running) : Color.rupuMute)
        }
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .onTapGesture(perform: onSelect)
        .onHover(perform: DefRowHover.apply)
    }
}

/// Trailing "Launch" text button — same shape/idiom `RupuLibrary.
/// LaunchButton` establishes, re-derived locally rather than imported (see
/// `DefinitionsTabContent`'s doc comment on why this file doesn't take a
/// `RupuLibrary` dependency for its row chrome).
private struct DefRowLaunchButton: View {
    let action: () -> Void

    var body: some View {
        Button("Launch", action: action)
            .buttonStyle(.plain)
            .font(.metaText.weight(.semibold))
            .foregroundStyle(Color.rupuBrand)
    }
}

/// Shared pointer-cursor hover handler for the three definition rows above
/// — same idiom `RunRow`/`SessionRow`'s own `onHover` closures use (see
/// those types' doc comments), factored to a single static function since
/// none of these three rows has a conditional "is this clickable" gate the
/// way `RunRow` does (every definition row is always navigable).
private enum DefRowHover {
    static func apply(_ hovering: Bool) {
        if hovering {
            NSCursor.pointingHand.push()
        } else {
            NSCursor.pop()
        }
    }
}

// MARK: - Shared row/note views

/// One run row — status dot, workflow name, trigger, duration, cost, and a
/// relative "started" timestamp. Reused by the Overview tab's recent-runs
/// preview and the Runs tab's windowed list. Navigation reuses `ActivityRow
/// (_:APIRunListRow)`'s own `.navigation.route` mapping (`RupuStore`) rather
/// than re-deriving "which route does a run row push" here — the exact
/// "smallest equivalent" the brief calls for instead of pulling in
/// `RupuActivity`'s `ActivityTable` (which needs an `ActivityStore`/
/// `BackendController` for its own inline gate actions, well beyond what
/// this read-only row needs).
private struct RunRow: View {
    let row: APIRunListRow
    let onSelect: (Route) -> Void

    private var activityRow: ActivityRow { ActivityRow(row) }
    private var isClickable: Bool { activityRow.navigation.route != nil }

    var body: some View {
        HStack(spacing: 10) {
            Circle().fill(Color.status(activityRow.status.tone)).frame(width: 6, height: 6)
            Text(row.workflowName)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(row.trigger)
                .font(.metaText)
                .foregroundStyle(Color.rupuDim)
            Text(row.durationMS.map { Fmt.duration(ms: $0) } ?? "—")
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
                .frame(width: 48, alignment: .trailing)
            Text(Fmt.cost(row.usage.costUSD))
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
                .frame(width: 56, alignment: .trailing)
            Text(startedLabel)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
                .frame(width: 72, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .onTapGesture {
            guard isClickable, let route = activityRow.navigation.route else { return }
            onSelect(route)
        }
        // Pointer-cursor hover (review fix, final wave) — matches
        // `ProjectsScreen.ProjectListRow`'s idiom; gated on `isClickable` so
        // a row with no navigable route (e.g. a synthetic session with no
        // `sessionID`) doesn't advertise a click that does nothing.
        .onHover { hovering in
            guard isClickable else { return }
            if hovering {
                NSCursor.pointingHand.push()
            } else {
                NSCursor.pop()
            }
        }
    }

    private var startedLabel: String {
        guard let date = ActivityRow.parseISO(row.startedAt) else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}

/// One session row — same navigation-reuse rationale as `RunRow` above,
/// via `ActivityRow(_:APISessionRow)`.
private struct SessionRow: View {
    let row: APISessionRow
    let onSelect: (Route) -> Void

    private var activityRow: ActivityRow { ActivityRow(row) }
    private var isClickable: Bool { activityRow.navigation.route != nil }

    var body: some View {
        HStack(spacing: 10) {
            Circle().fill(Color.status(activityRow.status.tone)).frame(width: 6, height: 6)
            Text(row.agentName)
                .font(.uiText)
                .foregroundStyle(Color.rupuInk)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(row.model)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
            Text("\(row.totalTurns) turns")
                .font(.metaText)
                .foregroundStyle(Color.rupuMute)
                .frame(width: 64, alignment: .trailing)
            Text(Fmt.cost(row.usage?.costUSD))
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuDim)
                .frame(width: 56, alignment: .trailing)
            Text(startedLabel)
                .font(.dataMono(10))
                .foregroundStyle(Color.rupuMute)
                .frame(width: 72, alignment: .trailing)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .onTapGesture {
            guard let route = activityRow.navigation.route else { return }
            onSelect(route)
        }
        // Pointer-cursor hover (review fix, final wave) — same idiom
        // `RunRow`'s `onHover` above adds; see that one's doc comment.
        .onHover { hovering in
            guard isClickable else { return }
            if hovering {
                NSCursor.pointingHand.push()
            } else {
                NSCursor.pop()
            }
        }
    }

    private var startedLabel: String {
        guard let date = ActivityRow.parseISO(row.createdAt) else { return "—" }
        return Self.relativeFormatter.localizedString(for: date, relativeTo: Date())
    }

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()
}

// MARK: - Shared formatting (header meta line)

/// `@MainActor` global — same rationale `LibraryScreen.swift`'s own
/// `relativeFormatter`/`relativeLabel` pair documents: `RelativeDateTimeFormatter`
/// isn't `Sendable`, and every call site here is already MainActor (called
/// from `View` bodies). Kept as its own copy rather than sharing `RunRow`/
/// `SessionRow`'s private instance-scoped formatters above — those are
/// `private` to their own types.
@MainActor
private let headerRelativeFormatter: RelativeDateTimeFormatter = {
    let formatter = RelativeDateTimeFormatter()
    formatter.unitsStyle = .abbreviated
    return formatter
}()

@MainActor
private func relativeLabel(_ iso: String?) -> String {
    guard let iso, let date = ActivityRow.parseISO(iso) else { return "—" }
    return headerRelativeFormatter.localizedString(for: date, relativeTo: Date())
}

private struct FailedNote: View {
    let message: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Failed to load")
                .font(.noteText)
                .foregroundStyle(Color.status(.failed))
            Text(message)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(3)
        }
    }
}
