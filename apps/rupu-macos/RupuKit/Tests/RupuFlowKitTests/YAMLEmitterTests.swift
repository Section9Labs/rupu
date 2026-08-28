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
