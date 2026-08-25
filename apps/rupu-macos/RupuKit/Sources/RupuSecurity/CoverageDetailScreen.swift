import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

/// The Coverage Detail screen's tab bar — same text-button-plus-underline
/// idiom `ProjectDetailScreen`'s own (private) tab bar establishes,
/// re-derived locally rather than shared for the same "not worth the
/// indirection for one more screen" reasoning that file's own doc comment
/// already gives for its version.
public enum CoverageDetailTab: String, CaseIterable, Sendable {
    case overview, catalog

    var title: String {
        switch self {
        case .overview: "Overview"
        case .catalog: "Catalog"
        }
    }
}

/// The Coverage Detail screen (Phase 5B, Task 4), pushed from the Security
/// screen's Coverage-tab row tap (`.coverageDetail(target:wsID:)`, `Coverage
/// List.swift`'s `CoverageRow`): header (back chevron, breadcrumb, project/
/// assertion-lines/findings figures, a catalog chip) + a two-tab panel —
/// Overview (this target's concern assertions, mono lines, plus its
/// findings) and Catalog (the flattened concern catalog effective for this
/// target).
///
/// **Two tabs only, deliberately — no stubs for the rest.** The umbrella
/// spec's audit (per-file heatmap)/gap (catalog-vs-assertion delta)/runs
/// (per-target run history)/diff (against a prior run) cuts are NOT built
/// this task, and there is no placeholder tab for any of them: a tab that
/// always renders "coming soon" is dead chrome regardless of how honest its
/// label is — the same "no dead controls" posture `ProjectDetailScreen`'s
/// `CoverageTabContent` doc comment already takes for its own placeholder
/// (that one exists only because the *spec* explicitly calls for a Coverage
/// tab stub on the Project Detail screen; nothing here calls for stub tabs
/// on THIS screen). `APICoverageDetail.files` — the per-file heatmap
/// `CoverageDetailStore.detail` already carries as part of one fetch — is
/// likewise unrendered: it's the future audit tab's content, not Overview's.
///
/// Owns a `CoverageDetailStore` lifecycle the same way `ProjectDetailScreen`
/// owns a `ProjectDetailStore`: built lazily once `backend.client()` exists,
/// rebuilt on a `(target, wsID)` change OR a backend client swap (the same
/// `storeTarget`/`storeWsID`/`storeClientID` recipe that screen uses for its
/// own `storeWsID`/`storeClientID`), `store.activate()`d on appear (loads
/// `detail` only — `catalog` is the lazy Catalog-tab block; see
/// `CoverageDetailStore`'s own doc comment for its `hasCatalog` gating).
///
/// **Does NOT need `OverviewScreen`'s cold-launch fix** — same reasoning
/// `ProjectDetailScreen`/`RunDetailScreen` already document: `.coverageDetail`
/// is only ever reached by pushing from `.security`'s Coverage tab, never a
/// cold-launch route.
public struct CoverageDetailScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController
    let target: String
    let wsID: String

    @State private var store: CoverageDetailStore?
    @State private var storeTarget: String?
    @State private var storeWsID: String?
    @State private var storeClientID: ObjectIdentifier?
    @State private var tab: CoverageDetailTab = .overview

    public init(model: AppModel, backend: BackendController, target: String, wsID: String) {
        self.model = model
        self.backend = backend
        self.target = target
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
        .task(id: "\(target)|\(wsID)") {
            await activate()
        }
    }

    /// Builds (or rebuilds, on a `target`/`wsID` change OR a backend client
    /// swap) the store, resets `tab` back to `.overview` for the new target,
    /// and activates it. Same `storeTarget`/`storeWsID`/`storeClientID`
    /// recipe `ProjectDetailScreen` uses for its own `storeWsID`/
    /// `storeClientID` — see that type's `activate()` doc comment.
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if storeTarget != target || storeWsID != wsID || storeClientID != clientID {
            store = CoverageDetailStore(target: target, wsID: wsID, client: client)
            storeTarget = target
            storeWsID = wsID
            storeClientID = clientID
            tab = .overview
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: CoverageDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            header(store: store)
            tabPanel(store: store)
        }
        .padding(16)
    }

    // MARK: - Header

    private func header(store: CoverageDetailStore) -> some View {
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
                    Text("Security ▸ Coverage ▸ \(detail.targetID)")
                        .font(.leadText)
                        .foregroundStyle(Color.rupuInk)
                        .lineLimit(1)
                        .truncationMode(.middle)
                } else {
                    Text("Security ▸ Coverage")
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
            }
        }
    }

    private func factsLine(_ detail: APICoverageDetail) -> some View {
        HStack(spacing: 14) {
            fact("Project", detail.project)
            fact("Assertion lines", Fmt.count(detail.assertionLines))
            fact("Findings", Fmt.count(detail.findings.count))
            catalogChip(detail.hasCatalog)
            Spacer(minLength: 0)
        }
    }

    private func fact(_ label: String, _ value: String) -> some View {
        HStack(spacing: 4) {
            Text(label).font(.metaText).foregroundStyle(Color.rupuMute)
            Text(value).font(.dataMono(11)).foregroundStyle(Color.rupuInk)
        }
    }

    /// Same tone convention `CoverageList.swift`'s `CoverageRow.catalogCell`
    /// uses for the Security screen's own Coverage table — this header chip
    /// is that same signal, carried onto the detail screen.
    @ViewBuilder
    private func catalogChip(_ hasCatalog: Bool) -> some View {
        if hasCatalog {
            Badge("catalog", tone: Color.rupuBrand)
        } else {
            Badge("no catalog", tone: Color.rupuMute)
        }
    }

    // MARK: - Tab panel

    private func tabPanel(store: CoverageDetailStore) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            CoverageDetailTabBar(tab: $tab)
            Divider()
            tabContent(store: store)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .panelStyle(.panel)
        // Folds `target`/`wsID` into the id alongside `tab`, same
        // "wsID + tab together" reasoning `ProjectDetailScreen.tabPanel`'s
        // doc comment gives — a target/wsID change that leaves `tab`
        // unchanged (still `.overview` after `activate()` resets it) would
        // otherwise miss re-dispatching the lazy Catalog fetch.
        //
        // Also folds in whether `store.detail` has resolved
        // (`store.detail.value != nil`, true only for `.content`) —
        // `loadCatalogIfNeeded()` is additionally gated on `detail.hasCatalog`
        // (`CoverageDetailStore`'s own doc comment), which is unknown until
        // `detail` lands. Without this, selecting Catalog while `detail` is
        // still `.loading` (the header's `activate()` fetch, still in
        // flight) dispatches a no-op — `catalogRequested` never latches
        // true — and once `detail` resolves nothing re-fires this task
        // (`tab`/`target`/`wsID` are all unchanged), stranding the Catalog
        // tab at `.loading` forever. Appending detail's resolution to the id
        // makes that transition its own re-fire.
        .task(id: "\(target)|\(wsID)|\(tab.rawValue)|\(store.detail.value != nil)") {
            await loadTab(tab, store: store)
        }
    }

    /// Dispatches the lazy fetch for whichever tab is now selected —
    /// `.overview` needs nothing beyond `detail` (already loaded by
    /// `activate()`).
    private func loadTab(_ tab: CoverageDetailTab, store: CoverageDetailStore) async {
        switch tab {
        case .overview:
            break
        case .catalog:
            await store.loadCatalogIfNeeded()
        }
    }

    @ViewBuilder
    private func tabContent(store: CoverageDetailStore) -> some View {
        switch tab {
        case .overview:
            OverviewTabContent(detail: store.detail, onSelect: { model.navigate(to: $0) })
        case .catalog:
            CatalogTabContent(detail: store.detail, catalog: store.catalog)
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

private struct CoverageDetailTabBar: View {
    @Binding var tab: CoverageDetailTab

    var body: some View {
        HStack(spacing: 18) {
            ForEach(CoverageDetailTab.allCases, id: \.self) { candidate in
                tabButton(candidate)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
    }

    private func tabButton(_ candidate: CoverageDetailTab) -> some View {
        let active = tab == candidate
        return Button {
            tab = candidate
        } label: {
            VStack(spacing: 6) {
                Text(candidate.title)
                    .font(.uiText)
                    .foregroundStyle(active ? Color.rupuInk : Color.rupuDim)
                Rectangle()
                    .fill(active ? Color.rupuBrand : Color.clear)
                    .frame(height: 2)
            }
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Overview tab

/// Assertions (mono concern id + file path, per the brief) plus this
/// target's own findings — `APICoverageFinding`, NOT `RupuAPI.APIFinding`.
/// `CoverageFindingRow` below reuses `FindingsTable.swift`'s
/// `findingNavigationRoute(surface:runID:)` (package-internal in this same
/// module) for its route, the one piece that genuinely is shared between
/// the two finding shapes — `declaredBy` carries the identical `{run_id,
/// model, surface}` attribution both types use. The row rendering itself is
/// NOT reused from that file's `FindingRow`: `APICoverageFinding` has no
/// `project`/`targetID`/`workflowName` (this screen is already scoped to
/// one target, so repeating it per row would be noise) and instead carries
/// `concernID`/`scope`/a structured `evidence` block that `APIFinding` has
/// no equivalent for — forcing one row type to cover both shapes would mean
/// either padding `APICoverageFinding` rows with blank columns or growing
/// `FindingRow` conditional on which type it got, worse than two small,
/// honest row types.
private struct OverviewTabContent: View {
    let detail: BlockState<APICoverageDetail>
    let onSelect: (Route) -> Void

    var body: some View {
        Group {
            switch detail {
            case .loading:
                securityLoadingBlock()
            case .failed(let message):
                securityFailedBlock(message, subject: "coverage detail")
            case .empty:
                securityEmptyBlock("No data")
            case .content(let value):
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        assertionsSection(value.assertions)
                        findingsSection(value.findings)
                    }
                    .padding(12)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func assertionsSection(_ assertions: [APICoverageAssertion]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Assertions")
            if assertions.isEmpty {
                Text("No assertions recorded").font(.noteText).foregroundStyle(Color.rupuMute)
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(assertions.enumerated()), id: \.offset) { _, assertion in
                        AssertionRow(assertion: assertion)
                        Divider()
                    }
                }
            }
        }
    }

    private func findingsSection(_ findings: [APICoverageFinding]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Findings")
            if findings.isEmpty {
                Text("No findings").font(.noteText).foregroundStyle(Color.rupuMute)
            } else {
                VStack(spacing: 0) {
                    ForEach(findings, id: \.id) { finding in
                        CoverageFindingRow(finding: finding, onSelect: onSelect)
                        Divider()
                    }
                }
            }
        }
    }
}

/// One `concern_id × file_path` assertion — mono concern id + file path
/// (the brief's "mono lines"), a status-tone edge bar, and the evidence
/// summary beneath (free text, so sans rather than mono — v2's "mono
/// reserved for DATA" rule, `Typography.swift`'s own doc comment).
private struct AssertionRow: View {
    let assertion: APICoverageAssertion

    /// `status` is one of `"clean"` | `"finding"` | `"examined"` |
    /// `"not_applicable"` (`APICoverageAssertion`'s own doc comment).
    /// `"finding"` reads as a failure tone, `"clean"`/`"examined"` as
    /// affirmative/neutral progress tones, and anything else (only
    /// `"not_applicable"` today, plus any future unrecognized value) falls
    /// back to a quiet dim tone rather than guessing.
    private var statusTone: Color {
        switch assertion.status {
        case "finding": Color.status(.failed)
        case "clean": Color.status(.done)
        case "examined": Color.rupuBrand
        default: Color.rupuMute
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            Rectangle()
                .fill(statusTone)
                .frame(width: 2)
                .frame(width: 10, alignment: .leading)
                .padding(.trailing, 8)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    Text(assertion.concernID)
                        .font(.dataMono(11))
                        .foregroundStyle(Color.rupuInk)
                    Badge(assertion.status, tone: statusTone)
                }
                Text(assertion.filePath)
                    .font(.dataMono(10))
                    .foregroundStyle(Color.rupuMute)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if !assertion.evidence.summary.isEmpty {
                    Text(assertion.evidence.summary)
                        .font(.noteText)
                        .foregroundStyle(Color.rupuDim)
                        .lineLimit(2)
                }
            }
        }
        .padding(.vertical, 6)
    }
}

/// See `OverviewTabContent`'s doc comment for why this is its own row type
/// rather than a reuse of `FindingsTable.swift`'s `FindingRow` — same
/// severity-edge-bar visual language, `findingNavigationRoute(surface:
/// runID:)` reused verbatim for navigation, fields adapted to what
/// `APICoverageFinding` actually carries.
private struct CoverageFindingRow: View {
    let finding: APICoverageFinding
    let onSelect: (Route) -> Void

    private var severity: Severity { Severity(wireString: finding.severity) }
    private var route: Route? { findingNavigationRoute(surface: finding.declaredBy.surface, runID: finding.declaredBy.runID) }

    var body: some View {
        if let route {
            securityRowTapModifiers(rowContent, onSelect: { onSelect(route) })
        } else {
            rowContent
        }
    }

    private var rowContent: some View {
        HStack(spacing: 0) {
            Rectangle()
                .fill(Color.severity(severity))
                .frame(width: 2)
                .frame(width: 10, alignment: .leading)
                .padding(.trailing, 8)
            VStack(alignment: .leading, spacing: 2) {
                Text(finding.summary)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuInk)
                    .lineLimit(2)
                HStack(spacing: 8) {
                    if let filePath = finding.filePath {
                        Text(fileLabel(filePath, finding.lineRange))
                            .font(.dataMono(10))
                            .foregroundStyle(Color.rupuMute)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    if let concernID = finding.concernID {
                        Text(concernID)
                            .font(.dataMono(10))
                            .foregroundStyle(Color.rupuDim)
                    }
                }
            }
        }
        .padding(.vertical, 6)
    }

    private func fileLabel(_ path: String, _ lineRange: [UInt32]?) -> String {
        guard let lineRange, lineRange.count == 2 else { return path }
        return "\(path):\(lineRange[0])-\(lineRange[1])"
    }
}

// MARK: - Catalog tab

/// The flattened concern catalog effective for this target — lazy-fetched
/// (`CoverageDetailStore.loadCatalogIfNeeded()`) ONLY when `detail` has
/// resolved `hasCatalog == true`. When `false`, this renders `noCatalogNote`
/// straight off `detail` and never even looks at `catalog` — that block
/// stays permanently `.loading` in this case (no fetch ever dispatched, see
/// the store's own doc comment), which would otherwise misrender as a
/// spinner that never resolves if this view read it directly.
private struct CatalogTabContent: View {
    let detail: BlockState<APICoverageDetail>
    let catalog: BlockState<APICoverageCatalog>

    var body: some View {
        Group {
            switch detail {
            case .loading:
                securityLoadingBlock()
            case .failed(let message):
                securityFailedBlock(message, subject: "coverage detail")
            case .empty:
                securityEmptyBlock("No data")
            case .content(let value):
                if value.hasCatalog {
                    catalogContent
                } else {
                    noCatalogNote
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    @ViewBuilder
    private var catalogContent: some View {
        switch catalog {
        case .loading:
            securityLoadingBlock()
        case .failed(let message):
            securityFailedBlock(message, subject: "catalog")
        case .empty:
            securityEmptyBlock("No concerns in catalog")
        case .content(let value):
            ScrollView {
                VStack(spacing: 0) {
                    ForEach(Array(value.concerns.enumerated()), id: \.offset) { _, concern in
                        ConcernRow(concern: concern, source: value.sources[concern.id], renderMode: value.renderModes[concern.id])
                        Divider()
                    }
                }
                .padding(.vertical, 4)
            }
        }
    }

    private var noCatalogNote: some View {
        VStack(spacing: 8) {
            Spacer(minLength: 0)
            Text("No catalog").font(.noteText.weight(.semibold)).foregroundStyle(Color.rupuInk)
            Text("This target has no concern catalog resolved").font(.noteText).foregroundStyle(Color.rupuMute)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, minHeight: 120, maxHeight: .infinity)
    }
}

/// One resolved concern — name/id, source (template name or `"inline"`),
/// description, tags, and the requested render mode. `source`/`renderMode`
/// are looked up by `concern.id` in `APICoverageCatalog.sources`/
/// `renderModes` (both `[String: String]`, keyed by concern id per that
/// type's doc comment) — absent for a concern id the maps don't cover,
/// rendered as nothing rather than a fabricated placeholder.
private struct ConcernRow: View {
    let concern: APICoverageConcern
    let source: String?
    let renderMode: String?

    private var severity: Severity { Severity(wireString: concern.severity) }

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            Rectangle()
                .fill(Color.severity(severity))
                .frame(width: 2)
                .frame(width: 10, alignment: .leading)
                .padding(.trailing, 8)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    Text(concern.name).font(.uiText).foregroundStyle(Color.rupuInk)
                    Text(concern.id).font(.dataMono(10)).foregroundStyle(Color.rupuMute)
                    if let source {
                        Badge(source)
                    }
                }
                Text(concern.description)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuDim)
                    .lineLimit(2)
                HStack(spacing: 8) {
                    ForEach(concern.tags, id: \.self) { tag in
                        Badge(tag)
                    }
                    if let renderMode {
                        Text(renderMode).font(.dataMono(10)).foregroundStyle(Color.rupuMute)
                    }
                    Spacer(minLength: 0)
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

// MARK: - Shared header failure note

/// Same rendering `ProjectDetailScreen`'s (private) `FailedNote` uses for
/// its own header — re-derived locally rather than shared, same "not worth
/// the indirection for one more screen" reasoning that type's neighbors
/// already give.
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
