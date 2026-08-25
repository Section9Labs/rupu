import SwiftUI

/// Canonical (tone, label, icon) triple for a `StatusTone` — the single source
/// of truth every status-facing view (pill, timeline row, graph node) reads
/// from, ported from web's `STATUS` record in `crates/rupu-cp/web/src/lib/status.ts`.
/// `done` there is spelled `completed`; `awaiting` there is `awaiting_approval` —
/// same states, `StatusTone`'s own case names win here.
public struct StatusDescriptor {
    public let tone: StatusTone
    public let label: String
    public let icon: LucideIcon

    public static func descriptor(for tone: StatusTone) -> StatusDescriptor {
        switch tone {
        case .running: StatusDescriptor(tone: tone, label: "Running", icon: .play)
        case .done: StatusDescriptor(tone: tone, label: "Completed", icon: .checkCircle2)
        case .failed: StatusDescriptor(tone: tone, label: "Failed", icon: .xCircle)
        case .awaiting: StatusDescriptor(tone: tone, label: "Awaiting approval", icon: .pause)
        case .paused: StatusDescriptor(tone: tone, label: "Paused", icon: .pauseCircle)
        case .pending: StatusDescriptor(tone: tone, label: "Pending", icon: .clock)
        case .skipped: StatusDescriptor(tone: tone, label: "Skipped", icon: .skipForward)
        case .cancelled: StatusDescriptor(tone: tone, label: "Cancelled", icon: .ban)
        case .rejected: StatusDescriptor(tone: tone, label: "Rejected", icon: .xOctagon)
        }
    }
}

/// Web-parity status pill: icon + mono label on a `ChromeShape.pill` (4pt
/// rounded rect, matching `StatusPill.tsx:79`'s `rounded` class — see
/// `ChromeShape`'s own doc comment). Tint follows `StatusTone.isFlatPill`:
/// `pending`/`cancelled`/`skipped` render flat/neutral (`Color.
/// statusPillBackground`/`Ring`/`Ink`, ported from `status.ts`'s `bg-surface
/// ... ring-border` rows); every other tone keeps the tone color at a 12%
/// fill with a 30% ring (matches `StatusPill.tsx`'s `bg-status-x/10
/// ring-status-x/30`; 12% is this design's own fill step, see
/// `docs/macOS_design/V2-CONTRACT.md`). `compact` mirrors the web pill's `xs`
/// size (9pt icon, tighter label) vs. the default `sm` size (11pt icon).
public struct StatusPill: View {
    private let tone: StatusTone
    private let compact: Bool

    public init(_ tone: StatusTone, compact: Bool = false) {
        self.tone = tone
        self.compact = compact
    }

    public var body: some View {
        let descriptor = StatusDescriptor.descriptor(for: tone)
        let ink = Color.statusPillInk(tone)
        HStack(spacing: 4) {
            Icon(descriptor.icon, size: compact ? 9 : 11)
                .foregroundStyle(ink)
            Text(descriptor.label)
                .font(.dataMono(compact ? 9 : 10))
                .foregroundStyle(ink)
        }
        .padding(.horizontal, compact ? 6 : 8)
        .padding(.vertical, compact ? 2 : 3)
        .background(Color.statusPillBackground(tone))
        .clipShape(ChromeShape.pill)
        .overlay(ChromeShape.pill.stroke(Color.statusPillRing(tone), lineWidth: 1))
    }
}
