use rupu_agent::ActionEnvelope;
use rupu_orchestrator::validate_actions;
use serde_json::json;

#[test]
fn action_allowed_when_kind_in_list() {
    let action = ActionEnvelope {
        kind: "open_pr".into(),
        payload: json!({}),
    };
    let res = validate_actions(&action, &["open_pr".into(), "comment".into()]);
    assert!(res.allowed);
    assert!(res.reason.is_none());
}

#[test]
fn action_denied_when_kind_not_in_list() {
    let action = ActionEnvelope {
        kind: "delete_branch".into(),
        payload: json!({}),
    };
    let res = validate_actions(&action, &["open_pr".into()]);
    assert!(!res.allowed);
    assert_eq!(res.reason.as_deref(), Some("not in step allowlist"));
}

#[test]
fn empty_allowlist_denies_all() {
    let action = ActionEnvelope {
        kind: "anything".into(),
        payload: json!({}),
    };
    let res = validate_actions(&action, &[]);
    assert!(!res.allowed);
}
