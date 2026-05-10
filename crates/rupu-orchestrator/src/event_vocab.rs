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
}
