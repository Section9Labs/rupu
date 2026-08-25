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
    // 35 icons per the extractor's ICONS table (apps/rupu-macos/scripts/extract-lucide.mjs) —
    // pinning the count here means an enum case silently dropped from that table (rather than
    // failing to parse) still fails a test. Phase 6A Task 6 added `lock` for the Config tab's
    // Policy lock glyphs (was 34); it also added an `unlock` that nothing ever rendered, removed
    // end-to-end in the final-review wave (M5) — extractor entry, svg asset, and generated case.
    #expect(LucideIcon.allCases.count == 35)
}

@Test func repeatIconRawValueIsRepeat() {
    // "repeat" isn't a valid Swift identifier as a bare case name; the raw value is what the
    // extractor's ICONS table and this enum agree on as the canonical name.
    #expect(LucideIcon.repeatIcon.rawValue == "repeat")
}
