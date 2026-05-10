//! Derived semantic event vocabulary layered on top of canonical
//! vendor event ids.
//!
//! Connectors and webhook receivers continue to emit canonical ids
//! like `github.issue.labeled`. This module derives broader semantic
//! aliases such as `github.issue.queue_entered` and `issue.queue_entered`
//! so workflow authors can target higher-level orchestration patterns
//! without hardcoding every raw vendor action.

use serde_json::{json, Value};

use crate::event_match::event_matches;

pub fn derived_event_ids(canonical_id: &str, payload: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match canonical_id {
        id if native_issue_state_aliases(id, payload, &mut out) => {}
        "github.issue.labeled" | "github.issue.assigned" | "github.issue.milestoned" => {
            push_aliases(
                &mut out,
                &[
                    "github.issue.queue_changed",
                    "github.issue.queue_entered",
                    "issue.queue_changed",
                    "issue.queue_entered",
                ],
            );
        }
        "github.issue.unlabeled" | "github.issue.unassigned" | "github.issue.demilestoned" => {
            push_aliases(
                &mut out,
                &[
                    "github.issue.queue_changed",
                    "github.issue.queue_left",
                    "issue.queue_changed",
                    "issue.queue_left",
                ],
            );
        }
        "github.issue.edited" | "github.issue.commented" | "github.issue.comment_edited" => {
            push_aliases(&mut out, &["github.issue.activity", "issue.activity"]);
        }
        "github.pr.review_requested" => {
            push_aliases(
                &mut out,
                &[
                    "github.pr.review_activity",
                    "pr.review_activity",
                    "github.pr.queue_changed",
                    "pr.queue_changed",
                    "github.pr.queue_entered",
                    "pr.queue_entered",
                    "github.pr.activity",
                    "pr.activity",
                ],
            );
        }
        "github.pr.ready_for_review" => {
            push_aliases(
                &mut out,
                &[
                    "github.pr.queue_changed",
                    "pr.queue_changed",
                    "github.pr.queue_entered",
                    "pr.queue_entered",
                    "github.pr.activity",
                    "pr.activity",
                ],
            );
        }
        "github.pr.labeled" | "github.pr.assigned" => {
            push_aliases(
                &mut out,
                &[
                    "github.pr.queue_changed",
                    "pr.queue_changed",
                    "github.pr.queue_entered",
                    "pr.queue_entered",
                ],
            );
        }
        "github.pr.unlabeled" | "github.pr.unassigned" => {
            push_aliases(
                &mut out,
                &[
                    "github.pr.queue_changed",
                    "pr.queue_changed",
                    "github.pr.queue_left",
                    "pr.queue_left",
                ],
            );
        }
        "github.pr.review_submitted" => {
            push_aliases(
                &mut out,
                &[
                    "github.pr.review_activity",
                    "pr.review_activity",
                    "github.pr.activity",
                    "pr.activity",
                ],
            );
        }
        "github.pr.edited" | "github.pr.updated" => {
            push_aliases(&mut out, &["github.pr.activity", "pr.activity"]);
        }
        "gitlab.comment" => match gitlab_comment_target(payload) {
            Some(GitlabCommentTarget::Issue) => {
                push_aliases(
                    &mut out,
                    &[
                        "gitlab.issue.commented",
                        "gitlab.issue.activity",
                        "issue.activity",
                    ],
                );
            }
            Some(GitlabCommentTarget::MergeRequest) => {
                push_aliases(
                    &mut out,
                    &["gitlab.mr.commented", "gitlab.mr.activity", "pr.activity"],
                );
            }
            None => {}
        },
        "gitlab.issue.updated" => {
            push_aliases(&mut out, &["gitlab.issue.activity", "issue.activity"]);
            if gitlab_issue_queue_changed(payload) {
                push_aliases(
                    &mut out,
                    &["gitlab.issue.queue_changed", "issue.queue_changed"],
                );
            }
        }
        "gitlab.mr.updated" => {
            push_aliases(&mut out, &["gitlab.mr.activity", "pr.activity"]);
            if gitlab_mr_queue_changed(payload) {
                push_aliases(&mut out, &["gitlab.mr.queue_changed", "pr.queue_changed"]);
            }
        }
        _ => {}
    }
    out
}

pub fn candidate_event_ids(canonical_id: &str, payload: &Value) -> Vec<String> {
    let mut out = vec![canonical_id.to_string()];
    out.extend(derived_event_ids(canonical_id, payload));
    out
}

pub fn matching_event_id(pattern: &str, canonical_id: &str, payload: &Value) -> Option<String> {
    let candidates = candidate_event_ids(canonical_id, payload);
    matching_event_id_from_candidates(pattern, &candidates)
}

pub fn matching_event_id_from_candidates(pattern: &str, candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .find(|candidate| event_matches(pattern, candidate))
        .cloned()
}

pub fn annotate_event_payload(base_payload: &Value, canonical_id: &str, matched_id: &str) -> Value {
    let aliases = derived_event_ids(canonical_id, base_payload);
    let vendor = canonical_id.split('.').next().unwrap_or_default();
    let raw_payload = base_payload.clone();
    match raw_payload {
        Value::Object(mut map) => {
            map.insert("id".into(), Value::String(matched_id.to_string()));
            map.insert(
                "canonical_id".into(),
                Value::String(canonical_id.to_string()),
            );
            map.insert("matched_as".into(), Value::String(matched_id.to_string()));
            map.insert("aliases".into(), json!(aliases));
            map.entry("vendor")
                .or_insert_with(|| Value::String(vendor.to_string()));
            map.entry("payload").or_insert_with(|| base_payload.clone());
            Value::Object(map)
        }
        other => json!({
            "id": matched_id,
            "canonical_id": canonical_id,
            "matched_as": matched_id,
            "aliases": aliases,
            "vendor": vendor,
            "payload": other,
        }),
    }
}

fn push_aliases(into: &mut Vec<String>, aliases: &[&str]) {
    for alias in aliases {
        if !into.iter().any(|existing| existing == alias) {
            into.push((*alias).to_string());
        }
    }
}

fn native_issue_state_aliases(canonical_id: &str, payload: &Value, out: &mut Vec<String>) -> bool {
    let Some((vendor, event_kind)) = native_issue_event_kind(canonical_id) else {
        return false;
    };
    match event_kind {
        NativeIssueEventKind::StateChanged => {
            push_vendor_and_generic(out, vendor, "issue.state_changed");
            push_vendor_and_generic(out, vendor, "issue.entered_state");
            push_vendor_and_generic(out, vendor, "issue.left_state");
            if let Some(category) = payload
                .get("state")
                .and_then(|state| state.get("category"))
                .and_then(Value::as_str)
            {
                match category {
                    "workflow_state" => {
                        push_vendor_and_generic(out, vendor, "issue.workflow_state_changed");
                        push_vendor_and_generic(out, vendor, "issue.entered_workflow_state");
                        push_vendor_and_generic(out, vendor, "issue.left_workflow_state");
                    }
                    "project" => {
                        push_vendor_and_generic(out, vendor, "issue.project_changed");
                        push_vendor_and_generic(out, vendor, "issue.entered_project");
                        push_vendor_and_generic(out, vendor, "issue.left_project");
                    }
                    "cycle" => {
                        push_vendor_and_generic(out, vendor, "issue.cycle_changed");
                        push_vendor_and_generic(out, vendor, "issue.entered_cycle");
                        push_vendor_and_generic(out, vendor, "issue.left_cycle");
                    }
                    "sprint" => {
                        push_vendor_and_generic(out, vendor, "issue.sprint_changed");
                        push_vendor_and_generic(out, vendor, "issue.entered_sprint");
                        push_vendor_and_generic(out, vendor, "issue.left_sprint");
                    }
                    "priority" => {
                        push_vendor_and_generic(out, vendor, "issue.priority_changed");
                    }
                    _ => {}
                }
            }
            if let Some(before) = payload
                .get("state")
                .and_then(|state| state.get("before"))
                .and_then(event_state_name)
            {
                if let Some(before_slug) = slug_segment(before) {
                    push_vendor_and_generic_owned(
                        out,
                        vendor,
                        format!("issue.left_state.{before_slug}"),
                    );
                    if state_category_is(payload, "workflow_state") {
                        push_vendor_and_generic_owned(
                            out,
                            vendor,
                            format!("issue.left_workflow_state.{before_slug}"),
                        );
                    }
                }
            }
            if let Some(after) = payload
                .get("state")
                .and_then(|state| state.get("after"))
                .and_then(event_state_name)
            {
                if let Some(after_slug) = slug_segment(after) {
                    push_vendor_and_generic_owned(
                        out,
                        vendor,
                        format!("issue.entered_state.{after_slug}"),
                    );
                    if state_category_is(payload, "workflow_state") {
                        push_vendor_and_generic_owned(
                            out,
                            vendor,
                            format!("issue.entered_workflow_state.{after_slug}"),
                        );
                    }
                }
            }
            if let (Some(before), Some(after)) = (
                payload
                    .get("state")
                    .and_then(|state| state.get("before"))
                    .and_then(event_state_name),
                payload
                    .get("state")
                    .and_then(|state| state.get("after"))
                    .and_then(event_state_name),
            ) {
                if let (Some(before_slug), Some(after_slug)) =
                    (slug_segment(before), slug_segment(after))
                {
                    push_vendor_and_generic_owned(
                        out,
                        vendor,
                        format!("issue.state_changed.{before_slug}.to.{after_slug}"),
                    );
                    if state_category_is(payload, "workflow_state") {
                        push_vendor_and_generic_owned(
                            out,
                            vendor,
                            format!("issue.workflow_state_changed.{before_slug}.to.{after_slug}"),
                        );
                    }
                }
            }
        }
        NativeIssueEventKind::ProjectChanged => {
            push_vendor_and_generic(out, vendor, "issue.project_changed");
            push_vendor_and_generic(out, vendor, "issue.entered_project");
            push_vendor_and_generic(out, vendor, "issue.left_project");
            push_named_transition_aliases(
                out,
                vendor,
                payload,
                "project",
                "issue.entered_project",
                "issue.left_project",
            );
        }
        NativeIssueEventKind::CycleChanged => {
            push_vendor_and_generic(out, vendor, "issue.cycle_changed");
            push_vendor_and_generic(out, vendor, "issue.entered_cycle");
            push_vendor_and_generic(out, vendor, "issue.left_cycle");
            push_named_transition_aliases(
                out,
                vendor,
                payload,
                "cycle",
                "issue.entered_cycle",
                "issue.left_cycle",
            );
        }
        NativeIssueEventKind::SprintChanged => {
            push_vendor_and_generic(out, vendor, "issue.sprint_changed");
            push_vendor_and_generic(out, vendor, "issue.entered_sprint");
            push_vendor_and_generic(out, vendor, "issue.left_sprint");
            push_named_transition_aliases(
                out,
                vendor,
                payload,
                "sprint",
                "issue.entered_sprint",
                "issue.left_sprint",
            );
        }
        NativeIssueEventKind::PriorityChanged => {
            push_vendor_and_generic(out, vendor, "issue.priority_changed");
            if let Some(after) = payload
                .get("priority")
                .and_then(|priority| priority.get("after"))
                .and_then(event_state_name)
                .and_then(slug_segment)
            {
                push_vendor_and_generic_owned(
                    out,
                    vendor,
                    format!("issue.priority_changed.{after}"),
                );
            }
        }
        NativeIssueEventKind::Blocked => {
            push_vendor_and_generic(out, vendor, "issue.blocked");
        }
        NativeIssueEventKind::Unblocked => {
            push_vendor_and_generic(out, vendor, "issue.unblocked");
        }
    }
    true
}

fn push_vendor_and_generic(into: &mut Vec<String>, vendor: &str, alias: &str) {
    push_vendor_and_generic_owned(into, vendor, alias.to_string());
}

fn push_vendor_and_generic_owned(into: &mut Vec<String>, vendor: &str, alias: String) {
    let vendor_alias = format!("{vendor}.{alias}");
    if !into.iter().any(|existing| existing == &vendor_alias) {
        into.push(vendor_alias);
    }
    if !into.iter().any(|existing| existing == &alias) {
        into.push(alias);
    }
}

fn push_named_transition_aliases(
    into: &mut Vec<String>,
    vendor: &str,
    payload: &Value,
    field: &str,
    entered_alias: &str,
    left_alias: &str,
) {
    if let Some(before) = payload
        .get(field)
        .and_then(|value| value.get("before"))
        .and_then(event_state_name)
        .and_then(slug_segment)
    {
        push_vendor_and_generic_owned(into, vendor, format!("{left_alias}.{before}"));
    }
    if let Some(after) = payload
        .get(field)
        .and_then(|value| value.get("after"))
        .and_then(event_state_name)
        .and_then(slug_segment)
    {
        push_vendor_and_generic_owned(into, vendor, format!("{entered_alias}.{after}"));
    }
}

fn event_state_name(value: &Value) -> Option<&str> {
    if let Some(name) = value.get("name").and_then(Value::as_str) {
        return Some(name);
    }
    value.as_str()
}

fn state_category_is(payload: &Value, expected: &str) -> bool {
    payload
        .get("state")
        .and_then(|state| state.get("category"))
        .and_then(Value::as_str)
        == Some(expected)
}

fn slug_segment(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut last_was_sep = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };
        if mapped == '_' {
            if out.is_empty() || last_was_sep {
                continue;
            }
            last_was_sep = true;
            out.push(mapped);
        } else {
            last_was_sep = false;
            out.push(mapped);
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeIssueEventKind {
    StateChanged,
    ProjectChanged,
    CycleChanged,
    SprintChanged,
    PriorityChanged,
    Blocked,
    Unblocked,
}

fn native_issue_event_kind(canonical_id: &str) -> Option<(&str, NativeIssueEventKind)> {
    let parts: Vec<&str> = canonical_id.split('.').collect();
    if parts.len() != 3 || parts[1] != "issue" {
        return None;
    }
    let vendor = parts[0];
    let kind = match parts[2] {
        "state_changed" => NativeIssueEventKind::StateChanged,
        "project_changed" => NativeIssueEventKind::ProjectChanged,
        "cycle_changed" => NativeIssueEventKind::CycleChanged,
        "sprint_changed" => NativeIssueEventKind::SprintChanged,
        "priority_changed" => NativeIssueEventKind::PriorityChanged,
        "blocked" => NativeIssueEventKind::Blocked,
        "unblocked" => NativeIssueEventKind::Unblocked,
        _ => return None,
    };
    Some((vendor, kind))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitlabCommentTarget {
    Issue,
    MergeRequest,
}

fn gitlab_comment_target(payload: &Value) -> Option<GitlabCommentTarget> {
    let target_type = payload
        .get("object_attributes")
        .and_then(|attrs| attrs.get("noteable_type"))
        .and_then(|value| value.as_str())
        .or_else(|| payload.get("target_type").and_then(|value| value.as_str()));
    match target_type {
        Some("Issue") => Some(GitlabCommentTarget::Issue),
        Some("MergeRequest") => Some(GitlabCommentTarget::MergeRequest),
        _ => None,
    }
}

fn gitlab_issue_queue_changed(payload: &Value) -> bool {
    gitlab_changes_include_any(
        payload,
        &[
            "labels",
            "label_ids",
            "assignee_id",
            "assignees",
            "milestone",
        ],
    )
}

fn gitlab_mr_queue_changed(payload: &Value) -> bool {
    gitlab_changes_include_any(
        payload,
        &[
            "labels",
            "label_ids",
            "assignee_id",
            "assignees",
            "reviewers",
            "reviewer_ids",
            "milestone",
        ],
    )
}

fn gitlab_changes_include_any(payload: &Value, keys: &[&str]) -> bool {
    let Some(changes) = payload.get("changes").and_then(|value| value.as_object()) else {
        return false;
    };
    keys.iter().any(|key| changes.contains_key(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_issue_queue_aliases_expand() {
        let candidates = candidate_event_ids("github.issue.labeled", &json!({}));
        assert!(candidates.contains(&"github.issue.labeled".to_string()));
        assert!(candidates.contains(&"github.issue.queue_changed".to_string()));
        assert!(candidates.contains(&"github.issue.queue_entered".to_string()));
        assert!(candidates.contains(&"issue.queue_entered".to_string()));
    }

    #[test]
    fn github_pr_review_aliases_expand() {
        let aliases = derived_event_ids("github.pr.review_requested", &json!({}));
        assert!(aliases.contains(&"github.pr.review_activity".to_string()));
        assert!(aliases.contains(&"pr.review_activity".to_string()));
        assert!(aliases.contains(&"github.pr.queue_entered".to_string()));
    }

    #[test]
    fn gitlab_comment_target_aliases_expand() {
        let payload = json!({
            "object_attributes": { "noteable_type": "Issue" }
        });
        let aliases = derived_event_ids("gitlab.comment", &payload);
        assert!(aliases.contains(&"gitlab.issue.commented".to_string()));
        assert!(aliases.contains(&"issue.activity".to_string()));
    }

    #[test]
    fn matching_event_id_prefers_alias_when_canonical_misses() {
        let matched = matching_event_id("issue.queue_entered", "github.issue.labeled", &json!({}));
        assert_eq!(matched.as_deref(), Some("issue.queue_entered"));
    }

    #[test]
    fn annotate_event_payload_sets_metadata_without_losing_shape() {
        let raw = json!({
            "action": "opened",
            "repository": { "name": "rupu" }
        });
        let enriched = annotate_event_payload(&raw, "github.issue.opened", "github.issue.opened");
        assert_eq!(enriched["id"], "github.issue.opened");
        assert_eq!(enriched["canonical_id"], "github.issue.opened");
        assert_eq!(enriched["repository"]["name"], "rupu");
        assert_eq!(enriched["payload"]["action"], "opened");
    }

    #[test]
    fn native_workflow_state_transition_derives_generic_and_specific_aliases() {
        let payload = json!({
            "state": {
                "category": "workflow_state",
                "before": { "name": "Todo" },
                "after": { "name": "In Progress" }
            }
        });
        let candidates = candidate_event_ids("linear.issue.state_changed", &payload);
        assert!(candidates.contains(&"linear.issue.state_changed".to_string()));
        assert!(candidates.contains(&"issue.state_changed".to_string()));
        assert!(candidates.contains(&"issue.workflow_state_changed".to_string()));
        assert!(candidates.contains(&"issue.entered_state.in_progress".to_string()));
        assert!(candidates.contains(&"linear.issue.left_workflow_state.todo".to_string()));
        assert!(
            candidates.contains(&"issue.workflow_state_changed.todo.to.in_progress".to_string())
        );
    }

    #[test]
    fn native_project_transition_derives_named_project_aliases() {
        let payload = json!({
            "project": {
                "before": { "name": "Backlog" },
                "after": { "name": "Core Platform" }
            }
        });
        let aliases = derived_event_ids("jira.issue.project_changed", &payload);
        assert!(aliases.contains(&"jira.issue.project_changed".to_string()));
        assert!(aliases.contains(&"issue.entered_project.core_platform".to_string()));
        assert!(aliases.contains(&"jira.issue.left_project.backlog".to_string()));
    }

    #[test]
    fn matching_event_id_matches_native_specific_state_alias() {
        let payload = json!({
            "state": {
                "category": "workflow_state",
                "before": { "name": "Todo" },
                "after": { "name": "Ready For Review" }
            }
        });
        let matched = matching_event_id(
            "issue.entered_workflow_state.ready_for_review",
            "linear.issue.state_changed",
            &payload,
        );
        assert_eq!(
            matched.as_deref(),
            Some("issue.entered_workflow_state.ready_for_review")
        );
    }
}
