import SwiftUI

/// v2 type scale: sans for ALL UI text, mono reserved for DATA (identifiers, SHAs, numerals —
/// values that benefit from fixed-width alignment). Four sans sizes cover every UI role; anything
/// needing a size outside this scale should get a new named case here, not an ad hoc `.system(size:)`.
public extension Font {
    /// Sans, 10pt — the smallest UI text (meta rows: timestamps, counts, secondary labels).
    static let metaText = Font.system(size: 10)
    /// Sans, 11pt — secondary/note text.
    static let noteText = Font.system(size: 11)
    /// Sans, 12pt — standard UI text; the default body size for most controls and copy.
    static let uiText = Font.system(size: 12)
    /// Sans, 13pt — lead/emphasis text (titles, primary values).
    static let leadText = Font.system(size: 13)

    /// Monospaced + monospaced-digit font for DATA, sized per call site.
    static func dataMono(_ size: CGFloat) -> Font {
        Font.system(size: size, design: .monospaced).monospacedDigit()
    }

    /// Body monospace used for run IDs, SHAs, and other identifiers.
    @available(*, deprecated, message: "migrate to Font.dataMono (Task 5)")
    static let identifier = Font.dataMono(11.5)
    /// Monospaced-digit numeral for counters and durations, sized per call site.
    @available(*, deprecated, message: "migrate to Font.dataMono (Task 5)")
    static func numeral(size: CGFloat) -> Font {
        dataMono(size)
    }
}

/// Mono 10pt, uppercase, kerning 1.2, `.rupuMute` — the ONLY sanctioned uppercase-tracked
/// element in the v2 type system. Used for field labels and section headers.
public struct Eyebrow: View {
    private let text: String

    public init(_ text: String) {
        self.text = text
    }

    /// The uppercased text this view renders — kept as a (non-public) computed property, rather
    /// than folded straight into `body`, so the uppercasing is assertable without a SwiftUI
    /// render pass (`@testable import`).
    var displayText: String { text.uppercased() }

    public var body: some View {
        Text(displayText)
            .font(.dataMono(10))
            .kerning(1.2)
            .foregroundStyle(Color.rupuMute)
    }
}

/// Pre-v2 uppercase micro-caption. Deliberately NOT re-implemented in terms of `Eyebrow` — its
/// render must stay byte-for-byte unchanged (mono 10 *medium* weight, uppercase via `.textCase`,
/// kerning 1.2, no forced color) so the many existing call sites that rely on inheriting or
/// overriding the ambient foreground don't shift before Task 5 sweeps each one onto `Eyebrow`
/// (the call sites that are true eyebrow labels) or a plain sans `Text` (everything else).
@available(*, deprecated, message: "migrate to Eyebrow or sans Text (Task 5)")
public struct MicroLabel: View {
    private let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        Text(text)
            .font(.system(size: 10, design: .monospaced).weight(.medium))
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
            case .panel: 7
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
