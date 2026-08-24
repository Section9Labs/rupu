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
