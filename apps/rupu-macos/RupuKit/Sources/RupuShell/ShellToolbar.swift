import SwiftUI
import RupuStore
import RupuDesign

/// Detail-pane toolbar: screen title, time-range picker, live-stream pill,
/// appearance toggle. Project-scope picker, search, and Edit are
/// deliberately absent this phase — there is no backend to wire them to.
struct ShellToolbar: ToolbarContent {
    @Bindable var model: AppModel
    @AppStorage("appearance") private var appearance: String = "system"

    var body: some ToolbarContent {
        ToolbarItem(placement: .navigation) {
            Text(model.route.screenTitle)
                .font(.system(size: 13, weight: .semibold))
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
            livePill
            appearancePicker
        }
    }

    private var livePill: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(model.liveConnected ? Color.rupuBrand : Color.rupuMute)
                .frame(width: 6, height: 6)
            MicroLabel(model.liveConnected ? "Live" : "Offline")
                .foregroundStyle(model.liveConnected ? Color.rupuBrandHi : Color.rupuMute)
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
