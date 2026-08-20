import SwiftUI
import RupuShell
import RupuStore

@main
struct RupuApp: App {
    @State private var model = AppModel()
    @AppStorage("appearance") private var appearance: String = "system"

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
                .frame(minWidth: 1150, minHeight: 760)
                .preferredColorScheme(preferredColorScheme)
        }
        .defaultSize(width: 1440, height: 900)

        Settings {
            SettingsView()
        }
    }

    private var preferredColorScheme: ColorScheme? {
        switch appearance {
        case "light": .light
        case "dark": .dark
        default: nil
        }
    }
}
