use super::*;
use crate::transcript_mutator::{ExactTranscriptEffect, TranscriptAction, TranscriptMutator};
use std::sync::{Arc, Mutex};
struct Adapter {
    calls: Arc<Mutex<Vec<String>>>,
    wrong: bool,
}
fn ready(id: &str, scope: &ExactTranscriptScope) -> ExactTranscriptContext {
    ExactTranscriptContext {
        schema: EXACT_DELETE_FEATURE.into(),
        id: id.into(),
        workspace_id: scope.workspace_id.clone(),
        host_id: "local".into(),
        path: scope.path.clone(),
        state: "ready".into(),
        reason: None,
        source_kind: Some("global_active".into()),
        archived: Some(false),
        revision: Some(format!("sha256:{}", "a".repeat(64))),
        observed_at: "2026-11-09T10:00:00Z".into(),
        effects: vec![
            ExactTranscriptEffect {
                kind: "transcript".into(),
                path: scope.path.clone(),
                bytes: "100".into(),
            },
            ExactTranscriptEffect {
                kind: "metadata".into(),
                path: std::path::Path::new(&scope.path)
                    .with_file_name(format!("{id}.meta.json"))
                    .to_string_lossy()
                    .into(),
                bytes: "200".into(),
            },
        ],
        coordination: "advisory_retention_lease_metadata_revision".into(),
        limits: ExactTranscriptLimits::default(),
    }
}
#[async_trait::async_trait]
impl TranscriptMutator for Adapter {
    fn supports_exact_delete(&self) -> bool {
        true
    }
    async fn mutate(
        &self,
        _: &str,
        _: TranscriptAction,
        _: bool,
    ) -> Result<(), TranscriptMutateError> {
        self.calls.lock().unwrap().push("legacy".into());
        Ok(())
    }
    async fn exact_delete_context(
        &self,
        id: &str,
        scope: &ExactTranscriptScope,
    ) -> Result<ExactTranscriptContext, TranscriptMutateError> {
        self.calls.lock().unwrap().push("context".into());
        let mut c = ready(id, scope);
        if self.wrong {
            c.path = "/elsewhere/run_exact.jsonl".into();
        }
        Ok(c)
    }
    async fn delete_exact(
        &self,
        id: &str,
        scope: &ExactTranscriptScope,
        revision: &str,
    ) -> Result<(), TranscriptMutateError> {
        self.calls.lock().unwrap().push(format!(
            "exact:{id}:{}:{}:{revision}",
            scope.workspace_id, scope.path
        ));
        Ok(())
    }
}
fn fixture(wrong: bool) -> (tempfile::TempDir, AppState, Arc<Mutex<Vec<String>>>) {
    let t = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let s = AppState::new(t.path().into(), Default::default()).with_transcript_mutator(Some(
        Arc::new(Adapter {
            calls: calls.clone(),
            wrong,
        }),
    ));
    (t, s, calls)
}
fn query() -> TranscriptHostQuery {
    TranscriptHostQuery {
        host: Some("local".into()),
        ignore_liveness: false,
        ws_id: Some("ws_exact".into()),
        expected_path: Some("/fixture/run_exact.jsonl".into()),
        expected_revision: Some(format!("sha256:{}", "a".repeat(64))),
    }
}
#[tokio::test]
async fn exact_delete_dispatches_only_complete_scope_to_new_port() {
    let (_t, s, calls) = fixture(false);
    let q = query();
    let response = super::super::delete_transcript(State(s), Path("run_exact".into()), Query(q))
        .await
        .unwrap();
    assert_eq!(response.0["path"], "/fixture/run_exact.jsonl");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("exact:run_exact:ws_exact:/fixture/run_exact.jsonl:"));
}
#[tokio::test]
async fn partial_scope_and_liveness_override_never_fall_back_to_legacy_delete() {
    let (_t, s, calls) = fixture(false);
    let mut missing = query();
    missing.expected_revision = None;
    let mut override_pid = query();
    override_pid.ignore_liveness = true;
    let mut wrong_host = query();
    wrong_host.host = Some("remote".into());
    let mut path = query();
    path.expected_path = Some("/fixture/another.jsonl".into());
    for q in [missing, override_pid, wrong_host, path] {
        assert!(super::super::delete_transcript(
            State(s.clone()),
            Path("run_exact".into()),
            Query(q)
        )
        .await
        .is_err());
    }
    assert!(calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn legacy_delete_and_archive_remain_legacy_but_archive_rejects_exact_claim() {
    let (_t, s, calls) = fixture(false);
    let _ = super::super::delete_transcript(
        State(s.clone()),
        Path("run_exact".into()),
        Query(TranscriptHostQuery::default()),
    )
    .await
    .unwrap();
    assert!(
        super::super::archive_transcript(State(s), Path("run_exact".into()), Query(query()))
            .await
            .is_err()
    );
    assert_eq!(*calls.lock().unwrap(), vec!["legacy"]);
}
#[tokio::test]
async fn context_requires_supported_adapter_and_validates_returned_identity() {
    let (_t, s, _) = fixture(false);
    assert!(available(&s));
    let q = ContextQuery {
        host: None,
        ws_id: "ws_exact".into(),
        path: "/fixture/run_exact.jsonl".into(),
    };
    assert_eq!(
        context(State(s), Path("run_exact".into()), Query(q))
            .await
            .unwrap()
            .0
            .state,
        "ready"
    );
    let (_t, s, _) = fixture(true);
    let q = ContextQuery {
        host: None,
        ws_id: "ws_exact".into(),
        path: "/fixture/run_exact.jsonl".into(),
    };
    assert!(context(State(s), Path("run_exact".into()), Query(q))
        .await
        .is_err());
    let t = tempfile::tempdir().unwrap();
    let s = AppState::new(t.path().into(), Default::default());
    assert!(!available(&s));
    assert!(adapter(&s).is_err());
}
#[test]
fn context_rejects_wrong_effects_and_unavailable_revision() {
    let scope = ExactTranscriptScope {
        workspace_id: "ws_exact".into(),
        path: "/fixture/run_exact.jsonl".into(),
    };
    for kind in [
        "global_active",
        "global_archive",
        "project_active",
        "project_archive",
    ] {
        let mut c = ready("run_exact", &scope);
        c.source_kind = Some(kind.into());
        c.archived = Some(!kind.ends_with("_archive"));
        assert!(check_context(&c, "run_exact", &scope).is_err());
        c.archived = Some(kind.ends_with("_archive"));
        assert!(check_context(&c, "run_exact", &scope).is_ok());
    }
    let mut c = ready("run_exact", &scope);
    c.effects[1].path = "/other/run_exact.meta.json".into();
    assert!(check_context(&c, "run_exact", &scope).is_err());
    let mut c = ready("run_exact", &scope);
    c.state = "unavailable".into();
    c.reason = Some("unknown".into());
    c.effects.clear();
    assert!(check_context(&c, "run_exact", &scope).is_err());
    c.revision = None;
    assert!(check_context(&c, "run_exact", &scope).is_ok());
}

#[tokio::test]
async fn strict_route_requires_preconditions_and_rejects_unknown_override() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let (_t, s, calls) = fixture(false);
    let base=format!("/api/transcripts/run_exact/exact-source?ws_id=ws_exact&expected_path=/fixture/run_exact.jsonl&expected_revision=sha256:{}","a".repeat(64));
    for uri in [
        "/api/transcripts/run_exact/exact-source".into(),
        format!("{base}&ignore_liveness=true"),
        format!("{base}&path=/other/run_exact.jsonl"),
    ] {
        let response = super::super::routes()
            .with_state(s.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }
    assert!(calls.lock().unwrap().is_empty());
    let response = super::super::routes()
        .with_state(s)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(base)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(calls.lock().unwrap().len(), 1);
}
