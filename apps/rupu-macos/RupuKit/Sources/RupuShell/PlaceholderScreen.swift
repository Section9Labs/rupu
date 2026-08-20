import SwiftUI
import RupuDesign

/// Panel-chrome stand-in for a screen that hasn't landed yet. No fake
/// stats, no dead controls — just the title and which phase replaces it.
struct PlaceholderScreen: View {
    let title: String
    let phase: Int

    var body: some View {
        VStack {
            Spacer(minLength: 0)
            VStack(spacing: 8) {
                Text(title)
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundStyle(Color.rupuInk)
                MicroLabel("NOT BUILT YET \u{2014} PHASE \(phase)")
                    .foregroundStyle(Color.rupuMute)
            }
            .padding(40)
            .panelStyle(.panel)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color.rupuBg)
    }
}
