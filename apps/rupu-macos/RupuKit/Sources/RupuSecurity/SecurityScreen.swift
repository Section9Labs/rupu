import SwiftUI
import AppKit
import RupuAPI
import RupuStore
import RupuDesign

/// The Security screen (Phase 5B, Task 3), replacing the `.security`
/// placeholder: segmented Findings/Coverage tabs, both scoped across every
/// registered workspace (unlike `ProjectDetailScreen`'s own Findings tab,
/// which is scoped to one project) — the fleet-wide security surface.
/// `FindingsTabView`/`CoverageTabView` (this module's `FindingsTable.swift`/
/// `CoverageList.swift`) hold each tab's actual content; this file owns the
/// shell, store lifecycle, and lazy per-tab dispatch.
///
/// Owns a `SecurityStore` lifecycle the same lazy-build/`storeClientID`-
/// rebuild convention `ProjectsScreen`/`FleetScreen`/`LibraryScreen` already
/// established.
///
/// **Does NOT need `OverviewScreen`'s cold-launch `.onChange(of: backend.
/// health)` fix** — same reasoning `ProjectsScreen`/`FleetScreen`/
/// `LibraryScreen` document: `.security` is only ever reached by a sidebar
/// click, well after the shell's own connection attempt has already
/// resolved.
///
/// **Lazy tab loading**: the tab panel's `.task(id: tab)` fires
/// `SecurityStore.loadFindingsIfNeeded()`/`loadCoverageIfNeeded()` the first
/// time each tab is actually shown — same recipe `ProjectDetailScreen`'s
/// `tabPanel` uses, simplified to `tab` alone as the id (unlike that
/// screen's `"\(wsID)|\(tab.rawValue)"`, there is no second identity this
/// screen is scoped to — `SecurityStore` is a single global instance, not
/// rebuilt per anything but a backend client swap).
///
/// **No `.onDisappear`/`deactivate()`** — same "one-shot `activate()`,
/// nothing running in the background to tear down" reasoning `LibraryScreen`
/// documents for itself: `SecurityStore` has no reconcile loop.
///
/// **Route-driven tab (Task 0, sidebar disclosure sub-items)**: `tab` is no
/// longer local `@State` — `SecurityTab` moved to `RupuStore.Route` so the
/// sidebar's Findings/Coverage children and this screen's own segmented
/// control both read/write the same `model.route`, the same "tab survives a
/// round trip" contract `AppModel.swift`'s doc comment documents for
/// Activity's kind tabs. A tab click writes `model.route` directly (never
/// `navigate(to:)`) — it's a context switch, not a push, same as every other
/// direct sidebar/tab selection.
public struct SecurityScreen: View {
    @Bindable var model: AppModel
    let backend: BackendController

    @State private var store: SecurityStore?
    @State private var storeClientID: ObjectIdentifier?
    @State private var findingsSort = ListSort<FindingsSortKey>(key: .severity, ascending: false)
    @State private var coverageSort = ListSort<CoverageSortKey>(key: .target, ascending: true)

    public init(model: AppModel, backend: BackendController) {
        self.model = model
        self.backend = backend
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
        .task {
            await activate()
        }
    }

    /// Builds (or rebuilds, on a backend client swap) the store and
    /// activates it — same `storeClientID` recipe `ProjectsScreen`/
    /// `FleetScreen`/`LibraryScreen` use (see `BackendController.
    /// clientIdentity()`'s doc comment).
    private func activate() async {
        guard let client = backend.client() else { return }
        let clientID = backend.clientIdentity()
        if store == nil || storeClientID != clientID {
            store = SecurityStore(client: client)
            storeClientID = clientID
        }
        guard let store else { return }
        await store.activate()
    }

    // MARK: - Layout

    private func content(store: SecurityStore) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            tabPicker
            tabPanel(store: store)
        }
        .padding(16)
    }

    /// `model.route`'s associated tab, defaulting to `.findings` for any
    /// route that isn't `.security(_)` at all (unreached in practice — this
    /// screen only ever renders while `model.route` is `.security(_)` — but
    /// an honest fallback beats a force-unwrap).
    private var tab: SecurityTab {
        if case .security(let tab) = model.route { return tab }
        return .findings
    }

    private var tabBinding: Binding<SecurityTab> {
        Binding(get: { tab }, set: { model.route = .security($0) })
    }

    private var tabPicker: some View {
        Picker("Tab", selection: tabBinding) {
            ForEach(SecurityTab.allCases, id: \.self) { candidate in
                Text(candidate.title).tag(candidate)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: 240)
    }

    private func tabPanel(store: SecurityStore) -> some View {
        tabContent(store: store)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            // See the type doc comment's "Lazy tab loading" section.
            .task(id: tab) {
                await loadTab(tab, store: store)
            }
    }

    /// Dispatches the lazy fetch for whichever tab is now selected.
    private func loadTab(_ tab: SecurityTab, store: SecurityStore) async {
        switch tab {
        case .findings:
            await store.loadFindingsIfNeeded()
        case .coverage:
            await store.loadCoverageIfNeeded()
        }
    }

    @ViewBuilder
    private func tabContent(store: SecurityStore) -> some View {
        switch tab {
        case .findings:
            FindingsTabView(
                findings: store.findings, sort: $findingsSort, onSelect: { model.navigate(to: $0) },
                onRetry: { await store.loadFindings() }
            )
        case .coverage:
            CoverageTabView(
                coverage: store.coverage, sort: $coverageSort, onSelect: { model.navigate(to: $0) },
                onRetry: { await store.loadCoverage() }
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

// MARK: - Shared block chrome (used by `FindingsTable.swift`/`CoverageList.swift`)

/// Package-internal (not `private`) — `FindingsTabView`/`CoverageTabView`
/// live in separate files within this same module and share these two tiny
/// blocks rather than each carrying its own copy, same content every other
/// screen's `loadingBlock`/`emptyBlock` pair renders (`FleetScreen`/
/// `LibraryScreen`), just hoisted out since this screen's version is split
/// across files instead of living in one. The failed counterpart is
/// `RupuDesign.FailedBlock` — shared app-wide, with the retry affordance
/// these purely-local blocks never carried.
@MainActor
func securityLoadingBlock() -> some View {
    HStack {
        Spacer(minLength: 0)
        ProgressView().controlSize(.small)
        Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: 120)
    .panelStyle(.panel)
}

@MainActor
func securityEmptyBlock(_ label: String) -> some View {
    HStack {
        Spacer(minLength: 0)
        Text(label).font(.noteText).foregroundStyle(Color.rupuMute)
        Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: 120)
    .panelStyle(.panel)
}

/// Row tap target + hover cursor — same small idiom `LibraryScreen`'s
/// (private, file-scoped) `rowTapModifiers` establishes, re-hoisted here as
/// package-internal so every navigating row across this module's files can
/// share it: `FindingsTable.swift`'s `FindingRow`, `CoverageList.swift`'s
/// `CoverageRow`, and `CoverageDetailScreen.swift`'s `CoverageFindingRow`.
@MainActor
func securityRowTapModifiers<V: View>(_ view: V, onSelect: @escaping () -> Void) -> some View {
    view
        .contentShape(Rectangle())
        .onTapGesture(perform: onSelect)
        .onHover { hovering in
            if hovering {
                NSCursor.pointingHand.push()
            } else {
                NSCursor.pop()
            }
        }
}
