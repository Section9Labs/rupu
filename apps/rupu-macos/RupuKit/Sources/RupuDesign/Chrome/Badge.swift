import SwiftUI

/// Tone-tinted metadata chip (trigger kinds, counts, tags) — mono meta text on
/// a 12% tint of `tone`. NOT for status (use `StatusPill`) and NOT for
/// severity. Mirrors web's `Badge.tsx`, simplified to a single `Color` tone
/// (web's fixed per-tone Tailwind class pairs) since this design already gives
/// every semantic/status color both a foreground and matching `-Bg` step.
public struct Badge: View {
    private let text: String
    private let tone: Color

    public init(_ text: String, tone: Color = .rupuMute) {
        self.text = text
        self.tone = tone
    }

    public var body: some View {
        Text(text)
            .font(.dataMono(10))
            .foregroundStyle(tone)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(tone.opacity(0.12))
            .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}
