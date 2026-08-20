import SwiftUI

public extension Font {
    /// Uppercase micro-caption used for field labels and section headers.
    static let microLabel = Font.system(size: 10, design: .monospaced).weight(.medium)
    /// Body monospace used for run IDs, SHAs, and other identifiers.
    static let identifier = Font.system(size: 11.5, design: .monospaced)
    /// Monospaced-digit numeral for counters and durations, sized per call site.
    static func numeral(size: CGFloat) -> Font {
        Font.system(size: size, design: .monospaced).monospacedDigit()
    }
}

/// Renders `text` uppercased and tracked out using `Font.microLabel`.
public struct MicroLabel: View {
    private let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        Text(text)
            .font(.microLabel)
            .textCase(.uppercase)
            .kerning(1.2)
    }
}

/// Panel chrome: `Color.rupuPanel` background, a 1px `Color.rupuBorder` stroke, no shadow.
/// `.innerCard` is the smaller-radius variant used for cards nested inside a panel.
public struct PanelStyle: ViewModifier {
    public enum Variant {
        case panel
        case innerCard

        var cornerRadius: CGFloat {
            switch self {
            case .panel: 8
            case .innerCard: 6
            }
        }
    }

    private let variant: Variant

    public init(_ variant: Variant = .panel) {
        self.variant = variant
    }

    public func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: variant.cornerRadius)
        content
            .background(Color.rupuPanel)
            .clipShape(shape)
            .overlay(shape.stroke(Color.rupuBorder, lineWidth: 1))
    }
}

public extension View {
    func panelStyle(_ variant: PanelStyle.Variant = .panel) -> some View {
        modifier(PanelStyle(variant))
    }
}
