import Foundation

/// Row from `GET /api/agents` (`AgentDto` on the Rust side). `usage` is
/// returned by the server but not decoded here — the definitions screens
/// this feeds only need `runCount`/`lastRun`, not the full usage summary.
public struct AgentDefinition: Decodable, Equatable, Sendable {
    public let name: String
    public let slug: String
    public let description: String?
    public let provider: String?
    public let model: String?
    public let effort: String?
    public let maxTokens: Int?
    /// The agent's frontmatter `permissionMode:` verbatim (`"ask"` /
    /// `"bypass"` / `"readonly"`), or `nil` when the frontmatter omits it —
    /// see `AgentDto.permission_mode`'s Rust doc comment (`crates/rupu-cp/
    /// src/api/agents.rs`) for why this is never resolved against project/
    /// global config here. Feeds the Library screen's permission-tone badge
    /// (`RupuStore.agentPermissionTone(mode:)`) — never parsed from `raw`
    /// markdown, always this DTO field.
    public let mode: String?
    public let tools: [String]
    public let scope: String
    public let scopeKind: String
    public let scopeID: String?
    public let runCount: Int
    public let lastRun: String?

    public init(
        name: String,
        slug: String,
        description: String?,
        provider: String?,
        model: String?,
        effort: String?,
        maxTokens: Int?,
        mode: String? = nil,
        tools: [String],
        scope: String,
        scopeKind: String,
        scopeID: String?,
        runCount: Int,
        lastRun: String?
    ) {
        self.name = name
        self.slug = slug
        self.description = description
        self.provider = provider
        self.model = model
        self.effort = effort
        self.maxTokens = maxTokens
        self.mode = mode
        self.tools = tools
        self.scope = scope
        self.scopeKind = scopeKind
        self.scopeID = scopeID
        self.runCount = runCount
        self.lastRun = lastRun
    }

    private enum CodingKeys: String, CodingKey {
        case name
        case slug
        case description
        case provider
        case model
        case effort
        case maxTokens = "max_tokens"
        case mode = "permission_mode"
        case tools
        case scope
        case scopeKind = "scope_kind"
        case scopeID = "scope_id"
        case runCount = "run_count"
        case lastRun = "last_run"
    }
}

extension AgentDefinition {
    /// Stable per-row identity for a sortable table of `AgentDefinition`
    /// rows (`RupuLibrary/LibraryScreen.swift`'s agents tab) — `name` alone
    /// is NOT unique: `list_agents`'s own doc comment (`crates/rupu-cp/src/
    /// api/agents.rs`) says "a project def shadows a same-named GLOBAL row;
    /// two different repos defining the same name both appear (distinguished
    /// by `scope`)". Composited exactly like `RupuStore.ActionKey.
    /// autoflow(name:scopeKind:scopeID:verb:)` (`RupuStore/Primitives/
    /// PendingAction.swift`) already does for the identical ambiguity on the
    /// autoflow-toggle pending-action key — same `"nil"` token for a `nil`
    /// `scopeID` — so this is the same row-identity contract the app already
    /// treats as canonical, not a fresh one. Used as a `ForEach` `id:` so a
    /// column-header sort re-ordering the array doesn't churn row identity
    /// (a row keeps following its own definition, never whatever offset it
    /// happens to land on).
    public var rowIdentity: String { "\(name):\(scopeKind):\(scopeID ?? "nil")" }
}

/// `GET /api/agents/:name` — `AgentDetailDto` on the Rust side: every
/// `AgentDto` field (see `AgentDefinition` above, which this deliberately
/// duplicates rather than reuses — the wire shape is FLAT/flattened via
/// serde's `#[serde(flatten)]`, not nested, so one `Decodable` struct with
/// its own `CodingKeys` is simpler than bolting `system_prompt`/`raw` onto
/// `AgentDefinition` via a wrapper) plus the two detail-only fields.
///
/// **`usage`/`runCount`/`lastRun` are deliberately NOT decoded here** —
/// unlike the list endpoint (`GET /api/agents`, which `AgentDefinition`
/// decodes), `load_detail` on the Rust side never enriches them: they are
/// always `0`/`nil` on this endpoint's response (confirmed by reading
/// `crates/rupu-cp/src/api/agents.rs::load_detail`, which builds every
/// branch via `AgentDto::from_spec` alone, with no run-activity pass
/// afterward). Decoding and then rendering an always-zero "0 runs" chip
/// would be a lie by omission of context — the Library list's own row
/// already has the real, enriched counts if the operator wants them.
public struct AgentDetail: Decodable, Sendable {
    public let name: String
    public let slug: String
    public let description: String?
    public let provider: String?
    public let model: String?
    public let effort: String?
    public let maxTokens: Int?
    public let mode: String?
    public let tools: [String]
    public let scope: String
    public let scopeKind: String
    public let scopeID: String?
    public let systemPrompt: String
    public let raw: String

    public init(
        name: String,
        slug: String,
        description: String?,
        provider: String?,
        model: String?,
        effort: String?,
        maxTokens: Int?,
        mode: String?,
        tools: [String],
        scope: String,
        scopeKind: String,
        scopeID: String?,
        systemPrompt: String,
        raw: String
    ) {
        self.name = name
        self.slug = slug
        self.description = description
        self.provider = provider
        self.model = model
        self.effort = effort
        self.maxTokens = maxTokens
        self.mode = mode
        self.tools = tools
        self.scope = scope
        self.scopeKind = scopeKind
        self.scopeID = scopeID
        self.systemPrompt = systemPrompt
        self.raw = raw
    }

    private enum CodingKeys: String, CodingKey {
        case name
        case slug
        case description
        case provider
        case model
        case effort
        case maxTokens = "max_tokens"
        case mode = "permission_mode"
        case tools
        case scope
        case scopeKind = "scope_kind"
        case scopeID = "scope_id"
        case systemPrompt = "system_prompt"
        case raw
    }
}

/// Row from `GET /api/workflows` (`WorkflowDto` on the Rust side) — no
/// `model` field (workflows don't have one). `usage` is returned by the
/// server but not decoded here, same rationale as `AgentDefinition`.
public struct WorkflowDefinition: Decodable, Equatable, Sendable {
    public let name: String
    public let scope: String
    public let scopeKind: String
    public let scopeID: String?
    public let runCount: Int
    public let lastRun: String?
    public let autoflowEnabled: Bool?

    public init(
        name: String,
        scope: String,
        scopeKind: String,
        scopeID: String?,
        runCount: Int,
        lastRun: String?,
        autoflowEnabled: Bool?
    ) {
        self.name = name
        self.scope = scope
        self.scopeKind = scopeKind
        self.scopeID = scopeID
        self.runCount = runCount
        self.lastRun = lastRun
        self.autoflowEnabled = autoflowEnabled
    }

    private enum CodingKeys: String, CodingKey {
        case name
        case scope
        case scopeKind = "scope_kind"
        case scopeID = "scope_id"
        case runCount = "run_count"
        case lastRun = "last_run"
        case autoflowEnabled = "autoflow_enabled"
    }
}

extension WorkflowDefinition {
    /// See `AgentDefinition.rowIdentity`'s doc comment — identical rationale
    /// (`list_workflows` shadows/duplicates the same way `list_agents` does),
    /// identical composite shape.
    public var rowIdentity: String { "\(name):\(scopeKind):\(scopeID ?? "nil")" }
}

/// Row from `GET /api/autoflows` (and `GET /api/projects/:ws_id/autoflows`)
/// — `AutoflowDefRow` on the Rust side. A workflow definition carrying a
/// top-level `autoflow:` block, enabled or disabled alike (a disabled def is
/// still listed — see `scan_autoflow_defs`'s doc comment on the Rust side).
/// `slug` is the file stem (the workflow detail route's key), which can
/// differ from `name` (the parsed frontmatter/YAML `name:`).
public struct AutoflowDefinition: Decodable, Equatable, Sendable {
    public let name: String
    public let slug: String
    public let trigger: String
    public let scope: String
    public let scopeKind: String
    public let scopeID: String?
    public let enabled: Bool

    public init(
        name: String,
        slug: String,
        trigger: String,
        scope: String,
        scopeKind: String,
        scopeID: String?,
        enabled: Bool
    ) {
        self.name = name
        self.slug = slug
        self.trigger = trigger
        self.scope = scope
        self.scopeKind = scopeKind
        self.scopeID = scopeID
        self.enabled = enabled
    }

    private enum CodingKeys: String, CodingKey {
        case name
        case slug
        case trigger
        case scope
        case scopeKind = "scope_kind"
        case scopeID = "scope_id"
        case enabled
    }
}

extension AutoflowDefinition {
    /// See `AgentDefinition.rowIdentity`'s doc comment — identical rationale
    /// and shape. Deliberately `name`-based, matching `ActionKey.
    /// autoflow(name:scopeKind:scopeID:verb:)`'s own composite exactly (not
    /// `slug`-based) — the app already treats `(name, scopeKind, scopeID)`
    /// as "the same row" for this exact entity via that key (see its doc
    /// comment), so the sortable table's row identity agrees with the
    /// pending-action identity already governing this row's toggle instead
    /// of introducing a second, subtly different notion of "same row".
    public var rowIdentity: String { "\(name):\(scopeKind):\(scopeID ?? "nil")" }
}

/// One declared input on a workflow's `workflow.inputs` map (`GET
/// /api/workflows/:name`'s `workflow.inputs.<name>` shape) — the Launcher's
/// declared-input rows source.
public struct WorkflowInputDef: Decodable, Equatable, Sendable {
    public let type: String
    public let required: Bool
    public let `default`: String?
    /// The wire field is `"enum"` (a YAML/JSON Schema-ism carried through
    /// from the workflow definition), always present — `[]` rather than
    /// absent when the input has no restricted value set. Modeled here as
    /// `allowedValues` since `enum` is a reserved word and a poor label for
    /// callers regardless.
    public let allowedValues: [String]

    public init(type: String, required: Bool, `default` value: String?, allowedValues: [String]) {
        self.type = type
        self.required = required
        self.default = value
        self.allowedValues = allowedValues
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case required
        case `default`
        case `enum`
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        type = try container.decodeIfPresent(String.self, forKey: .type) ?? "string"
        required = try container.decode(Bool.self, forKey: .required)
        allowedValues = try container.decodeIfPresent([String].self, forKey: .enum) ?? []
        self.default = try Self.tolerantString(container, forKey: .default)
    }

    /// `default` arrives as an arbitrary YAML scalar on the wire (string,
    /// number, bool, or null/absent) — this coerces whichever JSON scalar
    /// shape it takes into a `String`, so the UI always has a plain string
    /// to prefill an input field with regardless of the workflow author's
    /// YAML type choice.
    private static func tolerantString(
        _ container: KeyedDecodingContainer<CodingKeys>,
        forKey key: CodingKeys
    ) throws -> String? {
        guard container.contains(key), try !container.decodeNil(forKey: key) else { return nil }
        if let value = try? container.decode(String.self, forKey: key) { return value }
        if let value = try? container.decode(Bool.self, forKey: key) { return String(value) }
        if let value = try? container.decode(Int.self, forKey: key) { return String(value) }
        if let value = try? container.decode(Double.self, forKey: key) { return String(value) }
        return nil
    }
}

/// `GET /api/workflows/:name` response — a custom decode that reaches into
/// the nested `workflow` object for `name`/`inputs` (the endpoint also
/// returns `usage`/`scope`/`scope_kind`/`scope_id` alongside `yaml`, not
/// needed by this phase's UI and so not decoded).
public struct WorkflowDetail: Decodable, Sendable {
    public let name: String
    public let inputs: [String: WorkflowInputDef]
    public let yaml: String

    public init(name: String, inputs: [String: WorkflowInputDef], yaml: String) {
        self.name = name
        self.inputs = inputs
        self.yaml = yaml
    }

    private enum CodingKeys: String, CodingKey {
        case workflow
        case yaml
    }

    private enum WorkflowKeys: String, CodingKey {
        case name
        case inputs
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let workflow = try container.nestedContainer(keyedBy: WorkflowKeys.self, forKey: .workflow)
        name = try workflow.decode(String.self, forKey: .name)
        inputs = try workflow.decodeIfPresent([String: WorkflowInputDef].self, forKey: .inputs) ?? [:]
        yaml = try container.decode(String.self, forKey: .yaml)
    }
}

/// One entry of `GET /api/tools`'s `tools` array. Only `name`/`description`/
/// `kind` are decoded — `input_schema` (a JSON Schema object describing the
/// tool's `with:` args) is ignored this phase.
public struct ToolSpec: Decodable, Equatable, Sendable {
    public let name: String
    public let description: String
    public let kind: String

    public init(name: String, description: String, kind: String) {
        self.name = name
        self.description = description
        self.kind = kind
    }

    private enum CodingKeys: String, CodingKey {
        case name
        case description
        case kind
    }
}

/// `GET /api/tools`'s top-level envelope (`{tools: [...]}`) — internal, not
/// part of the public model surface; `CPClient.tools()` unwraps it.
struct ToolsListResponse: Decodable, Sendable {
    let tools: [ToolSpec]
}
