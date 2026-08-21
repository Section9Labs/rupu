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
public struct APIFinding: Decodable, Sendable {
    public let id: String
    public let summary: String
    public let severity: String
    public let scope: String
    public let filePath: String?
    public let lineRange: [UInt32]?
    public let project: String
    public let workflowName: String?
    public let permalink: String?
    public let rationale: String
    public let declaredAt: String

    public init(
        id: String,
        summary: String,
        severity: String,
        scope: String,
        filePath: String?,
        lineRange: [UInt32]?,
        project: String,
        workflowName: String?,
        permalink: String?,
        rationale: String,
        declaredAt: String
    ) {
        self.id = id
        self.summary = summary
        self.severity = severity
        self.scope = scope
        self.filePath = filePath
        self.lineRange = lineRange
        self.project = project
        self.workflowName = workflowName
        self.permalink = permalink
        self.rationale = rationale
        self.declaredAt = declaredAt
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case summary
        case severity
        case scope
        case filePath = "file_path"
        case lineRange = "line_range"
        case project
        case workflowName = "workflow_name"
        case permalink
        case evidence
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
        project = try container.decode(String.self, forKey: .project)
        workflowName = try container.decodeIfPresent(String.self, forKey: .workflowName)
        permalink = try container.decodeIfPresent(String.self, forKey: .permalink)
        declaredAt = try container.decode(String.self, forKey: .declaredAt)

        let evidence = try container.nestedContainer(keyedBy: EvidenceKeys.self, forKey: .evidence)
        rationale = try evidence.decode(String.self, forKey: .rationale)
    }
}
