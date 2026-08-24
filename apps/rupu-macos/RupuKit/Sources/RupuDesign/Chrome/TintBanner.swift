import SwiftUI

/// Tone-tinted callout banner — a `toneBg` fill with a `tone/30%` 1px border,
/// radius 7 (`PanelStyle.panel`'s radius), 16/12 padding (horizontal/vertical).
/// Used for inline error/warning/info callouts wherever a full `PanelStyle`
/// panel would be too heavy.
public struct TintBanner<Content: View>: View {
    private let tone: Color
    private let toneBg: Color
    private let content: Content

    public init(tone: Color, toneBg: Color, @ViewBuilder content: () -> Content) {
        self.tone = tone
        self.toneBg = toneBg
        self.content = content()
    }

    public var body: some View {
        let shape = RoundedRectangle(cornerRadius: 7)
        content
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(toneBg)
            .clipShape(shape)
            .overlay(shape.stroke(tone.opacity(0.3), lineWidth: 1))
    }
}
