import Foundation

/// `GET /api/coverage` row — one coverage target's rollup, aggregated across
/// every registered workspace (`CoverageSummary` on the Rust side, `crates/
/// rupu-cp/src/api/coverage.rs`). `wsID` disambiguates `targetID`s that
/// collide across workspaces and scopes the detail/catalog fetch.
public struct APICoverageSummary: Decodable, Equatable, Sendable {
    public let wsID: String
    public let project: String
    public let targetID: String
    public let assertionLines: Int
    public let hasCatalog: Bool
    public let findings: Int

    public init(
        wsID: String,
        project: String,
        targetID: String,
        assertionLines: Int,
        hasCatalog: Bool,
        findings: Int
    ) {
        self.wsID = wsID
        self.project = project
        self.targetID = targetID
        self.assertionLines = assertionLines
        self.hasCatalog = hasCatalog
        self.findings = findings
    }

    private enum CodingKeys: String, CodingKey {
        case wsID = "ws_id"
        case project
        case targetID = "target_id"
        case assertionLines = "assertion_lines"
        case hasCatalog = "has_catalog"
        case findings
    }
}

public extension APICoverageSummary {
    /// Globally unique identity for this row on the Security screen's
    /// Coverage table — review fix. The type doc comment above already
    /// warned that `targetID` collides across workspaces; that warning
    /// went unheeded once (the Coverage table's `ForEach` originally keyed
    /// its rows by a per-project-group positional offset, which a nested
    /// `ForEach` inside a `LazyVStack` does not scope safely across
    /// sibling group closures — most rows across a real multi-workspace
    /// fleet silently collapsed onto whichever row claimed each offset
    /// value first, leaving reserved-but-empty space for every other row
    /// that lost the collision). `wsID` is the qualifier that makes this
    /// composite safe; `RupuSecurity/CoverageList.swift`'s table keys its
    /// `ForEach` rows by this instead.
    var rowID: String { "\(wsID)/\(targetID)" }
}

/// Provenance shared by assertions, findings, and file touches under a
/// coverage target (`Attribution` on the Rust side, `crates/rupu-coverage/
/// src/ledger/events.rs`). `surface` is one of `"workflow"` | `"agent"` |
/// `"autoflow"` | `"session"`.
public struct APICoverageAttribution: Decodable, Equatable, Sendable {
    public let runID: String
    public let model: String
    public let surface: String

    public init(runID: String, model: String, surface: String) {
        self.runID = runID
        self.model = model
        self.surface = surface
    }

    private enum CodingKeys: String, CodingKey {
        case runID = "run_id"
        case model
        case surface
    }
}

/// A concern-assertion's supporting evidence (`Evidence` on the Rust side) —
/// a free-text summary plus the file line ranges and finding ids it cites.
public struct APICoverageEvidence: Decodable, Equatable, Sendable {
    public let summary: String
    public let lineRanges: [[UInt32]]
    public let findingIDs: [String]

    public init(summary: String, lineRanges: [[UInt32]], findingIDs: [String]) {
        self.summary = summary
        self.lineRanges = lineRanges
        self.findingIDs = findingIDs
    }

    private enum CodingKeys: String, CodingKey {
        case summary
        case lineRanges = "line_ranges"
        case findingIDs = "finding_ids"
    }
}

/// One `concern_id × file_path` assertion under a coverage target
/// (`ConcernAssertion` on the Rust side). `status` is one of `"clean"` |
/// `"finding"` | `"examined"` | `"not_applicable"`.
public struct APICoverageAssertion: Decodable, Equatable, Sendable {
    public let concernID: String
    public let filePath: String
    public let status: String
    public let evidence: APICoverageEvidence
    public let declaredBy: APICoverageAttribution
    public let declaredAt: String

    public init(
        concernID: String,
        filePath: String,
        status: String,
        evidence: APICoverageEvidence,
        declaredBy: APICoverageAttribution,
        declaredAt: String
    ) {
        self.concernID = concernID
        self.filePath = filePath
        self.status = status
        self.evidence = evidence
        self.declaredBy = declaredBy
        self.declaredAt = declaredAt
    }

    private enum CodingKeys: String, CodingKey {
        case concernID = "concern_id"
        case filePath = "file_path"
        case status
        case evidence
        case declaredBy = "declared_by"
        case declaredAt = "declared_at"
    }
}

/// A finding's supporting evidence as embedded in a coverage-target detail
/// (`FindingEvidence` on the Rust side).
public struct APICoverageFindingEvidence: Decodable, Equatable, Sendable {
    public let codeExcerpt: String?
    public let rationale: String
    public let references: [String]

    public init(codeExcerpt: String?, rationale: String, references: [String]) {
        self.codeExcerpt = codeExcerpt
        self.rationale = rationale
        self.references = references
    }

    private enum CodingKeys: String, CodingKey {
        case codeExcerpt = "code_excerpt"
        case rationale
        case references
    }
}

/// A finding as embedded in `GET /api/coverage/:target` (`FindingRecord` on
/// the Rust side) — the RAW record, without the provenance wrapper
/// (`ws_id`/`project`/`workflow_name`/`permalink`) `APIFinding` decodes for
/// `GET /api/findings`. Same underlying Rust type, two different wire
/// shapes at two different routes — this is deliberately its own type
/// rather than a reuse of `APIFinding`.
public struct APICoverageFinding: Decodable, Equatable, Sendable {
    public let id: String
    public let filePath: String?
    public let lineRange: [UInt32]?
    public let scope: String
    public let summary: String
    public let severity: String
    public let concernID: String?
    public let evidence: APICoverageFindingEvidence
    public let declaredBy: APICoverageAttribution
    public let declaredAt: String

    public init(
        id: String,
        filePath: String?,
        lineRange: [UInt32]?,
        scope: String,
        summary: String,
        severity: String,
        concernID: String?,
        evidence: APICoverageFindingEvidence,
        declaredBy: APICoverageAttribution,
        declaredAt: String
    ) {
        self.id = id
        self.filePath = filePath
        self.lineRange = lineRange
        self.scope = scope
        self.summary = summary
        self.severity = severity
        self.concernID = concernID
        self.evidence = evidence
        self.declaredBy = declaredBy
        self.declaredAt = declaredAt
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case filePath = "file_path"
        case lineRange = "line_range"
        case scope
        case summary
        case severity
        case concernID = "concern_id"
        case evidence
        case declaredBy = "declared_by"
        case declaredAt = "declared_at"
    }
}

/// One file's aggregated touch history under a coverage target (`FileView`
/// on the Rust side) — the per-file heatmap. `strongest`/`touchModes` are
/// one of `"glob"` | `"cmd"` | `"grep"` | `"read"` | `"edit"`.
public struct APICoverageFileView: Decodable, Equatable, Sendable {
    public let path: String
    public let touchModes: [String]
    public let strongest: String
    public let readLines: [[UInt32]]
    public let grepMatches: Int
    public let edits: Int
    public let firstAt: String
    public let lastAt: String
    public let touchedBy: [APICoverageAttribution]

    public init(
        path: String,
        touchModes: [String],
        strongest: String,
        readLines: [[UInt32]],
        grepMatches: Int,
        edits: Int,
        firstAt: String,
        lastAt: String,
        touchedBy: [APICoverageAttribution]
    ) {
        self.path = path
        self.touchModes = touchModes
        self.strongest = strongest
        self.readLines = readLines
        self.grepMatches = grepMatches
        self.edits = edits
        self.firstAt = firstAt
        self.lastAt = lastAt
        self.touchedBy = touchedBy
    }

    private enum CodingKeys: String, CodingKey {
        case path
        case touchModes = "touch_modes"
        case strongest
        case readLines = "read_lines"
        case grepMatches = "grep_matches"
        case edits
        case firstAt = "first_at"
        case lastAt = "last_at"
        case touchedBy = "touched_by"
    }
}

/// `GET /api/coverage/:target[?ws_id=]` — one target's full detail. Built
/// inline as a `serde_json::json!` object in `get_coverage` (`crates/
/// rupu-cp/src/api/coverage.rs`) rather than a single named Rust struct, so
/// this mirrors the handler's 8 keys field-for-field. `files` is a plain
/// (possibly empty) array on the Rust side, never `Option` — a target
/// predating the per-file ledger still gets `"files": []`, not an absent
/// key — so it decodes as non-optional here too.
public struct APICoverageDetail: Decodable, Equatable, Sendable {
    public let wsID: String
    public let project: String
    public let targetID: String
    public let assertionLines: Int
    public let hasCatalog: Bool
    public let assertions: [APICoverageAssertion]
    public let findings: [APICoverageFinding]
    public let files: [APICoverageFileView]

    public init(
        wsID: String,
        project: String,
        targetID: String,
        assertionLines: Int,
        hasCatalog: Bool,
        assertions: [APICoverageAssertion],
        findings: [APICoverageFinding],
        files: [APICoverageFileView]
    ) {
        self.wsID = wsID
        self.project = project
        self.targetID = targetID
        self.assertionLines = assertionLines
        self.hasCatalog = hasCatalog
        self.assertions = assertions
        self.findings = findings
        self.files = files
    }

    private enum CodingKeys: String, CodingKey {
        case wsID = "ws_id"
        case project
        case targetID = "target_id"
        case assertionLines = "assertion_lines"
        case hasCatalog = "has_catalog"
        case assertions
        case findings
        case files
    }
}

/// A single resolved concern definition (`Concern` on the Rust side,
/// `crates/rupu-coverage/src/catalog/types.rs`) inside a flattened catalog.
/// `minStrength` is one of `"glob"` | `"cmd"` | `"grep"` | `"read"` |
/// `"edit"`.
public struct APICoverageConcern: Decodable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let description: String
    public let severity: String
    public let applicableGlobs: [String]
    public let minStrength: String
    public let references: [String]
    public let tags: [String]

    public init(
        id: String,
        name: String,
        description: String,
        severity: String,
        applicableGlobs: [String],
        minStrength: String,
        references: [String],
        tags: [String]
    ) {
        self.id = id
        self.name = name
        self.description = description
        self.severity = severity
        self.applicableGlobs = applicableGlobs
        self.minStrength = minStrength
        self.references = references
        self.tags = tags
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case name
        case description
        case severity
        case applicableGlobs = "applicable_globs"
        case minStrength = "min_strength"
        case references
        case tags
    }
}

/// `GET /api/coverage/:target/catalog[?ws_id=]` — the flattened concern
/// catalog effective for one target (`FlatCatalog` on the Rust side): every
/// concern resolved with overrides applied, plus per-concern provenance
/// (`sources`, concern id → template name or `"inline"`) and requested
/// render mode (`renderModes`, concern id → `"full"` | `"index"` |
/// `"auto"`). Bare top-level object with no `ws_id`/`target_id` wrapper,
/// unlike the summary/detail routes — a catalog is resolved template state,
/// not per-target stored state. A SEPARATE route and fixture from
/// `APICoverageDetail` (`coverage_catalog.json` vs `coverage_detail.json`).
public struct APICoverageCatalog: Decodable, Equatable, Sendable {
    public let concerns: [APICoverageConcern]
    public let sources: [String: String]
    public let renderModes: [String: String]

    public init(concerns: [APICoverageConcern], sources: [String: String], renderModes: [String: String]) {
        self.concerns = concerns
        self.sources = sources
        self.renderModes = renderModes
    }

    private enum CodingKeys: String, CodingKey {
        case concerns
        case sources
        case renderModes = "render_modes"
    }
}
