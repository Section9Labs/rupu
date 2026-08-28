import Testing
@testable import RupuFlowKit

@Test func dumpsCanonicalBlockStyle() {
    let v = YAMLValue.mapping([
        ("name", .string("nightly")),
        ("steps", .sequence([.mapping([
            ("id", .string("scan")),
            ("prompt", .string("line1\nline2\n")),
            ("when", .string("true")),  // must quote: parses as bool
        ])])),
    ])
    let out = YAMLEmitter.dump(v)
    let expected = """
    name: nightly
    steps:
      - id: scan
        prompt: |
          line1
          line2
        when: "true"

    """
    #expect(out == expected)
}

@Test func quotesOnlyWhenNeeded() {
    #expect(YAMLEmitter.dump(.mapping([("a", .string("plain text"))])) == "a: plain text\n")
    #expect(YAMLEmitter.dump(.mapping([("a", .string("- not a list"))])) == "a: \"- not a list\"\n")
    #expect(YAMLEmitter.dump(.mapping([("a", .string(""))])) == "a: \"\"\n")
    #expect(YAMLEmitter.dump(.mapping([("a", .string("3"))])) == "a: \"3\"\n")
}

@Test func emptyCollectionsInline() {
    #expect(YAMLEmitter.dump(.mapping([("j", .mapping([]))])) == "j: {}\n")
    #expect(YAMLEmitter.dump(.mapping([("s", .sequence([]))])) == "s: []\n")
}

@Test func roundTripsGoldenCorpusStructurally() throws {
    for (name, text) in YAMLGolden.samples {
        let once = try YAMLParser.parse(text)
        let dumped = YAMLEmitter.dump(once)
        let twice = try YAMLParser.parse(dumped)
        #expect(once == twice, "structural round-trip failed for \(name)")
        // Canonical form is a fixed point: dump(parse(dump(x))) == dump(x).
        #expect(YAMLEmitter.dump(twice) == dumped, "canonical dump not stable for \(name)")
    }
}

// MARK: - Round-trip regression coverage (post-review findings)

/// Emits `s` as the sole value of a one-key mapping, re-parses the dump,
/// and asserts the round-tripped value is exactly `.string(s)` — the
/// invariant every plain- or block-emitted scalar must uphold (see
/// `YAMLEmitter.needsQuoting`'s doc comment).
private func emitParseRoundTrip(_ s: String) throws {
    let dumped = YAMLEmitter.dump(.mapping([("k", .string(s))]))
    let parsed = try YAMLParser.parse(dumped)
    #expect(parsed["k"] == .string(s), "round-trip mismatch for \(s.debugDescription); dumped:\n\(dumped)")
}

@Test func indentedFirstLineFollowedByFlushLineRoundTrips() throws {
    // FINDING 1 regression: YAMLParser.parseBlockScalar derives its
    // baseIndent from the FIRST non-blank raw line only, then breaks out
    // of the block early on any later non-blank line indented less than
    // that base. A literal block scalar whose first content line is
    // indented more than a later line would silently drop everything from
    // that later line onward — this is the natural shape of a prompt
    // containing an indented code block followed by flush-left text. Must
    // fall back to a quoted scalar instead of a broken literal block.
    try emitParseRoundTrip("  foo\nbar\n")
}

@Test func indentedMiddleLineAfterFlushFirstLineStillUsesBlockScalarAndRoundTrips() throws {
    // Companion positive case for the Finding 1 fix: when the FIRST
    // content line is flush (no leading spaces), a LATER line being more
    // indented is safe — the parser's base ends up equal to our own fixed
    // contentIndent, which every other line's own (>= 0) leading spaces
    // can only meet or exceed. Confirms the fix isn't overly conservative
    // and still prefers the block-scalar form here.
    let s = "foo\n  nested\nbar\n"
    try emitParseRoundTrip(s)
    let dumped = YAMLEmitter.dump(.mapping([("k", .string(s))]))
    #expect(dumped.contains("k: |"), "expected the block-scalar path, got:\n\(dumped)")
}

@Test func tabBeforeHashRoundTrips() throws {
    // FINDING 2 regression: YAMLParser's stripCommentAndTrim treats '#'
    // preceded by a TAB (not just a space) as a comment start too — a
    // plain-emitted "cmd\t#tag" would re-parse truncated to "cmd".
    try emitParseRoundTrip("cmd\t#tag")
}

@Test func allWhitespaceInteriorLineFallsBackToQuoted() throws {
    // A blank-looking-but-not-actually-empty interior line: the parser's
    // blank-line detector is a whitespace-only check regardless of
    // position, so a literal block scalar would silently collapse this
    // line's three spaces to "". Must fall back to quoted.
    let s = "a\n   \nb\n"
    try emitParseRoundTrip(s)
    let dumped = YAMLEmitter.dump(.mapping([("k", .string(s))]))
    #expect(!dumped.contains("|"), "expected the quoted fallback, got:\n\(dumped)")
}

@Test func adversarialScalarShapesRoundTrip() throws {
    // Table-driven sweep over shapes chosen to individually exercise each
    // needsQuoting/blockScalarLines branch: keyword collisions, numeric
    // look-alikes, every leading-significant-char case, embedded ": "/
    // " #"/"\t#", leading/trailing whitespace, escape-worthy characters,
    // and the multiline edge shapes from Findings 1-2 plus the ones
    // reasoned through in the original implementation report (bare "\n",
    // 2+ trailing newlines, a leading blank line, an embedded blank line).
    let cases: [String] = [
        "",
        "true", "false", "null", "~",
        "3", "-42", "3.14", "1.0", "0",
        "- not a list", "? key", ": value", "# comment", "& anchor", "* alias",
        "a: b", "a #b", "a\t#b",
        "plain text",
        " leading space", "trailing space ",
        "back\\slash", "quote\"inside", "mixed\\and\"both",
        "line1\nline2\n",
        "line1\nline2",
        "\n",
        "a\n\n\n",
        "a\n\nb\n",
        "\nfoo\n",
        "  foo\nbar\n",
        "foo\n  nested\nbar\n",
        "a\n   \nb\n",
        "unicode: héllo wörld 🎉 — em dash",
    ]
    for s in cases {
        try emitParseRoundTrip(s)
    }
}
