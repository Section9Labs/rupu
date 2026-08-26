import RupuAPI
import RupuDesign
import RupuStore
import SwiftUI

// ---------------------------------------------------------------------------
// Wire parsing — `report_finding` tool_call input (ToolCard.tsx's `asFinding`)
// ---------------------------------------------------------------------------

/// One `report_finding` tool_call's parsed input fields — a Swift-side
/// `FindingView` (`crates/rupu-cp/web/src/components/transcript/
/// transcriptView.ts:61-71`).
///
/// **Read from `entry.input` (the tool_call's OWN input), never `entry.
/// structured` (the paired tool_result).** Verified against BOTH sides:
///
/// - **Web**: `asFinding` (`transcriptView.ts:186-211`) is called as
///   `asFinding(data.input)` at the `tool_call` case (`transcriptView.ts:
///   323`) — never against the `tool_result` branch.
/// - **Rust wire**: `report_finding`'s actual argument struct,
///   `ReportFindingInput` (`crates/rupu-coverage/src/tools/report_finding.rs:
///   8-20`): `file_path: Option<String>`, `line_range: Option<[u32; 2]>`,
///   `scope: FindingScope` (`#[serde(rename_all = "lowercase")]` over
///   `Line|File|Repo` — `crates/rupu-coverage/src/ledger/events.rs:150-156`,
///   wire strings `"line"|"file"|"repo"`), `summary: String`, `severity:
///   Severity` (same lowercase rename — `crates/rupu-coverage/src/catalog/
///   types.rs:4-13`, wire strings `"info"|"low"|"medium"|"high"|"critical"`),
///   `concern_id: Option<String>`, `evidence: FindingEvidence { code_excerpt:
///   Option<String>, rationale: String, references: Vec<String> }`
///   (`crates/rupu-coverage/src/ledger/events.rs:158-165`). Every one of
///   these lands on the tool's `input` argument — its OUTPUT,
///   `ReportFindingOutput { id: String }` (`report_finding.rs:22-25`), carries
///   only the generated finding id, none of the above. Reading `entry.
///   structured` (the paired `tool_result.structured`, which decodes to that
///   `{id}` shape) would render every field of a real `report_finding` call
///   empty.
struct FindingFields: Equatable {
    struct LineRange: Equatable {
        let start: Int
        let end: Int
    }

    /// Raw wire string (`"info"|"low"|"medium"|"high"|"critical"`, or
    /// anything else for an unrecognized/absent value) — resolved to a
    /// `RupuDesign.Severity` via `Severity(wireString:)` at the view layer,
    /// which already treats any unrecognized string as `.info` (this app's
    /// "never crash on new data" posture), so no separate validation is
    /// needed here.
    let severityWire: String
    let summary: String
    let scope: String
    let filePath: String?
    let lineRange: LineRange?
    let concernID: String?
    let rationale: String
    let codeExcerpt: String?
    let references: [String]
}

/// Parses a `report_finding` tool_call's `input` into `FindingFields` — a
/// line-for-line port of `asFinding` (`transcriptView.ts:186-211`). Unlike
/// the web (which returns `null` when neither `summary` nor `rationale` is
/// present, and the caller then risks rendering `finding` as `undefined`),
/// this always returns a value: every field simply reads empty/absent when
/// `input` isn't an object, or the object carries none of these keys — the
/// "render what exists, `—` never fabricated" contract the view applies on
/// top (`FindingCard.body`'s own field-by-field `nonEmpty` guards), rather
/// than a card that renders nothing at all for a malformed input.
func parseFinding(_ input: JSONValue) -> FindingFields {
    guard case .object(let rec) = input else {
        return FindingFields(
            severityWire: "info", summary: "", scope: "", filePath: nil,
            lineRange: nil, concernID: nil, rationale: "", codeExcerpt: nil, references: []
        )
    }
    let evidence: [String: JSONValue] = {
        if case .object(let e)? = rec["evidence"] { return e }
        return [:]
    }()

    func string(_ obj: [String: JSONValue], _ key: String) -> String? {
        if case .string(let s)? = obj[key] { return s }
        return nil
    }

    let severityWire = string(rec, "severity") ?? "info"
    let summary = string(rec, "summary") ?? ""
    let scope = string(rec, "scope") ?? ""
    let rationale = string(evidence, "rationale") ?? ""
    let filePath = string(rec, "file_path")
    let concernID = string(rec, "concern_id")
    let codeExcerpt = string(evidence, "code_excerpt")

    var lineRange: FindingFields.LineRange?
    if case .array(let arr)? = rec["line_range"], arr.count == 2,
       case .number(let start) = arr[0], case .number(let end) = arr[1] {
        lineRange = FindingFields.LineRange(start: Int(start), end: Int(end))
    }

    var references: [String] = []
    if case .array(let arr)? = evidence["references"] {
        references = arr.compactMap {
            if case .string(let s) = $0 { return s }
            return nil
        }
    }

    return FindingFields(
        severityWire: severityWire, summary: summary, scope: scope, filePath: filePath,
        lineRange: lineRange, concernID: concernID, rationale: rationale,
        codeExcerpt: codeExcerpt, references: references
    )
}

// ---------------------------------------------------------------------------
// FindingCard — `.finding`-kind entries' own full-chrome card (`FindingCard.
// tsx`)
// ---------------------------------------------------------------------------

/// One `report_finding` tool call's finding card — `TranscriptFeed`'s direct
/// replacement for the `StructuredView` fallback `ToolCardView` used for
/// `.finding`-kind entries before this task. Renders its OWN full chrome
/// (severity hairline, header, body) with no `ToolCardView` header wrapped
/// around it — matching `ToolCard.tsx`'s own dispatch comment ("`finding` →
/// FindingCard (its own full chrome; no outer header)", `ToolCard.tsx:6`),
/// so `TranscriptFeed` routes `.finding`-kind entries here directly rather
/// than through `ToolCardView`.
///
/// Anatomy (top → bottom), direct port of `FindingCard.tsx`:
///   1. Severity hairline — 2px `Color.severity` bar at the very top.
///   2. Header row — severity pill + scope chip (when non-empty) + concern-id
///      chip (when present).
///   3. Summary — severity-tinted bold title (`"—"` when empty, the app's
///      null-discipline convention).
///   4. Location chip — `path[:start-end]` in mono, only when `filePath` is
///      non-empty; a clickable button toggling an inline `SourcePreview`
///      when `sourcePreviewStore` is available (mirrors `AstGrepMatchRow`'s
///      own "omit the toggle rather than invent plumbing" convention), else
///      plain non-clickable text.
///   5. Rationale — via `MarkdownView`, only when non-empty.
///   6. Code excerpt — via `CodeBlock` (language `nil`; the wire carries no
///      language hint for a finding excerpt), only when present.
///   7. References — a link list, only when non-empty.
public struct FindingCard: View {
    private let entry: ToolEntry
    private let runID: String?
    private let host: String?
    private let sourcePreviewStore: SourcePreviewStore?

    @State private var previewOpen = false

    public init(entry: ToolEntry, runID: String?, host: String?, sourcePreviewStore: SourcePreviewStore? = nil) {
        self.entry = entry
        self.runID = runID
        self.host = host
        self.sourcePreviewStore = sourcePreviewStore
    }

    private var fields: FindingFields { parseFinding(entry.input) }
    private var severity: Severity { Severity(wireString: fields.severityWire) }

    private var location: String? {
        guard let filePath = fields.filePath, !filePath.isEmpty else { return nil }
        if let range = fields.lineRange {
            return "\(filePath):\(range.start)-\(range.end)"
        }
        return filePath
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Rectangle()
                .fill(Color.severity(severity))
                .frame(height: 2)
            VStack(alignment: .leading, spacing: 8) {
                header
                summaryText
                if let location {
                    locationChip(location)
                }
                if previewOpen, let filePath = fields.filePath, !filePath.isEmpty, let sourcePreviewStore {
                    SourcePreview(store: sourcePreviewStore, path: filePath, line: fields.lineRange?.start ?? 1)
                }
                if !fields.rationale.isEmpty {
                    MarkdownView(fields.rationale)
                }
                if let codeExcerpt = fields.codeExcerpt, !codeExcerpt.isEmpty {
                    CodeBlock(codeExcerpt, language: nil)
                }
                if !fields.references.isEmpty {
                    referencesSection
                }
            }
            .padding(10)
        }
        .panelStyle(.innerCard)
    }

    private var header: some View {
        HStack(spacing: 6) {
            SeverityPill(severity: severity)
            if !fields.scope.isEmpty {
                Badge(fields.scope, tone: .rupuMute)
            }
            if let concernID = fields.concernID, !concernID.isEmpty {
                Badge(concernID, tone: .rupuMute)
            }
            Spacer(minLength: 0)
        }
    }

    private var summaryText: some View {
        Text(fields.summary.isEmpty ? "—" : fields.summary)
            .font(.leadText.weight(.semibold))
            .foregroundStyle(Color.severity(severity))
            .textSelection(.enabled)
    }

    @ViewBuilder
    private func locationChip(_ label: String) -> some View {
        Group {
            if sourcePreviewStore != nil {
                Button {
                    previewOpen.toggle()
                } label: {
                    Text(label)
                }
                .buttonStyle(.plain)
                .foregroundStyle(previewOpen ? Color.rupuBrand700 : Color.rupuDim)
                .accessibilityLabel(previewOpen ? "Hide source preview" : "Show source preview")
            } else {
                Text(label)
                    .foregroundStyle(Color.rupuDim)
            }
        }
        .font(.dataMono(10.5))
        .lineLimit(1)
        .truncationMode(.middle)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(Color.rupuSurface)
        .overlay(RoundedRectangle(cornerRadius: 4).stroke(Color.rupuBorder, lineWidth: 1))
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }

    private var referencesSection: some View {
        VStack(alignment: .leading, spacing: 3) {
            Eyebrow("References")
            ForEach(Array(fields.references.enumerated()), id: \.offset) { _, ref in
                referenceLink(ref)
            }
        }
    }

    @ViewBuilder
    private func referenceLink(_ ref: String) -> some View {
        if let url = URL(string: ref), url.scheme != nil {
            Link(ref, destination: url)
                .font(.noteText)
                .foregroundStyle(Color.rupuBrand700)
                .lineLimit(1)
                .truncationMode(.middle)
        } else {
            Text(ref)
                .font(.noteText)
                .foregroundStyle(Color.rupuDim)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }
}

/// The severity badge pill (`FindingCard.tsx`'s `s.pill`, `SEVERITY_STYLE`) —
/// uppercase label, tinted background + text in the severity color, a 30%
/// stroke ring (the same "ring-inset" treatment `statusPillRing` establishes
/// for `StatusPill`).
private struct SeverityPill: View {
    let severity: Severity

    var body: some View {
        Text(label)
            .font(.dataMono(10).weight(.bold))
            .kerning(0.6)
            .foregroundStyle(Color.severity(severity))
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(Color.severityBg(severity))
            .clipShape(ChromeShape.pill)
            .overlay(ChromeShape.pill.stroke(Color.severity(severity).opacity(0.3), lineWidth: 1))
    }

    private var label: String {
        switch severity {
        case .crit: "CRITICAL"
        case .high: "HIGH"
        case .med: "MEDIUM"
        case .low: "LOW"
        case .info: "INFO"
        }
    }
}
