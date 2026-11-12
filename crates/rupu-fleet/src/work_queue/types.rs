//! Board work Queue v1 public contract. This module does not implement storage.
//!
//! The SQLite implementation commits an item/attempt change, event and retry
//! receipt in one transaction. DTOs below are independent of the storage schema.
//! Actors and source locations are runtime-owned, never model request fields.

use crate::{FleetError, OperatorVia};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const WORK_QUEUE_SCHEMA_VERSION: u32 = 1;
pub const MAX_TITLE_BYTES: usize = 256;
pub const MAX_INSTRUCTIONS_BYTES: usize = 16 * 1024;
pub const MAX_RESULT_BYTES: usize = 8 * 1024;
pub const DEFAULT_PAGE_LIMIT: usize = 50;
pub const MAX_PAGE_LIMIT: usize = 200;
pub const MAX_PAGE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_DATABASE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    Queued,
    Claimed,
    Working,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkAction {
    Post,
    Claim,
    Start,
    Block,
    Resume,
    Complete,
    Fail,
    Cancel,
    Requeue,
}

/// An exact recording rooted at a runtime-validated location. Owner means the
/// resolved Queue owner's directory, not its `messages` subdirectory. Project
/// recordings require the immutable workspace ID. Relative paths never escape
/// their anchor. An absent/unavailable recording does not erase its provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkRecording {
    pub anchor: WorkRecordingAnchor,
    pub relative_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkRecordingAnchor {
    Owner,
    GlobalTranscripts,
    ProjectTranscripts,
}

/// Exact execution identity includes recording/round: a FlowLead may reuse a
/// run ID in another round, but that round cannot inherit an old attempt.
/// `Operator` has no invented sending run or recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkActor {
    Operator {
        via: OperatorVia,
    },
    Agent {
        address: String,
        run_id: String,
        /// Minted per assembled execution and recorded with its Bus join.
        execution_id: String,
        agent: String,
        recording: WorkRecording,
    },
}

/// Invocation provenance is kept from the first successful commit and excluded
/// from the idempotency digest. All fields are runtime supplied; provider tool
/// call IDs alone are not unique. Agent mutations require this entire record;
/// operator mutations use None, without synthetic transcript coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkSource {
    pub turn_index: u32,
    pub call_sequence: u64,
    pub tool_call_id: String,
}

/// Trusted in-process context, deliberately not Deserialize. Callers construct
/// it from the assembled launch and current tool invocation, not JSON input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkContext {
    pub actor: WorkActor,
    pub source: Option<WorkSource>,
}

/// Post compares the Queue revision; every other operation compares the item
/// revision. Changed requests never reuse a request ID. Runtime source indices
/// do not belong in this input shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkCommand {
    pub request_id: String,
    /// None only when posting into an explicitly uninitialized queue.
    pub expected_queue_id: Option<String>,
    pub expected_revision: u64,
    pub operation: WorkOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkOperation {
    Post {
        title: String,
        instructions: String,
    },
    Claim {
        work_id: String,
    },
    Start {
        work_id: String,
        work_attempt_id: String,
    },
    Block {
        work_id: String,
        work_attempt_id: String,
        reason: String,
    },
    Resume {
        work_id: String,
        work_attempt_id: String,
    },
    Complete {
        work_id: String,
        work_attempt_id: String,
        summary: String,
    },
    Fail {
        work_id: String,
        work_attempt_id: String,
        summary: String,
    },
    Cancel {
        work_id: String,
        reason: String,
    },
    Requeue {
        work_id: String,
        reason: String,
    },
}

/// Full current item, without unbounded historical attempts or events. Earlier
/// attempts and results remain available through the paged immutable timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItem {
    pub work_id: String,
    pub creation_sequence: u64,
    pub revision: u64,
    pub title: String,
    pub instructions: String,
    pub creator: WorkActor,
    pub posted_at: String,
    pub updated_at: String,
    pub state: WorkState,
    /// Most recent attempt, including after requeue or terminal closure.
    /// Presence is not a claim that execution remains active.
    pub latest_attempt: Option<WorkAttempt>,
    /// Bounded current result/reason with the exact actor and source retained.
    pub latest_event: WorkEvent,
    /// Conservative warning retained after cancel/requeue of unfinished work.
    /// A new claim does not prove that an earlier execution stopped.
    pub prior_attempt_may_still_run: bool,
    pub attempt_count: u64,
    pub event_count: u64,
}

/// Compact list row. No instructions, result text or full historical records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemSummary {
    pub work_id: String,
    pub creation_sequence: u64,
    pub revision: u64,
    pub title: String,
    pub creator: WorkActor,
    pub posted_at: String,
    pub updated_at: String,
    pub state: WorkState,
    pub work_attempt_id: Option<String>,
    pub claimant: Option<WorkActor>,
    pub executor: Option<WorkActor>,
    pub attempt_count: u64,
}

/// Claimant and executor identities are independently captured once. Executor
/// is absent until explicit Start; v1 then requires the claimant execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkAttempt {
    pub work_attempt_id: String,
    pub work_id: String,
    pub number: u64,
    pub claimant: WorkActor,
    pub claimed_at: String,
    pub executor: Option<WorkActor>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub closure: Option<WorkAttemptClosure>,
    pub claim_event_id: String,
    pub start_event_id: Option<String>,
    pub terminal_event_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkAttemptClosure {
    Completed,
    Failed,
    Cancelled,
    Requeued,
}

/// Canonical Queue source reference. A receipt proves this event committed,
/// not that the transcript writer subsequently recorded the tool response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkEvent {
    pub event_id: String,
    pub sequence: u64,
    pub work_id: String,
    pub item_revision: u64,
    pub queue_revision: u64,
    pub work_attempt_id: Option<String>,
    pub action: WorkAction,
    pub at: String,
    pub actor: WorkActor,
    pub source: Option<WorkSource>,
    pub from_state: Option<WorkState>,
    pub to_state: WorkState,
    /// Nonempty block/cancel/requeue reason or complete/fail result. Post text
    /// stays in the immutable item title/instructions instead of being copied.
    pub summary: Option<String>,
    pub prior_attempt_may_still_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkExecutionEffect {
    None,
}

/// Immutable original acknowledgement. Historical applied revisions remain
/// unchanged on replay; WorkReply supplies the current item separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkReceipt {
    pub request_id: String,
    pub queue_id: String,
    pub work_id: String,
    pub item_revision: u64,
    pub queue_revision: u64,
    pub event_id: String,
    pub work_attempt_id: Option<String>,
    pub committed_at: String,
    pub execution_effect: WorkExecutionEffect,
    pub prior_attempt_may_still_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkReply {
    pub receipt: WorkReceipt,
    pub replayed: bool,
    pub current_item: WorkItem,
    pub queue_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkAvailability {
    NotInitialized,
    Available,
}

/// Read metadata only, not a serialization of the complete database. There is
/// no v1 lifetime item/transaction cap or routine full-store scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSnapshot {
    pub schema_version: u32,
    pub availability: WorkAvailability,
    pub queue_id: Option<String>,
    pub revision: u64,
    pub capacity: WorkCapacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkCapacity {
    pub item_count: u64,
    pub event_count: u64,
    pub receipt_count: u64,
    pub database_bytes: u64,
    pub database_max_bytes: u64,
    pub admission: WorkAdmissionStatus,
    pub reserved_bytes: u64,
    pub remaining_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkAdmissionStatus {
    Accepting,
    CapacityLimited,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkFilters {
    pub state: Option<WorkState>,
    pub claimant_run_id: Option<String>,
}

/// CP authenticates its opaque cursor and maps it to these already-bound store
/// preconditions. A store caller must not silently ignore a revision mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkPageQuery {
    pub filters: WorkFilters,
    pub limit: usize,
    pub cursor: Option<WorkPagePosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPagePosition {
    pub queue_id: String,
    pub source_revision: u64,
    pub filters: WorkFilters,
    pub before_creation_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkTimelineQuery {
    pub work_id: String,
    pub limit: usize,
    pub cursor: Option<WorkTimelinePosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkTimelinePosition {
    pub queue_id: String,
    pub work_id: String,
    pub source_item_revision: u64,
    pub after_event_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkCountScope {
    MatchingFiltersAtSourceRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkCounts {
    pub scope: WorkCountScope,
    pub returned: u64,
    pub matching_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkOrder {
    CreationSequenceDescending,
    EventSequenceAscending,
}

/// `next` is a typed internal cursor payload, not a trustworthy client token.
/// CP signs it together with endpoint and exact owner identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPage {
    pub snapshot: WorkSnapshot,
    pub filters: WorkFilters,
    pub order: WorkOrder,
    pub items: Vec<WorkItemSummary>,
    pub counts: WorkCounts,
    pub next: Option<WorkPagePosition>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkTimelinePage {
    pub queue_id: String,
    pub work_id: String,
    pub source_item_revision: u64,
    pub order: WorkOrder,
    pub events: Vec<WorkEvent>,
    pub counts: WorkCounts,
    pub next: Option<WorkTimelinePosition>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkCapacityResource {
    TitleBytes,
    InstructionsBytes,
    ResultBytes,
    PageBytes,
    DatabaseBytes,
}

/// Machine-readable transport mapping belongs at the CP/tool boundary. Unknown
/// persistence outcomes retain the original request ID for exact reconciliation.
#[derive(Debug, Error)]
pub enum WorkError {
    #[error("invalid work Queue input: {0}")]
    InvalidInput(String),
    #[error("work Queue operation denied: {0}")]
    Denied(String),
    #[error("work item {work_id} was not found")]
    NotFound { work_id: String },
    #[error("work Queue owner changed")]
    OwnerChanged,
    #[error("work Queue is read-only: {0}")]
    ReadOnly(String),
    #[error("work revision changed: expected {expected}, current {current}")]
    RevisionConflict { expected: u64, current: u64 },
    #[error("work attempt or executor changed")]
    AttemptConflict,
    #[error("request {request_id} was already committed with another payload")]
    IdempotencyConflict { request_id: String },
    #[error("action {action:?} is invalid while work is {state:?}")]
    TransitionConflict {
        state: WorkState,
        action: WorkAction,
    },
    #[error("work Queue cursor is stale or belongs to another query")]
    CursorConflict,
    #[error("work Queue capacity exceeded for {resource:?}: limit {limit}, requested {requested}")]
    Capacity {
        resource: WorkCapacityResource,
        limit: u64,
        current: u64,
        requested: u64,
    },
    #[error("work Queue writer is busy; retry the same request")]
    Busy,
    #[error("unsupported work Queue schema {version}")]
    UnsupportedSchema { version: u32 },
    #[error("work Queue storage is corrupt: {0}")]
    Corrupt(String),
    #[error("work Queue persistence failed before commit: {0}")]
    Persistence(String),
    #[error("work request {request_id} requires reconciliation: {message}")]
    Reconciliation { request_id: String, message: String },
    #[error(transparent)]
    Fleet(#[from] FleetError),
}
