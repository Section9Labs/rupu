import AppKit
import SwiftUI
import Testing
@testable import RupuDesign

/// Web parity table — labels/icons transcribed verbatim from
/// `crates/rupu-cp/web/src/lib/status.ts`'s `STATUS` record (`done` there is
/// spelled `completed`; `awaiting` there is `awaiting_approval`). Kept as data
/// here (not derived from the implementation) so this test actually catches a
/// drifted case in `StatusDescriptor.descriptor(for:)`.
private let expected: [(tone: StatusTone, label: String, icon: LucideIcon)] = [
    (.running, "Running", .play),
    (.done, "Completed", .checkCircle2),
    (.failed, "Failed", .xCircle),
    (.awaiting, "Awaiting approval", .pause),
    (.paused, "Paused", .pauseCircle),
    (.pending, "Pending", .clock),
    (.skipped, "Skipped", .skipForward),
    (.cancelled, "Cancelled", .ban),
    (.rejected, "Rejected", .xOctagon),
]

@Test func descriptorTableMatchesWebStatusTs() {
    // Iterate StatusTone.allCases (not just `expected`) so a future case added
    // to the enum without a matching row here fails loudly instead of being
    // silently skipped.
    #expect(expected.count == StatusTone.allCases.count)
    for tone in StatusTone.allCases {
        guard let row = expected.first(where: { $0.tone == tone }) else {
            Issue.record("no expected row for StatusTone.\(tone)")
            continue
        }
        let descriptor = StatusDescriptor.descriptor(for: tone)
        #expect(descriptor.tone == tone, "StatusDescriptor.descriptor(for: .\(tone)).tone")
        #expect(descriptor.label == row.label, "StatusDescriptor.descriptor(for: .\(tone)).label")
        #expect(descriptor.icon == row.icon, "StatusDescriptor.descriptor(for: .\(tone)).icon")
    }
}

// MARK: - Pill tint policy (Task 2, chrome-kit fidelity)
//
// Ported verbatim from `crates/rupu-cp/web/src/lib/status.ts`'s `STATUS`
// record: exactly `pending` (`:58-65`), `cancelled` (`:121-127`), `skipped`
// (`:129-135`) render `bg-surface ... ring-border` (flat/neutral, no status
// tint); every other tone keeps the tinted `/10` bg + status ink + `/30`
// ring. `skipped` uses `text-ink-mute`; `pending`/`cancelled` use
// `text-ink-dim` — a real distinction, not a typo, so it's asserted
// per-tone rather than folded into one shared "dim ink" case.
private let flatInk: [StatusTone: Color] = [
    .pending: .rupuDim,
    .cancelled: .rupuDim,
    .skipped: .rupuMute,
]

/// Resolves a `Color` to its 8-bit sRGBA components under a forced
/// appearance. Headless `swift test` has no window server, so we push the
/// appearance onto the current-drawing-appearance stack rather than relying
/// on `NSApp.effectiveAppearance` (same technique `TokensTests.swift` uses).
private func rgba(_ color: Color, appearance name: NSAppearance.Name) -> (r: Int, g: Int, b: Int, a: Int) {
    let appearance = NSAppearance(named: name)!
    var resolved: NSColor?
    appearance.performAsCurrentDrawingAppearance {
        resolved = NSColor(color).usingColorSpace(.sRGB)
    }
    guard let c = resolved else {
        Issue.record("failed to resolve color under \(name.rawValue)")
        return (-1, -1, -1, -1)
    }
    return (
        Int((c.redComponent * 255).rounded()),
        Int((c.greenComponent * 255).rounded()),
        Int((c.blueComponent * 255).rounded()),
        Int((c.alphaComponent * 255).rounded())
    )
}

private func expectColorsMatch(_ a: Color, _ b: Color, _ name: String,
                                sourceLocation: SourceLocation = #_sourceLocation) {
    for appearance: NSAppearance.Name in [.aqua, .darkAqua] {
        #expect(rgba(a, appearance: appearance) == rgba(b, appearance: appearance),
                "\(name) mismatch under \(appearance.rawValue)", sourceLocation: sourceLocation)
    }
}

@Test func pillPolicyMatchesWebStatusTs() {
    for tone in StatusTone.allCases {
        let expectFlat = flatInk[tone] != nil
        #expect(tone.isFlatPill == expectFlat, "StatusTone.\(tone).isFlatPill")

        if let ink = flatInk[tone] {
            expectColorsMatch(Color.statusPillBackground(tone), .rupuSurface, "statusPillBackground(.\(tone))")
            expectColorsMatch(Color.statusPillRing(tone), .rupuBorder, "statusPillRing(.\(tone))")
            expectColorsMatch(Color.statusPillInk(tone), ink, "statusPillInk(.\(tone))")
        } else {
            expectColorsMatch(Color.statusPillBackground(tone), Color.status(tone).opacity(0.12),
                               "statusPillBackground(.\(tone))")
            expectColorsMatch(Color.statusPillRing(tone), Color.status(tone).opacity(0.3),
                               "statusPillRing(.\(tone))")
            expectColorsMatch(Color.statusPillInk(tone), Color.status(tone), "statusPillInk(.\(tone))")
        }
    }
}
