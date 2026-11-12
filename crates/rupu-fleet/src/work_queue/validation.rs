use super::types::*;
use std::path::{Component, Path};

// JSON can expand each accepted text byte to six bytes. A full item contains
// 96 KiB instructions, 48 KiB result, bounded actor/source copies and attempts;
// 256 KiB safely bounds one record. Reserve twice that for terminal item/event/
// attempt/receipt/index growth, rather than assuming raw input equals disk use.
pub(crate) const MAX_RECORD_BYTES: usize = 256 * 1024;
pub(crate) const FINAL_RESERVE_BYTES: u64 = 512 * 1024;

pub(crate) fn identifier(value: &str) -> Result<(), WorkError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
    {
        return Err(WorkError::InvalidInput(
            "invalid bounded Queue identifier".into(),
        ));
    }
    Ok(())
}

fn text(value: &str, limit: usize, resource: WorkCapacityResource) -> Result<(), WorkError> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(WorkError::InvalidInput(
            "work text must be nonempty and contain no NUL".into(),
        ));
    }
    if value.len() > limit {
        return Err(WorkError::Capacity {
            resource,
            limit: limit as u64,
            current: 0,
            requested: value.len() as u64,
        });
    }
    Ok(())
}

pub(crate) fn validate_context(context: &WorkContext) -> Result<(), WorkError> {
    match &context.actor {
        WorkActor::Operator { .. } => {
            if context.source.is_some() {
                return Err(WorkError::Denied(
                    "operator cannot submit worker provenance".into(),
                ));
            }
        }
        WorkActor::Agent {
            address,
            run_id,
            execution_id,
            agent,
            recording,
        } => {
            identifier(run_id)?;
            identifier(execution_id)?;
            if address.is_empty()
                || address.len() > 256
                || address.contains('\0')
                || agent.is_empty()
                || agent.len() > 256
                || agent.contains('\0')
            {
                return Err(WorkError::InvalidInput("invalid execution metadata".into()));
            }
            let path = Path::new(&recording.relative_path);
            if recording.relative_path.is_empty()
                || recording.relative_path.len() > 1024
                || recording.relative_path.contains('\0')
                || path.is_absolute()
                || !path.components().all(|c| matches!(c, Component::Normal(_)))
            {
                return Err(WorkError::InvalidInput(
                    "recording must have a bounded owner-relative path".into(),
                ));
            }
            match (&recording.anchor, &recording.workspace_id) {
                (WorkRecordingAnchor::ProjectTranscripts, Some(id)) => identifier(id)?,
                (WorkRecordingAnchor::ProjectTranscripts, None) => {
                    return Err(WorkError::Denied(
                        "project recording requires a workspace".into(),
                    ))
                }
                (_, Some(_)) => {
                    return Err(WorkError::InvalidInput(
                        "workspace belongs only to project recordings".into(),
                    ))
                }
                _ => (),
            }
            let source = context.source.as_ref().ok_or_else(|| {
                WorkError::Denied("agent invocation provenance is required".into())
            })?;
            if source.tool_call_id.is_empty()
                || source.tool_call_id.len() > 512
                || source.tool_call_id.contains('\0')
            {
                return Err(WorkError::InvalidInput(
                    "invalid recorded tool-call ID".into(),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_command(
    command: &WorkCommand,
    context: &WorkContext,
) -> Result<(), WorkError> {
    identifier(&command.request_id)?;
    if let Some(id) = &command.expected_queue_id {
        identifier(id)?;
    }
    if command.expected_revision > i64::MAX as u64 {
        return Err(WorkError::InvalidInput(
            "revision exceeds supported range".into(),
        ));
    }
    let operator = matches!(context.actor, WorkActor::Operator { .. });
    if (operator
        && !matches!(
            command.operation,
            WorkOperation::Post { .. }
                | WorkOperation::Cancel { .. }
                | WorkOperation::Requeue { .. }
        ))
        || (!operator
            && matches!(
                command.operation,
                WorkOperation::Cancel { .. } | WorkOperation::Requeue { .. }
            ))
    {
        return Err(WorkError::Denied(
            "action is not allowed for this Queue actor".into(),
        ));
    }
    if !matches!(command.operation, WorkOperation::Post { .. })
        && command.expected_queue_id.is_none()
    {
        return Err(WorkError::InvalidInput(
            "existing work requires expected_queue_id".into(),
        ));
    }
    if let Some(id) = command.operation.work_id() {
        identifier(id)?;
    }
    if let Some(id) = command.operation.attempt_id() {
        identifier(id)?;
    }
    match &command.operation {
        WorkOperation::Post {
            title,
            instructions,
        } => {
            text(title, MAX_TITLE_BYTES, WorkCapacityResource::TitleBytes)?;
            text(
                instructions,
                MAX_INSTRUCTIONS_BYTES,
                WorkCapacityResource::InstructionsBytes,
            )?;
        }
        WorkOperation::Block { reason, .. }
        | WorkOperation::Cancel { reason, .. }
        | WorkOperation::Requeue { reason, .. } => {
            text(reason, MAX_RESULT_BYTES, WorkCapacityResource::ResultBytes)?
        }
        WorkOperation::Complete { summary, .. } | WorkOperation::Fail { summary, .. } => {
            text(summary, MAX_RESULT_BYTES, WorkCapacityResource::ResultBytes)?
        }
        _ => (),
    }
    Ok(())
}

impl WorkState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl WorkOperation {
    pub fn action(&self) -> WorkAction {
        match self {
            Self::Post { .. } => WorkAction::Post,
            Self::Claim { .. } => WorkAction::Claim,
            Self::Start { .. } => WorkAction::Start,
            Self::Block { .. } => WorkAction::Block,
            Self::Resume { .. } => WorkAction::Resume,
            Self::Complete { .. } => WorkAction::Complete,
            Self::Fail { .. } => WorkAction::Fail,
            Self::Cancel { .. } => WorkAction::Cancel,
            Self::Requeue { .. } => WorkAction::Requeue,
        }
    }
    pub fn work_id(&self) -> Option<&str> {
        match self {
            Self::Post { .. } => None,
            Self::Claim { work_id }
            | Self::Start { work_id, .. }
            | Self::Block { work_id, .. }
            | Self::Resume { work_id, .. }
            | Self::Complete { work_id, .. }
            | Self::Fail { work_id, .. }
            | Self::Cancel { work_id, .. }
            | Self::Requeue { work_id, .. } => Some(work_id),
        }
    }
    pub(crate) fn attempt_id(&self) -> Option<&str> {
        match self {
            Self::Start {
                work_attempt_id, ..
            }
            | Self::Block {
                work_attempt_id, ..
            }
            | Self::Resume {
                work_attempt_id, ..
            }
            | Self::Complete {
                work_attempt_id, ..
            }
            | Self::Fail {
                work_attempt_id, ..
            } => Some(work_attempt_id),
            _ => None,
        }
    }
    pub(crate) fn summary(&self) -> Option<&str> {
        match self {
            Self::Block { reason, .. }
            | Self::Cancel { reason, .. }
            | Self::Requeue { reason, .. } => Some(reason),
            Self::Complete { summary, .. } | Self::Fail { summary, .. } => Some(summary),
            _ => None,
        }
    }
}

pub(crate) fn checked_increment(value: u64) -> Result<u64, WorkError> {
    value
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .ok_or_else(|| WorkError::Corrupt("Queue sequence is exhausted".into()))
}

pub(crate) fn summary(item: &WorkItem) -> WorkItemSummary {
    WorkItemSummary {
        work_id: item.work_id.clone(),
        creation_sequence: item.creation_sequence,
        revision: item.revision,
        title: item.title.clone(),
        creator: item.creator.clone(),
        posted_at: item.posted_at.clone(),
        updated_at: item.updated_at.clone(),
        state: item.state,
        work_attempt_id: item
            .latest_attempt
            .as_ref()
            .map(|a| a.work_attempt_id.clone()),
        claimant: item.latest_attempt.as_ref().map(|a| a.claimant.clone()),
        executor: item
            .latest_attempt
            .as_ref()
            .and_then(|a| a.executor.clone()),
        attempt_count: item.attempt_count,
        prior_attempt_may_still_run: item.prior_attempt_may_still_run,
    }
}

pub(crate) fn claimant(item: &WorkItem) -> Option<&str> {
    match &item.latest_attempt.as_ref()?.claimant {
        WorkActor::Agent { run_id, .. } => Some(run_id),
        _ => None,
    }
}
