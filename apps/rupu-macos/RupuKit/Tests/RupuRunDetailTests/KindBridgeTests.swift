import AppKit
import SwiftUI
import Testing
import RupuDesign
@testable import RupuRunDetail

/// Resolves a `Color` to its 8-bit sRGB components under a forced appearance — same technique
/// `RupuDesignTests/TokensTests.swift` uses. `Color.status`/`Color.severity` mint a fresh dynamic
/// `NSColor` catalog entry on every call (see `Tokens.swift`'s `dynamicColor`), so two calls that
/// resolve to the same RGB pair are NOT `==` to each other — comparing resolved components (not
/// `Color` equality) is the only reliable way to assert "this kind's accent is token X".
private func resolvedRGB(_ color: Color, appearance name: NSAppearance.Name) -> (r: Int, g: Int, b: Int) {
    let appearance = NSAppearance(named: name)!
    var resolved: NSColor?
    appearance.performAsCurrentDrawingAppearance {
        resolved = NSColor(color).usingColorSpace(.sRGB)
    }
    guard let c = resolved else { return (-1, -1, -1) }
    return (
        Int((c.redComponent * 255).rounded()),
        Int((c.greenComponent * 255).rounded()),
        Int((c.blueComponent * 255).rounded())
    )
}

private func expectSameColor(_ a: Color, _ b: Color, _ message: String, sourceLocation: SourceLocation = #_sourceLocation) {
    #expect(resolvedRGB(a, appearance: .aqua) == resolvedRGB(b, appearance: .aqua), "\(message): light mismatch", sourceLocation: sourceLocation)
    #expect(
        resolvedRGB(a, appearance: .darkAqua) == resolvedRGB(b, appearance: .darkAqua), "\(message): dark mismatch",
        sourceLocation: sourceLocation
    )
}

/// `StepKind` is the run graph's kind vocabulary (Plan 3, Task 1) — a Swift-side mirror of the
/// web CP's `StepKind` union (`crates/rupu-cp/web/src/lib/workflowGraph.ts`) restricted to the
/// run model's seven kinds (no `branch`/`split`/`join` — those are editor-only). `accent`/`icon`/
/// `label` are the kind channel of the run graph's two-channel color rule (kind on accent
/// bar/pill, run state on glyph/state label elsewhere) — see `kindVisuals.ts` for the source of
/// truth this ports.
@Test func everyRawStringRoundTrips() {
    let cases: [(raw: String, kind: StepKind)] = [
        ("step", .step),
        ("for_each", .forEach),
        ("parallel", .parallel),
        ("panel", .panel),
        ("gate", .gate),
        ("action", .action),
        ("run", .run),
    ]
    for (raw, kind) in cases {
        #expect(StepKind(raw: raw) == kind, "raw \"\(raw)\" should map to \(kind)")
        #expect(kind.rawValue == raw, "\(kind).rawValue should be \"\(raw)\"")
    }
}

@Test func unknownRawFallsBackToStep() {
    #expect(StepKind(raw: "branch") == .step)
    #expect(StepKind(raw: "split") == .step)
    #expect(StepKind(raw: "join") == .step)
    #expect(StepKind(raw: "bogus") == .step)
    #expect(StepKind(raw: "") == .step)
}

@Test func everyKindIconHasAtLeastOneParseablePath() {
    // Mirrors RupuDesignTests' `LucideIconDataTests.everyIconCaseHasAtLeastOnePath` /
    // `everyPathParses` for just the icons this bridge hands out — a bad wiring here (e.g. a
    // kind pointed at an icon whose extractor entry silently produced no paths) fails here
    // rather than showing up as a blank glyph on a run graph node.
    for kind in StepKind.allCases {
        let paths = LucideIconData.paths(for: kind.icon)
        #expect(!paths.isEmpty, "\(kind).icon (\(kind.icon)) has no paths")
        for d in paths {
            #expect(SVGPath(d: d) != nil, "\(kind).icon (\(kind.icon)) has an unparseable path: \(d)")
        }
    }
}

@Test func labelsMatchExactly() {
    let expected: [StepKind: String] = [
        .step: "step",
        .forEach: "for each",
        .parallel: "parallel",
        .panel: "panel",
        .gate: "gate",
        .action: "action",
        .run: "run",
    ]
    for kind in StepKind.allCases {
        #expect(kind.label == expected[kind], "\(kind).label mismatch")
    }
}

@Test func iconsMatchThePerKindTable() {
    let expected: [StepKind: LucideIcon] = [
        .step: .bot,
        .forEach: .repeatIcon,
        .parallel: .columns3,
        .panel: .shieldCheck,
        .gate: .userCheck,
        .action: .zap,
        .run: .terminal,
    ]
    for kind in StepKind.allCases {
        #expect(kind.icon == expected[kind], "\(kind).icon mismatch")
    }
}

@Test func accentMatchesTheGlobalConstraintsTable() {
    let expected: [StepKind: Color] = [
        .step: .status(.running),
        .forEach: .rupuBrand,
        .parallel: .severity(.crit),
        .panel: .status(.awaiting),
        .gate: .status(.paused),
        .action: .severity(.info),
        .run: .severity(.med),
    ]
    for kind in StepKind.allCases {
        guard let want = expected[kind] else {
            Issue.record("no expected accent for \(kind)")
            continue
        }
        expectSameColor(kind.accent, want, "\(kind).accent")
    }
}

@Test func stepKindCaseCountIsSeven() {
    // Pins the count so a future kind added to the run model (mirroring the editor's `branch`/
    // `split`/`join`) doesn't silently fall through `init(raw:)`'s default without this test
    // (and the switch statements below) being updated in lockstep.
    #expect(StepKind.allCases.count == 7)
}
