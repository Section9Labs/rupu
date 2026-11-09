use super::*;
use crate::state::AppState;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt;

const RAW: &str = "name: draft\nlead: lead\nengagement_profiles: [code]\nscope: {authorized: true}\npool: {agents: [lead], workflows: all}\ngoals:\n  - id: goal\n    objective: '  preserve this text  '\n    target: {findings: {}, count_gte: 18446744073709551615}\nbudget: {tokens: 9007199254740993, rounds: 3, soft_at: 0.5}\nround: {lead_max_turns: 4}\nmessaging: false\n";

fn definition() -> AgentiflowDef {
    AgentiflowDef::parse_str(RAW).unwrap()
}
fn fixture() -> (tempfile::TempDir, AppState) {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(
        tmp.path().join("global"),
        rupu_config::PricingConfig::default(),
    );
    (tmp, state)
}
async fn post(state: AppState, path: &str, body: Value) -> (StatusCode, Value) {
    let response = super::super::routes()
        .with_state(state)
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), MAX_RESPONSE + 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn complete_echo_preserves_integer_precision_defaults_and_untrimmed_values() {
    let value = response(&definition(), RAW, Some("ws_exact")).unwrap();
    assert_eq!(
        value["definition"]["goals"][0]["target"]["count_gte"].as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        value["definition"]["budget"]["tokens"].as_u64(),
        Some(9_007_199_254_740_993)
    );
    assert_eq!(
        value["definition"]["goals"][0]["objective"],
        "  preserve this text  "
    );
    assert_eq!(value["definition"]["goals"][0]["required"], true);
    assert_eq!(value["definition"]["messaging"], false);
    assert_eq!(value["definition"]["pool"]["workflows"], "all");
    assert_eq!(value["definition"]["round"]["lead_max_turns"], 4);
    assert_eq!(value["scope_id"], "ws_exact");
    assert_eq!(value["definition_schema"], "agentiflow.def.v1");
    assert_eq!(
        value["source_sha256"],
        format!("{:x}", Sha256::digest(RAW.as_bytes()))
    );
    // Semantic defaults are typed, not raw source or an effective run projection.
    assert_eq!(
        value["definition"],
        serde_json::to_value(definition()).unwrap()
    );
    for bits in [0x3fd5_5555_5555_5555_u64, 0x3fef_ffff_ffff_fffd_u64] {
        let mut def = definition();
        def.budget.as_mut().unwrap().soft_at = Some(f64::from_bits(bits));
        let echo = response(&def, RAW, None).unwrap();
        assert_eq!(
            echo["definition"]["budget"]["soft_at"]
                .as_f64()
                .unwrap()
                .to_bits(),
            bits
        );
    }
}

#[test]
fn consecutive_source_edits_return_their_own_whole_definition_and_hash() {
    let second = RAW
        .replace("rounds: 3", "rounds: 9")
        .replace("workflows: all", "workflows: []");
    let third = second.replace("preserve this text", "next objective");
    let first = response(&definition(), RAW, None).unwrap();
    let second_echo = response(&AgentiflowDef::parse_str(&second).unwrap(), &second, None).unwrap();
    let third_echo = response(&AgentiflowDef::parse_str(&third).unwrap(), &third, None).unwrap();
    assert_eq!(first["definition"]["budget"]["rounds"], 3);
    assert_eq!(second_echo["definition"]["budget"]["rounds"], 9);
    assert_eq!(third_echo["definition"]["pool"]["workflows"], json!([]));
    assert_eq!(
        third_echo["definition"]["goals"][0]["target"],
        first["definition"]["goals"][0]["target"]
    );
    assert_ne!(first["source_sha256"], second_echo["source_sha256"]);
    assert_ne!(second_echo["source_sha256"], third_echo["source_sha256"]);
    assert!(third_echo["scope_id"].is_null());
}

#[test]
fn yaml_only_values_and_nonfinite_numbers_are_never_silently_coerced() {
    for opaque in [
        "!custom value",
        "{1: value}",
        ".nan",
        ".inf",
        "{nested: [-.inf]}",
    ] {
        let mut def = definition();
        def.scope.roots.push(rupu_agentiflow::ScopeRoot {
            kind: "code:repo".into(),
            fields: [("opaque".into(), serde_yaml::from_str(opaque).unwrap())].into(),
        });
        assert!(
            matches!(response(&def, RAW, None), Err(ValidationError::Unavailable)),
            "{opaque}"
        );
    }
    let mut def = definition();
    def.budget.as_mut().unwrap().usd = Some(f64::INFINITY);
    assert!(matches!(
        response(&def, RAW, None),
        Err(ValidationError::Unavailable)
    ));
    def.budget.as_mut().unwrap().usd = None;
    def.description = Some("${NOT_EXPANDED}".into());
    assert_eq!(
        response(&def, RAW, None).unwrap()["definition"]["description"],
        "${NOT_EXPANDED}"
    );
}

#[test]
fn source_budget_counts_utf8_bytes_and_writer_never_retains_an_oversized_reply() {
    assert!(check_source(&"é".repeat(MAX_SOURCE / 2)).is_ok());
    assert!(matches!(
        check_source(&"é".repeat(MAX_SOURCE / 2 + 1)),
        Err(ValidationError::SourceTooLarge)
    ));
    let mut writer = BoundedBytes::default();
    writer.write_all(&vec![b'x'; MAX_RESPONSE]).unwrap();
    assert!(writer.write_all(b"x").is_err());
    assert_eq!(writer.0.len(), MAX_RESPONSE);
    let mut def = definition();
    def.description = Some("\u{1}".repeat(MAX_RESPONSE / 6));
    assert!(matches!(
        response(&def, RAW, None),
        Err(ValidationError::Unavailable)
    ));
}

#[test]
fn excessive_normalized_tree_depth_and_node_counts_refuse_complete_echo() {
    let mut def = definition();
    def.pool.agents = vec!["lead".into(); MAX_NODES];
    assert!(matches!(
        response(&def, RAW, None),
        Err(ValidationError::Unavailable)
    ));
    let mut def = definition();
    let mut nested = serde_yaml::Value::Null;
    for _ in 0..MAX_DEPTH {
        nested = serde_yaml::Value::Sequence(vec![nested]);
    }
    def.scope.roots.push(rupu_agentiflow::ScopeRoot {
        kind: "code:repo".into(),
        fields: [("nested".into(), nested)].into(),
    });
    // Full definition nesting includes the path through scope/roots/field.
    assert!(matches!(
        response(&def, RAW, None),
        Err(ValidationError::Unavailable)
    ));
}

#[tokio::test]
async fn legacy_validation_stays_exact_and_opt_in_is_read_only() {
    let (_tmp, state) = fixture();
    let dir = state.global_dir.join("agentiflows");
    std::fs::create_dir_all(&dir).unwrap();
    let saved = dir.join("draft.yaml");
    std::fs::write(&saved, "untouched source").unwrap();
    let path = "/api/agentiflow-defs/validate";
    let (status, legacy) = post(state.clone(), path, json!({"raw":RAW})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        legacy,
        json!({"ok":true,"name":"draft","lead":"lead","engagement_profiles":["code"]})
    );
    let (status, echo) = post(state, path, json!({"raw":RAW,"include_definition":true})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        echo["definition"]["budget"]["tokens"].as_u64(),
        Some(9_007_199_254_740_993)
    );
    assert!(echo["scope_id"].is_null());
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "untouched source");
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
}

#[tokio::test]
async fn scoped_echo_keeps_registered_project_profile_and_local_host_checks() {
    let (tmp, state) = fixture();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let workspace = rupu_workspace::register(
        &rupu_workspace::WorkspaceStore {
            root: state.global_dir.join("workspaces"),
        },
        &project,
    )
    .unwrap();
    let path = "/api/agentiflow-defs/validate";
    let (status, value) = post(
        state.clone(),
        path,
        json!({"raw":RAW,"scope_id":workspace.id,"include_definition":true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["scope_id"], workspace.id);
    let (status, _) = post(
        state.clone(),
        path,
        json!({"raw":RAW,"scope_id":"ws_missing","include_definition":true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = post(
        state.clone(),
        "/api/agentiflow-defs/validate?host=elsewhere",
        json!({"raw":RAW,"include_definition":true}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    std::fs::create_dir_all(project.join(".rupu/profiles")).unwrap();
    std::fs::write(project.join(".rupu/profiles/broken.toml"), "not [toml").unwrap();
    let (status, _) = post(
        state,
        path,
        json!({"raw":RAW,"scope_id":workspace.id,"include_definition":true}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn opt_in_failures_are_explicit_and_do_not_change_legacy_success() {
    let (_tmp, state) = fixture();
    let path = "/api/agentiflow-defs/validate";
    let nonfinite = RAW.replace("tokens: 9007199254740993", "usd: .inf");
    assert_eq!(
        post(state.clone(), path, json!({"raw":nonfinite})).await.0,
        StatusCode::OK
    );
    let (status, value) = post(
        state.clone(),
        path,
        json!({"raw":nonfinite,"include_definition":true}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(value["code"], "definition_echo_unavailable");
    let (status, value) = post(
        state,
        path,
        json!({"raw":" ".repeat(MAX_SOURCE+1),"include_definition":true}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(value["code"], "definition_source_too_large");
}
