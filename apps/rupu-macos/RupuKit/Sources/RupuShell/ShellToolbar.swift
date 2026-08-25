import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign
import RupuOverview

/// Detail-pane toolbar: project-scope menu, time-range picker, the
/// Overview-only Customize menu, search, "+ New Run" launcher, and the
/// live-stream status. The screen title is not an item here — `RootView`
/// sets `.navigationTitle` on the detail pane, which puts the title in
/// the toolbar's leading slot (and keeps the window title in sync) the
/// way native macOS apps do.
///
/// Toolbar controls use SF Symbols with visible text labels
/// (`.titleAndIcon`), per the Finder/Mail toolbar idiom. Lucide glyphs
/// remain the icon contract for the sidebar and pane content (web
/// parity); window chrome is deliberately native. Appearance selection
/// lives in Settings (⌘,), not here.
///
/// The ⌘N/⌘K shortcuts live on separate hidden buttons in `RootView` (so
/// they fire regardless of toolbar focus), not on the visible buttons —
/// see that type's doc comment.
///
/// Flows-composition Task 2: the scope menu loads
/// `backend.client()?.projects()` once when it first appears — failure or
/// no client yet leaves `projects` empty, so the menu silently, honestly
/// offers only "All Projects" rather than surfacing an error box for a
/// toolbar affordance.
///
/// Flows-composition Task 3: the search button opens `palette` (built
/// lazily by `RootView` once a backend client exists — see that type's
/// `handleHealthChange`). `palette` is `nil` for the brief window before
/// the first healthy connection; the button silently no-ops then, same
/// "no client yet" degrade `scopeMenu`'s `loadProjects()` already uses.
struct ShellToolbar: ToolbarContent {
    @Bindable var model: AppModel
    @Binding var showLauncher: Bool
    let backend: BackendController
    let palette: PaletteStore?
    @AppStorage(OverviewWidgets.storageKey) private var overviewWidgetsData: Data = Data()
    @State private var projects: [APIProjectRow] = []

    var body: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            scopeMenu
        }

        ToolbarItem(placement: .principal) {
            Picker("Range", selection: $model.range) {
                ForEach(TimeRange.allCases, id: \.self) { range in
                    Text(range.rawValue).tag(range)
                }
            }
            .pickerStyle(.segmented)
            .frame(width: 160)
        }

        ToolbarItemGroup(placement: .primaryAction) {
            if case .overview = model.route {
                customizeMenu
            }
            searchButton
            newRunButton
            liveStatus
        }
    }

    /// Overview-only Customize menu (Task 6): five checkmark toggles over
    /// `OverviewWidgets`' visibility fields — keyed off `model.route` being
    /// `.overview` itself (a real route-case check, not a string compare
    /// against `screenTitle`, which is just display text and could
    /// coincidentally collide with another screen's title). `Toggle` inside
    /// a `Menu` renders as a checkmark item on macOS, same chrome as any
    /// system menu's option toggles.
    ///
    /// Reads/writes `overviewWidgetsData` directly (this view's own
    /// `@AppStorage(OverviewWidgets.storageKey)` declaration) — the same
    /// `"appearance"` trick `SettingsView` uses to keep two unrelated views
    /// in sync on one `UserDefaults` key with no custom observation code:
    /// `OverviewScreen` declares the identical `@AppStorage` key
    /// independently, so a toggle here is reflected there automatically the
    /// next time SwiftUI re-evaluates its body.
    private var customizeMenu: some View {
        Menu {
            Toggle("Needs you", isOn: overviewWidgetToggle(\.needsYou))
            Toggle("Instruments", isOn: overviewWidgetToggle(\.instruments))
            Toggle("Charts", isOn: overviewWidgetToggle(\.charts))
            Toggle("Cycle summary", isOn: overviewWidgetToggle(\.cycles))
            Toggle("Fleet strip", isOn: overviewWidgetToggle(\.fleet))
        } label: {
            Label("Customize", systemImage: "slider.horizontal.3")
                .labelStyle(.titleAndIcon)
        }
        .help("Choose which Overview widgets are shown")
    }

    private func overviewWidgetToggle(_ keyPath: WritableKeyPath<OverviewWidgets, Bool>) -> Binding<Bool> {
        Binding(
            get: { OverviewWidgets.decode(overviewWidgetsData)[keyPath: keyPath] },
            set: { newValue in
                var widgets = OverviewWidgets.decode(overviewWidgetsData)
                widgets[keyPath: keyPath] = newValue
                overviewWidgetsData = widgets.encoded
            }
        )
    }

    private var scopeTitle: String {
        guard let wsID = model.scopeWsID,
              let project = projects.first(where: { $0.wsID == wsID }) else {
            return "All Projects"
        }
        return project.name
    }

    private var scopeMenu: some View {
        Menu {
            Picker("Scope", selection: $model.scopeWsID) {
                Text("All Projects").tag(nil as String?)
                ForEach(projects, id: \.wsID) { project in
                    Text(project.name).tag(project.wsID as String?)
                }
            }
            .pickerStyle(.inline)
            .labelsHidden()
        } label: {
            Label(scopeTitle, systemImage: "folder")
                .labelStyle(.titleAndIcon)
        }
        .help("Filter to a project")
        .task {
            await loadProjects()
        }
    }

    /// Loaded once when the scope menu first appears. No `backend.client()`
    /// yet (shouldn't happen by the time the shell can route here — matches
    /// `ActivityScreen`'s own reasoning) or a failed request both leave
    /// `projects` at its empty default — an honest "All Projects only"
    /// menu rather than a toolbar error surface.
    private func loadProjects() async {
        guard let client = backend.client() else { return }
        projects = (try? await client.projects()) ?? []
    }

    private var searchButton: some View {
        Button {
            Task { await palette?.open() }
        } label: {
            Label("Search", systemImage: "magnifyingglass")
                .labelStyle(.titleAndIcon)
        }
        .help("Search (⌘K)")
    }

    private var newRunButton: some View {
        Button {
            // Close the palette first (see the hidden ⌘N button in
            // `RootView`) so a launcher sheet opened over it doesn't strand
            // an unreachable Esc-close behind the sheet.
            palette?.close()
            showLauncher = true
        } label: {
            Label("New Run", systemImage: "plus")
                .labelStyle(.titleAndIcon)
        }
        .buttonStyle(.borderedProminent)
        .help("New run (⌘N)")
    }

    private var liveStatus: some View {
        Label(model.liveConnected ? "Live" : "Offline",
              systemImage: "dot.radiowaves.left.and.right")
            .labelStyle(.titleAndIcon)
            .font(.metaText)
            .foregroundStyle(model.liveConnected ? Color.rupuBrand700 : Color.rupuMute)
            .help(model.liveConnected ? "Event stream connected" : "Event stream disconnected")
    }
}
