import Testing
import RupuAPI
import RupuDesign
@testable import RupuRunDetail

/// Pure tests for `parseFinding` — the `report_finding` tool_call `input`
/// parser (`FindingCard.swift`), verified against the real wire shape:
/// `ReportFindingInput` (`crates/rupu-coverage/src/tools/report_finding.rs:
/// 8-20`) — `file_path`, `line_range: [u32; 2]`, `scope` (lowercase
/// `FindingScope`), `summary`, `severity` (lowercase `Severity`),
/// `concern_id`, `evidence: { code_excerpt, rationale, references }`. No
/// SwiftUI/`@MainActor` needed.
@Suite
struct FindingCardModelTests {

    private func obj(_ pairs: [String: JSONValue]) -> JSONValue { .object(pairs) }

    @Test func parsesEveryFieldOffARealReportFindingInput() {
        let input = obj([
            "file_path": .string("crypto-svc/keyring.rs"),
            "line_range": .array([.number(42), .number(48)]),
            "scope": .string("file"),
            "summary": .string("Hardcoded AES-256 key compiled into the binary"),
            "severity": .string("high"),
            "concern_id": .string("stride:tampering"),
            "evidence": obj([
                "code_excerpt": .string("const KEY: [u8;32] = [0x4a, 0x1f];"),
                "rationale": .string("A static 32-byte AES key is embedded in the binary."),
                "references": .array([.string("https://cwe.mitre.org/data/definitions/798.html")]),
            ]),
        ])

        let fields = parseFinding(input)

        #expect(fields.severityWire == "high")
        #expect(Severity(wireString: fields.severityWire) == .high)
        #expect(fields.summary == "Hardcoded AES-256 key compiled into the binary")
        #expect(fields.scope == "file")
        #expect(fields.filePath == "crypto-svc/keyring.rs")
        #expect(fields.lineRange == FindingFields.LineRange(start: 42, end: 48))
        #expect(fields.concernID == "stride:tampering")
        #expect(fields.rationale == "A static 32-byte AES key is embedded in the binary.")
        #expect(fields.codeExcerpt == "const KEY: [u8;32] = [0x4a, 0x1f];")
        #expect(fields.references == ["https://cwe.mitre.org/data/definitions/798.html"])
    }

    @Test func severityWireStringsMapToTheExpectedDesignSeverity() {
        for (wire, expected) in [
            ("info", Severity.info), ("low", .low), ("medium", .med), ("high", .high), ("critical", .crit),
        ] {
            let input = obj(["severity": .string(wire), "summary": .string("x")])
            #expect(Severity(wireString: parseFinding(input).severityWire) == expected)
        }
    }

    @Test func aSerendipitousFindingWithNoConcernIDOrFilePathDegradesGracefully() {
        let input = obj([
            "scope": .string("repo"),
            "summary": .string("Spotted while looking for something else."),
            "severity": .string("low"),
            "evidence": obj([
                "rationale": .string("ad-hoc"),
                "references": .array([]),
            ]),
        ])

        let fields = parseFinding(input)

        #expect(fields.filePath == nil)
        #expect(fields.lineRange == nil)
        #expect(fields.concernID == nil)
        #expect(fields.codeExcerpt == nil)
        #expect(fields.references.isEmpty)
        #expect(fields.rationale == "ad-hoc")
    }

    @Test func aNonObjectInputDegradesToAllDefaultsRatherThanCrashing() {
        for input: JSONValue in [.string("oops"), .array([.number(1)]), .null, .number(3)] {
            let fields = parseFinding(input)
            #expect(fields.severityWire == "info")
            #expect(fields.summary == "")
            #expect(fields.scope == "")
            #expect(fields.filePath == nil)
            #expect(fields.lineRange == nil)
            #expect(fields.rationale == "")
            #expect(fields.references.isEmpty)
        }
    }

    @Test func missingSeverityDefaultsToTheInfoWireString() {
        let input = obj(["summary": .string("no severity field at all")])
        #expect(parseFinding(input).severityWire == "info")
    }

    @Test func lineRangeWithTheWrongArityIsIgnoredRatherThanMisread() {
        let input = obj([
            "summary": .string("x"),
            "line_range": .array([.number(1), .number(2), .number(3)]),
        ])
        #expect(parseFinding(input).lineRange == nil)
    }

    @Test func nonStringReferencesAreFilteredOutRatherThanCrashingOrStringifying() {
        let input = obj([
            "summary": .string("x"),
            "evidence": obj([
                "rationale": .string("r"),
                "references": .array([.string("https://a.example"), .number(7), .string("https://b.example")]),
            ]),
        ])
        #expect(parseFinding(input).references == ["https://a.example", "https://b.example"])
    }
}
