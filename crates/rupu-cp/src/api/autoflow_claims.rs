//! Autoflow claim control endpoints.
//!
//! Pure-state reads/mutations over the library stores:
//! - `GET  /api/autoflows/claims`         — list tracked autoflow claims.
//! - `POST /api/autoflows/claims/release` — delete a claim (release the issue).
//! - `POST /api/autoflows/claims/requeue` — enqueue a manual wake for an issue.
//!
//! Issue refs embed `/` and `:` (e.g. `github:Section9Labs/rupu/issues/42`),
//! so they travel in the JSON body rather than a path segment.

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{extract::State, routing::post, Json, Router};
use chrono::{DateTime, Utc};
use rupu_runtime::{
    WakeEnqueueRequest, WakeEntity, WakeEntityKind, WakeEvent, WakeSource, WakeStore,
};
use rupu_workspace::{AutoflowClaimRecord, AutoflowClaimStore};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/autoflows/claims", axum::routing::get(list_claims))
        .route("/api/autoflows/claims/release", post(release_claim))
        .route("/api/autoflows/claims/requeue", post(requeue_claim))
}

/// Build the claim store rooted at `<global>/autoflows/claims`.
fn claim_store(s: &AppState) -> AutoflowClaimStore {
    AutoflowClaimStore {
        root: s.global_dir.join("autoflows").join("claims"),
    }
}

/// Build the wake store rooted at `<global>/autoflows/wakes`.
fn wake_store(s: &AppState) -> WakeStore {
    WakeStore::new(s.global_dir.join("autoflows").join("wakes"))
}

/// Slim wire DTO for an autoflow claim. `status` is the lowercase
/// `snake_case` form of [`ClaimStatus`] (e.g. `"await_human"`) so the
/// frontend gets a stable string.
#[derive(Serialize)]
pub(crate) struct ClaimRow {
    pub(crate) issue_ref: String,
    pub(crate) issue_display_ref: Option<String>,
    pub(crate) repo_ref: String,
    pub(crate) issue_title: Option<String>,
    pub(crate) issue_url: Option<String>,
    pub(crate) workflow: String,
    pub(crate) status: String,
    pub(crate) last_run_id: Option<String>,
    pub(crate) last_error: Option<String>,
    pub(crate) last_summary: Option<String>,
    pub(crate) pr_url: Option<String>,
    pub(crate) claim_owner: Option<String>,
    pub(crate) lease_expires_at: Option<String>,
    pub(crate) updated_at: String,
}

impl From<AutoflowClaimRecord> for ClaimRow {
    fn from(r: AutoflowClaimRecord) -> Self {
        // `ClaimStatus` serializes as a snake_case string; round-trip through
        // serde_json to get that lowercase form without a hand-written match.
        let status = serde_json::to_value(r.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        Self {
            issue_ref: r.issue_ref,
            issue_display_ref: r.issue_display_ref,
            repo_ref: r.repo_ref,
            issue_title: r.issue_title,
            issue_url: r.issue_url,
            workflow: r.workflow,
            status,
            last_run_id: r.last_run_id,
            last_error: r.last_error,
            last_summary: r.last_summary,
            pr_url: r.pr_url,
            claim_owner: r.claim_owner,
            lease_expires_at: r.lease_expires_at,
            updated_at: r.updated_at,
        }
    }
}

/// `GET /api/autoflows/claims` — list tracked autoflow claims as [`ClaimRow`]s.
async fn list_claims(State(s): State<AppState>) -> ApiResult<Json<Vec<ClaimRow>>> {
    let rows = claim_store(&s)
        .list()
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .map(ClaimRow::from)
        .collect();
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct ReleaseBody {
    issue_ref: String,
}

/// `POST /api/autoflows/claims/release` — delete the claim for `issue_ref`.
///
/// Idempotent: releasing an untracked issue returns `200` with
/// `{ "released": false }`.
async fn release_claim(
    State(s): State<AppState>,
    Json(body): Json<ReleaseBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let released = claim_store(&s)
        .delete(&body.issue_ref)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "released": released })))
}

#[derive(Deserialize)]
struct RequeueBody {
    issue_ref: String,
    #[serde(default)]
    not_before: Option<String>,
}

/// Build the manual requeue wake request for an issue.
///
/// Mirrors the CLI's `enqueue_issue_wake`: `source: Manual`, the issue as the
/// [`WakeEntity`], and a fixed `event.id`. `received_at` is always `now`;
/// `not_before` defaults to `now` unless a deferral instant is supplied.
fn build_manual_wake(
    claim_repo_ref: &str,
    issue_ref: &str,
    now: DateTime<Utc>,
    not_before: Option<DateTime<Utc>>,
) -> WakeEnqueueRequest {
    WakeEnqueueRequest {
        source: WakeSource::Manual,
        repo_ref: claim_repo_ref.to_string(),
        entity: WakeEntity {
            kind: WakeEntityKind::Issue,
            ref_text: issue_ref.to_string(),
        },
        event: WakeEvent {
            id: "autoflow.manual.requeue".to_string(),
            delivery_id: None,
            dedupe_key: None,
        },
        payload: None,
        received_at: now.to_rfc3339(),
        not_before: not_before.unwrap_or(now).to_rfc3339(),
    }
}

/// `POST /api/autoflows/claims/requeue` — enqueue a manual wake for the issue
/// behind `issue_ref`, reusing the claim's `repo_ref`.
///
/// `404` if no claim is tracked for the ref. An optional `not_before` defers
/// the wake when it parses as RFC 3339; an unparseable value falls back to now.
async fn requeue_claim(
    State(s): State<AppState>,
    Json(body): Json<RequeueBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let claim = claim_store(&s)
        .load(&body.issue_ref)
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("no claim for {}", body.issue_ref)))?;

    let not_before = body.not_before.as_deref().and_then(|raw| {
        DateTime::parse_from_rfc3339(raw)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    });
    let req = build_manual_wake(&claim.repo_ref, &body.issue_ref, Utc::now(), not_before);
    let rec = wake_store(&s)
        .enqueue(req)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "wake_id": rec.wake_id })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_workspace::ClaimStatus;

    fn seed_record(issue_ref: &str) -> AutoflowClaimRecord {
        AutoflowClaimRecord {
            issue_ref: issue_ref.into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: Some("42".into()),
            issue_title: Some("finish autoflow".into()),
            issue_url: Some("https://github.com/Section9Labs/rupu/issues/42".into()),
            issue_state_name: Some("open".into()),
            issue_tracker: Some("github".into()),
            workflow: "issue-supervisor-dispatch".into(),
            status: ClaimStatus::AwaitHuman,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_123".into()),
            last_error: None,
            last_summary: Some("phase 1 ready".into()),
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: Some("host:user:pid".into()),
            lease_expires_at: Some("2026-05-08T23:00:00Z".into()),
            pending_dispatch: None,
            contenders: vec![],
            updated_at: "2026-05-08T20:00:00Z".into(),
        }
    }

    #[test]
    fn build_manual_wake_shapes_request() {
        let now = Utc::now();
        let req = build_manual_wake(
            "github:Section9Labs/rupu",
            "github:Section9Labs/rupu/issues/42",
            now,
            None,
        );
        assert_eq!(req.source, WakeSource::Manual);
        assert_eq!(req.repo_ref, "github:Section9Labs/rupu");
        assert_eq!(req.entity.kind, WakeEntityKind::Issue);
        assert_eq!(req.entity.ref_text, "github:Section9Labs/rupu/issues/42");
        assert_eq!(req.event.id, "autoflow.manual.requeue");
        assert!(req.event.delivery_id.is_none());
        assert!(req.event.dedupe_key.is_none());
        // not_before defaults to received_at (now) when no deferral given.
        assert_eq!(req.not_before, req.received_at);
    }

    #[test]
    fn build_manual_wake_honors_defer() {
        let now = Utc::now();
        let later = now + chrono::Duration::minutes(5);
        let req = build_manual_wake("repo", "issue", now, Some(later));
        assert_eq!(req.received_at, now.to_rfc3339());
        assert_eq!(req.not_before, later.to_rfc3339());
    }

    #[test]
    fn claim_row_status_is_lowercase_snake() {
        let row = ClaimRow::from(seed_record("github:Section9Labs/rupu/issues/42"));
        assert_eq!(row.status, "await_human");
        assert_eq!(row.issue_ref, "github:Section9Labs/rupu/issues/42");
        assert_eq!(row.repo_ref, "github:Section9Labs/rupu");
    }

    #[test]
    fn claim_store_list_delete_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("autoflows").join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        store.save(&seed_record(issue_ref)).unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].issue_ref, issue_ref);

        assert!(store.delete(issue_ref).unwrap());
        assert!(store.list().unwrap().is_empty());

        // Releasing an absent ref reports `false` (idempotent).
        assert!(!store.delete("github:Section9Labs/rupu/issues/999").unwrap());
    }

    #[test]
    fn requeue_enqueues_manual_wake() {
        let tmp = tempfile::tempdir().unwrap();
        let issue_ref = "github:Section9Labs/rupu/issues/42";

        let claims = AutoflowClaimStore {
            root: tmp.path().join("autoflows").join("claims"),
        };
        claims.save(&seed_record(issue_ref)).unwrap();
        let claim = claims.load(issue_ref).unwrap().unwrap();

        let wakes = WakeStore::new(tmp.path().join("autoflows").join("wakes"));
        let req = build_manual_wake(&claim.repo_ref, issue_ref, Utc::now(), None);
        wakes.enqueue(req).unwrap();

        let queued = wakes.list_queued().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].entity.ref_text, issue_ref);
        assert_eq!(queued[0].source, WakeSource::Manual);
        assert_eq!(queued[0].repo_ref, "github:Section9Labs/rupu");
    }
}
