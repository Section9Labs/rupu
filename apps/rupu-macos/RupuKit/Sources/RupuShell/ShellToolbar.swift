import SwiftUI
import RupuStore
import RupuDesign

/// Detail-pane toolbar: screen title, time-range picker, "+ New run"
/// launcher, live-stream pill, appearance toggle. Project-scope picker and
/// search are deliberately absent this phase — there is no backend to wire
/// them to. The ⌘N shortcut for "+ New run" lives on a separate hidden
/// button in `RootView` (so it fires regardless of toolbar focus), not on
/// this visible button — see that type's doc comment.
struct ShellToolbar: ToolbarContent {
    @Bindable var model: AppModel
    @Binding var showLauncher: Bool
    @AppStorage("appearance") private var appearance: String = "system"

    var body: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Text(model.route.screenTitle)
                .font(.leadText.weight(.semibold))
                .foregroundStyle(Color.rupuInk)
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
            newRunButton
            livePill
            appearancePicker
        }
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
