import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign
import RupuOverview

/// Detail-pane toolbar: project-scope menu, time-range picker, the
/// Overview Customize menu, search, and the "+ New Run" launcher —
/// interactive controls only, deliberately. The live-stream status lives
/// in `Sidebar`'s footer with the other passive status lines: a
/// non-interactive item in a row of buttons has no honest chrome (it
/// reads as a broken or disabled button whatever it's styled as).
/// The screen title is not an item here — `RootView`
/// sets `.navigationTitle` on the detail pane, which puts the title in
/// the toolbar's leading slot (and keeps the window title in sync) the
/// way native macOS apps do.
///
/// This is `CustomizableToolbarContent`, attached with `.toolbar(id:)` in
/// `RootView`, so users get the native right-click → "Customize Toolbar…"
/// / display-mode menu. Two contracts fall out of that:
///
/// - Every item's `id` is what macOS keys the saved arrangement on —
///   treat the strings as persisted contract; renaming one silently drops
///   it from users' saved layouts.
/// - The item set must be static (customizable builders reject `if`), so
///   the Overview-only Customize menu is always present and `.disabled`
///   off-Overview — the standard macOS idiom for contextual toolbar items
///   (Mail's Reply with no selection), not a route-conditional item. Do
///   NOT reintroduce a second plain `.toolbar {}` for conditional items:
///   mixing plain and `id:` toolbar content on the same view makes
///   AppKit drop "Customize Toolbar…" entirely (verified empirically on
///   macOS 26).
///
/// Toolbar controls carry a `Label` (SF Symbol + title) but do NOT force
/// a label style: the toolbar's display mode ("Icon Only" / "Icon and
/// Text" in its context menu) decides whether the title renders beneath
/// the icon. Forcing `.titleAndIcon` inside the control — the pre-fix
/// idiom — printed the title twice in Icon-and-Text mode, once inside
/// the button and again below it. Lucide glyphs remain the icon contract
/// for the sidebar and pane content (web parity); window chrome is
/// deliberately native. Appearance selection lives in Settings (⌘,), not
/// here.
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
struct ShellToolbar: CustomizableToolbarContent {
    @Bindable var model: AppModel
    @Binding var showLauncher: Bool
    let backend: BackendController
    let palette: PaletteStore?
    @AppStorage(OverviewWidgets.storageKey) private var overviewWidgetsData: Data = Data()
    @State private var projects: [APIProjectRow] = []

    var body: some CustomizableToolbarContent {
        ToolbarItem(id: "scope", placement: .navigation) {
            scopeMenu
        }

        ToolbarItem(id: "range", placement: .principal) {
            rangeControl
        }

        ToolbarItem(id: "customizeOverview", placement: .primaryAction) {
            customizeMenu
                .disabled(!isOverview)
        }

        ToolbarItem(id: "search", placement: .primaryAction) {
            searchButton
        }

        ToolbarItem(id: "newRun", placement: .primaryAction) {
            newRunButton
        }
    }

    private var isOverview: Bool {
        if case .overview = model.route { return true }
        return false
    }

    /// A bare segmented picker — no fixed width, no wrapper chrome. The
    /// toolbar renders this as one unified segmented control (the Finder
    /// view-switcher idiom), which is the "single area" look; the previous
    /// `.frame(width: 160)` + in-control text treatment read as buttons
    /// stacked inside a capsule. (A `ControlGroup` wrapper was tried first
    /// and collapses to zero width inside a customizable toolbar item.)
    private var rangeControl: some View {
        Picker("Range", selection: $model.range) {
            ForEach(TimeRange.allCases, id: \.self) { range in
                Text(range.rawValue).tag(range)
            }
        }
        .pickerStyle(.segmented)
        .help("Time range for lists and charts")
    }

    /// Overview Customize menu (Task 6): five checkmark toggles over
    /// `OverviewWidgets`' visibility fields — `.disabled` off-Overview via
    /// a real route-case check (not a string compare against
    /// `screenTitle`, which is just display text and could coincidentally
    /// collide with another screen's title). `Toggle` inside a `Menu`
    /// renders as a checkmark item on macOS, same chrome as any system
    /// menu's option toggles.
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
        }
        .buttonStyle(.borderedProminent)
        .help("New run (⌘N)")
    }

}
