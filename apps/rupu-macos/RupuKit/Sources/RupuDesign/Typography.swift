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

    /// Sans, 14pt semibold — card/section subheads one step above `leadText`
    /// (e.g. the Launcher's onboarding-card titles).
    static let subheadText = Font.system(size: 14, weight: .semibold)
    /// Sans, 15pt semibold — sheet/dialog titles.
    static let sheetTitleText = Font.system(size: 15, weight: .semibold)
    /// Sans, 17pt semibold — the largest headline this app uses (onboarding's
    /// primary question).
    static let dialogTitleText = Font.system(size: 17, weight: .semibold)
    /// Sans, 18pt semibold — placeholder-screen titles.
    static let placeholderTitleText = Font.system(size: 18, weight: .semibold)

    /// Sans, 12.5pt semibold — the Workflow Builder canvas node's own id
    /// (`NodeView.nodeContent`'s row 2), per the approved Workflow Builder
    /// spec's node-content sizing (final review fix, Minor b — this was
    /// previously an ad hoc `.system(size: 12.5, weight: .semibold)` inline
    /// at the call site).
    static let nodeTitle = Font.system(size: 12.5, weight: .semibold)
    /// Sans, 10.5pt — the Workflow Builder canvas node's sub-line (`NodeView.
    /// nodeContent`'s row 3; the mono variant of this same row still goes
    /// through `dataMono(10.5)` for `run`/`action` kinds — see `subLine(for:)`'s
    /// doc comment), per the approved Workflow Builder spec's node-content
    /// sizing (final review fix, Minor b — same ad hoc inline-size history
    /// as `nodeTitle` above).
    static let nodeSub = Font.system(size: 10.5)

    /// Monospaced + monospaced-digit font for DATA, sized per call site.
    static func dataMono(_ size: CGFloat) -> Font {
        Font.system(size: size, design: .monospaced).monospacedDigit()
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
