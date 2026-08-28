import Testing
@testable import RupuFlowKit

@Test func parsesScalars() throws {
    let y = """
    name: review
    count: 3
    ratio: 0.5
    on: true
    off: false
    nothing: null
    tilde: ~
    quoted: "a: b"
    single: 'it''s'
    """
    let v = try YAMLParser.parse(y)
    #expect(v["name"] == .string("review"))
    #expect(v["count"] == .int(3))
    #expect(v["ratio"] == .double(0.5))
    #expect(v["on"] == .bool(true))
    #expect(v["off"] == .bool(false))
    #expect(v["nothing"] == .null)
    #expect(v["tilde"] == .null)
    #expect(v["quoted"] == .string("a: b"))
    #expect(v["single"] == .string("it's"))
}

@Test func parsesNegativeIntsAndFloats() throws {
    let y = """
    neg: -42
    negFloat: -0.5
    exp: 1.0
    """
    let v = try YAMLParser.parse(y)
    #expect(v["neg"] == .int(-42))
    #expect(v["negFloat"] == .double(-0.5))
    #expect(v["exp"] == .double(1.0))
}

@Test func parsesNestedBlockStructures() throws {
    let y = """
    steps:
      - id: triage
        agent: triager
        prompt: |
          Review the diff.
          Be thorough.
      - id: route
        branch:
          condition: "{{ steps.triage.output }}"
          then: [ship]
          else: [fix]
    """
    let v = try YAMLParser.parse(y)
    let steps = v["steps"]?.sequenceValue
    #expect(steps?.count == 2)
    #expect(steps?[0]["prompt"] == .string("Review the diff.\nBe thorough.\n"))
    #expect(steps?[1]["branch"]?["then"] == .sequence([.string("ship")]))
    #expect(steps?[1]["branch"]?["else"] == .sequence([.string("fix")]))
    #expect(steps?[1]["branch"]?["condition"] == .string("{{ steps.triage.output }}"))
}

@Test func parsesFlowSequenceOfFlowMappings() throws {
    // Downstream tasks feed the parser inline flow-style YAML like this.
    let y = "steps: [{id: a, agent: x, prompt: p}, {id: b, agent: y}]"
    let v = try YAMLParser.parse(y)
    let steps = v["steps"]?.sequenceValue
    #expect(steps?.count == 2)
    #expect(steps?[0]["id"] == .string("a"))
    #expect(steps?[0]["agent"] == .string("x"))
    #expect(steps?[0]["prompt"] == .string("p"))
    #expect(steps?[1]["id"] == .string("b"))
    #expect(steps?[1]["agent"] == .string("y"))
}

@Test func parsesFlowMappingContainingFlowSequence() throws {
    let y = "branch: {condition: c, then: [ghost]}"
    let v = try YAMLParser.parse(y)
    #expect(v["branch"]?["condition"] == .string("c"))
    #expect(v["branch"]?["then"] == .sequence([.string("ghost")]))
}

@Test func quotedTemplateBracesStaySrings() throws {
    // Quote detection must run before flow detection: a quoted scalar
    // containing `{{ ... }}` must stay a string, not be parsed as a flow
    // mapping.
    let y = #"subject: "{{ inputs.diff }}""#
    let v = try YAMLParser.parse(y)
    #expect(v["subject"] == .string("{{ inputs.diff }}"))
}

@Test func literalChompingVariants() throws {
    #expect(try YAMLParser.parse("a: |-\n  x\n  y\n")["a"] == .string("x\ny"))
    #expect(try YAMLParser.parse("a: |\n  x\n")["a"] == .string("x\n"))
    #expect(try YAMLParser.parse("a: |+\n  x\n\n\n")["a"] == .string("x\n\n\n"))
}

@Test func foldedBlockScalarFoldsLines() throws {
    let y = """
    a: >
      one
      two

      three
    """
    let v = try YAMLParser.parse(y)
    #expect(v["a"] == .string("one two\nthree\n"))
}

@Test func foldedBlockScalarStrip() throws {
    let v = try YAMLParser.parse("a: >-\n  one\n  two\n")
    #expect(v["a"] == .string("one two"))
}

@Test func commentsAreSkipped() throws {
    let v = try YAMLParser.parse("# top\na: 1 # trailing\nb: \"#not\"\n")
    #expect(v["a"] == .int(1))
    #expect(v["b"] == .string("#not"))
}

@Test func documentStartMarkerIgnored() throws {
    let v = try YAMLParser.parse("---\na: 1\n")
    #expect(v["a"] == .int(1))
}

@Test func emptyInputParsesToNull() throws {
    #expect(try YAMLParser.parse("") == .null)
    #expect(try YAMLParser.parse("   \n  \n") == .null)
}

@Test func rejectsAnchorsAndTags() {
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: &x 1\nb: *x\n") }
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: !!str 1\n") }
}

@Test func rejectsMultipleDocuments() {
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: 1\n---\nb: 2\n") }
}

@Test func rejectsComplexKeys() {
    #expect(throws: YAMLError.self) { try YAMLParser.parse("? a\n: 1\n") }
}

@Test func duplicateKeyIsMalformed() {
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: 1\na: 2\n") }
}

@Test func duplicateKeyInFlowMappingIsMalformed() {
    #expect(throws: YAMLError.self) { try YAMLParser.parse("a: {x: 1, x: 2}\n") }
}

@Test func errorsCarrySourceLine() {
    do {
        _ = try YAMLParser.parse("a: 1\nb: 2\nb: 3\n")
        Issue.record("expected an error")
    } catch YAMLError.malformed(_, let line) {
        #expect(line == 3)
    } catch {
        Issue.record("unexpected error type: \(error)")
    }
}

@Test func parsesEveryRepoWorkflowSample() throws {
    // Golden corpus committed in Task 2 Step 3 from .rupu/workflows/*.yaml.
    for (name, text) in YAMLGolden.samples {
        let v = try YAMLParser.parse(text)
        #expect(v["name"] != nil, "sample \(name) lost its name key")
        #expect(v["steps"]?.sequenceValue?.isEmpty == false, "sample \(name) lost steps")
    }
}
