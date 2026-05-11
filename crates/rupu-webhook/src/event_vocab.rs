//! Mapping from raw vendor webhook (event name + payload action)
//! to a stable rupu event identifier matched against the workflow's
//! `trigger.event:` field.
//!
//! The vocabulary is dotted: `<vendor>.<noun>.<verb>`. Examples:
//!
//!   github.pr.opened
//!   github.pr.merged
//!   github.pr.closed
//!   github.pr.review_requested
//!   github.issue.opened
//!   github.issue.closed
//!   github.issue.commented
//!   github.push
//!   gitlab.mr.opened
//!   gitlab.mr.merged
//!   gitlab.issue.opened
//!   gitlab.push
//!   linear.issue.updated
//!   jira.issue.updated
//!
//! Workflow-side glob matching *is* supported by the orchestrator.
//! This module only maps raw vendor deliveries onto canonical rupu
//! event ids; broader semantic aliases are derived one layer up.

use serde_json::{json, Value};

/// Map a GitHub webhook delivery to a rupu event id. `event_header`
/// is `X-GitHub-Event` (e.g. `pull_request`, `issues`, `push`); the
/// payload's `action` field disambiguates within the noun.
///
/// Returns `None` for events we don't yet recognize — the receiver
/// answers 200 OK but doesn't fire any workflow.
pub fn map_github_event(event_header: &str, payload: &Value) -> Option<String> {
    let action = payload.get("action").and_then(|v| v.as_str());
    match (event_header, action) {
        ("pull_request", Some("opened")) => Some("github.pr.opened".into()),
        ("pull_request", Some("reopened")) => Some("github.pr.reopened".into()),
        ("pull_request", Some("edited")) => Some("github.pr.edited".into()),
        ("pull_request", Some("closed")) => {
            // GitHub fires `closed` for both merged and merely-closed
            // PRs; differentiate by checking `pull_request.merged`.
            let merged = payload
                .get("pull_request")
                .and_then(|p| p.get("merged"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Some(if merged {
                "github.pr.merged".into()
            } else {
                "github.pr.closed".into()
            })
        }
        ("pull_request", Some("ready_for_review")) => Some("github.pr.ready_for_review".into()),
        ("pull_request", Some("labeled")) => Some("github.pr.labeled".into()),
        ("pull_request", Some("unlabeled")) => Some("github.pr.unlabeled".into()),
        ("pull_request", Some("assigned")) => Some("github.pr.assigned".into()),
        ("pull_request", Some("unassigned")) => Some("github.pr.unassigned".into()),
        ("pull_request", Some("review_requested")) => Some("github.pr.review_requested".into()),
        ("pull_request", Some("synchronize")) => Some("github.pr.updated".into()),
        ("pull_request_review", Some("submitted")) => Some("github.pr.review_submitted".into()),
        ("issues", Some("opened")) => Some("github.issue.opened".into()),
        ("issues", Some("closed")) => Some("github.issue.closed".into()),
        ("issues", Some("reopened")) => Some("github.issue.reopened".into()),
        ("issues", Some("edited")) => Some("github.issue.edited".into()),
        ("issues", Some("labeled")) => Some("github.issue.labeled".into()),
        ("issues", Some("unlabeled")) => Some("github.issue.unlabeled".into()),
        ("issues", Some("assigned")) => Some("github.issue.assigned".into()),
        ("issues", Some("unassigned")) => Some("github.issue.unassigned".into()),
        ("issues", Some("milestoned")) => Some("github.issue.milestoned".into()),
        ("issues", Some("demilestoned")) => Some("github.issue.demilestoned".into()),
        ("issue_comment", Some("created")) => Some("github.issue.commented".into()),
        ("issue_comment", Some("edited")) => Some("github.issue.comment_edited".into()),
        ("push", _) => Some("github.push".into()),
        ("ping", _) => Some("github.ping".into()),
        _ => None,
    }
}

/// Map a GitLab webhook delivery to a rupu event id. GitLab uses
/// `X-Gitlab-Event` (e.g. `Merge Request Hook`) AND the payload's
/// `object_attributes.action` to disambiguate.
pub fn map_gitlab_event(event_header: &str, payload: &Value) -> Option<String> {
    let action = payload
        .get("object_attributes")
        .and_then(|o| o.get("action"))
        .and_then(|v| v.as_str());
    match (event_header, action) {
        ("Merge Request Hook", Some("open")) => Some("gitlab.mr.opened".into()),
        ("Merge Request Hook", Some("reopen")) => Some("gitlab.mr.reopened".into()),
        ("Merge Request Hook", Some("close")) => Some("gitlab.mr.closed".into()),
        ("Merge Request Hook", Some("merge")) => Some("gitlab.mr.merged".into()),
        ("Merge Request Hook", Some("update")) => Some("gitlab.mr.updated".into()),
        ("Issue Hook", Some("open")) => Some("gitlab.issue.opened".into()),
        ("Issue Hook", Some("close")) => Some("gitlab.issue.closed".into()),
        ("Issue Hook", Some("reopen")) => Some("gitlab.issue.reopened".into()),
        ("Issue Hook", Some("update")) => Some("gitlab.issue.updated".into()),
        ("Note Hook", _) => Some("gitlab.comment".into()),
        ("Push Hook", _) => Some("gitlab.push".into()),
        _ => None,
    }
}

/// Map a Linear webhook delivery to a rupu event id. Linear uses the
/// `Linear-Event` header (resource type such as `Issue`) plus the
/// payload's `action` field (`create` / `update` / `remove`).
///
/// Native workflow-state transitions are exposed through
/// `linear.issue.updated` plus a normalized payload shape in the
/// server layer. One delivery can therefore match multiple derived
/// aliases such as `issue.state_changed` and `issue.cycle_changed`.
pub fn map_linear_event(event_header: &str, payload: &Value) -> Option<String> {
    let action = payload.get("action").and_then(|value| value.as_str());
    let type_field = payload.get("type").and_then(|value| value.as_str());
    let effective_type = if !event_header.is_empty() {
        event_header
    } else {
        type_field?
    };
    match (effective_type, action) {
        ("Issue", Some("create")) => Some("linear.issue.opened".into()),
        ("Issue", Some("update")) => Some("linear.issue.updated".into()),
        ("Issue", Some("remove")) => Some("linear.issue.removed".into()),
        _ => None,
    }
}

/// Map a Jira webhook delivery to a rupu event id. Jira Cloud issue
/// webhooks are changelog-shaped: `jira:issue_updated` carries the
/// field transitions, while `jira:issue_created` / `jira:issue_deleted`
/// describe lifecycle edges.
pub fn map_jira_event(event_header: &str, _payload: &Value) -> Option<String> {
    match event_header {
        "jira:issue_created" => Some("jira.issue.opened".into()),
        "jira:issue_updated" => Some("jira.issue.updated".into()),
        "jira:issue_deleted" => Some("jira.issue.deleted".into()),
        _ => None,
    }
}

/// Normalize a Linear Issue webhook payload into the shape expected by
/// the orchestrator's native tracker state alias layer. Raw vendor
/// payload is preserved at `payload`.
pub fn normalize_linear_event_payload(payload: &Value) -> Value {
    let data = payload.get("data").and_then(Value::as_object);
    let updated_from = payload.get("updatedFrom").and_then(Value::as_object);

    let subject_ref = data
        .and_then(|data| data.get("identifier"))
        .and_then(Value::as_str)
        .or_else(|| data.and_then(|data| data.get("id")).and_then(Value::as_str))
        .unwrap_or_default();
    let issue_id = data
        .and_then(|data| data.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let state = transition_object(
        updated_from.and_then(|obj| obj.get("stateId")),
        data.and_then(|obj| obj.get("stateId")),
        Some("workflow_state"),
    );
    let project = transition_object(
        updated_from.and_then(|obj| obj.get("projectId")),
        data.and_then(|obj| obj.get("projectId")),
        None,
    );
    let cycle = transition_object(
        updated_from.and_then(|obj| obj.get("cycleId")),
        data.and_then(|obj| obj.get("cycleId")),
        None,
    );
    let priority = transition_object(
        updated_from.and_then(|obj| obj.get("priority")),
        data.and_then(|obj| obj.get("priority")),
        None,
    );

    let mut out = json!({
        "vendor": "linear",
        "delivery": payload
            .get("webhookId")
            .or_else(|| payload.get("deliveryId"))
            .cloned()
            .unwrap_or(Value::Null),
        "subject": {
            "kind": "issue",
            "id": issue_id,
            "ref": subject_ref,
            "url": payload.get("url").cloned().unwrap_or(Value::Null),
        },
        "actor": payload.get("actor").cloned().unwrap_or(Value::Null),
        "organization": {
            "id": payload.get("organizationId").cloned().unwrap_or(Value::Null),
        },
        "team": {
            "id": data.and_then(|obj| obj.get("teamId")).cloned().unwrap_or(Value::Null),
            "key": subject_ref.split_once('-').map(|(key, _)| key).unwrap_or_default(),
        },
        "payload": payload.clone(),
    });

    if let Some(state) = state {
        out["state"] = state;
    }
    if let Some(project) = project {
        out["project"] = project;
    }
    if let Some(cycle) = cycle {
        out["cycle"] = cycle;
    }
    if let Some(priority) = priority {
        out["priority"] = priority;
    }
    if let Some(blocked) = updated_from
        .and_then(|obj| obj.get("blockedByCount"))
        .or_else(|| updated_from.and_then(|obj| obj.get("blocked")))
    {
        let current = data
            .and_then(|obj| obj.get("blockedByCount"))
            .or_else(|| data.and_then(|obj| obj.get("blocked")))
            .cloned()
            .unwrap_or(Value::Null);
        let blocked_after = blocked_flag(&current);
        let blocked_before = blocked_flag(blocked);
        if blocked_after != blocked_before {
            out["blocked"] = Value::Bool(blocked_after);
        }
    }

    out
}

/// Normalize a Jira issue webhook payload into the shape expected by
/// the orchestrator's native tracker state alias layer. Field-level
/// transitions are read from `changelog.items`; raw vendor payload is
/// preserved at `payload`.
pub fn normalize_jira_event_payload(payload: &Value) -> Value {
    let issue = payload.get("issue").and_then(Value::as_object);
    let fields = issue
        .and_then(|issue| issue.get("fields"))
        .and_then(Value::as_object);

    let issue_id = issue
        .and_then(|issue| issue.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let issue_ref = issue
        .and_then(|issue| issue.get("key"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let issue_url = issue
        .and_then(|issue| issue.get("self"))
        .cloned()
        .unwrap_or(Value::Null);

    let current_project = fields.and_then(|fields| fields.get("project"));

    let mut out = json!({
        "vendor": "jira",
        "subject": {
            "kind": "issue",
            "id": issue_id,
            "ref": issue_ref,
            "url": issue_url,
        },
        "actor": payload.get("user").cloned().unwrap_or(Value::Null),
        "tenant": {
            "base_url": payload.get("baseUrl").cloned().unwrap_or(Value::Null),
        },
        "context": {
            "project": current_project.cloned().unwrap_or(Value::Null),
            "issue_type": fields.and_then(|fields| fields.get("issuetype")).cloned().unwrap_or(Value::Null),
        },
        "payload": payload.clone(),
    });

    if let Some(state) = jira_transition_from_changelog(payload, JiraChangeKind::Status) {
        out["state"] = state;
    }
    if let Some(project) = jira_transition_from_changelog(payload, JiraChangeKind::Project) {
        out["project"] = project;
    }
    if let Some(sprint) = jira_transition_from_changelog(payload, JiraChangeKind::Sprint) {
        out["sprint"] = sprint;
    }
    if let Some(priority) = jira_transition_from_changelog(payload, JiraChangeKind::Priority) {
        out["priority"] = priority;
    }

    out
}

fn transition_object(
    before: Option<&Value>,
    after: Option<&Value>,
    category: Option<&str>,
) -> Option<Value> {
    let before_norm = stateish_value(before)?;
    let after_norm = stateish_value(after)?;
    if before_norm == after_norm {
        return None;
    }
    let mut value = json!({
        "before": before_norm,
        "after": after_norm,
    });
    if let Some(category) = category {
        value["category"] = Value::String(category.to_string());
    }
    Some(value)
}

fn stateish_value(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    match value {
        Value::Null => Some(Value::Null),
        Value::String(text) => Some(json!({ "id": text })),
        Value::Number(number) => Some(json!({ "id": number })),
        Value::Object(map) => {
            if map.contains_key("id") || map.contains_key("name") {
                Some(Value::Object(map.clone()))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn blocked_flag(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_i64().unwrap_or_default() > 0,
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum JiraChangeKind {
    Status,
    Project,
    Sprint,
    Priority,
}

fn jira_transition_from_changelog(payload: &Value, kind: JiraChangeKind) -> Option<Value> {
    let item = payload
        .get("changelog")
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array)?
        .iter()
        .find(|item| jira_change_item_matches(item, kind))?;
    let before = jira_change_endpoint(item.get("from"), item.get("fromString"));
    let after = jira_change_endpoint(item.get("to"), item.get("toString"));
    if before == after {
        return None;
    }

    let mut transition = json!({
        "before": before,
        "after": after,
    });
    if let Some(category) = jira_change_category(kind) {
        transition["category"] = Value::String(category.to_string());
    }
    Some(transition)
}

fn jira_change_item_matches(item: &Value, kind: JiraChangeKind) -> bool {
    let field = item
        .get("field")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let field_id = item
        .get("fieldId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();

    match kind {
        JiraChangeKind::Status => field == "status" || field_id == "status",
        JiraChangeKind::Project => field == "project" || field_id == "project",
        JiraChangeKind::Sprint => field == "sprint" || field_id == "sprint",
        JiraChangeKind::Priority => field == "priority" || field_id == "priority",
    }
}

fn jira_change_category(kind: JiraChangeKind) -> Option<&'static str> {
    match kind {
        JiraChangeKind::Status => Some("workflow_state"),
        JiraChangeKind::Project => Some("project"),
        JiraChangeKind::Sprint => Some("sprint"),
        JiraChangeKind::Priority => None,
    }
}

fn jira_change_endpoint(id: Option<&Value>, text: Option<&Value>) -> Value {
    let mut object = serde_json::Map::new();
    if let Some(id) = id.and_then(value_as_scalar_string) {
        object.insert("id".into(), Value::String(id));
    }
    if let Some(text) = text.and_then(Value::as_str) {
        if let Some(parsed) = parse_jira_named_transition(text) {
            for (key, value) in parsed {
                object.insert(key, value);
            }
        } else if !text.trim().is_empty() {
            object.insert("name".into(), Value::String(text.to_string()));
        }
    }
    Value::Object(object)
}

fn value_as_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn parse_jira_named_transition(text: &str) -> Option<Vec<(String, Value)>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bracketed = trimmed
        .split_once('[')
        .and_then(|(_, rest)| rest.rsplit_once(']'))
        .map(|(inside, _)| inside)
        .unwrap_or(trimmed);
    if !bracketed.contains('=') {
        return None;
    }
    let mut out = Vec::new();
    for token in bracketed.split(',') {
        let (raw_key, raw_value) = token.split_once('=')?;
        let key = raw_key.trim();
        let value = raw_value.trim();
        if key.eq_ignore_ascii_case("id") && !value.is_empty() {
            out.push(("id".into(), Value::String(value.to_string())));
        } else if key.eq_ignore_ascii_case("name") && !value.is_empty() {
            out.push(("name".into(), Value::String(value.to_string())));
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_pr_opened() {
        let payload = json!({ "action": "opened", "pull_request": { "number": 1 } });
        assert_eq!(
            map_github_event("pull_request", &payload),
            Some("github.pr.opened".into())
        );
    }

    #[test]
    fn github_pr_closed_distinguishes_merged_vs_closed() {
        let merged = json!({ "action": "closed", "pull_request": { "merged": true } });
        let closed = json!({ "action": "closed", "pull_request": { "merged": false } });
        assert_eq!(
            map_github_event("pull_request", &merged),
            Some("github.pr.merged".into())
        );
        assert_eq!(
            map_github_event("pull_request", &closed),
            Some("github.pr.closed".into())
        );
    }

    #[test]
    fn github_issue_events() {
        for (action, expected) in [
            ("opened", "github.issue.opened"),
            ("closed", "github.issue.closed"),
            ("labeled", "github.issue.labeled"),
            ("unlabeled", "github.issue.unlabeled"),
            ("assigned", "github.issue.assigned"),
            ("unassigned", "github.issue.unassigned"),
            ("milestoned", "github.issue.milestoned"),
            ("demilestoned", "github.issue.demilestoned"),
        ] {
            let payload = json!({ "action": action });
            assert_eq!(
                map_github_event("issues", &payload),
                Some(expected.into()),
                "for action={action}"
            );
        }
    }

    #[test]
    fn github_unknown_event_is_none() {
        let payload = json!({ "action": "speculated" });
        assert!(map_github_event("never_heard_of_it", &payload).is_none());
    }

    #[test]
    fn github_push_does_not_require_action() {
        let payload = json!({ "ref": "refs/heads/main" });
        assert_eq!(
            map_github_event("push", &payload),
            Some("github.push".into())
        );
    }

    #[test]
    fn github_pr_queue_and_review_events() {
        for (action, expected) in [
            ("edited", "github.pr.edited"),
            ("ready_for_review", "github.pr.ready_for_review"),
            ("labeled", "github.pr.labeled"),
            ("unlabeled", "github.pr.unlabeled"),
            ("assigned", "github.pr.assigned"),
            ("unassigned", "github.pr.unassigned"),
            ("review_requested", "github.pr.review_requested"),
        ] {
            let payload = json!({ "action": action });
            assert_eq!(
                map_github_event("pull_request", &payload),
                Some(expected.into()),
                "for action={action}"
            );
        }
        let payload = json!({ "action": "edited" });
        assert_eq!(
            map_github_event("issue_comment", &payload),
            Some("github.issue.comment_edited".into())
        );
    }

    #[test]
    fn gitlab_mr_opened() {
        let payload = json!({ "object_attributes": { "action": "open" } });
        assert_eq!(
            map_gitlab_event("Merge Request Hook", &payload),
            Some("gitlab.mr.opened".into())
        );
    }

    #[test]
    fn gitlab_unknown_event() {
        let payload = json!({});
        assert!(map_gitlab_event("Magic Hook", &payload).is_none());
    }

    #[test]
    fn linear_issue_events() {
        for (action, expected) in [
            ("create", "linear.issue.opened"),
            ("update", "linear.issue.updated"),
            ("remove", "linear.issue.removed"),
        ] {
            let payload = json!({ "type": "Issue", "action": action });
            assert_eq!(
                map_linear_event("Issue", &payload),
                Some(expected.into()),
                "for action={action}"
            );
        }
    }

    #[test]
    fn linear_unknown_event_is_none() {
        let payload = json!({ "type": "Project", "action": "update" });
        assert!(map_linear_event("Project", &payload).is_none());
    }

    #[test]
    fn jira_issue_events() {
        assert_eq!(
            map_jira_event("jira:issue_created", &json!({})),
            Some("jira.issue.opened".into())
        );
        assert_eq!(
            map_jira_event("jira:issue_updated", &json!({})),
            Some("jira.issue.updated".into())
        );
        assert_eq!(
            map_jira_event("jira:issue_deleted", &json!({})),
            Some("jira.issue.deleted".into())
        );
    }

    #[test]
    fn linear_issue_update_normalizes_transition_fields() {
        let payload = json!({
            "action": "update",
            "type": "Issue",
            "url": "https://linear.app/acme/issue/ENG-123",
            "organizationId": "org-1",
            "actor": { "id": "user-1", "name": "Matt" },
            "data": {
                "id": "issue-1",
                "identifier": "ENG-123",
                "stateId": "state-in-progress",
                "projectId": "project-core",
                "cycleId": "cycle-42",
                "priority": 1,
                "teamId": "team-1",
                "blockedByCount": 2
            },
            "updatedFrom": {
                "stateId": "state-todo",
                "projectId": "project-backlog",
                "cycleId": "cycle-41",
                "priority": 3,
                "blockedByCount": 0
            },
            "webhookId": "delivery-1"
        });
        let normalized = normalize_linear_event_payload(&payload);
        assert_eq!(normalized["subject"]["ref"], "ENG-123");
        assert_eq!(normalized["state"]["category"], "workflow_state");
        assert_eq!(normalized["state"]["before"]["id"], "state-todo");
        assert_eq!(normalized["state"]["after"]["id"], "state-in-progress");
        assert_eq!(normalized["project"]["before"]["id"], "project-backlog");
        assert_eq!(normalized["cycle"]["after"]["id"], "cycle-42");
        assert_eq!(normalized["priority"]["after"]["id"], 1);
        assert_eq!(normalized["blocked"], true);
        assert_eq!(normalized["payload"]["data"]["identifier"], "ENG-123");
    }

    #[test]
    fn jira_issue_update_normalizes_changelog_transitions() {
        let payload = json!({
            "timestamp": 1731430163000u64,
            "webhookEvent": "jira:issue_updated",
            "user": { "accountId": "user-1", "displayName": "Matt" },
            "issue": {
                "id": "10001",
                "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
                "key": "ENG-123",
                "fields": {
                    "project": { "id": "10000", "key": "ENG", "name": "Engineering" },
                    "issuetype": { "id": "10004", "name": "Task" }
                }
            },
            "changelog": {
                "items": [
                    {
                        "field": "status",
                        "fieldId": "status",
                        "from": "3",
                        "fromString": "To Do",
                        "to": "4",
                        "toString": "In Progress"
                    },
                    {
                        "field": "Sprint",
                        "from": "41",
                        "fromString": "com.atlassian.greenhopper.service.sprint.Sprint[id=41,name=Sprint 41]",
                        "to": "42",
                        "toString": "com.atlassian.greenhopper.service.sprint.Sprint[id=42,name=Sprint 42]"
                    },
                    {
                        "field": "priority",
                        "fieldId": "priority",
                        "from": "2",
                        "fromString": "Medium",
                        "to": "1",
                        "toString": "High"
                    }
                ]
            }
        });

        let normalized = normalize_jira_event_payload(&payload);
        assert_eq!(normalized["subject"]["ref"], "ENG-123");
        assert_eq!(normalized["state"]["category"], "workflow_state");
        assert_eq!(normalized["state"]["before"]["id"], "3");
        assert_eq!(normalized["state"]["after"]["name"], "In Progress");
        assert_eq!(normalized["sprint"]["before"]["name"], "Sprint 41");
        assert_eq!(normalized["sprint"]["after"]["id"], "42");
        assert_eq!(normalized["priority"]["after"]["name"], "High");
        assert_eq!(normalized["payload"]["issue"]["key"], "ENG-123");
    }
}
