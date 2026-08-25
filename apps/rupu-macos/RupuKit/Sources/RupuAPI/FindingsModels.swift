import Foundation

/// `GET /api/findings` response envelope.
public struct APIFindings: Decodable, Sendable {
    public let findings: [APIFinding]
    public let summary: APIFindingsSummary

    public init(findings: [APIFinding], summary: APIFindingsSummary) {
        self.findings = findings
        self.summary = summary
    }
}

/// Severity-bucketed counts alongside `findings.summary`.
public struct APIFindingsSummary: Decodable, Sendable {
    public let total: Int
    public let critical: Int
    public let high: Int
    public let medium: Int
    public let low: Int
    public let info: Int

    public init(total: Int, critical: Int, high: Int, medium: Int, low: Int, info: Int) {
        self.total = total
        self.critical = critical
        self.high = high
        self.medium = medium
        self.low = low
        self.info = info
    }
}

/// A flattened `FindingOut` (workspace/project/target/permalink context) +
/// `FindingRecord` (the finding itself). `rationale` is nested one level
/// down, inside `evidence`, so this needs a custom decoder rather than a
/// flat `CodingKeys` mapping.
///
/// `wsID`/`targetID` were added for the global (unfiltered) `GET
/// /api/findings` view (Phase 5B): every row on that view spans multiple
/// workspaces/targets, so — unlike the single-project `findings(wsID:)` and
/// `runFindings(id:)` views, which already carry `project` for display —
/// the global view needs the raw ids too, to scope a click-through. Both
/// keys are always present on the wire (`FindingOut.ws_id`/`.target_id` on
/// the Rust side are plain `String`, never `Option`), so they decode as
/// non-optional here; the memberwise `init` still defaults them to `""` so
/// existing call sites built before this field existed keep compiling.
///
/// `declaredBy` (Phase 5B, Task 3 review fix): `FindingOut`'s flattened
/// `FindingRecord` (`crates/rupu-coverage/src/ledger/events.rs`) carries a
/// `declared_by: Attribution { run_id, model, surface }` field — NOT
/// flattened further (`Attribution` has no `#[serde(flatten)]` on it inside
/// `FindingRecord`), so it decodes as one nested object, same shape
/// `APICoverageAttribution` already models for the coverage routes. Reused
/// here rather than a duplicate type — see that type's doc comment. This is
/// the finding's actual run linkage: a first pass at this table wrongly
/// claimed `APIFinding` carried none at all and shipped non-navigating rows
/// on that false premise; `declaredBy` is what makes an honest per-surface
/// navigation decision possible (`RupuSecurity/FindingsTable.swift`'s
/// `findingNavigationRoute(surface:runID:)`). Always present on the wire
/// (`Attribution`'s three fields are all plain, non-`Option` on the Rust
/// side), so it decodes non-optional; the memberwise `init` still defaults
/// it to an all-empty `APICoverageAttribution` so existing call sites built
/// before this field existed keep compiling (same `wsID`/`targetID`
/// precedent above) — that empty default is deliberately unroutable (see
/// `findingNavigationRoute`'s empty-`runID` guard).
public struct APIFinding: Decodable, Sendable {
    public let id: String
    public let summary: String
    public let severity: String
    public let scope: String
    public let filePath: String?
    public let lineRange: [UInt32]?
    public let wsID: String
    public let project: String
    public let targetID: String
    public let workflowName: String?
    public let permalink: String?
    public let rationale: String
    public let declaredBy: APICoverageAttribution
    public let declaredAt: String

    public init(
        id: String,
        summary: String,
        severity: String,
        scope: String,
        filePath: String?,
        lineRange: [UInt32]?,
        wsID: String = "",
        project: String,
        targetID: String = "",
        workflowName: String?,
        permalink: String?,
        rationale: String,
        declaredBy: APICoverageAttribution = APICoverageAttribution(runID: "", model: "", surface: ""),
        declaredAt: String
    ) {
        self.id = id
        self.summary = summary
        self.severity = severity
        self.scope = scope
        self.filePath = filePath
        self.lineRange = lineRange
        self.wsID = wsID
        self.project = project
        self.targetID = targetID
        self.workflowName = workflowName
        self.permalink = permalink
        self.rationale = rationale
        self.declaredBy = declaredBy
        self.declaredAt = declaredAt
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case summary
        case severity
        case scope
        case filePath = "file_path"
        case lineRange = "line_range"
        case wsID = "ws_id"
        case project
        case targetID = "target_id"
        case workflowName = "workflow_name"
        case permalink
        case evidence
        case declaredBy = "declared_by"
        case declaredAt = "declared_at"
    }

    private enum EvidenceKeys: String, CodingKey {
        case rationale
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        summary = try container.decode(String.self, forKey: .summary)
        severity = try container.decode(String.self, forKey: .severity)
        scope = try container.decode(String.self, forKey: .scope)
        filePath = try container.decodeIfPresent(String.self, forKey: .filePath)
        lineRange = try container.decodeIfPresent([UInt32].self, forKey: .lineRange)
        wsID = try container.decode(String.self, forKey: .wsID)
        project = try container.decode(String.self, forKey: .project)
        targetID = try container.decode(String.self, forKey: .targetID)
        workflowName = try container.decodeIfPresent(String.self, forKey: .workflowName)
        permalink = try container.decodeIfPresent(String.self, forKey: .permalink)
        declaredBy = try container.decode(APICoverageAttribution.self, forKey: .declaredBy)
        declaredAt = try container.decode(String.self, forKey: .declaredAt)

        let evidence = try container.nestedContainer(keyedBy: EvidenceKeys.self, forKey: .evidence)
        rationale = try evidence.decode(String.self, forKey: .rationale)
    }
}
