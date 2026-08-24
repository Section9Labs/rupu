import SwiftUI
import RupuAPI
import RupuStore
import RupuDesign

/// Detail-pane toolbar: screen title, project-scope picker, time-range
/// picker, search affordance, "+ New run" launcher, live-stream pill,
/// appearance toggle. The ⌘N shortcut for "+ New run" lives on a separate
/// hidden button in `RootView` (so it fires regardless of toolbar focus),
/// not on this visible button — see that type's doc comment.
///
/// Flows-composition Task 2: the scope picker (`Picker`, `.menu`) loads
/// `backend.client()?.projects()` once when it first appears — failure or
/// no client yet leaves `projects` empty, so the picker silently, honestly
/// shows only "All projects" rather than surfacing an error box for a
/// toolbar affordance.
///
/// Flows-composition Task 3: the search button opens `palette` (built
/// lazily by `RootView` once a backend client exists — see that type's
/// `handleHealthChange`). `palette` is `nil` for the brief window before
/// the first healthy connection; the button silently no-ops then, same
/// "no client yet" degrade `scopePicker`'s `loadProjects()` already uses.
struct ShellToolbar: ToolbarContent {
    @Bindable var model: AppModel
    @Binding var showLauncher: Bool
    let backend: BackendController
    let palette: PaletteStore?
    @AppStorage("appearance") private var appearance: String = "system"
    @State private var projects: [APIProjectRow] = []

    var body: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Text(model.route.screenTitle)
                .font(.leadText.weight(.semibold))
                .foregroundStyle(Color.rupuInk)
        }

        ToolbarItem(placement: .navigation) {
            scopePicker
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
            searchButton
            newRunButton
            livePill
            appearancePicker
        }
    }

    private var scopePicker: some View {
        Picker("Scope", selection: $model.scopeWsID) {
            Text("All projects").tag(nil as String?)
            ForEach(projects, id: \.wsID) { project in
                Text(project.name).tag(project.wsID as String?)
            }
        }
        .pickerStyle(.menu)
        .frame(width: 140)
        .task {
            await loadProjects()
        }
    }

    /// Loaded once when the scope picker first appears. No `backend.client()`
    /// yet (shouldn't happen by the time the shell can route here — matches
    /// `ActivityScreen`'s own reasoning) or a failed request both leave
    /// `projects` at its empty default — an honest "All projects only"
    /// picker rather than a toolbar error surface.
    private func loadProjects() async {
        guard let client = backend.client() else { return }
        projects = (try? await client.projects()) ?? []
    }

    /// Search-field-styled button: magnifier + "Search…" + a ⌘K badge.
    /// **No magnifier glyph**: `LucideIcon` (the pinned extracted set) has
    /// no search/magnifying-glass case yet — per the brief, this task
    /// doesn't hand-author path data or extend the extractor (Task 5 owns
    /// extractor changes and can add `search` there); the badge renders
    /// text-only until that lands.
    private var searchButton: some View {
        Button {
            Task { await palette?.open() }
        } label: {
            HStack(spacing: 8) {
                Text("Search…")
                    .font(.uiText)
                    .foregroundStyle(Color.rupuMute)
                Spacer(minLength: 12)
                Text("⌘K")
                    .font(.metaText)
                    .foregroundStyle(Color.rupuMute)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .frame(width: 160)
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(Color.rupuBorderStrong, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }

    private var newRunButton: some View {
        Button("+ New run") {
            showLauncher = true
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
    }

    private var livePill: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(model.liveConnected ? Color.rupuBrand : Color.rupuMute)
                .frame(width: 6, height: 6)
            Text(model.liveConnected ? "Live" : "Offline")
                .font(.metaText)
                .foregroundStyle(model.liveConnected ? Color.rupuBrand700 : Color.rupuMute)
        }
        .padding(.horizontal, 4)
    }

    private var appearancePicker: some View {
        Picker("Appearance", selection: $appearance) {
            Text("System").tag("system")
            Text("Light").tag("light")
            Text("Dark").tag("dark")
        }
        .pickerStyle(.menu)
        .frame(width: 96)
    }
}
