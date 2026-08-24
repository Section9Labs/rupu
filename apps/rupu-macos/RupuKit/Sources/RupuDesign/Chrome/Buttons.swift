import SwiftUI

/// Shared button chrome. Mirrors web's `Button.tsx` variant set (`primary` /
/// `secondary` / `ghost` / `danger-outline`) with the macOS naming this design
/// uses (`outline` for web's `secondary`). Heights sit in the 24–28pt band the
/// web `sm`/`md` sizes occupy; corner radius 6 (`PanelStyle.innerCard`'s
/// radius — buttons are inner-chrome, not panels).
public struct RupuButtonStyle {
    public static var primary: some ButtonStyle {
        ChromeButtonStyle(fill: .rupuBrand, hoverFill: .rupuBrand600, textColor: .white, borderColor: nil)
    }

    /// Same chrome as `primary`, tinted `.rupuOk` instead of brand — the
    /// "primary, ok-filled" variant web's approve control uses (a positive,
    /// confirming action gets the affirmative color rather than the generic
    /// brand tint).
    public static var primaryOk: some ButtonStyle {
        ChromeButtonStyle(fill: .rupuOk, hoverFill: .rupuOk.opacity(0.85), textColor: .white, borderColor: nil)
    }

    public static var outline: some ButtonStyle {
        ChromeButtonStyle(fill: .rupuPanel, hoverFill: .rupuSurfaceHover, textColor: .rupuInk, borderColor: .rupuBorderStrong)
    }

    public static var dangerOutline: some ButtonStyle {
        ChromeButtonStyle(fill: .rupuPanel, hoverFill: .rupuErrBg, textColor: .rupuErr, borderColor: .rupuErr.opacity(0.3))
    }

    public static var ghost: some ButtonStyle {
        ChromeButtonStyle(fill: .clear, hoverFill: .rupuSurfaceHover, textColor: .rupuDim, borderColor: nil)
    }
}

/// Shared render path for every `RupuButtonStyle` variant — only the color set
/// differs. Hover state isn't available on `ButtonStyleConfiguration`, so it's
/// tracked locally via `.onHover` (mouse-driven; trackpad/keyboard focus never
/// sets it, matching AppKit control conventions).
private struct ChromeButtonStyle: ButtonStyle {
    let fill: Color
    let hoverFill: Color
    let textColor: Color
    let borderColor: Color?

    func makeBody(configuration: Configuration) -> some View {
        ChromeButtonBody(configuration: configuration, fill: fill, hoverFill: hoverFill, textColor: textColor, borderColor: borderColor)
    }
}

private struct ChromeButtonBody: View {
    let configuration: ButtonStyleConfiguration
    let fill: Color
    let hoverFill: Color
    let textColor: Color
    let borderColor: Color?

    @State private var isHovering = false

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 6)
        configuration.label
            .font(.uiText)
            .foregroundStyle(textColor)
            .padding(.horizontal, 12)
            .frame(height: 26)
            .background((isHovering ? hoverFill : fill).opacity(configuration.isPressed ? 0.75 : 1))
            .clipShape(shape)
            .overlay(borderColor.map { shape.stroke($0, lineWidth: 1) })
            .onHover { isHovering = $0 }
    }
}
