//! `EventConnector` — polled-events tier for workflow triggers.
//!
//! Each tick of `rupu cron tick` calls `poll_events` on the registered
//! connectors for every repo configured in `[triggers].poll_sources`.
//! Returns `(events, next_cursor)`; the cursor is opaque to rupu and
//! persisted as-is for the next call.
//!
//! Implementations live next to the existing `RepoConnector` /
//! `IssueConnector` impls under `connectors/<platform>/events.rs`.
//! See `docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md`,
//! §10 for the contract.
//!
//! Spec invariants:
//! - `next_cursor` is always returned, even when `events` is empty
//!   (304-style "no change" path advances the cursor's internal etag
//!   without emitting events).
//! - `events` is oldest-first so the caller can stable-sort by arrival.
//! - `delivery` is unique within the connector's namespace and is what
//!   the orchestrator hashes into the deterministic run-id for
//!   idempotent dispatch.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ScmError;
use crate::types::RepoRef;

/// Result of one `poll_events` call.
#[derive(Debug, Clone)]
pub struct EventPollResult {
    /// New events since the input cursor, oldest-first.
    pub events: Vec<PolledEvent>,
    /// Opaque cursor to persist; pass back on the next call.
    pub next_cursor: String,
}

/// One event lifted from a vendor's events feed and mapped onto the
/// rupu event vocabulary.
#[derive(Debug, Clone)]
pub struct PolledEvent {
    /// Stable rupu event id, e.g. `github.issue.opened`. Matched
    /// against the workflow's `trigger.event:` field.
    pub id: String,
    /// Vendor-side unique id for this delivery. Used by the dispatcher
    /// to derive a deterministic run-id for idempotent fires.
    pub delivery: String,
    /// Repo this event came from. Forms part of the `{{event.repo.*}}`
    /// template binding.
    pub repo: RepoRef,
    /// Vendor's raw payload, passed through unmodified. Templates can
    /// reach inside via `{{event.payload.*}}`.
    pub payload: Value,
}

#[async_trait]
pub trait EventConnector: Send + Sync {
    /// Return events for `repo` strictly newer than `cursor`, oldest-
    /// first. `limit` caps the returned count to honor rate-limit
    /// budgets; on overflow the cursor advances to the last-emitted
    /// event so the next call resumes correctly.
    ///
    /// On first call (`cursor: None`) implementations MUST return zero
    /// events and a fresh cursor pointing to "now" — emitting the last
    /// 90 days of activity on warmup would cause a workflow stampede.
    /// This matches the documented behavior in §15 of the spec.
    async fn poll_events(
        &self,
        repo: &RepoRef,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventPollResult, ScmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _assert_object_safe() {
        let _: Option<std::sync::Arc<dyn EventConnector>> = None;
    }
}
