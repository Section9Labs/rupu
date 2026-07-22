//! Workflow + Step structs + YAML parser.
//!
//! Supports linear orchestrations with conditional step execution
//! (`when:`), per-step / workflow-level error tolerance
//! (`continue_on_error`), typed workflow inputs (`inputs:`), a
//! `trigger:` declaration (manual / cron / event), data fan-out
//! (`for_each:`) — one agent across N items, results in
//! `steps.<id>.results[*]` — agent fan-out (`parallel:`) — N
//! distinct sub-steps over the same input, results in
//! `steps.<id>.results.<sub_id>` — and panel steps (`panel:`) — N
//! reviewer agents in parallel over a shared subject with optional
//! gate-loop and fixer dispatch (see [`Panel`] and
//! `runner::run_panel_step`).

use rupu_coverage::ConcernsBlock;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowParseError {
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("workflow YAML key `{key}` is not yet supported (deferred — see TODO.md)")]
    UnsupportedKey { key: &'static str },
    #[error("workflow has no steps")]
    Empty,
    #[error("duplicate step id: {0}")]
    DuplicateStep(String),
    #[error("input `{name}`: invalid `default` for type `{ty}`: {reason}")]
    InvalidInputDefault {
        name: String,
        ty: &'static str,
        reason: String,
    },
    #[error("input `{name}`: `default` value is not in the declared `enum` ({allowed:?})")]
    DefaultNotInEnum { name: String, allowed: Vec<String> },
    #[error("trigger.on=`cron` requires a non-empty `cron:` field")]
    TriggerCronMissing,
    #[error("trigger.on=`cron`: `cron:` value `{value}` is not a valid 5-field cron expression: {reason}")]
    TriggerCronInvalid { value: String, reason: String },
    #[error("trigger.on=`event` requires a non-empty `event:` field")]
    TriggerEventMissing,
    #[error("trigger.on=`{kind}` does not accept a `{field}:` field; remove it")]
    TriggerExtraneousField {
        kind: &'static str,
        field: &'static str,
    },
    #[error("step `{step}`: `max_parallel` must be at least 1, got {value}")]
    InvalidMaxParallel { step: String, value: i64 },
    #[error(
        "step `{step}`: `parallel:` is mutually exclusive with `for_each:` and with the top-level `agent`/`prompt`"
    )]
    ParallelMutuallyExclusive { step: String },
    #[error("step `{step}`: `parallel:` block must contain at least one sub-step")]
    ParallelEmpty { step: String },
    #[error("step `{step}`: duplicate sub-step id `{sub}` inside `parallel:`")]
    ParallelDuplicateSubId { step: String, sub: String },
    #[error(
        "step `{step}`: missing required field `{field}` (linear and `for_each:` steps need `agent:` and `prompt:`)"
    )]
    MissingStepField { step: String, field: &'static str },
    #[error(
        "step `{step}`: `panel:` is mutually exclusive with `for_each:`, `parallel:`, and the top-level `agent`/`prompt`"
    )]
    PanelMutuallyExclusive { step: String },
    #[error("step `{step}`: `panel.panelists` must contain at least one agent")]
    PanelEmpty { step: String },
    #[error("step `{step}`: `panel.gate.max_iterations` must be at least 1, got {value}")]
    PanelMaxIterationsInvalid { step: String, value: u32 },
    #[error(
        "autoflow field `{field}` has invalid duration `{value}`; expected `<int><unit>` where unit is one of `s`, `m`, `h`, `d`"
    )]
    InvalidAutoflowDuration { field: &'static str, value: String },
    #[error("workflow output contract `{output}` references unknown step `{step}`")]
    ContractOutputUnknownStep { output: String, step: String },
    #[error("autoflow outcome references unknown workflow output `{output}`")]
    AutoflowOutcomeUnknownOutput { output: String },
    #[error(
        "autoflow.selector.{field} is only valid when `entity: pull_request` (this workflow declares `entity: {entity}`)"
    )]
    AutoflowSelectorFieldWrongEntity {
        field: &'static str,
        entity: &'static str,
    },
    #[error(
        "workflow output `{output}` and step `{step}` contract disagree on `{field}`: workflow declares `{workflow_declared}`, step declares `{step_declared}`"
    )]
    ContractStepMismatch {
        output: String,
        step: String,
        field: &'static str,
        workflow_declared: String,
        step_declared: String,
    },
    #[error(
        "step `{step}` {template_kind} references `steps.{referenced}` but no step with that id exists"
    )]
    TemplateUnknownStepRef {
        step: String,
        template_kind: &'static str,
        referenced: String,
    },
    #[error(
        "step `{step}` {template_kind} references `steps.{referenced}` but that step runs *after* this one (forward reference — its output isn't bound yet)"
    )]
    TemplateForwardStepRef {
        step: String,
        template_kind: &'static str,
        referenced: String,
    },
    #[error(
        "step `{step}` {template_kind} references `steps.{referenced_step}.{field}` but `{field}` is not a known step-output field (valid: output, success, skipped, results, sub_results, findings, max_severity, iterations, resolved)"
    )]
    TemplateUnknownStepField {
        step: String,
        template_kind: &'static str,
        referenced_step: String,
        field: String,
    },
    #[error("step `{step}`: `distribute:` is only valid on a `for_each:` step")]
    DistributeWithoutForEach { step: String },
    #[error("step `{step}`: `distribute.hosts` must be non-empty")]
    DistributeEmptyHosts { step: String },
    #[error("step `{step}`: `host:` is only valid on a linear step (agent + prompt), not on `for_each:`/`parallel:`/`panel:`")]
    HostOnNonLinearStep { step: String },
    #[error("step `{step}`: `host:` must not be empty")]
    HostEmpty { step: String },
    #[error("step `{step}`: `workspace: sync` is only valid on a remote step (`host:` or `distribute:`)")]
    WorkspaceSyncOnLocalStep { step: String },
}

/// How a workflow gets kicked off. Manual is the existing behavior
/// (CLI `rupu workflow run <name>`); cron and event declarations
/// parse + validate today but the scheduler / webhook receiver that
/// actually fire them are deferred to follow-up PRs (see TODO.md →
/// "Workflow triggers" multi-PR initiative).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    #[default]
    Manual,
    Cron,
    Event,
}

impl TriggerKind {
    fn name(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Cron => "cron",
            Self::Event => "event",
        }
    }
}

/// Top-level `trigger:` block. The cron + event fields are mutually
/// exclusive with each other and with the implicit manual default.
/// Validation happens in [`Workflow::parse`] so users see a clear
/// error before the workflow loads.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    #[serde(default)]
    pub on: TriggerKind,
    /// Standard 5-field cron expression (`min hour dom mon dow`).
    /// Required when `on: cron`; rejected otherwise.
    #[serde(default)]
    pub cron: Option<String>,
    /// Event identifier, e.g. `github.pr.opened`,
    /// `github.issue.created`, `issue.state_changed`. Required when
    /// `on: event`; rejected otherwise. The vocabulary is enforced by
    /// the webhook receiver in a future PR; this PR accepts any
    /// non-empty string so the schema can land before the runtime.
    #[serde(default)]
    pub event: Option<String>,
    /// Optional minijinja-style filter expression evaluated against
    /// the event payload (`{{event.repo.name == 'rupu'}}`). Only
    /// meaningful when `on: event`. Rendering happens at trigger time
    /// in the future event-receiver PR; this PR just preserves the
    /// string verbatim.
    #[serde(default)]
    pub filter: Option<String>,
}

/// Declared type for a workflow input. Drives default-value
/// coercion + error messages on `--input` parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    String,
    Int,
    Bool,
}

impl InputType {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Int => "int",
            Self::Bool => "bool",
        }
    }
}

/// A workflow-level input declaration. Authors `inputs:` block in YAML;
/// users provide values at runtime via `rupu workflow run --input k=v`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    #[serde(rename = "type", default = "InputDef::default_type")]
    pub ty: InputType,
    #[serde(default)]
    pub required: bool,
    /// Default value as a YAML scalar. Coerced against `ty` at parse
    /// time so an invalid default shows up before the workflow runs.
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    /// Allowed values; if non-empty, the runtime rejects inputs not
    /// in this list. Stored as strings for simplicity (covers all
    /// three input types via stringification).
    #[serde(rename = "enum", default)]
    pub allowed: Vec<String>,
    /// Free-form human description. Surfaced in `rupu workflow show`
    /// and similar listings; ignored by the runtime. Matches the
    /// convention from GitHub Actions / Argo / etc. so authors can
    /// drop it in without surprise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl InputDef {
    fn default_type() -> InputType {
        InputType::String
    }
}

/// Top-level `autoflow:` block. This extends the existing workflow
/// YAML schema with autonomous-execution metadata while keeping the
/// same `steps:` DSL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Autoflow {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub entity: AutoflowEntity,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub selector: AutoflowSelector,
    #[serde(default)]
    pub wake_on: Vec<String>,
    #[serde(default)]
    pub reconcile_every: Option<String>,
    #[serde(default)]
    pub claim: Option<AutoflowClaim>,
    #[serde(default)]
    pub workspace: Option<AutoflowWorkspace>,
    #[serde(default)]
    pub outcome: Option<AutoflowOutcomeRef>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoflowEntity {
    #[default]
    Issue,
    PullRequest,
}

impl AutoflowEntity {
    fn name(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::PullRequest => "pull_request",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutoflowSelector {
    #[serde(default)]
    pub states: Vec<AutoflowIssueState>,
    #[serde(default)]
    pub labels_all: Vec<String>,
    #[serde(default)]
    pub labels_any: Vec<String>,
    #[serde(default)]
    pub labels_none: Vec<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    /// Filter on draft status. Only meaningful for `entity:
    /// pull_request`; rejected on `entity: issue` (see
    /// [`WorkflowParseError::AutoflowSelectorFieldWrongEntity`]).
    #[serde(default)]
    pub draft: Option<DraftFilter>,
    /// Restrict to pull requests targeting this base branch (e.g.
    /// `main`). Only meaningful for `entity: pull_request`; rejected
    /// on `entity: issue`.
    #[serde(default)]
    pub base: Option<String>,
    /// Explicit allowlist of author logins. Empty (the default) means
    /// no explicit-list restriction; see [`author_allowed`].
    #[serde(default)]
    pub authors: Vec<String>,
    /// Broader author-scope check (e.g. "any repo collaborator").
    /// `None` (the default) means no scope-based restriction.
    #[serde(default)]
    pub authors_from: Option<AuthorScope>,
    /// What to do when an event is skipped because its author isn't
    /// allowed. `None` behaves as [`SkipAction::Skip`].
    #[serde(default)]
    pub on_skip: Option<SkipAction>,
}

/// Broader author-scope check used by [`AutoflowSelector::authors_from`].
/// Whether a given login satisfies one of these scopes is resolved by
/// the SCM connector at tick time; this crate only carries the enum and
/// the pure allow/deny decision in [`author_allowed`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorScope {
    /// Author must be a collaborator on the repository.
    Collaborators,
    /// Author must be a member of the repository's owning organization.
    OrgMembers,
}

/// What an autoflow tick should do when an otherwise-eligible event is
/// excluded solely by the author allowlist.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkipAction {
    /// Silently skip the event (default).
    Skip,
    /// Skip, but label the issue/PR to flag it for a human.
    LabelNeedsHuman,
}

/// Pure author-eligibility gate for autoflow selectors.
///
/// The network/SCM lookup (is this login a collaborator or org member?)
/// happens in the tick/SCM layer; this function only encodes the
/// decision rule so it stays unit-testable without any I/O:
///
/// - If `selector.authors` is non-empty and contains `author_login`,
///   the author is allowed regardless of `authors_from`.
/// - Otherwise, if `selector.authors_from` is set, the author is
///   allowed iff `is_collaborator` is `true` (the caller has already
///   resolved whatever scope `authors_from` names against the SCM).
/// - Otherwise, if both `authors` is empty and `authors_from` is
///   `None`, there is no author restriction at all: allowed (this
///   preserves the behavior of existing autoflows that predate the
///   author-allowlist fields).
/// - Otherwise (`authors` is non-empty, no match, and `authors_from`
///   is `None`), the author is denied.
pub fn author_allowed(
    selector: &AutoflowSelector,
    author_login: &str,
    is_collaborator: bool,
) -> bool {
    if !selector.authors.is_empty() && selector.authors.iter().any(|a| a == author_login) {
        return true;
    }
    if selector.authors_from.is_some() {
        return is_collaborator;
    }
    selector.authors.is_empty()
}

/// Draft-status filter for `entity: pull_request` autoflows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftFilter {
    /// Match both draft and ready-for-review pull requests.
    Include,
    /// Match only ready-for-review pull requests (exclude drafts).
    Exclude,
    /// Match only draft pull requests.
    Only,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoflowIssueState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutoflowClaim {
    #[serde(default)]
    pub key: AutoflowClaimKey,
    #[serde(default)]
    pub ttl: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoflowClaimKey {
    #[default]
    Issue,
    PrHeadSha,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutoflowWorkspace {
    #[serde(default)]
    pub strategy: AutoflowWorkspaceStrategy,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoflowWorkspaceStrategy {
    #[default]
    Worktree,
    InPlace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutoflowOutcomeRef {
    pub output: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Contracts {
    #[serde(default)]
    pub outputs: BTreeMap<String, WorkflowOutputContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowOutputContract {
    pub from_step: String,
    pub format: ContractFormat,
    pub schema: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContractFormat {
    Json,
    Yaml,
}

impl ContractFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StepContract {
    pub emits: String,
    pub format: ContractFormat,
}

/// Severity ordering for panel-step findings. Compares as
/// `Low < Medium < High < Critical`. The gate threshold compares
/// against the *maximum* severity in the aggregated findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Parse a case-insensitive severity from JSON / agent output.
    /// Unknown values default to `Low` rather than failing — agent
    /// outputs are messy and a permissive parse keeps the gate
    /// loop from crashing on a typo.
    pub fn parse_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "critical" | "crit" => Self::Critical,
            "high" => Self::High,
            "medium" | "med" => Self::Medium,
            _ => Self::Low,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// Panel-step block. The runner dispatches every panelist in
/// parallel against the rendered `subject:` (think: the diff /
/// proposal under review), collects each panelist's findings JSON
/// from their final assistant message, and aggregates them. Each
/// panelist must emit, in its final assistant text, a JSON object
/// of the shape:
///
/// ```text
/// { "findings": [
///     { "severity": "high|medium|low|critical",
///       "title": "<short>",
///       "body":  "<details>" },
///     ...
/// ] }
/// ```
///
/// The runner extracts the first JSON object that parses cleanly
/// (so panelists may include surrounding prose). Panelists that
/// emit malformed JSON contribute zero findings and a warning is
/// logged.
///
/// When a `gate:` is present, the runner re-dispatches the panel
/// after a fixer agent addresses the findings, and loops until
/// the maximum severity drops below the threshold or
/// `max_iterations` is hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    /// One agent name per panelist. They run in parallel against
    /// the same rendered subject.
    pub panelists: Vec<String>,
    /// minijinja template that renders to the input every panelist
    /// reviews. Bound as `{{ subject }}` inside the panelist's
    /// rendered prompt; also passed through verbatim if the
    /// panelist agent has no prompt template (less common).
    pub subject: String,
    /// Optional per-panelist prompt template. When omitted, the
    /// runner sends `subject:` as the user message verbatim — the
    /// panelist's agent file's `system_prompt` carries the
    /// review instructions.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Cap on concurrent in-flight panelist runs. Same semantics
    /// as `Step.max_parallel:`. Defaults to 1 (serial) when `None`.
    #[serde(default)]
    pub max_parallel: Option<u32>,
    /// Optional gate that drives a fix-loop. When present, after
    /// each panel iteration the runner classifies findings by
    /// severity; if the max severity is `>=
    /// until_no_findings_at_severity_or_above`, the runner
    /// dispatches `fix_with` (with the aggregated findings as
    /// input) and re-runs the panel. Loops until the gate clears
    /// or `max_iterations` runs out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<PanelGate>,
}

/// Loop-termination policy for a panel step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PanelGate {
    /// Loop while the max-severity finding is at or above this
    /// threshold. Once the panel emits zero findings at that
    /// threshold (or higher), the gate clears and the workflow
    /// proceeds.
    pub until_no_findings_at_severity_or_above: Severity,
    /// Agent name dispatched between panel iterations to address
    /// the findings. Receives the aggregated findings as the user
    /// message; its final assistant text becomes the next
    /// iteration's `subject`.
    pub fix_with: String,
    /// Safety cap. The loop exits with `resolved=false` when the
    /// gate hasn't cleared after this many iterations. Required;
    /// no implicit default — authors should think about it.
    pub max_iterations: u32,
}

/// Branch step block. Mutually exclusive with `for_each:`,
/// `parallel:`, `panel:`, and the linear `agent`/`prompt`. The
/// runner renders `condition:` and dispatches the step ids listed in
/// `then:` when it's truthy, or the ones in `else:` otherwise.
///
/// Adding this field is additive-only: a branch step currently falls
/// through to the linear validation/dispatch path (Task 3 adds shape
/// validation, Task 5 adds runner dispatch).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    /// minijinja template rendered to decide which branch to take.
    /// Truthy per the same rules as `when:`.
    pub condition: String,
    /// Step ids to dispatch when `condition:` is truthy.
    #[serde(default)]
    pub then: Vec<String>,
    /// Step ids to dispatch when `condition:` is falsy. `else` is a
    /// reserved word, hence the raw identifier.
    #[serde(default)]
    pub r#else: Vec<String>,
}

/// Optional approval gate on a step. When present and `required:
/// true`, the runner persists `RunStatus::AwaitingApproval` and exits
/// cleanly *before* dispatching the step. The operator approves with
/// `rupu workflow approve <run-id>`, which mutates the persisted
/// state and resumes execution from the awaited step.
///
/// Approval is checked AFTER the `when:` gate — a step skipped
/// because of `when:` doesn't ask for approval. Approval pauses
/// regardless of fan-out shape (linear / `for_each:` / `parallel:`)
/// since pausing happens before any dispatch.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    /// When `true`, the runner pauses before dispatching this step.
    /// `false` (or the field absent) is a no-op — the step runs
    /// normally. Authors can flip this with a minijinja-rendered
    /// expression too, but for parse-time clarity we accept a plain
    /// bool here; conditional gating is best expressed via `when:`.
    #[serde(default)]
    pub required: bool,
    /// Optional human-readable prompt the operator sees when
    /// approving. Rendered with the same template engine and
    /// context as `prompt:`, so authors can include
    /// `{{ inputs.tag }}` etc.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Optional timeout. When set and the run hasn't been
    /// approved/rejected within this window, the next interaction
    /// (`rupu workflow runs` / `approve` / `reject`) marks the run
    /// `Failed` with an `expired` error message rather than
    /// resuming. v0 evaluates the timeout lazily on operator
    /// interaction; a future ticker daemon could enforce it eagerly.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Workflow-level defaults inherited by every step. A step's own
/// override (when present) wins over the default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefaults {
    /// When `Some(true)`, every step inherits `continue_on_error: true`
    /// unless the step explicitly overrides. `Some(false)` and `None`
    /// behave the same way at runtime — the step-level default is also
    /// false — but `Some(false)` is preserved for round-trip clarity.
    #[serde(default)]
    pub continue_on_error: Option<bool>,
    /// Workflow-wide default workspace mode for remote steps. A step's
    /// `workspace:` overrides this. Absent ⇒ `None` (self-contained).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceMode>,
}

/// Whether a step's file workspace is synced to the remote host it runs on.
/// `None` (the default) keeps the self-contained behavior of Slices 3a/3b:
/// the remote step sees only its rendered prompt + prior-step string outputs.
/// `Sync` makes the coordinator's workspace available on the host and brings
/// the file changes back (Slice 3c). Only meaningful on a remote step
/// (`host:` or `distribute:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMode {
    Sync,
    None,
}

/// Fleet placement for a `for_each:` step — spreads units across hosts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distribute {
    /// Fleet host ids/names to spread this step's units across (round-robin).
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    /// Required for linear and `for_each:` steps; absent for
    /// `parallel:` steps (which carry their own per-sub-step `agent`).
    /// Validation enforces presence in the right shapes.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    /// Optional minijinja expression rendered against the step
    /// context (`inputs.*`, `steps.<id>.*`). When the rendered value
    /// is "false", "0", "", "no", or "off" (case-insensitive), the
    /// step is skipped. Any other rendered value runs the step.
    #[serde(default)]
    pub when: Option<String>,
    /// When `Some(true)`, a failure in this step is logged and the
    /// workflow continues to the next step. Overrides
    /// `WorkflowDefaults.continue_on_error`. For fan-out steps,
    /// applies per-item / per-sub-step: a failed unit is recorded
    /// with `success=false` and the remaining units still dispatch.
    #[serde(default)]
    pub continue_on_error: Option<bool>,
    /// Optional minijinja expression that, when rendered against the
    /// step context, yields a list of items to fan out across. Each
    /// item is dispatched to the same `agent:` with the same
    /// `prompt:` template, except the prompt also binds `{{item}}`
    /// (the current value) and `{{loop.index}}` (1-based).
    ///
    /// The renderer accepts:
    /// - a YAML / JSON array (parsed when the rendered string starts
    ///   with `[`),
    /// - one item per non-empty line otherwise.
    ///
    /// Per-item results are bound as `steps.<id>.results[*]` (a list
    /// of strings — each item's final assistant text) and
    /// `steps.<id>.output` is the JSON array of those strings, so
    /// existing template paths still see *something* useful.
    /// Mutually exclusive with `parallel:`.
    #[serde(default)]
    pub for_each: Option<String>,
    /// Optional list of sub-steps to dispatch in parallel. Each
    /// sub-step gets its own `id`, `agent`, and `prompt`. Mutually
    /// exclusive with `for_each:` and with the step-level
    /// `agent`/`prompt`. Per-sub-step results are bound as
    /// `steps.<id>.results.<sub_id>.output` (and `.success`) plus the
    /// list-form `steps.<id>.results[*]` for compatibility with
    /// `for_each:`-style consumers (entries appear in declared order).
    #[serde(default)]
    pub parallel: Option<Vec<SubStep>>,
    /// Cap on concurrent in-flight unit runs for a fan-out step
    /// (`for_each:` items or `parallel:` sub-steps). `None` (the
    /// default) is treated as 1 — units dispatch serially in declared
    /// order. Ignored for non-fan-out steps. Must be >= 1.
    #[serde(default)]
    pub max_parallel: Option<u32>,
    /// Required for linear and `for_each:` steps; absent for
    /// `parallel:` steps (each sub-step carries its own prompt).
    #[serde(default)]
    pub prompt: Option<String>,
    /// Optional approval gate. When `required: true`, the runner
    /// persists the run as `awaiting_approval` and exits before
    /// dispatching this step. See [`Approval`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
    /// Panel step block. Mutually exclusive with `for_each:`,
    /// `parallel:`, and the linear `agent`/`prompt`. See [`Panel`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<Panel>,
    /// Branch step block. Mutually exclusive with `for_each:`,
    /// `parallel:`, `panel:`, and the linear `agent`/`prompt`. See
    /// [`Branch`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
    /// Optional authoring metadata describing the structured output this
    /// step is expected to emit. Workflow-level `contracts.outputs.*`
    /// remain authoritative for runtime validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<StepContract>,
    /// Optional fleet placement for a `for_each:` step. When present,
    /// the step's units are spread across the named hosts (round-robin).
    /// Only valid on `for_each:` steps; an error if present without
    /// `for_each:`. Ignored when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribute: Option<Distribute>,
    /// Optional fleet host placement for a **linear** step. When present,
    /// the whole step's agent runs on the named host (via the
    /// `UnitDispatcher` port) instead of locally, and its output feeds
    /// downstream steps exactly as a local step would. Valid only on a
    /// linear step (`agent` + `prompt`; not `for_each`/`parallel`/`panel`)
    /// and not together with `distribute:` (which is `for_each`-only).
    /// Absent ⇒ runs locally (backward compatible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Per-step workspace mode override. When the effective mode (this, else
    /// `defaults.workspace`, else `None`) is `Sync`, the coordinator's
    /// workspace is synced to the remote host this step runs on. Valid only
    /// on a remote step (`host:` or `distribute:`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceMode>,
}

/// One sub-step inside a `parallel:` block. Same surface as a linear
/// step except `actions:` and `continue_on_error:` are not allowed —
/// the parent step controls those for the whole block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubStep {
    pub id: String,
    pub agent: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// How this workflow gets kicked off. Defaults to manual when
    /// omitted (the legacy behavior — `rupu workflow run <name>` from
    /// the CLI). Cron + event declarations are accepted at parse time
    /// but the runtime that fires them is in a follow-up PR.
    #[serde(default)]
    pub trigger: Trigger,
    /// Typed input declarations. The runtime validates `--input k=v`
    /// values against these (required-ness, enum membership, type
    /// coercion) before the first step runs.
    #[serde(default)]
    pub inputs: BTreeMap<String, InputDef>,
    /// Per-workflow defaults shared by every step.
    #[serde(default)]
    pub defaults: WorkflowDefaults,
    /// Optional autonomous-execution metadata. Present workflows still
    /// run normally via `rupu workflow run`; this block is consumed by
    /// the future `rupu autoflow ...` runtime.
    #[serde(default)]
    pub autoflow: Option<Autoflow>,
    /// Optional machine-readable workflow output declarations.
    #[serde(default)]
    pub contracts: Contracts,
    /// When `true` AND the run-target resolves to an issue, the CLI
    /// posts an auto-comment on the issue at run start (with the
    /// run-id) and at terminal state (with the outcome — completed /
    /// failed / awaiting_approval). Renders empty when no issue
    /// target. Default is `false` so existing workflows don't
    /// suddenly start writing to issue threads after upgrade.
    #[serde(default, rename = "notifyIssue")]
    pub notify_issue: bool,
    /// Coverage catalog for this workflow run. When `Some`, every step
    /// runs with this catalog, overriding any `concerns:` in the step's
    /// agent frontmatter. The workflow is the single source of truth for
    /// coverage concerns: all steps share the same `target_id` (derived
    /// from the workflow name) so their ledger entries accumulate together.
    #[serde(default)]
    pub concerns: Option<ConcernsBlock>,
    pub steps: Vec<Step>,
}

impl Workflow {
    /// Parse a YAML string. Validates step-id uniqueness and input
    /// defaults / enum constraints; returns clear errors on failure.
    pub fn parse(s: &str) -> Result<Self, WorkflowParseError> {
        // No deferred-key pre-scan anymore — every workflow keyword
        // documented in TODO.md is now wired up. Unknown fields are
        // caught by `#[serde(deny_unknown_fields)]` on the Step /
        // Workflow / Panel types instead.

        let wf: Workflow = serde_yaml::from_str(s)?;
        if wf.steps.is_empty() {
            return Err(WorkflowParseError::Empty);
        }
        let mut seen = BTreeSet::new();
        for step in &wf.steps {
            if !seen.insert(step.id.clone()) {
                return Err(WorkflowParseError::DuplicateStep(step.id.clone()));
            }
            if let Some(mp) = step.max_parallel {
                if mp < 1 {
                    return Err(WorkflowParseError::InvalidMaxParallel {
                        step: step.id.clone(),
                        value: mp as i64,
                    });
                }
            }
            validate_step_shape(step)?;
        }
        for (name, def) in &wf.inputs {
            validate_input_def(name, def)?;
        }
        validate_trigger(&wf.trigger)?;
        validate_contracts(&wf)?;
        validate_autoflow(&wf)?;
        validate_template_refs(&wf)?;
        Ok(wf)
    }

    pub fn parse_file(path: &std::path::Path) -> Result<Self, WorkflowParseError> {
        let s = std::fs::read_to_string(path)?;
        Self::parse(&s)
    }
}

/// Validate the trigger block's cross-field constraints. The cron
/// runtime + webhook receiver come in follow-up PRs; here we just
/// reject malformed declarations so authors see clear errors at
/// parse time.
fn validate_trigger(trigger: &Trigger) -> Result<(), WorkflowParseError> {
    match trigger.on {
        TriggerKind::Manual => {
            if trigger.cron.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Manual.name(),
                    field: "cron",
                });
            }
            if trigger.event.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Manual.name(),
                    field: "event",
                });
            }
            if trigger.filter.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Manual.name(),
                    field: "filter",
                });
            }
        }
        TriggerKind::Cron => {
            let value = trigger
                .cron
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or(WorkflowParseError::TriggerCronMissing)?;
            validate_cron_expression(value)?;
            if trigger.event.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Cron.name(),
                    field: "event",
                });
            }
            if trigger.filter.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Cron.name(),
                    field: "filter",
                });
            }
        }
        TriggerKind::Event => {
            let _ = trigger
                .event
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or(WorkflowParseError::TriggerEventMissing)?;
            if trigger.cron.is_some() {
                return Err(WorkflowParseError::TriggerExtraneousField {
                    kind: TriggerKind::Event.name(),
                    field: "cron",
                });
            }
        }
    }
    Ok(())
}

/// Lightweight 5-field cron validator. We don't need to interpret the
/// expression here (the cron runtime in a follow-up PR will use a
/// proper crate); this just catches obviously-malformed values at
/// parse time so the author isn't surprised at scheduler-startup.
fn validate_cron_expression(expr: &str) -> Result<(), WorkflowParseError> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(WorkflowParseError::TriggerCronInvalid {
            value: expr.to_string(),
            reason: format!(
                "expected 5 fields (min hour dom mon dow); got {}",
                fields.len()
            ),
        });
    }
    // Each field must be `*`, `*/N`, a number, a range `N-M`, a list
    // `N,M,...`, or any combination thereof. Reject patently bogus
    // characters; leave full semantic validation to the scheduler.
    for (idx, field) in fields.iter().enumerate() {
        if field.is_empty()
            || !field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '*' | '/' | '-' | ','))
        {
            return Err(WorkflowParseError::TriggerCronInvalid {
                value: expr.to_string(),
                reason: format!(
                    "field {} (`{}`) contains invalid characters",
                    idx + 1,
                    field
                ),
            });
        }
    }
    Ok(())
}

/// Resolve a step's effective workspace mode: the step's own `workspace`,
/// else the workflow `defaults.workspace`, else `WorkspaceMode::None`.
pub fn effective_workspace_mode(step: &Step, defaults: &WorkflowDefaults) -> WorkspaceMode {
    step.workspace
        .or(defaults.workspace)
        .unwrap_or(WorkspaceMode::None)
}

/// Validate per-step shape constraints. The four shapes (linear,
/// `for_each:`, `parallel:`, `panel:`) are mutually exclusive, and
/// each carries its own set of required fields.
fn validate_step_shape(step: &Step) -> Result<(), WorkflowParseError> {
    if let Some(panel) = &step.panel {
        // Panel step — top-level agent/prompt, for_each, and parallel
        // must all be absent.
        if step.agent.is_some()
            || step.prompt.is_some()
            || step.for_each.is_some()
            || step.parallel.is_some()
        {
            return Err(WorkflowParseError::PanelMutuallyExclusive {
                step: step.id.clone(),
            });
        }
        if panel.panelists.is_empty() {
            return Err(WorkflowParseError::PanelEmpty {
                step: step.id.clone(),
            });
        }
        if let Some(gate) = &panel.gate {
            if gate.max_iterations < 1 {
                return Err(WorkflowParseError::PanelMaxIterationsInvalid {
                    step: step.id.clone(),
                    value: gate.max_iterations,
                });
            }
        }
    } else if let Some(subs) = &step.parallel {
        // `parallel:` block — top-level agent/prompt and for_each must
        // be absent.
        if step.agent.is_some() || step.prompt.is_some() || step.for_each.is_some() {
            return Err(WorkflowParseError::ParallelMutuallyExclusive {
                step: step.id.clone(),
            });
        }
        if subs.is_empty() {
            return Err(WorkflowParseError::ParallelEmpty {
                step: step.id.clone(),
            });
        }
        let mut seen_sub = BTreeSet::new();
        for sub in subs {
            if !seen_sub.insert(sub.id.clone()) {
                return Err(WorkflowParseError::ParallelDuplicateSubId {
                    step: step.id.clone(),
                    sub: sub.id.clone(),
                });
            }
        }
    } else {
        // Linear / for_each step — agent + prompt are required.
        if step.agent.is_none() {
            return Err(WorkflowParseError::MissingStepField {
                step: step.id.clone(),
                field: "agent",
            });
        }
        if step.prompt.is_none() {
            return Err(WorkflowParseError::MissingStepField {
                step: step.id.clone(),
                field: "prompt",
            });
        }
    }

    // Validate distribute: only valid on for_each steps, and hosts must be non-empty.
    if let Some(dist) = &step.distribute {
        if step.for_each.is_none() {
            return Err(WorkflowParseError::DistributeWithoutForEach {
                step: step.id.clone(),
            });
        }
        if dist.hosts.is_empty() {
            return Err(WorkflowParseError::DistributeEmptyHosts {
                step: step.id.clone(),
            });
        }
    }

    // Validate host placement: only valid on a linear step (not panel /
    // parallel / for_each), non-empty, and never alongside `distribute:`
    // (which is for_each-only — structurally exclusive with a linear host
    // step, but assert it for a clear message).
    if let Some(host) = &step.host {
        let is_linear = step.panel.is_none() && step.parallel.is_none() && step.for_each.is_none();
        if !is_linear || step.distribute.is_some() {
            return Err(WorkflowParseError::HostOnNonLinearStep {
                step: step.id.clone(),
            });
        }
        if host.trim().is_empty() {
            return Err(WorkflowParseError::HostEmpty {
                step: step.id.clone(),
            });
        }
    }

    // `workspace: sync` is only meaningful on a remote step — one with `host:`
    // (3b) or `distribute:` (3a). On a purely-local step it would be a no-op
    // and signals author confusion, so reject it.
    if step.workspace == Some(WorkspaceMode::Sync)
        && step.host.is_none()
        && step.distribute.is_none()
    {
        return Err(WorkflowParseError::WorkspaceSyncOnLocalStep {
            step: step.id.clone(),
        });
    }

    Ok(())
}

/// Validate that an input declaration is internally consistent: the
/// `default` (if any) coerces to the declared `type`, and the `enum`
/// (if any) contains the default.
fn validate_input_def(name: &str, def: &InputDef) -> Result<(), WorkflowParseError> {
    if let Some(default) = &def.default {
        match def.ty {
            InputType::String => {
                if !default.is_string() {
                    return Err(WorkflowParseError::InvalidInputDefault {
                        name: name.to_string(),
                        ty: def.ty.name(),
                        reason: format!("expected string, got {default:?}"),
                    });
                }
            }
            InputType::Int => {
                if !default.is_i64() && !default.is_u64() {
                    return Err(WorkflowParseError::InvalidInputDefault {
                        name: name.to_string(),
                        ty: def.ty.name(),
                        reason: format!("expected integer, got {default:?}"),
                    });
                }
            }
            InputType::Bool => {
                if !default.is_bool() {
                    return Err(WorkflowParseError::InvalidInputDefault {
                        name: name.to_string(),
                        ty: def.ty.name(),
                        reason: format!("expected bool, got {default:?}"),
                    });
                }
            }
        }
        if !def.allowed.is_empty() {
            let stringified = yaml_scalar_to_string(default);
            if !def.allowed.contains(&stringified) {
                return Err(WorkflowParseError::DefaultNotInEnum {
                    name: name.to_string(),
                    allowed: def.allowed.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_contracts(wf: &Workflow) -> Result<(), WorkflowParseError> {
    for (output, contract) in &wf.contracts.outputs {
        let Some(step) = wf.steps.iter().find(|step| step.id == contract.from_step) else {
            return Err(WorkflowParseError::ContractOutputUnknownStep {
                output: output.clone(),
                step: contract.from_step.clone(),
            });
        };
        if let Some(step_contract) = &step.contract {
            if step_contract.emits != contract.schema {
                return Err(WorkflowParseError::ContractStepMismatch {
                    output: output.clone(),
                    step: step.id.clone(),
                    field: "schema",
                    workflow_declared: contract.schema.clone(),
                    step_declared: step_contract.emits.clone(),
                });
            }
            if step_contract.format != contract.format {
                return Err(WorkflowParseError::ContractStepMismatch {
                    output: output.clone(),
                    step: step.id.clone(),
                    field: "format",
                    workflow_declared: contract.format.as_str().to_string(),
                    step_declared: step_contract.format.as_str().to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Known fields on `StepOutput` (see `templates::StepOutput`).
/// Used by [`validate_template_refs`] to flag typos like
/// `{{ steps.x.findngs }}` at parse time.
const STEP_OUTPUT_FIELDS: &[&str] = &[
    "output",
    "success",
    "skipped",
    "results",
    "sub_results",
    "findings",
    "max_severity",
    "iterations",
    "resolved",
];

/// Walk every templated string in the workflow and validate
/// `steps.<id>.<field>` references against the actual step graph.
/// Catches:
///   - References to step ids that don't exist anywhere in the workflow.
///   - References to step ids that come *later* in the linear order
///     (forward reference — the value isn't bound yet at render time).
///   - References to fields that aren't on `StepOutput`.
///
/// Limitations of the MVP scanner:
///   - Doesn't validate deeper paths like `steps.x.sub_results.<sub_id>`
///     beyond the first two segments. The first two suffice to catch
///     the vast majority of authoring mistakes.
///   - Doesn't see references that are computed at runtime
///     (`{{ steps[var] }}`). We accept the false negative — those
///     are rare in workflow YAML and would still fail loudly at render.
fn validate_template_refs(wf: &Workflow) -> Result<(), WorkflowParseError> {
    // Linear order of step ids — every reference must point at a
    // step earlier in this list.
    let step_order: Vec<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
    for (idx, step) in wf.steps.iter().enumerate() {
        let prior: BTreeSet<&str> = step_order[..idx].iter().copied().collect();
        // Top-level prompt / when / for_each / panel.subject.
        for (kind, src) in collect_templates_for_step(step) {
            for (referenced, field) in scan_step_refs(&src) {
                if !prior.contains(referenced.as_str()) {
                    // Distinguish "doesn't exist anywhere" from "forward
                    // reference" so the error message is actionable.
                    let exists_later = wf.steps.iter().any(|s| s.id == referenced);
                    if exists_later {
                        return Err(WorkflowParseError::TemplateForwardStepRef {
                            step: step.id.clone(),
                            template_kind: kind,
                            referenced,
                        });
                    } else {
                        return Err(WorkflowParseError::TemplateUnknownStepRef {
                            step: step.id.clone(),
                            template_kind: kind,
                            referenced,
                        });
                    }
                }
                if let Some(f) = field {
                    if !STEP_OUTPUT_FIELDS.contains(&f.as_str()) {
                        return Err(WorkflowParseError::TemplateUnknownStepField {
                            step: step.id.clone(),
                            template_kind: kind,
                            referenced_step: referenced,
                            field: f,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// Yield every templated string on a Step paired with a short
/// kind tag (`"prompt"`, `"when"`, `"for_each"`, `"panel.subject"`,
/// `"parallel.<id>.prompt"`).
fn collect_templates_for_step(step: &Step) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    if let Some(p) = &step.prompt {
        out.push(("prompt", p.clone()));
    }
    if let Some(w) = &step.when {
        out.push(("when", w.clone()));
    }
    if let Some(f) = &step.for_each {
        out.push(("for_each", f.clone()));
    }
    if let Some(panel) = &step.panel {
        out.push(("panel.subject", panel.subject.clone()));
        // Panelists are bare agent names; the agent file owns its own
        // prompt template — nothing workflow-level to lint here.
    }
    if let Some(subs) = &step.parallel {
        for sub in subs {
            out.push(("parallel.prompt", sub.prompt.clone()));
        }
    }
    out
}

/// Scan a template string for `steps.<id>(.<field>)?` references and
/// return them as `(referenced_step_id, optional_field)` tuples.
/// Both the step id and field segments must be ASCII identifier
/// characters (`[A-Za-z0-9_]`); anything else terminates the match
/// (so `steps.review-each` would yield `("review", None)` — but
/// step ids that contain hyphens can't be referenced in jinja
/// templates anyway, since `-` isn't part of a jinja identifier).
fn scan_step_refs(template: &str) -> Vec<(String, Option<String>)> {
    fn is_ident_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'_'
    }
    let bytes = template.as_bytes();
    let needle = b"steps.";
    let mut refs = Vec::new();
    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] != needle {
            i += 1;
            continue;
        }
        // Must be a word-boundary before "steps." — otherwise we'd
        // match things like `mysteps.foo`. Accept start-of-string.
        if i > 0 && is_ident_byte(bytes[i - 1]) {
            i += 1;
            continue;
        }
        let id_start = i + needle.len();
        let mut j = id_start;
        while j < bytes.len() && is_ident_byte(bytes[j]) {
            j += 1;
        }
        if j == id_start {
            // `steps.` not followed by an identifier — skip.
            i = j;
            continue;
        }
        // SAFETY: id_start..j is a contiguous ASCII identifier slice.
        let step_id = std::str::from_utf8(&bytes[id_start..j])
            .unwrap()
            .to_string();
        let field = if j < bytes.len() && bytes[j] == b'.' {
            let f_start = j + 1;
            let mut k = f_start;
            while k < bytes.len() && is_ident_byte(bytes[k]) {
                k += 1;
            }
            if k > f_start {
                let f = std::str::from_utf8(&bytes[f_start..k]).unwrap().to_string();
                j = k;
                Some(f)
            } else {
                None
            }
        } else {
            None
        };
        refs.push((step_id, field));
        i = j;
    }
    refs
}

fn validate_autoflow(wf: &Workflow) -> Result<(), WorkflowParseError> {
    let Some(autoflow) = &wf.autoflow else {
        return Ok(());
    };

    if let Some(reconcile_every) = &autoflow.reconcile_every {
        validate_duration_field("autoflow.reconcile_every", reconcile_every)?;
    }
    if let Some(claim) = &autoflow.claim {
        if let Some(ttl) = &claim.ttl {
            validate_duration_field("autoflow.claim.ttl", ttl)?;
        }
    }
    if let Some(outcome) = &autoflow.outcome {
        if !wf.contracts.outputs.contains_key(&outcome.output) {
            return Err(WorkflowParseError::AutoflowOutcomeUnknownOutput {
                output: outcome.output.clone(),
            });
        }
    }
    if autoflow.entity != AutoflowEntity::PullRequest {
        if autoflow.selector.draft.is_some() {
            return Err(WorkflowParseError::AutoflowSelectorFieldWrongEntity {
                field: "draft",
                entity: autoflow.entity.name(),
            });
        }
        if autoflow.selector.base.is_some() {
            return Err(WorkflowParseError::AutoflowSelectorFieldWrongEntity {
                field: "base",
                entity: autoflow.entity.name(),
            });
        }
    }
    Ok(())
}

fn validate_duration_field(field: &'static str, value: &str) -> Result<(), WorkflowParseError> {
    let trimmed = value.trim();
    let Some(unit) = trimmed.chars().last() else {
        return Err(WorkflowParseError::InvalidAutoflowDuration {
            field,
            value: value.to_string(),
        });
    };
    let number = &trimmed[..trimmed.len().saturating_sub(1)];
    let valid_unit = matches!(unit, 's' | 'm' | 'h' | 'd');
    let valid_number = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    if valid_unit && valid_number {
        Ok(())
    } else {
        Err(WorkflowParseError::InvalidAutoflowDuration {
            field,
            value: value.to_string(),
        })
    }
}

/// Render a YAML scalar to the same string form `--input k=v` would
/// produce, so default + user value can be compared against `enum`
/// uniformly.
pub(crate) fn yaml_scalar_to_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .into(),
    }
}

#[cfg(test)]
mod distribute_tests {
    use super::*;

    #[test]
    fn distribute_parses_on_for_each() {
        let yaml = r#"
id: scan
for_each: "{{ steps.list.results }}"
agent: scanner
distribute:
  hosts: [edge-1, edge-2]
"#;
        let step: Step = serde_yaml::from_str(yaml).unwrap();
        let d = step.distribute.expect("distribute present");
        assert_eq!(d.hosts, vec!["edge-1".to_string(), "edge-2".to_string()]);
    }

    #[test]
    fn distribute_omitted_is_none() {
        let step: Step =
            serde_yaml::from_str("id: s\nfor_each: \"x\"\nagent: a\nprompt: p\n").unwrap();
        assert!(step.distribute.is_none());
    }

    #[test]
    fn validate_rejects_distribute_without_for_each() {
        // a linear step (agent+prompt, no for_each) with distribute → validation error
        let step: Step =
            serde_yaml::from_str("id: s\nagent: a\nprompt: hi\ndistribute:\n  hosts: [h1]\n")
                .unwrap();
        let err = validate_step_shape(&step).unwrap_err();
        assert!(err.to_string().contains("distribute"));
    }

    #[test]
    fn validate_rejects_empty_hosts() {
        let step: Step = serde_yaml::from_str(
            "id: s\nfor_each: \"x\"\nagent: a\nprompt: p\ndistribute:\n  hosts: []\n",
        )
        .unwrap();
        assert!(validate_step_shape(&step).is_err());
    }
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn host_parses_on_linear_step() {
        let yaml = r#"
name: placed
steps:
  - id: build
    agent: builder
    prompt: "do it"
    host: worker-1
"#;
        let wf = Workflow::parse(yaml).expect("valid");
        assert_eq!(wf.steps[0].host.as_deref(), Some("worker-1"));
    }

    #[test]
    fn host_absent_is_none() {
        let yaml = r#"
name: local
steps:
  - id: build
    agent: builder
    prompt: "do it"
"#;
        let wf = Workflow::parse(yaml).expect("valid");
        assert_eq!(wf.steps[0].host, None);
    }

    #[test]
    fn host_round_trips_skipping_none() {
        let yaml = r#"
name: local
steps:
  - id: build
    agent: builder
    prompt: "do it"
"#;
        let wf = Workflow::parse(yaml).expect("valid");
        let out = serde_yaml::to_string(&wf).expect("serialize");
        assert!(!out.contains("host"), "None host must be skipped: {out}");
    }

    #[test]
    fn host_rejected_on_for_each() {
        let yaml = r#"
name: bad
steps:
  - id: fan
    for_each: "a\nb"
    agent: a
    prompt: "p"
    host: worker-1
"#;
        let err = Workflow::parse(yaml).expect_err("for_each + host invalid");
        assert!(matches!(
            err,
            WorkflowParseError::HostOnNonLinearStep { .. }
        ));
    }

    #[test]
    fn host_rejected_on_parallel() {
        let yaml = r#"
name: bad
steps:
  - id: par
    host: worker-1
    parallel:
      - id: s1
        agent: a
        prompt: p
"#;
        let err = Workflow::parse(yaml).expect_err("parallel + host invalid");
        assert!(matches!(
            err,
            WorkflowParseError::HostOnNonLinearStep { .. }
        ));
    }

    #[test]
    fn host_rejected_on_panel() {
        let yaml = r#"
name: bad
steps:
  - id: pan
    host: worker-1
    panel:
      panelists: [reviewer]
      subject: "{{ inputs.x }}"
"#;
        let err = Workflow::parse(yaml).expect_err("panel + host invalid");
        assert!(matches!(
            err,
            WorkflowParseError::HostOnNonLinearStep { .. }
        ));
    }

    #[test]
    fn empty_host_rejected() {
        let yaml = r#"
name: bad
steps:
  - id: build
    agent: builder
    prompt: "do it"
    host: ""
"#;
        let err = Workflow::parse(yaml).expect_err("empty host invalid");
        assert!(matches!(err, WorkflowParseError::HostEmpty { .. }));
    }

    #[test]
    fn branch_struct_parses() {
        let b: Branch = serde_yaml::from_str(
            "condition: \"{{ steps.a.output }}\"\nthen: [x, y]\nelse: [z]\n",
        )
        .unwrap();
        assert_eq!(b.condition, "{{ steps.a.output }}");
        assert_eq!(b.then, vec!["x", "y"]);
        assert_eq!(b.r#else, vec!["z"]);
    }
}

#[cfg(test)]
mod workspace_mode_tests {
    use super::*;

    #[test]
    fn workspace_parses_on_placed_step() {
        let wf = Workflow::parse(
            r#"
name: ws
steps:
  - id: build
    agent: a
    prompt: p
    host: worker-1
    workspace: sync
"#,
        )
        .unwrap();
        assert_eq!(wf.steps[0].workspace, Some(WorkspaceMode::Sync));
    }

    #[test]
    fn workspace_default_and_override_resolve() {
        let wf = Workflow::parse(
            r#"
name: ws
defaults:
  workspace: sync
steps:
  - id: a
    agent: a
    prompt: p
    host: w1
  - id: b
    agent: a
    prompt: p
    host: w2
    workspace: none
"#,
        )
        .unwrap();
        // step a inherits the default; step b overrides to none.
        assert_eq!(
            effective_workspace_mode(&wf.steps[0], &wf.defaults),
            WorkspaceMode::Sync
        );
        assert_eq!(
            effective_workspace_mode(&wf.steps[1], &wf.defaults),
            WorkspaceMode::None
        );
    }

    #[test]
    fn workspace_absent_resolves_to_none() {
        let wf = Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    agent: a
    prompt: p
    host: w1
"#,
        )
        .unwrap();
        assert_eq!(
            effective_workspace_mode(&wf.steps[0], &wf.defaults),
            WorkspaceMode::None
        );
        assert_eq!(wf.steps[0].workspace, None);
    }

    #[test]
    fn workspace_none_skipped_in_serialize() {
        let wf = Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    agent: a
    prompt: p
"#,
        )
        .unwrap();
        let out = serde_yaml::to_string(&wf).unwrap();
        assert!(!out.contains("workspace"), "None must be skipped: {out}");
    }

    #[test]
    fn workspace_sync_rejected_on_local_step() {
        let err = Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    agent: a
    prompt: p
    workspace: sync
"#,
        )
        .expect_err("sync on a local step is invalid");
        assert!(matches!(
            err,
            WorkflowParseError::WorkspaceSyncOnLocalStep { .. }
        ));
    }

    #[test]
    fn workspace_sync_allowed_on_distribute_step() {
        // distribute => remote => sync is valid.
        Workflow::parse(
            r#"
name: ws
steps:
  - id: a
    for_each: "x\ny"
    agent: a
    prompt: p
    workspace: sync
    distribute:
      hosts: [w1, w2]
"#,
        )
        .expect("sync on a distribute step is valid");
    }
}

#[cfg(test)]
mod workflow_concerns_tests {
    use super::*;

    #[test]
    fn workflow_parses_concerns_block() {
        let yaml = r#"
name: test-workflow
description: coverage test
concerns:
  - include: stride
steps:
  - id: step1
    agent: ag
    actions: []
    prompt: "do the thing"
"#;
        let wf = Workflow::parse(yaml).expect("should parse");
        let block = wf.concerns.expect("concerns should be Some");
        assert_eq!(block.entries.len(), 1, "one concern entry");
    }

    #[test]
    fn workflow_without_concerns_parses_as_none() {
        let yaml = r#"
name: no-concerns
steps:
  - id: s1
    agent: ag
    actions: []
    prompt: "do it"
"#;
        let wf = Workflow::parse(yaml).expect("should parse");
        assert!(wf.concerns.is_none(), "concerns should be None by default");
    }
}

#[cfg(test)]
mod pull_request_autoflow_tests {
    use super::*;

    #[test]
    fn pull_request_entity_and_selector_parse() {
        let y = "name: x\nautoflow:\n  enabled: true\n  entity: pull_request\n  selector:\n    states: [open]\n    draft: exclude\n    base: main\n  claim:\n    key: pr_head_sha\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
        let wf = Workflow::parse(y).unwrap();
        let af = wf.autoflow.unwrap();
        assert_eq!(af.entity, AutoflowEntity::PullRequest);
        assert_eq!(af.selector.base.as_deref(), Some("main"));
        assert_eq!(af.selector.draft, Some(DraftFilter::Exclude));
        assert_eq!(af.claim.unwrap().key, AutoflowClaimKey::PrHeadSha);
    }

    #[test]
    fn draft_filter_on_issue_entity_is_rejected() {
        let y = "name: x\nautoflow:\n  enabled: true\n  entity: issue\n  selector:\n    draft: exclude\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
        assert!(Workflow::parse(y).is_err());
    }

    #[test]
    fn base_on_issue_entity_is_rejected() {
        let y = "name: x\nautoflow:\n  enabled: true\n  entity: issue\n  selector:\n    base: main\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
        assert!(Workflow::parse(y).is_err());
    }

    #[test]
    fn issue_autoflow_without_pr_fields_still_parses() {
        let y = "name: x\nautoflow:\n  enabled: true\n  entity: issue\n  selector:\n    states: [open]\n    labels_all: [triaged]\n  claim:\n    key: issue\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
        let wf = Workflow::parse(y).unwrap();
        let af = wf.autoflow.unwrap();
        assert_eq!(af.entity, AutoflowEntity::Issue);
        assert!(af.selector.draft.is_none());
        assert!(af.selector.base.is_none());
        assert_eq!(af.claim.unwrap().key, AutoflowClaimKey::Issue);
    }
}

#[cfg(test)]
mod author_allowlist_tests {
    use super::*;

    fn selector_with(authors: Vec<&str>, authors_from: Option<AuthorScope>) -> AutoflowSelector {
        AutoflowSelector {
            authors: authors.into_iter().map(String::from).collect(),
            authors_from,
            ..Default::default()
        }
    }

    #[test]
    fn explicit_author_in_list_is_allowed() {
        let sel = selector_with(vec!["alice", "bob"], None);
        assert!(author_allowed(&sel, "alice", false));
    }

    #[test]
    fn authors_from_collaborators_allows_collaborator() {
        let sel = selector_with(vec![], Some(AuthorScope::Collaborators));
        assert!(author_allowed(&sel, "carol", true));
    }

    #[test]
    fn authors_from_collaborators_denies_non_collaborator_not_in_list() {
        let sel = selector_with(vec![], Some(AuthorScope::Collaborators));
        assert!(!author_allowed(&sel, "mallory", false));
    }

    #[test]
    fn no_authors_and_no_authors_from_allows_everyone() {
        let sel = selector_with(vec![], None);
        assert!(author_allowed(&sel, "anyone", false));
        assert!(author_allowed(&sel, "anyone", true));
    }

    #[test]
    fn authors_set_but_no_match_and_no_authors_from_is_denied() {
        let sel = selector_with(vec!["alice", "bob"], None);
        assert!(!author_allowed(&sel, "mallory", false));
        // Even a collaborator not on the explicit list is denied, since
        // authors_from wasn't specified to broaden the check.
        assert!(!author_allowed(&sel, "mallory", true));
    }

    #[test]
    fn authors_from_org_members_allows_collaborator() {
        let sel = selector_with(vec![], Some(AuthorScope::OrgMembers));
        assert!(author_allowed(&sel, "dave", true));
    }

    #[test]
    fn selector_with_author_fields_parses() {
        let y = "name: x\nautoflow:\n  enabled: true\n  entity: issue\n  selector:\n    authors: [alice, bob]\n    authors_from: collaborators\n  claim:\n    key: issue\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
        let wf = Workflow::parse(y).unwrap();
        let af = wf.autoflow.unwrap();
        assert_eq!(
            af.selector.authors,
            vec!["alice".to_string(), "bob".to_string()]
        );
        assert_eq!(af.selector.authors_from, Some(AuthorScope::Collaborators));
    }

    #[test]
    fn selector_without_author_fields_still_parses_backward_compat() {
        let y = "name: x\nautoflow:\n  enabled: true\n  entity: issue\n  selector:\n    states: [open]\n  claim:\n    key: issue\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
        let wf = Workflow::parse(y).unwrap();
        let af = wf.autoflow.unwrap();
        assert!(af.selector.authors.is_empty());
        assert!(af.selector.authors_from.is_none());
        assert!(af.selector.on_skip.is_none());
    }

    #[test]
    fn on_skip_label_needs_human_parses() {
        let y = "name: x\nautoflow:\n  enabled: true\n  entity: issue\n  selector:\n    on_skip: label_needs_human\n  claim:\n    key: issue\nsteps:\n  - id: s1\n    agent: a\n    prompt: p\n";
        let wf = Workflow::parse(y).unwrap();
        let af = wf.autoflow.unwrap();
        assert_eq!(af.selector.on_skip, Some(SkipAction::LabelNeedsHuman));
    }
}
