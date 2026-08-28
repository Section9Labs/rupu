import Testing
@testable import RupuDesign

/// Whole-set round-trip integrity for the generated icon data: every `LucideIcon` case must have
/// at least one path, and every path string `LucideIconData` returns must be parseable by
/// `SVGPathParser` — a generation bug (bad primitive conversion, truncated arc params, ...) fails
/// here rather than showing up as a blank glyph at runtime.
@Test func everyIconCaseHasAtLeastOnePath() {
    for icon in LucideIcon.allCases {
        let paths = LucideIconData.paths(for: icon)
        #expect(!paths.isEmpty, "\(icon) has no paths")
    }
}

@Test func everyPathParses() {
    for icon in LucideIcon.allCases {
        for d in LucideIconData.paths(for: icon) {
            #expect(SVGPath(d: d) != nil, "\(icon) has an unparseable path: \(d)")
        }
    }
}

@Test func iconCaseCountMatchesExtractorTable() {
    // 43 icons per the extractor's ICONS table (apps/rupu-macos/scripts/extract-lucide.mjs) —
    // pinning the count here means an enum case silently dropped from that table (rather than
    // failing to parse) still fails a test. Phase 6A Task 6 added `lock` for the Config tab's
    // Policy lock glyphs (was 34); it also added an `unlock` that nothing ever rendered, removed
    // end-to-end in the final-review wave (M5). Phase 6B Task 4 added `folder` for the Project
    // Code tab's directory-row glyph (was 35). Design-alignment Plan 3 Task 1 added
    // `bot`/`columns3`/`userCheck`/`zap`/`terminal` for the run graph's `KindBridge` (was 36).
    // Workflow Builder Task 7 added `split`/`merge` for the split/join node kind icons (was 41).
    #expect(LucideIcon.allCases.count == 43)
}

@Test func repeatIconRawValueIsRepeat() {
    // "repeat" isn't a valid Swift identifier as a bare case name; the raw value is what the
    // extractor's ICONS table and this enum agree on as the canonical name.
    #expect(LucideIcon.repeatIcon.rawValue == "repeat")
}
