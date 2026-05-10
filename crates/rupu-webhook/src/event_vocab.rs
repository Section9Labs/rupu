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
//!
//! Workflow-side glob matching *is* supported by the orchestrator.
//! This module only maps raw vendor deliveries onto canonical rupu
//! event ids; broader semantic aliases are derived one layer up.

use serde_json::Value;

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
}
