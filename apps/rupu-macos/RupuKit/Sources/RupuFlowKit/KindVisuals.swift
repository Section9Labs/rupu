// KindVisuals — the SINGLE shared source for "what does a StepKind look
// like": an accent color TOKEN, a lucide icon name, a flowchart shape, and
// a palette family. Line-for-line port of `crates/rupu-cp/web/src/
// components/workflow-editor/kindVisuals.ts`'s KIND_ACCENT/KIND_ICON/
// KIND_SHAPE/KIND_FAMILY tables.
//
// Per-kind accent → a THEMED palette token: step/blue (running), for_each/
// violet (brand), parallel/purple (sev-critical), panel/amber (awaiting),
// branch/green (done — a routing decision, distinct from every other
// kind).
//
// `KindAccent` carries the token NAME, not a resolved color — RupuFlowKit
// stays UI-free (no RupuDesign/SwiftUI dependency); RupuBuilder resolves
// each case to its themed `Color` the same way `ColorKey` resolution works
// elsewhere in the app.

public enum KindAccent: Sendable {
    case statusRunning
    case brand500
    case sevCritical
    case statusAwaiting
    case statusDone
    case statusPaused
    case sevInfo
    case sevMedium
    case brand600
    case brand700
}

/// Which palette family a kind belongs to — drives the node-palette rail's
/// "Work" / "Orchestration" subheadings. `work` kinds carry their own
/// agent/action work; `orchestration` kinds route/gate/fan the run without
/// doing any of their own.
public enum KindFamily: String, Sendable {
    case work
    case orchestration
}

public struct KindVisual: Sendable {
    public let accent: KindAccent
    /// `LucideIcon` rawValue-compatible name — RupuBuilder maps this to the
    /// actual `LucideIcon` case.
    public let iconName: String
    public let shape: ShapeName
    public let family: KindFamily
    public let tagline: String

    public init(accent: KindAccent, iconName: String, shape: ShapeName, family: KindFamily, tagline: String) {
        self.accent = accent
        self.iconName = iconName
        self.shape = shape
        self.family = family
        self.tagline = tagline
    }
}

/// What flowchart symbol/accent/icon/tagline a kind gets. `parallel` and
/// `panel` keep a rectangular-derived body (subroutine/stacked) deliberately
/// — they are the only kinds whose height grows with content. `split`/
/// `join` get their own fan-out/fan-in silhouettes — deliberate
/// placeholders (recognizable and geometrically correct, not final art).
public func kindVisual(_ k: StepKind) -> KindVisual {
    switch k {
    case .step:
        return KindVisual(accent: .statusRunning, iconName: "bot", shape: .rect, family: .work, tagline: "one agent")
    case .forEach:
        return KindVisual(accent: .brand500, iconName: "repeat", shape: .hexagon, family: .work, tagline: "over a list")
    case .parallel:
        return KindVisual(accent: .sevCritical, iconName: "columns3", shape: .subroutine, family: .work, tagline: "N at once")
    case .panel:
        return KindVisual(accent: .statusAwaiting, iconName: "shieldCheck", shape: .stacked, family: .work, tagline: "review+gate")
    case .action:
        return KindVisual(accent: .sevInfo, iconName: "zap", shape: .parallelogram, family: .work, tagline: "connector call")
    case .run:
        return KindVisual(accent: .sevMedium, iconName: "terminal", shape: .rect, family: .work, tagline: "command")
    case .branch:
        return KindVisual(accent: .statusDone, iconName: "gitBranch", shape: .vhex, family: .orchestration, tagline: "if / then / else")
    case .split:
        return KindVisual(accent: .brand600, iconName: "split", shape: .fanout, family: .orchestration, tagline: "fan out")
    case .join:
        return KindVisual(accent: .brand700, iconName: "merge", shape: .fanin, family: .orchestration, tagline: "barrier")
    case .approvalGate:
        return KindVisual(accent: .statusPaused, iconName: "userCheck", shape: .trapezoid, family: .orchestration, tagline: "human approval")
    }
}

// ── Block catalog (rail "Blocks" tab detail card) ───────────────────────────

/// One entry per `StepKind` the palette offers. Field names in `example`
/// mirror the real `Workflow`/`Step` schema (`crates/rupu-orchestrator/src/
/// workflow.rs`) exactly.
public struct BlockCatalogEntry: Sendable {
    public let kind: StepKind
    public let label: String
    /// One-sentence "what this is" blurb.
    public let what: String
    /// Required step keys, `*`-marked in the detail card.
    public let requiredFields: [String]
    /// A short, real YAML snippet.
    public let example: String

    public init(kind: StepKind, label: String, what: String, requiredFields: [String], example: String) {
        self.kind = kind
        self.label = label
        self.what = what
        self.requiredFields = requiredFields
        self.example = example
    }
}

/// Verbatim port of `NodePalette.tsx`'s `BLOCK_CATALOG` (lines 74-139) — the
/// eight web entries word-for-word — plus two macOS-only entries (`action`,
/// `run`) per the macOS Workflow Builder spec §3, since the web editor never
/// treats those as distinct palette blocks.
public let blockCatalog: [BlockCatalogEntry] = [
    BlockCatalogEntry(
        kind: .step,
        label: "step",
        what: "Runs one agent against a prompt and records its output for later steps to reference.",
        requiredFields: ["agent", "prompt"],
        example: "- id: review\n  agent: code-reviewer\n  prompt: \"Review the diff.\""
    ),
    BlockCatalogEntry(
        kind: .forEach,
        label: "for_each",
        what: "Runs one agent's prompt once per item in a list, up to max_parallel at a time.",
        requiredFields: ["agent", "for_each", "prompt"],
        example: "- id: review_each\n  agent: code-reviewer\n  for_each: \"{{ inputs.files }}\"\n  max_parallel: 4\n  prompt: \"Review {{ item }}.\""
    ),
    BlockCatalogEntry(
        kind: .parallel,
        label: "parallel",
        what: "Runs a fixed set of named sub-steps concurrently, each with its own agent and prompt.",
        requiredFields: ["parallel"],
        example: "- id: fanout\n  parallel:\n    - id: a\n      agent: writer\n      prompt: \"...\"\n    - id: b\n      agent: code-reviewer\n      prompt: \"...\""
    ),
    BlockCatalogEntry(
        kind: .panel,
        label: "panel",
        what: "Runs several agents (\"panelists\") against one subject in parallel and aggregates their findings.",
        requiredFields: ["panelists", "subject"],
        example: "- id: review\n  panel:\n    panelists: [security-reviewer, performance-reviewer]\n    subject: \"{{ inputs.diff }}\""
    ),
    BlockCatalogEntry(
        kind: .branch,
        label: "branch",
        what: "Evaluates a condition and routes the run to a then/else set of next steps — no agent runs on this node itself.",
        requiredFields: ["condition"],
        example: "- id: route\n  branch:\n    condition: \"{{ steps.assess.output == 'clean' }}\"\n    then: [ship]\n    else: [fix]"
    ),
    BlockCatalogEntry(
        kind: .approvalGate,
        label: "gate",
        what: "Pauses the run for a human approve/reject decision before continuing; can auto-approve from an expression. "
            + "Every field on `approval:` is optional (including prompt: — the approval message shown to approvers) — only the block's presence is required.",
        requiredFields: [],
        example: "- id: ship_gate\n  approval:\n    prompt: \"Approve to continue?\"\n    timeout_seconds: 86400\n    on_timeout: reject"
    ),
    BlockCatalogEntry(
        kind: .split,
        label: "split",
        what: "Fans the run out along explicit target step ids — every listed target runs; the split step itself carries no agent/action of its own.",
        requiredFields: [],
        example: "- id: fanout\n  split: [review_a, review_b]"
    ),
    BlockCatalogEntry(
        kind: .join,
        label: "join",
        what: "A barrier: waits for its inbound edges per a wait policy (all / any / a count) before the run continues past it.",
        requiredFields: [],
        example: "- id: barrier\n  join:\n    wait: all"
    ),
    BlockCatalogEntry(
        kind: .action,
        label: "action",
        what: "Invokes one SCM/issue/CI connector tool with parameters — no agent runs on this node.",
        requiredFields: ["action"],
        example: "- id: comment\n  action: github.issue_comment\n  with:\n    body: \"Done.\""
    ),
    BlockCatalogEntry(
        kind: .run,
        label: "run",
        what: "Runs one deterministic command; stdout binds to steps.<id>.output.",
        requiredFields: ["cmd"],
        example: "- id: scan\n  run:\n    cmd: semgrep\n    args: [scan, --config, auto]"
    ),
]
